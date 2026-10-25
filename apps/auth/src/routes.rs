use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, Path, State},
    http::StatusCode,
    middleware,
    routing::{delete, get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use cyanotype_service::{
    AnyCaller, ApiError, ApiResult, Scope, SessionUser, audit,
    auth::{SESSION_COOKIE as COOKIE, sha256},
    ratelimit::{self, LimitState, RateLimiter},
};
use sqlx::PgPool;

use cyanotype_core::limits::{self, Limits};

use crate::settings::{AuthConfig, RegistrationMode};
use crate::{cli_login, invites, twofactor, users_admin};

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
        .route("/auth/hidden/unlock", post(hidden_unlock))
        .route("/auth/invites/check", post(invites::check))
        .route("/auth/reset-password/check", post(users_admin::reset_check))
        .route("/auth/reset-password", post(users_admin::reset_password))
        .merge(cli_login::limited_routes())
        .route_layer(middleware::from_fn_with_state(limit, ratelimit::limit));
    Router::new()
        .merge(limited)
        .merge(twofactor::login_routes(&state))
        .merge(twofactor::routes(&state))
        .merge(twofactor::admin_routes())
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/auth/hidden", get(hidden_status).delete(hidden_lock))
        .route("/auth/registration", get(registration_info))
        .merge(invites::admin_routes())
        .merge(users_admin::admin_routes())
        .merge(users_admin::session_routes())
        .merge(cli_login::routes())
        .route("/auth/api-keys", get(list_api_keys).post(create_api_key))
        .route("/auth/api-keys/{id}", delete(revoke_api_key))
        .with_state(state)
}

pub(crate) fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    cyanotype_core::keys::hex(&b)
}

#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
}

/// The session cookie. It is built `Secure`; that is turned off only when `COOKIE_SECURE=false` was set
/// explicitly (plain-http local dev). The setting defaults to true and production pins it.
fn session_cookie(token: String, secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::build((COOKIE, token))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::days(SESSION_DAYS))
        .build();
    if !secure {
        cookie.set_secure(false);
    }
    cookie
}

/// Removal cookie for the session, carrying the same `Secure` attribute as the cookie it replaces.
pub(crate) fn session_removal_cookie(secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::build(COOKIE).http_only(true).secure(true).path("/").build();
    if !secure {
        cookie.set_secure(false);
    }
    cookie
}

/// `needs_mfa_enrol`: the administrator requires 2FA and this account has none, so the session may only enrol.
pub(crate) async fn start_session(
    db: &PgPool,
    cfg: &AuthConfig,
    jar: CookieJar,
    user_id: Uuid,
    needs_mfa_enrol: bool,
) -> ApiResult<CookieJar> {
    let token = random_token();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at, needs_mfa_enrol)
         VALUES ($1, $2, now() + make_interval(days => $3), $4)",
    )
    .bind(sha256(token.as_bytes()))
    .bind(user_id)
    .bind(SESSION_DAYS as i32)
    .bind(needs_mfa_enrol)
    .execute(db)
    .await?;
    let cookie = session_cookie(token, cfg.cookie_secure);
    Ok(jar.add(cookie))
}

#[derive(Deserialize)]
struct RegisterReq {
    email: String,
    password: String,
    /// The invite token (from the link's `?invite=`); needed in `invite` mode once the instance has an owner.
    #[serde(default)]
    invite: Option<String>,
}

async fn register(
    State(AppState { db, cfg }): State<AppState>,
    jar: CookieJar,
    Json(c): Json<RegisterReq>,
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
    // The lock makes check-then-insert atomic: a concurrent registrant waits here until the first commits, then sees a
    // non-empty table (so exactly one account becomes the admin). Released automatically at commit/rollback.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(REGISTER_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    let (any,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM users)")
        .fetch_one(&mut *tx)
        .await?;
    let token = c.invite.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let mut invite = None;
    if any {
        match cfg.registration {
            RegistrationMode::FirstUser => {
                return Err(ApiError::Forbidden(
                    "registration is closed: this instance already has an owner".into(),
                ));
            }
            RegistrationMode::Invite => {
                let Some(token) = token else {
                    return Err(ApiError::Forbidden(
                        "registration needs an invite link from the administrator".into(),
                    ));
                };
                match invites::consume(&mut tx, token).await? {
                    Some(i) => invite = Some(i),
                    None => return Err(invites::refusal(&mut tx, token).await?),
                }
            }
            // Open: anyone, and a valid invite still gets recorded as used.
            RegistrationMode::Open => {
                if let Some(token) = token {
                    invite = invites::consume(&mut tx, token).await?;
                }
            }
            RegistrationMode::Closed => unreachable!("refused above"),
        }
    }
    // The first account of an instance is its admin, and unlimited. Every later account starts with the server default
    // limits, with an invite's presets in front of them (docs/admin.md, "Limits").
    let role = if any { "member" } else { "admin" };
    let limits = if any {
        let preset = invite.as_ref().map(|i| i.limits).unwrap_or_default();
        preset.or(limits::server_default(&mut *tx).await?)
    } else {
        Limits::default()
    };
    let row: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO users (email, password_hash, role, quota_bytes, daily_upload_bytes, max_file_bytes)
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(&email)
    .bind(hash)
    .bind(role)
    .bind(limits.quota_bytes)
    .bind(limits.daily_upload_bytes)
    .bind(limits.max_file_bytes)
    .fetch_optional(&mut *tx)
    .await?;
    // A taken address rolls everything back, so the invite is not spent.
    let (id,) = row.ok_or_else(|| ApiError::Conflict("email already registered".into()))?;
    if let Some(i) = invite {
        sqlx::query("UPDATE invites SET used_by = $2, used_email = $3 WHERE id = $1")
            .bind(i.id)
            .bind(id)
            .bind(&email)
            .execute(&mut *tx)
            .await?;
        audit::record(
            &mut *tx,
            Some(id),
            "invite.use",
            Some("invite"),
            Some(&i.id.to_string()),
            json!({"email": email}),
        )
        .await?;
    }
    tx.commit().await?;
    let pending = twofactor::enforced(&db, &cfg).await?;
    let jar = start_session(&db, &cfg, jar, id, pending).await?;
    Ok((
        StatusCode::CREATED,
        jar,
        Json(json!({"id": id, "email": email, "role": role, "mfa_enrol_required": pending})),
    ))
}

/// What the register page needs to know without trying to register: the mode, and whether the instance still has no
/// account (the first one registers without an invite and becomes the admin).
async fn registration_info(State(AppState { db, cfg }): State<AppState>) -> ApiResult<Json<Value>> {
    let (any,): (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM users)")
        .fetch_one(&db)
        .await?;
    let mode = match cfg.registration {
        RegistrationMode::Open => "open",
        RegistrationMode::FirstUser => "first_user",
        RegistrationMode::Invite => "invite",
        RegistrationMode::Closed => "closed",
    };
    let (open, invite_required) = match cfg.registration {
        RegistrationMode::Closed => (false, false),
        RegistrationMode::Open => (true, false),
        RegistrationMode::FirstUser => (!any, false),
        RegistrationMode::Invite => (!any, any),
    };
    Ok(Json(
        json!({"mode": mode, "first_account": !any, "open": open, "invite_required": invite_required}),
    ))
}

async fn login(
    State(AppState { db, cfg }): State<AppState>,
    jar: CookieJar,
    Json(c): Json<Credentials>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let row: Option<(Uuid, String, bool)> =
        sqlx::query_as("SELECT id, password_hash, disabled_at IS NOT NULL FROM users WHERE lower(email) = lower($1)")
            .bind(c.email.trim())
            .fetch_optional(&db)
            .await?;
    // Same error for unknown user and bad password.
    let (id, hash, disabled) = row.ok_or(ApiError::Unauthorized)?;
    let parsed = PasswordHash::new(&hash).map_err(|e| anyhow::anyhow!("stored hash: {e}"))?;
    Argon2::default()
        .verify_password(c.password.as_bytes(), &parsed)
        .map_err(|_| ApiError::Unauthorized)?;
    // Said only to someone who knows the password, so it does not reveal which addresses have accounts.
    if disabled {
        return Err(ApiError::Limited {
            status: StatusCode::FORBIDDEN,
            code: "account_disabled",
            message: "This account has been disabled. Ask the administrator.".into(),
            details: json!({}),
            retry_after_secs: None,
        });
    }
    // A correct password is not a session yet when the account has a second factor: it earns a challenge to answer with a code.
    if twofactor::has_2fa(&db, id).await? {
        let body = twofactor::begin_challenge(&db, &cfg, id).await?;
        return Ok((jar, body));
    }
    let pending = twofactor::enforced(&db, &cfg).await?;
    let jar = start_session(&db, &cfg, jar, id, pending).await?;
    Ok((jar, Json(json!({"id": id, "mfa_enrol_required": pending}))))
}

#[derive(Deserialize)]
struct UnlockReq {
    password: String,
}

fn unlock_state(until: Option<time::OffsetDateTime>) -> Value {
    let open = until.filter(|t| *t > time::OffsetDateTime::now_utc());
    json!({"unlocked": open.is_some(), "expires_at": open.map(rfc3339)})
}

/// `POST /auth/hidden/unlock {password}`: opens the Hidden view for this login session for ten minutes. The password is the
/// account's, checked here with argon2 like a login; the unlock is stored on the session row (`hidden_unlocked_until`), where
/// the assets service reads it on every request, so the browser holds nothing that could be forged or extended. Not available
/// to API keys. A wrong password is `403 wrong_password` (a 401 would sign the web app out); the per-IP limiter that guards
/// login guards this too.
async fn hidden_unlock(
    State(AppState { db, .. }): State<AppState>,
    user: SessionUser,
    jar: CookieJar,
    Json(r): Json<UnlockReq>,
) -> ApiResult<Json<Value>> {
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&db)
        .await?;
    let parsed = PasswordHash::new(&hash).map_err(|e| anyhow::anyhow!("stored hash: {e}"))?;
    if Argon2::default()
        .verify_password(r.password.as_bytes(), &parsed)
        .is_err()
    {
        return Err(ApiError::Limited {
            status: StatusCode::FORBIDDEN,
            code: "wrong_password",
            message: "That password is not right.".into(),
            details: json!({}),
            retry_after_secs: None,
        });
    }
    let token = jar.get(COOKIE).ok_or(ApiError::Unauthorized)?;
    let until: Option<time::OffsetDateTime> = sqlx::query_scalar(
        "UPDATE sessions SET hidden_unlocked_until = now() + make_interval(mins => $2)
         WHERE token_hash = $1 RETURNING hidden_unlocked_until",
    )
    .bind(sha256(token.value().as_bytes()))
    .bind(cyanotype_service::auth::HIDDEN_UNLOCK_MINUTES)
    .fetch_optional(&db)
    .await?
    .flatten();
    Ok(Json(unlock_state(until)))
}

/// `GET /auth/hidden`: is the Hidden view open for this session, and until when.
async fn hidden_status(
    State(AppState { db, .. }): State<AppState>,
    _user: SessionUser,
    jar: CookieJar,
) -> ApiResult<Json<Value>> {
    let token = jar.get(COOKIE).ok_or(ApiError::Unauthorized)?;
    let until: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT hidden_unlocked_until FROM sessions WHERE token_hash = $1")
            .bind(sha256(token.value().as_bytes()))
            .fetch_optional(&db)
            .await?
            .flatten();
    Ok(Json(unlock_state(until)))
}

/// `DELETE /auth/hidden`: close the Hidden view now.
async fn hidden_lock(
    State(AppState { db, .. }): State<AppState>,
    _user: SessionUser,
    jar: CookieJar,
) -> ApiResult<StatusCode> {
    let token = jar.get(COOKIE).ok_or(ApiError::Unauthorized)?;
    sqlx::query("UPDATE sessions SET hidden_unlocked_until = NULL WHERE token_hash = $1")
        .bind(sha256(token.value().as_bytes()))
        .execute(&db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn logout(State(AppState { db, cfg }): State<AppState>, jar: CookieJar) -> ApiResult<(CookieJar, StatusCode)> {
    if let Some(c) = jar.get(COOKIE) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(sha256(c.value().as_bytes()))
            .execute(&db)
            .await?;
    }
    Ok((
        jar.remove(session_removal_cookie(cfg.cookie_secure)),
        StatusCode::NO_CONTENT,
    ))
}

/// Who am I: open to every scope (it is how `cyanotype status` checks a key) and to a session that still has to enrol in
/// 2FA (the web app reads `mfa_enrol_required` here and sends it to the enrolment page).
async fn me(State(db): State<PgPool>, AnyCaller(c): AnyCaller) -> ApiResult<Json<Value>> {
    let (email, role): (String, String) = sqlx::query_as("SELECT email, role FROM users WHERE id = $1")
        .bind(c.id)
        .fetch_one(&db)
        .await?;
    let two_factor = twofactor::has_2fa(&db, c.id).await?;
    Ok(Json(json!({
        "id": c.id, "email": email, "role": role,
        "two_factor": two_factor, "mfa_enrol_required": c.mfa_pending,
        "scope": c.scope.as_str(),
    })))
}

/// Plaintext API keys look like `cyk_<64 hex>` (32 random bytes). Only the sha256 is stored.
pub(crate) const API_KEY_PREFIX: &str = "cyk_";
/// Characters of the key (after `cyk_`) kept in clear so the list can tell keys apart.
pub(crate) const API_KEY_HINT_LEN: usize = 6;
pub(crate) const MAX_API_KEYS_PER_USER: i64 = 50;

pub(crate) fn new_api_key() -> String {
    format!("{API_KEY_PREFIX}{}", random_token())
}

#[derive(Deserialize)]
struct NewApiKey {
    name: String,
    /// `import`, `read` or `full`. Absent means `full`, what keys were before scopes existed.
    #[serde(default)]
    scope: Option<String>,
    /// Days until the key stops working (1 to 3650). Absent: it does not expire.
    #[serde(default)]
    expires_in_days: Option<i64>,
}

const MAX_KEY_DAYS: i64 = 3650;

/// Session only: a leaked API key must not be able to mint more keys.
async fn create_api_key(
    State(db): State<PgPool>,
    user: SessionUser,
    Json(req): Json<NewApiKey>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::BadRequest("name must be 1-100 characters".into()));
    }
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM api_keys WHERE user_id = $1")
        .bind(user.id)
        .fetch_one(&db)
        .await?;
    if n >= MAX_API_KEYS_PER_USER {
        return Err(ApiError::Conflict(format!(
            "at most {MAX_API_KEYS_PER_USER} API keys; revoke one first"
        )));
    }
    let scope = match req.scope.as_deref() {
        None => Scope::Full,
        Some(s) => Scope::parse(s).ok_or_else(|| ApiError::BadRequest("scope must be import, read or full".into()))?,
    };
    if let Some(d) = req.expires_in_days
        && !(1..=MAX_KEY_DAYS).contains(&d)
    {
        return Err(ApiError::BadRequest(format!(
            "expires_in_days must be between 1 and {MAX_KEY_DAYS}"
        )));
    }
    let key = new_api_key();
    let hint = format!("{API_KEY_PREFIX}{}", &key[API_KEY_PREFIX.len()..][..API_KEY_HINT_LEN]);
    let (id, created_at, expires_at): (Uuid, time::OffsetDateTime, Option<time::OffsetDateTime>) = sqlx::query_as(
        "INSERT INTO api_keys (user_id, name, key_hash, prefix, scope, expires_at)
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $6::int IS NULL THEN NULL ELSE now() + make_interval(days => $6::int) END)
         RETURNING id, created_at, expires_at",
    )
    .bind(user.id)
    .bind(name)
    .bind(sha256(key.as_bytes()))
    .bind(&hint)
    .bind(scope.as_str())
    .bind(req.expires_in_days.map(|d| d as i32))
    .fetch_one(&db)
    .await?;
    let created_at = rfc3339(created_at);
    // The only time the plaintext key is ever returned.
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id, "name": name, "prefix": hint, "created_at": created_at, "key": key,
            "scope": scope.as_str(), "expires_at": expires_at.map(rfc3339),
        })),
    ))
}

async fn list_api_keys(State(db): State<PgPool>, user: SessionUser) -> ApiResult<Json<Value>> {
    type Row = (
        Uuid,
        String,
        String,
        time::OffsetDateTime,
        Option<time::OffsetDateTime>,
        String,
        Option<time::OffsetDateTime>,
        bool,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, prefix, created_at, last_used_at, scope, expires_at,
                COALESCE(expires_at <= now(), false)
         FROM api_keys WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user.id)
    .fetch_all(&db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(
            |(id, name, prefix, created_at, last_used_at, scope, expires_at, expired)| {
                json!({
                    "id": id, "name": name, "prefix": prefix,
                    "created_at": rfc3339(created_at),
                    "last_used_at": last_used_at.map(rfc3339),
                    "scope": scope, "expires_at": expires_at.map(rfc3339), "expired": expired,
                })
            },
        )
        .collect();
    Ok(Json(json!({"items": items})))
}

pub(crate) fn rfc3339(t: time::OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

async fn revoke_api_key(State(db): State<PgPool>, user: SessionUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let n = sqlx::query("DELETE FROM api_keys WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&db)
        .await?
        .rows_affected();
    if n == 0 {
        Err(ApiError::NotFound)
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
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

    #[test]
    fn session_cookies_carry_secure_unless_switched_off() {
        for secure in [true, false] {
            for c in [session_cookie("t".into(), secure), session_removal_cookie(secure)] {
                let c = c.to_string();
                assert_eq!(c.contains("Secure"), secure, "{c}");
                assert!(c.contains("HttpOnly") && c.contains("Path=/"), "{c}");
            }
        }
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

    // ---- API keys

    fn cookie_of(res: &axum::response::Response) -> String {
        let c = res.headers()[header::SET_COOKIE].to_str().unwrap();
        c.split(';').next().unwrap().to_string()
    }

    async fn register_cookie(app: &Router, email: &str) -> String {
        let res = app.clone().oneshot(reg(email)).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        cookie_of(&res)
    }

    fn call(method: &str, path: &str, auth: (&str, &str), body: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().method(method).uri(path).header(auth.0, auth.1);
        if body.is_some() {
            b = b.header("content-type", "application/json");
        }
        b.body(Body::from(body.unwrap_or("").to_string())).unwrap()
    }

    async fn json_of(res: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    /// A request with a cookie (or bearer key) and a peer address, for the routes behind the rate limiter.
    fn limited_call(method: &str, path: &str, auth: (&str, &str), body: Option<&str>) -> Request<Body> {
        let mut req = call(method, path, auth, body);
        let addr: SocketAddr = "172.18.0.9:50000".parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));
        req
    }

    #[tokio::test]
    async fn the_hidden_view_unlocks_with_the_password_for_ten_minutes_on_that_session_only() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::Open);
        let cookie = register_cookie(&app, "h@example.com").await;
        let other = register_cookie(&app, "h@example.com.other").await;
        let sess = ("cookie", cookie.as_str());
        let state = |app: Router, c: String| async move {
            let res = app
                .oneshot(call("GET", "/auth/hidden", ("cookie", &c), None))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            json_of(res).await
        };
        assert_eq!(
            state(app.clone(), cookie.clone()).await,
            json!({"unlocked": false, "expires_at": null})
        );

        // A wrong password is a 403 the app can show (a 401 would sign it out), and unlocks nothing.
        let res = app
            .clone()
            .oneshot(limited_call(
                "POST",
                "/auth/hidden/unlock",
                sess,
                Some(r#"{"password":"wrong"}"#),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert_eq!(json_of(res).await["code"], "wrong_password");
        assert!(!state(app.clone(), cookie.clone()).await["unlocked"].as_bool().unwrap());

        // The right one opens it for ten minutes, on this session's row.
        let res = app
            .clone()
            .oneshot(limited_call(
                "POST",
                "/auth/hidden/unlock",
                sess,
                Some(r#"{"password":"correct horse battery"}"#),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = json_of(res).await;
        assert_eq!(body["unlocked"], true);
        assert!(body["expires_at"].is_string());
        let mins: f64 = sqlx::query_scalar(
            "SELECT max(extract(epoch FROM hidden_unlocked_until - now()) / 60)::float8 FROM sessions",
        )
        .fetch_one(&t.pool)
        .await
        .unwrap();
        assert!((9.0..=10.0).contains(&mins), "{mins}");
        let unlocked_rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sessions WHERE hidden_unlocked_until IS NOT NULL")
                .fetch_one(&t.pool)
                .await
                .unwrap();
        assert_eq!(unlocked_rows, 1, "only the session that entered the password");
        assert!(state(app.clone(), cookie.clone()).await["unlocked"].as_bool().unwrap());
        assert!(!state(app.clone(), other.clone()).await["unlocked"].as_bool().unwrap());

        // An API key cannot unlock (nor read the state), even next to a cookie.
        let key = {
            let res = app
                .clone()
                .oneshot(call("POST", "/auth/api-keys", sess, Some(r#"{"name":"cli"}"#)))
                .await
                .unwrap();
            format!("Bearer {}", json_of(res).await["key"].as_str().unwrap())
        };
        let mut req = limited_call(
            "POST",
            "/auth/hidden/unlock",
            ("authorization", &key),
            Some(r#"{"password":"correct horse battery"}"#),
        );
        req.headers_mut().insert("cookie", cookie.parse().unwrap());
        assert_eq!(status(&app, req).await, StatusCode::FORBIDDEN);
        let req = limited_call(
            "POST",
            "/auth/hidden/unlock",
            ("x", "y"),
            Some(r#"{"password":"correct horse battery"}"#),
        );
        assert_eq!(status(&app, req).await, StatusCode::UNAUTHORIZED);

        // Locking again takes it back at once; an expired unlock reads as locked.
        let res = app
            .clone()
            .oneshot(call("DELETE", "/auth/hidden", sess, None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert!(!state(app.clone(), cookie.clone()).await["unlocked"].as_bool().unwrap());
        sqlx::query("UPDATE sessions SET hidden_unlocked_until = now() - interval '1 second'")
            .execute(&t.pool)
            .await
            .unwrap();
        assert!(!state(app.clone(), cookie.clone()).await["unlocked"].as_bool().unwrap());
        t.teardown().await;
    }

    #[tokio::test]
    async fn api_keys_create_list_use_and_revoke() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(t.pool.clone(), |c| c.registration = RegistrationMode::Open);
        let cookie = register_cookie(&app, "k@example.com").await;
        let sess = ("cookie", cookie.as_str());

        // Create: plaintext returned once, in the documented shape.
        let res = app
            .clone()
            .oneshot(call("POST", "/auth/api-keys", sess, Some(r#"{"name":"laptop"}"#)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let created = json_of(res).await;
        let key = created["key"].as_str().unwrap().to_string();
        let id = created["id"].as_str().unwrap().to_string();
        assert!(key.starts_with("cyk_") && key.len() == 4 + 64, "{key}");
        assert!(key[4..].bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(created["prefix"].as_str().unwrap(), &key[..10]);

        // Only the sha256 is stored.
        let (stored, hint): (Vec<u8>, String) = sqlx::query_as("SELECT key_hash, prefix FROM api_keys")
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert_eq!(stored, sha256(key.as_bytes()));
        assert_eq!(hint, &key[..10]);

        // Empty / oversized names are rejected.
        for body in [r#"{"name":"  "}"#, &format!(r#"{{"name":"{}"}}"#, "x".repeat(101))] {
            let res = app
                .clone()
                .oneshot(call("POST", "/auth/api-keys", sess, Some(body)))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        }

        // List never contains the secret and has no last_used_at yet.
        let res = app
            .clone()
            .oneshot(call("GET", "/auth/api-keys", sess, None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let list = json_of(res).await;
        let item = &list["items"][0];
        assert_eq!(item["id"].as_str().unwrap(), id);
        assert_eq!(item["name"], "laptop");
        assert!(item["created_at"].is_string() && item["last_used_at"].is_null());
        assert!(!list.to_string().contains(&key[10..]));

        // The key authenticates (and stamps last_used_at).
        let bearer = format!("Bearer {key}");
        let res = app
            .clone()
            .oneshot(call("GET", "/auth/me", ("authorization", &bearer), None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let res = app
            .clone()
            .oneshot(call("GET", "/auth/api-keys", sess, None))
            .await
            .unwrap();
        assert!(json_of(res).await["items"][0]["last_used_at"].is_string());

        // ...but cannot mint, list or revoke keys, even when a valid session cookie rides along.
        for (m, path, body) in [
            ("POST", "/auth/api-keys".to_string(), Some(r#"{"name":"evil"}"#)),
            ("GET", "/auth/api-keys".to_string(), None),
            ("DELETE", format!("/auth/api-keys/{id}"), None),
        ] {
            let res = app
                .clone()
                .oneshot(call(m, &path, ("authorization", &bearer), body))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{m} {path}");
            let mut req = call(m, &path, ("authorization", &bearer), body);
            req.headers_mut().insert("cookie", cookie.parse().unwrap());
            assert_eq!(
                app.clone().oneshot(req).await.unwrap().status(),
                StatusCode::FORBIDDEN,
                "{m} {path} +cookie"
            );
        }
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM api_keys")
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert_eq!(n, 1);

        // Unauthenticated: 401.
        let req = Request::builder().uri("/auth/api-keys").body(Body::empty()).unwrap();
        assert_eq!(status(&app, req).await, StatusCode::UNAUTHORIZED);

        // Another user can neither see nor revoke it.
        let other = register_cookie(&app, "o@example.com").await;
        let res = app
            .clone()
            .oneshot(call(
                "DELETE",
                &format!("/auth/api-keys/{id}"),
                ("cookie", &other),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let res = app
            .clone()
            .oneshot(call("GET", "/auth/api-keys", ("cookie", &other), None))
            .await
            .unwrap();
        assert_eq!(json_of(res).await["items"].as_array().unwrap().len(), 0);

        // Revoke: the key stops working immediately.
        let res = app
            .clone()
            .oneshot(call("DELETE", &format!("/auth/api-keys/{id}"), sess, None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        let res = app
            .clone()
            .oneshot(call("GET", "/auth/me", ("authorization", &bearer), None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        t.teardown().await;
    }

    #[tokio::test]
    async fn api_keys_are_unique_and_random() {
        let a = new_api_key();
        let b = new_api_key();
        assert_ne!(a, b);
        assert_eq!(a.len(), 68);
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

    include!("invites_tests.rs");
    include!("twofactor_tests.rs");
    include!("cli_login_tests.rs");
}
