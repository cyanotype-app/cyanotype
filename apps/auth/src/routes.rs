use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, State},
    http::StatusCode,
    middleware,
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use cyanotype_service::{
    ApiError, ApiResult, AuthUser,
    auth::{SESSION_COOKIE as COOKIE, sha256},
};
use sqlx::PgPool;

use crate::{
    ratelimit::{self, LimitState, RateLimiter},
    settings::{AuthConfig, RegistrationMode},
};

const SESSION_DAYS: i64 = 30;
/// Serialises first-user registration across all auth replicas (transaction-scoped advisory lock).
const REGISTER_LOCK_KEY: i64 = 0x6379_616e_6f72_6567; // "cyanoreg"

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub cfg: Arc<AuthConfig>,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(s: &AppState) -> PgPool {
        s.db.clone()
    }
}

/// All `/auth/*` routes, with the per-IP rate limiter in front of register and login.
pub fn routes(state: AppState) -> Router {
    let limit = LimitState {
        limiter: Arc::new(RateLimiter::new(state.cfg.rate_limit)),
        trusted: Arc::new(state.cfg.trusted_proxies.clone()),
    };
    let limited = Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route_layer(middleware::from_fn_with_state(limit, ratelimit::limit));
    Router::new()
        .merge(limited)
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .with_state(state)
}

fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    cyanotype_core::keys::hex(&b)
}

#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
}

async fn start_session(db: &PgPool, cfg: &AuthConfig, jar: CookieJar, user_id: Uuid) -> ApiResult<CookieJar> {
    let token = random_token();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, now() + make_interval(days => $3))",
    )
    .bind(sha256(token.as_bytes()))
    .bind(user_id)
    .bind(SESSION_DAYS as i32)
    .execute(db)
    .await?;
    let cookie = Cookie::build((COOKIE, token))
        .http_only(true)
        .secure(cfg.cookie_secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::days(SESSION_DAYS))
        .build();
    Ok(jar.add(cookie))
}

async fn register(
    State(AppState { db, cfg }): State<AppState>,
    jar: CookieJar,
    Json(c): Json<Credentials>,
) -> ApiResult<(StatusCode, CookieJar, Json<Value>)> {
    if cfg.registration == RegistrationMode::Closed {
        return Err(ApiError::Forbidden("registration is closed".into()));
    }
    let email = c.email.trim().to_string();
    if !email.contains('@') || c.password.len() < 8 {
        return Err(ApiError::BadRequest(
            "valid email and a password of 8+ characters required".into(),
        ));
    }
    let hash = Argon2::default()
        .hash_password(c.password.as_bytes(), &SaltString::generate(&mut OsRng))
        .map_err(|e| anyhow::anyhow!("hash: {e}"))?
        .to_string();
    let mut tx = db.begin().await?;
    if cfg.registration == RegistrationMode::FirstUser {
        // The lock makes check-then-insert atomic: a concurrent registrant waits here until the
        // first commits, then sees a non-empty table. Released automatically at commit/rollback.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(REGISTER_LOCK_KEY)
            .execute(&mut *tx)
            .await?;
        let (any,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM users)")
            .fetch_one(&mut *tx)
            .await?;
        if any {
            return Err(ApiError::Forbidden(
                "registration is closed: this instance already has an owner".into(),
            ));
        }
    }
    let row: Option<(Uuid,)> =
        sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ($1, $2) ON CONFLICT DO NOTHING RETURNING id")
            .bind(&email)
            .bind(hash)
            .fetch_optional(&mut *tx)
            .await?;
    let (id,) = row.ok_or_else(|| ApiError::Conflict("email already registered".into()))?;
    tx.commit().await?;
    let jar = start_session(&db, &cfg, jar, id).await?;
    Ok((StatusCode::CREATED, jar, Json(json!({"id": id, "email": email}))))
}

async fn login(
    State(AppState { db, cfg }): State<AppState>,
    jar: CookieJar,
    Json(c): Json<Credentials>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE lower(email) = lower($1)")
            .bind(c.email.trim())
            .fetch_optional(&db)
            .await?;
    // Same error for unknown user and bad password.
    let (id, hash) = row.ok_or(ApiError::Unauthorized)?;
    let parsed = PasswordHash::new(&hash).map_err(|e| anyhow::anyhow!("stored hash: {e}"))?;
    Argon2::default()
        .verify_password(c.password.as_bytes(), &parsed)
        .map_err(|_| ApiError::Unauthorized)?;
    let jar = start_session(&db, &cfg, jar, id).await?;
    Ok((jar, Json(json!({"id": id}))))
}

async fn logout(State(db): State<PgPool>, jar: CookieJar) -> ApiResult<(CookieJar, StatusCode)> {
    if let Some(c) = jar.get(COOKIE) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(sha256(c.value().as_bytes()))
            .execute(&db)
            .await?;
    }
    Ok((
        jar.remove(Cookie::build(COOKIE).path("/").build()),
        StatusCode::NO_CONTENT,
    ))
}

async fn me(State(db): State<PgPool>, user: AuthUser) -> ApiResult<Json<Value>> {
    let (email,): (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&db)
        .await?;
    Ok(Json(json!({"id": user.id, "email": email})))
}

#[cfg(test)]
mod tests {
    //! DB-backed tests need `DATABASE_URL` (role must be able to CREATE DATABASE) and are skipped
    //! without it. The rate-limit tests never reach the database.
    use std::net::SocketAddr;

    use axum::{
        body::Body,
        extract::ConnectInfo,
        http::{Request, header},
    };
    use cyanotype_core::db;
    use sqlx::{Connection, Executor, PgConnection};
    use tower::ServiceExt;

    use super::*;

    struct TestDb {
        pool: PgPool,
        admin_url: String,
        name: String,
    }

    impl TestDb {
        async fn new() -> Option<Self> {
            let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
            let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
            let mut admin = PgConnection::connect(&admin_url).await.unwrap();
            admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
            let (base, _) = admin_url.rsplit_once('/').unwrap();
            let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
            db::migrate(&pool).await.unwrap();
            Some(Self { pool, admin_url, name })
        }

        async fn teardown(self) {
            self.pool.close().await;
            let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
            admin
                .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str())
                .await
                .unwrap();
        }
    }

    fn app(db: PgPool, mutate: impl FnOnce(&mut AuthConfig)) -> Router {
        let mut cfg = AuthConfig::default();
        // Generous limit: these tests are about registration and cookies, not throttling.
        cfg.rate_limit.burst = 1000;
        mutate(&mut cfg);
        routes(AppState { db, cfg: Arc::new(cfg) })
    }

    fn post_json(path: &str, body: &str, peer: &str, xff: Option<&str>) -> Request<Body> {
        let mut b = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if let Some(x) = xff {
            b = b.header("x-forwarded-for", x);
        }
        let mut req = b.body(Body::from(body.to_string())).unwrap();
        let addr: SocketAddr = format!("{peer}:50000").parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        req
    }

    fn reg(email: &str) -> Request<Body> {
        post_json(
            "/auth/register",
            &format!(r#"{{"email":"{email}","password":"correct horse battery"}}"#),
            "172.18.0.2",
            None,
        )
    }

    async fn status(app: &Router, req: Request<Body>) -> StatusCode {
        app.clone().oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn closed_mode_refuses_everyone() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::Closed);
        assert_eq!(status(&app, reg("a@example.com")).await, StatusCode::FORBIDDEN);
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM users")
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        t.teardown().await;
    }

    #[tokio::test]
    async fn first_user_mode_allows_exactly_one_account() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::FirstUser);
        assert_eq!(status(&app, reg("owner@example.com")).await, StatusCode::CREATED);
        assert_eq!(status(&app, reg("second@example.com")).await, StatusCode::FORBIDDEN);
        // Even a repeat of the owner's address is refused (403, not 409): the instance is claimed.
        assert_eq!(status(&app, reg("owner@example.com")).await, StatusCode::FORBIDDEN);
        t.teardown().await;
    }

    #[tokio::test]
    async fn first_user_mode_is_race_safe() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::FirstUser);
        let mut tasks = Vec::new();
        for i in 0..12 {
            let app = app.clone();
            tasks.push(tokio::spawn(async move {
                status(&app, reg(&format!("racer{i}@example.com"))).await
            }));
        }
        let mut created = 0;
        let mut forbidden = 0;
        for t in tasks {
            match t.await.unwrap() {
                StatusCode::CREATED => created += 1,
                StatusCode::FORBIDDEN => forbidden += 1,
                other => panic!("unexpected {other}"),
            }
        }
        assert_eq!((created, forbidden), (1, 11));
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM users")
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        t.teardown().await;
    }

    #[tokio::test]
    async fn open_mode_allows_many() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::Open);
        assert_eq!(status(&app, reg("a@example.com")).await, StatusCode::CREATED);
        assert_eq!(status(&app, reg("b@example.com")).await, StatusCode::CREATED);
        assert_eq!(status(&app, reg("a@example.com")).await, StatusCode::CONFLICT);
        t.teardown().await;
    }

    #[tokio::test]
    async fn session_cookie_secure_flag_follows_config() {
        let Some(t) = TestDb::new().await else { return };
        for (secure, email) in [(true, "s@example.com"), (false, "p@example.com")] {
            let app = app(t.pool.clone(), |c| {
                c.registration = RegistrationMode::Open;
                c.cookie_secure = secure;
            });
            let res = app.clone().oneshot(reg(email)).await.unwrap();
            assert_eq!(res.status(), StatusCode::CREATED);
            let cookie = res.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
            assert!(cookie.contains("HttpOnly"), "{cookie}");
            assert_eq!(cookie.contains("Secure"), secure, "{cookie}");
            // Login sets the same attributes.
            let login = post_json(
                "/auth/login",
                &format!(r#"{{"email":"{email}","password":"correct horse battery"}}"#),
                "172.18.0.2",
                None,
            );
            let res = app.oneshot(login).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let cookie = res.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
            assert_eq!(cookie.contains("Secure"), secure, "{cookie}");
        }
        t.teardown().await;
    }

    // ---- rate limiting: requests with an invalid body are rejected (422) before the database,
    // so a lazy pool pointing nowhere is enough. They still consume tokens.

    fn limited_app(burst: u32) -> Router {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://x:x@127.0.0.1:1/x")
            .unwrap();
        let mut cfg = AuthConfig::default();
        cfg.rate_limit.burst = burst;
        cfg.rate_limit.per_minute = 1; // effectively no refill during a test
        routes(AppState { db, cfg: Arc::new(cfg) })
    }

    #[tokio::test]
    async fn login_and_register_return_429_after_the_burst() {
        let app = limited_app(3);
        for path in ["/auth/login", "/auth/register", "/auth/login"] {
            let s = status(&app, post_json(path, "{}", "172.18.0.2", Some("198.51.100.7"))).await;
            assert_ne!(s, StatusCode::TOO_MANY_REQUESTS);
        }
        for path in ["/auth/login", "/auth/register"] {
            let res = app
                .clone()
                .oneshot(post_json(path, "{}", "172.18.0.2", Some("198.51.100.7")))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
            assert!(res.headers().contains_key(header::RETRY_AFTER));
        }
        // A different client behind the same gateway is unaffected.
        let s = status(&app, post_json("/auth/login", "{}", "172.18.0.2", Some("198.51.100.8"))).await;
        assert_ne!(s, StatusCode::TOO_MANY_REQUESTS);
        // Other auth routes are not throttled.
        let req = Request::builder().uri("/auth/me").body(Body::empty()).unwrap();
        assert_ne!(status(&app, req).await, StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn spoofed_forwarded_for_from_untrusted_peer_does_not_dodge_the_limit() {
        let app = limited_app(2);
        // The peer is a public address (not the gateway), so each rotating X-Forwarded-For is ignored.
        let mut last = StatusCode::OK;
        for i in 0..4 {
            let xff = format!("198.51.100.{i}");
            last = status(&app, post_json("/auth/login", "{}", "203.0.113.50", Some(&xff))).await;
        }
        assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    }
}
