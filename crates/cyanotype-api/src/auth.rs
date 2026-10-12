use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json, Router,
    extract::{FromRequestParts, State},
    http::{StatusCode, header, request::Parts},
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    AppState,
    error::{ApiError, ApiResult},
};

const COOKIE: &str = "cyanotype_session";
const SESSION_DAYS: i64 = 30;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
}

fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    cyanotype_core::keys::hex(&b)
}

/// The authenticated user, from a session cookie or `Authorization: Bearer <api key>`.
pub struct AuthUser {
    pub id: Uuid,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if let Some(key) = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        {
            let id: Option<(Uuid,)> =
                sqlx::query_as("UPDATE api_keys SET last_used_at = now() WHERE key_hash = $1 RETURNING user_id")
                    .bind(sha256(key.as_bytes()))
                    .fetch_optional(&state.db)
                    .await?;
            return id.map(|(id,)| AuthUser { id }).ok_or(ApiError::Unauthorized);
        }
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(COOKIE).ok_or(ApiError::Unauthorized)?;
        let id: Option<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > now()")
                .bind(sha256(token.value().as_bytes()))
                .fetch_optional(&state.db)
                .await?;
        id.map(|(id,)| AuthUser { id }).ok_or(ApiError::Unauthorized)
    }
}

#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
}

async fn start_session(state: &AppState, jar: CookieJar, user_id: Uuid) -> ApiResult<CookieJar> {
    let token = random_token();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, now() + make_interval(days => $3))",
    )
    .bind(sha256(token.as_bytes()))
    .bind(user_id)
    .bind(SESSION_DAYS as i32)
    .execute(&state.db)
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
    State(state): State<AppState>,
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
            .fetch_optional(&state.db)
            .await?;
    let (id,) = row.ok_or_else(|| ApiError::Conflict("email already registered".into()))?;
    let jar = start_session(&state, jar, id).await?;
    Ok((StatusCode::CREATED, jar, Json(json!({"id": id, "email": email}))))
}

async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(c): Json<Credentials>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE lower(email) = lower($1)")
            .bind(c.email.trim())
            .fetch_optional(&state.db)
            .await?;
    // Same error for unknown user and bad password.
    let (id, hash) = row.ok_or(ApiError::Unauthorized)?;
    let parsed = PasswordHash::new(&hash).map_err(|e| anyhow::anyhow!("stored hash: {e}"))?;
    Argon2::default()
        .verify_password(c.password.as_bytes(), &parsed)
        .map_err(|_| ApiError::Unauthorized)?;
    let jar = start_session(&state, jar, id).await?;
    Ok((jar, Json(json!({"id": id}))))
}

async fn logout(State(state): State<AppState>, jar: CookieJar) -> ApiResult<(CookieJar, StatusCode)> {
    if let Some(c) = jar.get(COOKIE) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(sha256(c.value().as_bytes()))
            .execute(&state.db)
            .await?;
    }
    Ok((
        jar.remove(Cookie::build(COOKIE).path("/").build()),
        StatusCode::NO_CONTENT,
    ))
}

async fn me(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let (email,): (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({"id": user.id, "email": email})))
}
