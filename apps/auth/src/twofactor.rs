//! Two-factor authentication (docs/admin.md, "Two-factor authentication"): enrolment, the second step of login, turning it
//! off, recovery codes, and what an administrator can do (require it, see who has it, reset someone's).
//!
//! Rules enforced here:
//! * No secret is stored in clear: `AUTH_TOTP_KEY` seals it (`totp.rs`). Without the key, enrolment is refused (503) and an
//!   account that already has 2FA cannot sign in: the check fails closed, never open.
//! * A correct password for an account with 2FA yields a short-lived *challenge*, not a session. The session exists only
//!   after a correct code (or recovery code).
//! * Wrong codes are throttled per account (5 in a row lock the account's second factor for 15 minutes, a correct code
//!   resets the count; the lock holds even against the right code) and per client address (the same token bucket as the
//!   password, on its own limiter).
//! * Disabling or regenerating recovery codes needs the password *and* a code.
//! * "Require 2FA" is a server setting. Accounts without 2FA then get a session that can only enrol (`needs_mfa_enrol`),
//!   enforced by every service's auth extractor, not by the web app.
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    routing::{get, post, put},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use cyanotype_service::{
    AdminUser, ApiError, ApiResult, EnrolUser, SessionUser, audit,
    auth::sha256,
    ratelimit::{self, LimitState, RateLimiter},
};

use crate::{
    routes::{AppState, random_token, start_session},
    totp::{self, TotpKey},
};

/// Wrong codes in a row before the account's second factor locks.
pub const MAX_FAILURES: i32 = 5;
pub const LOCK_MINUTES: i64 = 15;
/// A login challenge (a correct password waiting for its code) lasts this long.
pub const CHALLENGE_MINUTES: i64 = 5;
const ISSUER: &str = "Cyanotype";

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The second step of login, behind its own per-address limiter (a login flood must not eat the budget of people who are
/// typing a code, and the reverse).
pub fn login_routes(state: &AppState) -> Router<AppState> {
    let limit = LimitState {
        limiter: Arc::new(RateLimiter::new(state.cfg.rate_limit)),
        trusted: Arc::new(state.cfg.trusted_proxies.clone()),
    };
    Router::new()
        .route("/auth/login/2fa", post(login_second_step))
        .route_layer(middleware::from_fn_with_state(limit, ratelimit::limit))
}

/// What a person does with their own 2FA. The enrolment routes accept a session that is waiting to enrol; disabling and
/// regenerating do not (such an account has nothing to disable).
pub fn routes(state: &AppState) -> Router<AppState> {
    // Confirm, disable and regenerate also take a code: their own address limiter, on top of the per-account lock.
    let limit = LimitState {
        limiter: Arc::new(RateLimiter::new(state.cfg.rate_limit)),
        trusted: Arc::new(state.cfg.trusted_proxies.clone()),
    };
    let code_routes = Router::new()
        .route("/auth/2fa/confirm", post(confirm))
        .route("/auth/2fa/disable", post(disable))
        .route("/auth/2fa/recovery-codes", post(regenerate))
        .route_layer(middleware::from_fn_with_state(limit, ratelimit::limit));
    Router::new()
        .route("/auth/2fa", get(status))
        .route("/auth/2fa/enrol", post(enrol))
        .merge(code_routes)
}

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/admin/2fa", get(admin_overview))
        .route("/auth/admin/2fa/require", put(admin_require))
        .route("/auth/admin/users/{id}/2fa/reset", post(admin_reset))
}

/// Whether the administrator requires 2FA (the stored setting, whether or not this server can enforce it).
pub async fn required_setting(db: &PgPool) -> ApiResult<bool> {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM server_state WHERE key = 'require_2fa'")
        .fetch_optional(db)
        .await?;
    Ok(v.and_then(|v| v.get("required").and_then(Value::as_bool))
        .unwrap_or(false))
}

/// Whether sessions must enrol now: required, and this server has the key to do it. Without a key the requirement cannot
/// be met by anyone, so it is not enforced (locking everyone out would help nobody); the admin page says so.
pub async fn enforced(db: &PgPool, cfg: &crate::settings::AuthConfig) -> ApiResult<bool> {
    Ok(cfg.totp_key.is_some() && required_setting(db).await?)
}

pub async fn has_2fa(db: &PgPool, user: Uuid) -> ApiResult<bool> {
    let (n,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM user_totp WHERE user_id = $1 AND confirmed_at IS NOT NULL)")
            .bind(user)
            .fetch_one(db)
            .await?;
    Ok(n)
}

fn key_missing() -> ApiError {
    ApiError::Unavailable(
        "two-factor authentication is not available on this server: the administrator has not set AUTH_TOTP_KEY".into(),
    )
}

fn invalid_code() -> ApiError {
    ApiError::Limited {
        status: StatusCode::UNAUTHORIZED,
        code: "invalid_code",
        message: "That code is not right.".into(),
        details: json!({}),
        retry_after_secs: None,
    }
}

fn locked(secs: i64) -> ApiError {
    ApiError::Limited {
        status: StatusCode::TOO_MANY_REQUESTS,
        code: "two_factor_locked",
        message: "Too many wrong codes. Try again later.".into(),
        details: json!({"retry_after_secs": secs}),
        retry_after_secs: Some(secs.max(1) as u64),
    }
}

/// How a second factor was proven.
#[derive(Debug, PartialEq, Eq)]
pub enum Proof {
    Totp,
    Recovery { remaining: i64 },
}

/// Checks `input` (a 6-digit code or a recovery code) for `user` under the account's lock. `confirmed_only` is false only
/// for the first code of an enrolment. The failure counter is updated in the same transaction, which is committed on a
/// wrong code too (that is the point of the counter).
pub async fn verify_second_factor(
    db: &PgPool,
    key: &TotpKey,
    user: Uuid,
    input: &str,
    confirmed_only: bool,
    now: u64,
) -> ApiResult<Proof> {
    let mut tx = db.begin().await?;
    // The row lock serialises attempts on one account, so parallel guesses cannot all pass the counter.
    let row = sqlx::query(
        "SELECT secret_enc, confirmed_at IS NOT NULL AS confirmed, last_step, failed_attempts,
                COALESCE(EXTRACT(EPOCH FROM (locked_until - now()))::bigint, 0) AS lock_secs
         FROM user_totp WHERE user_id = $1 FOR UPDATE",
    )
    .bind(user)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else { return Err(invalid_code()) };
    let confirmed: bool = row.try_get("confirmed")?;
    if confirmed_only && !confirmed {
        return Err(invalid_code());
    }
    let lock_secs: i64 = row.try_get("lock_secs")?;
    if lock_secs > 0 {
        return Err(locked(lock_secs));
    }
    let sealed: Vec<u8> = row.try_get("secret_enc")?;
    let secret = key.open(user, &sealed).map_err(|e| {
        tracing::error!(error = %e, "cannot open a TOTP secret; was AUTH_TOTP_KEY changed?");
        ApiError::Unavailable(
            "two-factor authentication cannot be checked for this account (the server key changed); ask the administrator to reset it"
                .into(),
        )
    })?;
    let last_step: Option<i64> = row.try_get("last_step")?;
    let failed: i32 = row.try_get("failed_attempts")?;

    let mut proof = None;
    if totp::looks_like_totp(input) {
        let code: String = input.chars().filter(|c| !c.is_whitespace()).collect();
        if let Some(step) = totp::verify(&secret, &code, now, last_step) {
            sqlx::query("UPDATE user_totp SET last_step = $2 WHERE user_id = $1")
                .bind(user)
                .bind(step)
                .execute(&mut *tx)
                .await?;
            proof = Some(Proof::Totp);
        }
    } else if confirmed {
        let hash = key.recovery_hash(&totp::normalise_recovery(input));
        let used = sqlx::query(
            "UPDATE totp_recovery_codes SET used_at = now() WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL",
        )
        .bind(user)
        .bind(hash)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if used == 1 {
            let (remaining,): (i64,) =
                sqlx::query_as("SELECT count(*) FROM totp_recovery_codes WHERE user_id = $1 AND used_at IS NULL")
                    .bind(user)
                    .fetch_one(&mut *tx)
                    .await?;
            proof = Some(Proof::Recovery { remaining });
        }
    }
    match proof {
        Some(p) => {
            sqlx::query("UPDATE user_totp SET failed_attempts = 0, locked_until = NULL WHERE user_id = $1")
                .bind(user)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok(p)
        }
        None => {
            let failed = failed + 1;
            if failed >= MAX_FAILURES {
                sqlx::query(
                    "UPDATE user_totp SET failed_attempts = 0, locked_until = now() + make_interval(mins => $2) WHERE user_id = $1",
                )
                .bind(user)
                .bind(LOCK_MINUTES as i32)
                .execute(&mut *tx)
                .await?;
            } else {
                sqlx::query("UPDATE user_totp SET failed_attempts = $2 WHERE user_id = $1")
                    .bind(user)
                    .bind(failed)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            Err(invalid_code())
        }
    }
}

async fn check_password(db: &PgPool, user: Uuid, password: &str) -> ApiResult<()> {
    let (hash,): (String,) = sqlx::query_as("SELECT password_hash FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(db)
        .await?;
    let parsed = PasswordHash::new(&hash).map_err(|e| anyhow::anyhow!("stored hash: {e}"))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| ApiError::Limited {
            status: StatusCode::UNAUTHORIZED,
            code: "wrong_password",
            message: "That password is not right.".into(),
            details: json!({}),
            retry_after_secs: None,
        })
}

// ---- login ---------------------------------------------------------------------------------------------------------

/// Called by `login` after a correct password for an account with 2FA: the challenge the code must come with.
pub async fn begin_challenge(db: &PgPool, cfg: &crate::settings::AuthConfig, user: Uuid) -> ApiResult<Json<Value>> {
    if cfg.totp_key.is_none() {
        return Err(key_missing());
    }
    let token = random_token();
    sqlx::query("DELETE FROM login_challenges WHERE expires_at < now()")
        .execute(db)
        .await?;
    sqlx::query(
        "INSERT INTO login_challenges (token_hash, user_id, expires_at) VALUES ($1, $2, now() + make_interval(mins => $3))",
    )
    .bind(sha256(token.as_bytes()))
    .bind(user)
    .bind(CHALLENGE_MINUTES as i32)
    .execute(db)
    .await?;
    Ok(Json(json!({"mfa_required": true, "challenge": token})))
}

#[derive(Deserialize)]
struct SecondStep {
    challenge: String,
    code: String,
}

async fn login_second_step(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<SecondStep>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let key = st.cfg.totp_key.as_ref().ok_or_else(key_missing)?;
    let row: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT c.user_id, u.disabled_at IS NOT NULL FROM login_challenges c JOIN users u ON u.id = c.user_id
         WHERE c.token_hash = $1 AND c.expires_at > now()",
    )
    .bind(sha256(req.challenge.as_bytes()))
    .fetch_optional(&st.db)
    .await?;
    let (user, disabled) = row.ok_or(ApiError::Limited {
        status: StatusCode::UNAUTHORIZED,
        code: "challenge_expired",
        message: "Your sign-in timed out. Start again with your password.".into(),
        details: json!({}),
        retry_after_secs: None,
    })?;
    if disabled {
        return Err(ApiError::Unauthorized);
    }
    let proof = verify_second_factor(&st.db, key, user, &req.code, true, (st.cfg.clock)()).await?;
    sqlx::query("DELETE FROM login_challenges WHERE token_hash = $1")
        .bind(sha256(req.challenge.as_bytes()))
        .execute(&st.db)
        .await?;
    let jar = start_session(&st.db, &st.cfg, jar, user, false).await?;
    let mut body = json!({"id": user});
    if let Proof::Recovery { remaining } = proof {
        body["recovery_code_used"] = json!(true);
        body["recovery_codes_remaining"] = json!(remaining);
    }
    Ok((jar, Json(body)))
}

// ---- enrolment -----------------------------------------------------------------------------------------------------

async fn status(State(st): State<AppState>, user: EnrolUser) -> ApiResult<Json<Value>> {
    let enabled = has_2fa(&st.db, user.id).await?;
    let (remaining,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM totp_recovery_codes WHERE user_id = $1 AND used_at IS NULL")
            .bind(user.id)
            .fetch_one(&st.db)
            .await?;
    Ok(Json(json!({
        "available": st.cfg.totp_key.is_some(),
        "enabled": enabled,
        "recovery_codes_remaining": if enabled { remaining } else { 0 },
        "required": enforced(&st.db, &st.cfg).await?,
    })))
}

/// Starts (or restarts) an enrolment: a fresh secret, pending until a code confirms it. The secret is shown here, once;
/// asking again replaces it.
async fn enrol(State(st): State<AppState>, user: EnrolUser) -> ApiResult<Json<Value>> {
    let key = st.cfg.totp_key.as_ref().ok_or_else(key_missing)?;
    if has_2fa(&st.db, user.id).await? {
        return Err(ApiError::Conflict(
            "two-factor authentication is already on; turn it off first to set it up again".into(),
        ));
    }
    let (email,): (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&st.db)
        .await?;
    let secret = totp::new_secret();
    sqlx::query(
        "INSERT INTO user_totp (user_id, secret_enc) VALUES ($1, $2)
         ON CONFLICT (user_id) DO UPDATE SET secret_enc = $2, created_at = now(), confirmed_at = NULL, last_step = NULL,
                                             failed_attempts = 0, locked_until = NULL",
    )
    .bind(user.id)
    .bind(key.seal(user.id, &secret))
    .execute(&st.db)
    .await?;
    Ok(Json(json!({
        "secret": totp::base32_encode(&secret),
        "otpauth_uri": totp::otpauth_uri(ISSUER, &email, &secret),
        "issuer": ISSUER,
        "account": email,
    })))
}

#[derive(Deserialize)]
struct CodeReq {
    code: String,
}

/// Writes a fresh set of recovery codes (replacing any) and returns them in clear: the only time they are ever readable.
async fn issue_recovery_codes(db: &PgPool, key: &TotpKey, user: Uuid) -> ApiResult<Vec<String>> {
    let codes: Vec<String> = (0..totp::RECOVERY_CODES).map(|_| totp::new_recovery_code()).collect();
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM totp_recovery_codes WHERE user_id = $1")
        .bind(user)
        .execute(&mut *tx)
        .await?;
    for c in &codes {
        sqlx::query("INSERT INTO totp_recovery_codes (user_id, code_hash) VALUES ($1, $2)")
            .bind(user)
            .bind(key.recovery_hash(&totp::normalise_recovery(c)))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(codes)
}

async fn confirm(State(st): State<AppState>, user: EnrolUser, Json(req): Json<CodeReq>) -> ApiResult<Json<Value>> {
    let key = st.cfg.totp_key.as_ref().ok_or_else(key_missing)?;
    if has_2fa(&st.db, user.id).await? {
        return Err(ApiError::Conflict("two-factor authentication is already on".into()));
    }
    // Only a 6-digit code confirms an enrolment: a recovery code does not exist yet.
    if !totp::looks_like_totp(&req.code) {
        return Err(invalid_code());
    }
    verify_second_factor(&st.db, key, user.id, &req.code, false, (st.cfg.clock)()).await?;
    let codes = issue_recovery_codes(&st.db, key, user.id).await?;
    sqlx::query("UPDATE user_totp SET confirmed_at = now() WHERE user_id = $1")
        .bind(user.id)
        .execute(&st.db)
        .await?;
    // The requirement is met: this account's sessions are ordinary ones now.
    sqlx::query("UPDATE sessions SET needs_mfa_enrol = false WHERE user_id = $1")
        .bind(user.id)
        .execute(&st.db)
        .await?;
    Ok(Json(json!({"enabled": true, "recovery_codes": codes})))
}

#[derive(Deserialize)]
struct PasswordAndCode {
    password: String,
    code: String,
}

async fn disable(
    State(st): State<AppState>,
    user: SessionUser,
    Json(req): Json<PasswordAndCode>,
) -> ApiResult<Json<Value>> {
    let key = st.cfg.totp_key.as_ref().ok_or_else(key_missing)?;
    if !has_2fa(&st.db, user.id).await? {
        return Err(ApiError::Conflict("two-factor authentication is not on".into()));
    }
    check_password(&st.db, user.id, &req.password).await?;
    verify_second_factor(&st.db, key, user.id, &req.code, true, (st.cfg.clock)()).await?;
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM user_totp WHERE user_id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM totp_recovery_codes WHERE user_id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    // When 2FA is required, turning it off sends this account straight back to enrol.
    let again = enforced(&st.db, &st.cfg).await?;
    if again {
        sqlx::query("UPDATE sessions SET needs_mfa_enrol = true WHERE user_id = $1")
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"enabled": false, "enrol_required": again})))
}

async fn regenerate(
    State(st): State<AppState>,
    user: SessionUser,
    Json(req): Json<PasswordAndCode>,
) -> ApiResult<Json<Value>> {
    let key = st.cfg.totp_key.as_ref().ok_or_else(key_missing)?;
    if !has_2fa(&st.db, user.id).await? {
        return Err(ApiError::Conflict("two-factor authentication is not on".into()));
    }
    check_password(&st.db, user.id, &req.password).await?;
    verify_second_factor(&st.db, key, user.id, &req.code, true, (st.cfg.clock)()).await?;
    let codes = issue_recovery_codes(&st.db, key, user.id).await?;
    Ok(Json(json!({"recovery_codes": codes})))
}

// ---- administrators ------------------------------------------------------------------------------------------------

/// The requirement, the key's state and who has 2FA (counts of codes left, never anything secret).
async fn admin_overview(State(st): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Value>> {
    let rows = sqlx::query(
        "SELECT u.id, u.email, u.role, t.confirmed_at,
                (SELECT count(*) FROM totp_recovery_codes r WHERE r.user_id = u.id AND r.used_at IS NULL) AS codes_left
         FROM users u LEFT JOIN user_totp t ON t.user_id = u.id AND t.confirmed_at IS NOT NULL
         ORDER BY u.created_at, u.id",
    )
    .fetch_all(&st.db)
    .await?;
    let mut users = Vec::new();
    for r in rows {
        let confirmed: Option<time::OffsetDateTime> = r.try_get("confirmed_at")?;
        users.push(json!({
            "id": r.try_get::<Uuid, _>("id")?,
            "email": r.try_get::<String, _>("email")?,
            "role": r.try_get::<String, _>("role")?,
            "enabled": confirmed.is_some(),
            "since": confirmed.map(crate::routes::rfc3339),
            "recovery_codes_remaining": if confirmed.is_some() { r.try_get::<i64, _>("codes_left")? } else { 0 },
        }));
    }
    let required = required_setting(&st.db).await?;
    let key_configured = st.cfg.totp_key.is_some();
    Ok(Json(json!({
        "key_configured": key_configured,
        "required": required,
        "enforced": required && key_configured,
        "users": users,
    })))
}

#[derive(Deserialize)]
struct RequireReq {
    required: bool,
}

async fn admin_require(
    State(st): State<AppState>,
    admin: AdminUser,
    Json(req): Json<RequireReq>,
) -> ApiResult<Json<Value>> {
    if req.required {
        if st.cfg.totp_key.is_none() {
            return Err(key_missing());
        }
        // The administrator who turns it on must have it themselves, or their own next sign-in would be a dead end.
        if !has_2fa(&st.db, admin.id).await? {
            return Err(ApiError::Conflict(
                "set up two-factor authentication on your own account first (Settings > Security)".into(),
            ));
        }
    }
    let mut tx = st.db.begin().await?;
    sqlx::query(
        "INSERT INTO server_state (key, value) VALUES ('require_2fa', $1)
         ON CONFLICT (key) DO UPDATE SET value = $1, updated_at = now()",
    )
    .bind(json!({"required": req.required}))
    .execute(&mut *tx)
    .await?;
    // Live sessions follow at once: those of accounts without 2FA can only enrol now (or are released again).
    let flagged = if req.required {
        sqlx::query(
            "UPDATE sessions SET needs_mfa_enrol = true WHERE user_id NOT IN
             (SELECT user_id FROM user_totp WHERE confirmed_at IS NOT NULL)",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
    } else {
        sqlx::query("UPDATE sessions SET needs_mfa_enrol = false WHERE needs_mfa_enrol")
            .execute(&mut *tx)
            .await?
            .rows_affected()
    };
    audit::record(
        &mut *tx,
        Some(admin.id),
        if req.required { "2fa.require" } else { "2fa.unrequire" },
        Some("server"),
        None,
        json!({"sessions_changed": flagged}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"required": req.required})))
}

/// For someone who lost their authenticator and their recovery codes: removes their 2FA and signs them out everywhere. The
/// next sign-in is password-only (and, when 2FA is required, ends in enrolling again). Audit-logged.
async fn admin_reset(State(st): State<AppState>, admin: AdminUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let mut tx = st.db.begin().await?;
    let email: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let email = email.ok_or(ApiError::NotFound)?;
    let removed = sqlx::query("DELETE FROM user_totp WHERE user_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM totp_recovery_codes WHERE user_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM login_challenges WHERE user_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let (signed_out,): (i64,) =
        sqlx::query_as("WITH d AS (DELETE FROM sessions WHERE user_id = $1 RETURNING 1) SELECT count(*) FROM d")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    audit::record(
        &mut *tx,
        Some(admin.id),
        "account.2fa_reset",
        Some("user"),
        Some(&id.to_string()),
        json!({"email": email, "had_2fa": removed > 0, "sessions_ended": signed_out}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"reset": true, "had_2fa": removed > 0})))
}
