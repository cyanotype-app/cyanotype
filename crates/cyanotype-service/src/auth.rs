//! Shared authentication check, so every service can authenticate requests by itself.
//! Reads the same `sessions` / `api_keys` tables the auth service writes.
use axum::{
    extract::{FromRef, FromRequestParts},
    http::{Method, StatusCode, header, request::Parts},
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

/// What an API key may do. A login session is always `Full`. Keys made before scopes existed are `Full` too.
///
/// * `Import`: the upload path and its status only (`ImportUser` routes: check, parts, complete, the library count, the
///   processing status, and `/auth/me`). What `cyanotype import` needs, so a key left on a NAS cannot read photos.
/// * `Read`: everything a `GET`/`HEAD` can reach (the library, search, downloads), nothing that changes state.
/// * `Full`: everything a session can do except what is session-only (key management, the admin portal, 2FA).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Import,
    Read,
    Full,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Read => "read",
            Self::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "import" => Some(Self::Import),
            "read" => Some(Self::Read),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    pub fn allows_import(self) -> bool {
        matches!(self, Self::Import | Self::Full)
    }

    pub fn allows_read(self) -> bool {
        matches!(self, Self::Read | Self::Full)
    }

    pub fn allows_write(self) -> bool {
        self == Self::Full
    }
}

/// Who is calling, before any scope rule: a session (`Full`) or an API key.
#[derive(Clone, Copy, Debug)]
pub struct Caller {
    pub id: Uuid,
    pub scope: Scope,
    /// A session of an account that must still enrol in two-factor authentication (the admin requires it): it may only
    /// reach `/auth/me` and the enrolment routes. Always false for API keys.
    pub mfa_pending: bool,
}

async fn authenticate(parts: &Parts, db: &PgPool) -> Result<Caller, ApiError> {
    if let Some(key) = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        // A disabled account's keys are refused (and not stamped as used): they come back when it is enabled. An expired
        // key is refused for good.
        let row: Option<(Uuid, String)> = sqlx::query_as(
            "UPDATE api_keys SET last_used_at = now() WHERE key_hash = $1 \
             AND (expires_at IS NULL OR expires_at > now()) \
             AND EXISTS (SELECT 1 FROM users u WHERE u.id = api_keys.user_id AND u.disabled_at IS NULL) \
             RETURNING user_id, scope",
        )
        .bind(sha256(key.as_bytes()))
        .fetch_optional(db)
        .await?;
        let (id, scope) = row.ok_or(ApiError::Unauthorized)?;
        // An unknown scope string (a newer server wrote it) gets the least access, never the most.
        let scope = Scope::parse(&scope).unwrap_or(Scope::Import);
        return Ok(Caller {
            id,
            scope,
            mfa_pending: false,
        });
    }
    let jar = CookieJar::from_headers(&parts.headers);
    let token = jar.get(SESSION_COOKIE).ok_or(ApiError::Unauthorized)?;
    let row: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT s.user_id, s.needs_mfa_enrol FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.token_hash = $1 AND s.expires_at > now() AND u.disabled_at IS NULL",
    )
    .bind(sha256(token.value().as_bytes()))
    .fetch_optional(db)
    .await?;
    let (id, mfa_pending) = row.ok_or(ApiError::Unauthorized)?;
    Ok(Caller {
        id,
        scope: Scope::Full,
        mfa_pending,
    })
}

fn insufficient_scope(needed: &str, have: Scope) -> ApiError {
    ApiError::Limited {
        status: StatusCode::FORBIDDEN,
        code: "insufficient_scope",
        message: format!(
            "this API key has the {} scope, which cannot do this (it needs {needed})",
            have.as_str()
        ),
        details: serde_json::json!({"scope": have.as_str()}),
        retry_after_secs: None,
    }
}

fn mfa_enrol_required() -> ApiError {
    ApiError::Limited {
        status: StatusCode::FORBIDDEN,
        code: "mfa_enrol_required",
        message: "the administrator requires two-factor authentication: set it up in Settings > Security".into(),
        details: serde_json::json!({}),
        retry_after_secs: None,
    }
}

/// Whoever is calling, with no scope rule applied (a pending 2FA enrolment included). Only for routes that decide for
/// themselves: `/auth/me`.
pub struct AnyCaller(pub Caller);

impl<S> FromRequestParts<S> for AnyCaller
where
    S: Send + Sync,
    PgPool: FromRef<S>,
{
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        Ok(Self(authenticate(parts, &PgPool::from_ref(state)).await?))
    }
}

/// The authenticated user, from a session cookie or `Authorization: Bearer <api key>`, for every route that is not an
/// import route. The rule is in the type, so a new route is covered without anyone remembering it: a `GET`/`HEAD` needs a
/// `read` or `full` key, any other method needs `full`; an `import` key reaches none of these.
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
        let c = authenticate(parts, &PgPool::from_ref(state)).await?;
        if c.mfa_pending {
            return Err(mfa_enrol_required());
        }
        let safe = matches!(parts.method, Method::GET | Method::HEAD);
        if safe && !c.scope.allows_read() {
            return Err(insufficient_scope("the read or full scope", c.scope));
        }
        if !safe && !c.scope.allows_write() {
            return Err(insufficient_scope("the full scope", c.scope));
        }
        Ok(AuthUser { id: c.id })
    }
}

/// The caller of an import route (check, upload parts, complete, the library count, the processing status): a session or a
/// key with the `import` or `full` scope. A `read` key cannot upload.
pub struct ImportUser {
    pub id: Uuid,
}

impl<S> FromRequestParts<S> for ImportUser
where
    S: Send + Sync,
    PgPool: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        let c = authenticate(parts, &PgPool::from_ref(state)).await?;
        if c.mfa_pending {
            return Err(mfa_enrol_required());
        }
        if !c.scope.allows_import() {
            return Err(insufficient_scope("the import or full scope", c.scope));
        }
        Ok(ImportUser { id: c.id })
    }
}

/// The session behind a request, refusing API keys. `allow_pending` lets a session that still has to enrol in 2FA through
/// (the enrolment routes).
async fn session_only(parts: &Parts, db: &PgPool, allow_pending: bool) -> Result<Caller, ApiError> {
    // Any Authorization header is an API-key attempt: refuse it outright, even next to a valid
    // cookie, so the rule is simply "session cookie only" and never depends on header precedence.
    if parts.headers.contains_key(header::AUTHORIZATION) {
        return Err(ApiError::Forbidden(
            "this endpoint needs a login session, not an API key".into(),
        ));
    }
    let c = authenticate(parts, db).await?;
    if c.mfa_pending && !allow_pending {
        return Err(mfa_enrol_required());
    }
    Ok(c)
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
        let c = session_only(parts, &PgPool::from_ref(state), false).await?;
        Ok(SessionUser { id: c.id })
    }
}

/// A session that may still be waiting to enrol in 2FA: the 2FA enrolment routes take this.
pub struct EnrolUser {
    pub id: Uuid,
}

impl<S> FromRequestParts<S> for EnrolUser
where
    S: Send + Sync,
    PgPool: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        let c = session_only(parts, &PgPool::from_ref(state), true).await?;
        Ok(EnrolUser { id: c.id })
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
        let row: Option<(Uuid, String, bool)> = sqlx::query_as(
            "SELECT u.id, u.role, s.needs_mfa_enrol FROM sessions s JOIN users u ON u.id = s.user_id \
             WHERE s.token_hash = $1 AND s.expires_at > now() AND u.disabled_at IS NULL",
        )
        .bind(sha256(token.value().as_bytes()))
        .fetch_optional(&db)
        .await?;
        match row {
            None => Err(ApiError::Unauthorized),
            Some((_, _, true)) => Err(mfa_enrol_required()),
            Some((id, role, false)) if role == "admin" => Ok(AdminUser { id }),
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
