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

/// Digest used to look up session tokens and API keys. Those are 256-bit random values, so a fast hash is the right
/// tool (nothing to brute-force); passwords are never passed here, they go through argon2.
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
            // A disabled account's keys are refused (and not stamped as used): they come back when it is enabled.
            let id: Option<(Uuid,)> = sqlx::query_as(
                "UPDATE api_keys SET last_used_at = now() WHERE key_hash = $1 \
                 AND EXISTS (SELECT 1 FROM users u WHERE u.id = api_keys.user_id AND u.disabled_at IS NULL) \
                 RETURNING user_id",
            )
            .bind(sha256(key.as_bytes()))
            .fetch_optional(&db)
            .await?;
            return id.map(|(id,)| AuthUser { id }).ok_or(ApiError::Unauthorized);
        }
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(SESSION_COOKIE).ok_or(ApiError::Unauthorized)?;
        let id: Option<(Uuid,)> = sqlx::query_as(
            "SELECT s.user_id FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = $1 AND s.expires_at > now() AND u.disabled_at IS NULL",
        )
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
        let id: Option<(Uuid,)> = sqlx::query_as(
            "SELECT s.user_id FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = $1 AND s.expires_at > now() AND u.disabled_at IS NULL",
        )
        .bind(sha256(token.value().as_bytes()))
        .fetch_optional(&db)
        .await?;
        id.map(|(id,)| SessionUser { id }).ok_or(ApiError::Unauthorized)
    }
}

/// An administrator, by login session only (never an API key: a leaked CLI key must not reach the admin portal).
/// The role is read from `users` on every request, so a demotion takes effect at once. Members get 403.
pub struct AdminUser {
    pub id: Uuid,
}

impl<S> FromRequestParts<S> for AdminUser
where
    S: Send + Sync,
    PgPool: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        if parts.headers.contains_key(header::AUTHORIZATION) {
            return Err(ApiError::Forbidden(
                "this endpoint needs a login session, not an API key".into(),
            ));
        }
        let db = PgPool::from_ref(state);
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(SESSION_COOKIE).ok_or(ApiError::Unauthorized)?;
        let row: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT u.id, u.role FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = $1 AND s.expires_at > now() AND u.disabled_at IS NULL",
        )
        .bind(sha256(token.value().as_bytes()))
        .fetch_optional(&db)
        .await?;
        match row {
            None => Err(ApiError::Unauthorized),
            Some((id, role)) if role == "admin" => Ok(AdminUser { id }),
            Some(_) => Err(ApiError::Forbidden("administrators only".into())),
        }
    }
}

/// How long the Hidden view stays open after the account password was entered again.
pub const HIDDEN_UNLOCK_MINUTES: i32 = 10;

/// Whether this request's login session has the Hidden view unlocked (`sessions.hidden_unlocked_until`, set by
/// `POST /auth/hidden/unlock`). Never rejects: a request that is not unlocked simply gets `Unlock(false)`.
///
/// Always false for an API key (the CLI has no way to unlock) and for a request with an `Authorization` header, so the rule
/// is "the session cookie, unlocked, and nothing else". The check is one indexed lookup of the session row on every request,
/// so it is enforced by the server and never trusted from the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unlock(pub bool);

impl<S> FromRequestParts<S> for Unlock
where
    S: Send + Sync,
    PgPool: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        if parts.headers.contains_key(header::AUTHORIZATION) {
            return Ok(Unlock(false));
        }
        let db = PgPool::from_ref(state);
        let jar = CookieJar::from_headers(&parts.headers);
        let Some(token) = jar.get(SESSION_COOKIE) else {
            return Ok(Unlock(false));
        };
        let open: Option<bool> = sqlx::query_scalar(
            "SELECT coalesce(s.hidden_unlocked_until > now(), false) FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = $1 AND s.expires_at > now() AND u.disabled_at IS NULL",
        )
        .bind(sha256(token.value().as_bytes()))
        .fetch_optional(&db)
        .await?;
        Ok(Unlock(open.unwrap_or(false)))
    }
}

impl Unlock {
    /// `403 hidden_locked` unless the Hidden view is unlocked. For the endpoints that exist only for the Hidden view.
    pub fn require(self) -> Result<(), ApiError> {
        if self.0 {
            Ok(())
        } else {
            Err(ApiError::Limited {
                status: axum::http::StatusCode::FORBIDDEN,
                code: "hidden_locked",
                message: "The Hidden view is locked. Enter your password to open it.".into(),
                details: serde_json::json!({}),
                retry_after_secs: None,
            })
        }
    }
}
