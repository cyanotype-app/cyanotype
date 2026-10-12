use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
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

const SESSION_DAYS: i64 = 30;

pub fn routes() -> Router<PgPool> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
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

async fn start_session(db: &PgPool, jar: CookieJar, user_id: Uuid) -> ApiResult<CookieJar> {
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
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::days(SESSION_DAYS))
        .build();
    Ok(jar.add(cookie))
}

async fn register(
    State(db): State<PgPool>,
    jar: CookieJar,
    Json(c): Json<Credentials>,
) -> ApiResult<(StatusCode, CookieJar, Json<Value>)> {
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
    let row: Option<(Uuid,)> =
        sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ($1, $2) ON CONFLICT DO NOTHING RETURNING id")
            .bind(&email)
            .bind(hash)
            .fetch_optional(&db)
            .await?;
    let (id,) = row.ok_or_else(|| ApiError::Conflict("email already registered".into()))?;
    let jar = start_session(&db, jar, id).await?;
    Ok((StatusCode::CREATED, jar, Json(json!({"id": id, "email": email}))))
}

async fn login(
    State(db): State<PgPool>,
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
    let jar = start_session(&db, jar, id).await?;
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
