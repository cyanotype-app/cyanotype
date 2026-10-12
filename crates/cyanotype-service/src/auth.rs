//! Shared authentication check, so every service can authenticate requests by itself.
//! Reads the same `sessions` / `api_keys` tables the auth service writes.
use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts},
};
use axum_extra::extract::cookie::CookieJar;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ApiError;

pub const SESSION_COOKIE: &str = "cyanotype_session";

pub fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

/// The authenticated user, from a session cookie or `Authorization: Bearer <api key>`.
/// Works with any app state that can yield a `PgPool` via `FromRef`.
pub struct AuthUser {
    pub id: Uuid,
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    PgPool: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        let db = PgPool::from_ref(state);
        if let Some(key) = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        {
            let id: Option<(Uuid,)> =
                sqlx::query_as("UPDATE api_keys SET last_used_at = now() WHERE key_hash = $1 RETURNING user_id")
                    .bind(sha256(key.as_bytes()))
                    .fetch_optional(&db)
                    .await?;
            return id.map(|(id,)| AuthUser { id }).ok_or(ApiError::Unauthorized);
        }
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(SESSION_COOKIE).ok_or(ApiError::Unauthorized)?;
        let id: Option<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > now()")
                .bind(sha256(token.value().as_bytes()))
                .fetch_optional(&db)
                .await?;
        id.map(|(id,)| AuthUser { id }).ok_or(ApiError::Unauthorized)
    }
}

/// Like [`AuthUser`], but only a login *session* is accepted, never an API key. Use it for anything
/// that must not be reachable with a leaked key (e.g. minting or listing API keys).
pub struct SessionUser {
    pub id: Uuid,
}

impl<S> FromRequestParts<S> for SessionUser
where
    S: Send + Sync,
    PgPool: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        // Any Authorization header is an API-key attempt: refuse it outright, even next to a valid
        // cookie, so the rule is simply "session cookie only" and never depends on header precedence.
        if parts.headers.contains_key(header::AUTHORIZATION) {
            return Err(ApiError::Forbidden(
                "this endpoint needs a login session, not an API key".into(),
            ));
        }
        let db = PgPool::from_ref(state);
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(SESSION_COOKIE).ok_or(ApiError::Unauthorized)?;
        let id: Option<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > now()")
                .bind(sha256(token.value().as_bytes()))
                .fetch_optional(&db)
                .await?;
        id.map(|(id,)| SessionUser { id }).ok_or(ApiError::Unauthorized)
    }
}
