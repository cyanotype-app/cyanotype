//! Account actions for admins (docs/admin.md, "Account actions"): disable and enable, promote and demote, one-time password
//! reset links, a person's sessions, deleting an account, and the audit log. Plus what the reset link and a person's own
//! sessions page call.
//!
//! Guards, enforced here and not in the web app: nobody disables or deletes themselves, and the last active administrator can
//! be neither demoted, disabled nor deleted. They run under one advisory lock, so two admins demoting each other at the same
//! moment cannot leave an instance without one.
use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use cyanotype_core::admin_tasks;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use cyanotype_service::{
    AdminUser, ApiError, ApiResult, SessionUser, audit,
    auth::{SESSION_COOKIE, sha256},
};

use crate::routes::{AppState, random_token, rfc3339};

/// Password reset links last this long.
pub const RESET_HOURS: i32 = 24;
/// Serialises changes that could remove the last active administrator.
const ADMIN_LOCK_KEY: i64 = 0x6379_616e_6f61_646d; // "cyanoadm"

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/admin/users/{id}/disable", post(disable))
        .route("/auth/admin/users/{id}/enable", post(enable))
        .route("/auth/admin/users/{id}/role", post(set_role))
        .route("/auth/admin/users/{id}/reset-link", post(reset_link))
        .route(
            "/auth/admin/users/{id}/sessions",
            get(user_sessions).delete(revoke_user_sessions),
        )
        .route("/auth/admin/users/{id}/delete", post(delete_account))
        .route("/auth/admin/audit", get(audit_log))
}

/// What a person may do with their own sessions.
pub fn session_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/sessions", get(my_sessions))
        .route("/auth/sessions/revoke-others", post(revoke_others))
        .route("/auth/sessions/revoke-all", post(revoke_all))
        .route("/auth/sessions/{id}", delete(revoke_one))
}

struct Target {
    email: String,
    role: String,
    disabled: bool,
}

async fn target(conn: &mut PgConnection, id: Uuid) -> ApiResult<Target> {
    let row: Option<(String, String, bool)> =
        sqlx::query_as("SELECT email, role, disabled_at IS NOT NULL FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await?;
    let (email, role, disabled) = row.ok_or(ApiError::NotFound)?;
    Ok(Target { email, role, disabled })
}

/// Takes the administrators lock for this transaction.
async fn lock_admins(tx: &mut Transaction<'_, Postgres>) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_LOCK_KEY)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Refuses the change when `t` is an active administrator and nobody else is one.
async fn ensure_not_last_admin(tx: &mut Transaction<'_, Postgres>, id: Uuid, t: &Target, doing: &str) -> ApiResult<()> {
    if t.role != "admin" || t.disabled {
        return Ok(());
    }
    let (others,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM users WHERE role = 'admin' AND disabled_at IS NULL AND id <> $1")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    if others == 0 {
        return Err(ApiError::Conflict(format!(
            "this is the last administrator: {doing} would leave nobody able to manage the server"
        )));
    }
    Ok(())
}

fn not_yourself(admin: &AdminUser, id: Uuid, doing: &str) -> ApiResult<()> {
    if admin.id == id {
        return Err(ApiError::Conflict(format!("you cannot {doing} your own account")));
    }
    Ok(())
}

async fn disable(State(st): State<AppState>, admin: AdminUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    not_yourself(&admin, id, "disable")?;
    let mut tx = st.db.begin().await?;
    lock_admins(&mut tx).await?;
    let t = target(&mut tx, id).await?;
    if t.disabled {
        return Ok(Json(json!({"disabled": true, "changed": false})));
    }
    ensure_not_last_admin(&mut tx, id, &t, "disabling it").await?;
    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
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
        "account.disable",
        Some("user"),
        Some(&id.to_string()),
        json!({"email": t.email, "sessions_ended": signed_out}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"disabled": true, "changed": true})))
}

async fn enable(State(st): State<AppState>, admin: AdminUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let mut tx = st.db.begin().await?;
    let t = target(&mut tx, id).await?;
    if !t.disabled {
        return Ok(Json(json!({"disabled": false, "changed": false})));
    }
    // An account being deleted stays disabled.
    let (deleting,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM admin_tasks WHERE kind = 'delete_account' AND params ->> 'user_id' = $1::text
                        AND state IN ('queued', 'running'))",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if deleting {
        return Err(ApiError::Conflict("this account is being deleted".into()));
    }
    sqlx::query("UPDATE users SET disabled_at = NULL WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut *tx,
        Some(admin.id),
        "account.enable",
        Some("user"),
        Some(&id.to_string()),
        json!({"email": t.email}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"disabled": false, "changed": true})))
}

#[derive(Deserialize)]
struct RoleReq {
    role: String,
}

async fn set_role(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<Uuid>,
    Json(req): Json<RoleReq>,
) -> ApiResult<Json<Value>> {
    if req.role != "admin" && req.role != "member" {
        return Err(ApiError::BadRequest("role must be admin or member".into()));
    }
    let mut tx = st.db.begin().await?;
    lock_admins(&mut tx).await?;
    let t = target(&mut tx, id).await?;
    if t.role == req.role {
        return Ok(Json(json!({"role": t.role, "changed": false})));
    }
    if req.role == "member" {
        ensure_not_last_admin(&mut tx, id, &t, "demoting it").await?;
    }
    sqlx::query("UPDATE users SET role = $2 WHERE id = $1")
        .bind(id)
        .bind(&req.role)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut *tx,
        Some(admin.id),
        "account.role",
        Some("user"),
        Some(&id.to_string()),
        json!({"email": t.email, "from": t.role, "to": req.role}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"role": req.role, "changed": true})))
}

/// A one-time link for the account to choose a new password. Earlier unused links stop working. The token is in this
/// response only; the admin copies it to the person (the server sends no email).
async fn reset_link(State(st): State<AppState>, admin: AdminUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let mut tx = st.db.begin().await?;
    let t = target(&mut tx, id).await?;
    sqlx::query(
        "UPDATE password_resets SET expires_at = now() WHERE user_id = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let token = random_token();
    let (expires,): (time::OffsetDateTime,) = sqlx::query_as(
        "INSERT INTO password_resets (user_id, token_hash, created_by, expires_at)
         VALUES ($1, $2, $3, now() + make_interval(hours => $4)) RETURNING expires_at",
    )
    .bind(id)
    .bind(sha256(token.as_bytes()))
    .bind(admin.id)
    .bind(RESET_HOURS)
    .fetch_one(&mut *tx)
    .await?;
    audit::record(
        &mut *tx,
        Some(admin.id),
        "account.reset_link",
        Some("user"),
        Some(&id.to_string()),
        json!({"email": t.email}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({
        "token": token,
        "path": format!("/reset-password?token={token}"),
        "email": t.email,
        "expires_at": rfc3339(expires),
    })))
}

// ---- the page the link opens (no login; throttled with login and register)

#[derive(Deserialize)]
pub struct TokenReq {
    pub token: String,
}

/// Whether a reset link can still be used, and for which address (the holder of the link has the secret).
pub async fn reset_check(State(st): State<AppState>, Json(req): Json<TokenReq>) -> ApiResult<Json<Value>> {
    let row: Option<(String, bool, bool, bool)> = sqlx::query_as(
        "SELECT u.email, r.used_at IS NOT NULL, r.expires_at <= now(), u.disabled_at IS NOT NULL
         FROM password_resets r JOIN users u ON u.id = r.user_id WHERE r.token_hash = $1",
    )
    .bind(sha256(req.token.trim().as_bytes()))
    .fetch_optional(&st.db)
    .await?;
    Ok(Json(match row {
        None => json!({"valid": false, "reason": "unknown"}),
        Some((_, true, _, _)) => json!({"valid": false, "reason": "used"}),
        Some((_, _, true, _)) => json!({"valid": false, "reason": "expired"}),
        Some((_, _, _, true)) => json!({"valid": false, "reason": "disabled"}),
        Some((email, ..)) => json!({"valid": true, "email": email}),
    }))
}

#[derive(Deserialize)]
pub struct ResetReq {
    pub token: String,
    pub password: String,
}

/// Sets the new password with a valid link, once, and signs the account out everywhere (including whoever knew the old one).
pub async fn reset_password(State(st): State<AppState>, Json(req): Json<ResetReq>) -> ApiResult<StatusCode> {
    if req.password.len() < 8 {
        return Err(ApiError::BadRequest("a password of 8+ characters is required".into()));
    }
    let hash = Argon2::default()
        .hash_password(req.password.as_bytes(), &SaltString::generate(&mut OsRng))
        .map_err(|e| anyhow::anyhow!("hash: {e}"))?
        .to_string();
    let mut tx = st.db.begin().await?;
    // Claim the link. The row lock makes two uses of one link race safely.
    let claimed: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE password_resets r SET used_at = now()
         WHERE r.token_hash = $1 AND r.used_at IS NULL AND r.expires_at > now()
           AND EXISTS (SELECT 1 FROM users u WHERE u.id = r.user_id AND u.disabled_at IS NULL)
         RETURNING r.user_id",
    )
    .bind(sha256(req.token.trim().as_bytes()))
    .fetch_optional(&mut *tx)
    .await?;
    let Some((user,)) = claimed else {
        return Err(ApiError::Forbidden(
            "this reset link is not valid, was already used or has expired".into(),
        ));
    };
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(user)
        .bind(hash)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut *tx,
        Some(user),
        "account.password_reset",
        Some("user"),
        Some(&user.to_string()),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- sessions

/// A session's public id: the first 8 bytes of the hash of its token (the hash is stored, never the token).
fn session_id(hash: &[u8]) -> String {
    cyanotype_core::keys::hex(&hash[..8.min(hash.len())])
}

async fn sessions_of(db: &PgPool, user: Uuid, current: Option<&[u8]>) -> ApiResult<Vec<Value>> {
    let rows: Vec<(Vec<u8>, time::OffsetDateTime, time::OffsetDateTime)> = sqlx::query_as(
        "SELECT token_hash, created_at, expires_at FROM sessions
         WHERE user_id = $1 AND expires_at > now() ORDER BY created_at DESC",
    )
    .bind(user)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(hash, created, expires)| {
            json!({
                "id": session_id(&hash),
                "created_at": rfc3339(created),
                "expires_at": rfc3339(expires),
                "current": current.is_some_and(|c| c == hash.as_slice()),
            })
        })
        .collect())
}

fn current_hash(jar: &CookieJar) -> Option<Vec<u8>> {
    jar.get(SESSION_COOKIE).map(|c| sha256(c.value().as_bytes()))
}

async fn user_sessions(State(st): State<AppState>, _admin: AdminUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    target(&mut *st.db.acquire().await?, id).await?;
    Ok(Json(json!({"items": sessions_of(&st.db, id, None).await?})))
}

/// Sign the account out everywhere.
async fn revoke_user_sessions(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let t = target(&mut *st.db.acquire().await?, id).await?;
    let n = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(id)
        .execute(&st.db)
        .await?
        .rows_affected();
    audit::record(
        &st.db,
        Some(admin.id),
        "account.sessions_revoked",
        Some("user"),
        Some(&id.to_string()),
        json!({"email": t.email, "sessions": n}),
    )
    .await?;
    Ok(Json(json!({"revoked": n})))
}

async fn my_sessions(State(st): State<AppState>, user: SessionUser, jar: CookieJar) -> ApiResult<Json<Value>> {
    let current = current_hash(&jar);
    Ok(Json(
        json!({"items": sessions_of(&st.db, user.id, current.as_deref()).await?}),
    ))
}

async fn revoke_others(State(st): State<AppState>, user: SessionUser, jar: CookieJar) -> ApiResult<Json<Value>> {
    let current = current_hash(&jar).unwrap_or_default();
    let n = sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_hash <> $2")
        .bind(user.id)
        .bind(current)
        .execute(&st.db)
        .await?
        .rows_affected();
    Ok(Json(json!({"revoked": n})))
}

/// Sign out everywhere, this browser included.
async fn revoke_all(
    State(st): State<AppState>,
    user: SessionUser,
    jar: CookieJar,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let n = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user.id)
        .execute(&st.db)
        .await?
        .rows_affected();
    Ok((
        jar.remove(Cookie::build(SESSION_COOKIE).path("/").build()),
        Json(json!({"revoked": n})),
    ))
}

/// End one session of the caller's (by the id the list gave); the current one signs this browser out.
async fn revoke_one(State(st): State<AppState>, user: SessionUser, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::NotFound);
    }
    let n = sqlx::query(
        "DELETE FROM sessions WHERE user_id = $1 AND encode(substring(token_hash from 1 for 8), 'hex') = lower($2)",
    )
    .bind(user.id)
    .bind(&id)
    .execute(&st.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- deleting an account

#[derive(Deserialize)]
struct DeleteReq {
    /// The account's email, typed by the admin as the second step.
    confirm_email: String,
}

/// Step two of deleting an account (step one, in the web app, is the dialog): the admin typed its email. The account is
/// disabled at once and signed out, and a background task removes every object (every version) and every row. Refuses
/// yourself and the last active administrator.
async fn delete_account(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<Uuid>,
    Json(req): Json<DeleteReq>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    not_yourself(&admin, id, "delete")?;
    let mut tx = st.db.begin().await?;
    lock_admins(&mut tx).await?;
    let t = target(&mut tx, id).await?;
    if !t.email.eq_ignore_ascii_case(req.confirm_email.trim()) {
        return Err(ApiError::BadRequest(
            "the email you typed does not match this account".into(),
        ));
    }
    ensure_not_last_admin(&mut tx, id, &t, "deleting it").await?;
    sqlx::query("UPDATE users SET disabled_at = coalesce(disabled_at, now()) WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let inserted: Option<(i64,)> = sqlx::query_as(
        "INSERT INTO admin_tasks (kind, requested_by, params) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(admin_tasks::KIND_DELETE_ACCOUNT)
    .bind(admin.id)
    .bind(json!({"user_id": id, "email": t.email}))
    .fetch_optional(&mut *tx)
    .await?;
    let already = inserted.is_none();
    if !already {
        audit::record(
            &mut *tx,
            Some(admin.id),
            "account.delete.requested",
            Some("user"),
            Some(&id.to_string()),
            json!({"email": t.email}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"deleting": true, "already_requested": already})),
    ))
}

// ---- the audit log

#[derive(Deserialize, Default)]
pub struct AuditQuery {
    /// Start of the action name (`invite.`, `account.delete`), or all of it.
    action: Option<String>,
    /// Part of the actor's email.
    actor: Option<String>,
    /// A target id, or part of an email in the entry's detail.
    target: Option<String>,
    /// `YYYY-MM-DD` or RFC 3339: entries from this moment on.
    from: Option<String>,
    /// The same: entries before this moment.
    to: Option<String>,
    /// Entries older than this id (the previous page's `next_before`).
    before: Option<i64>,
    limit: Option<i64>,
}

fn parse_time(v: &str) -> ApiResult<time::OffsetDateTime> {
    use time::format_description::well_known::Rfc3339;
    if let Ok(t) = time::OffsetDateTime::parse(v, &Rfc3339) {
        return Ok(t);
    }
    time::Date::parse(v, &time::macros::format_description!("[year]-[month]-[day]"))
        .map(|d| d.midnight().assume_utc())
        .map_err(|_| ApiError::BadRequest(format!("{v:?} is not a date (use YYYY-MM-DD or RFC 3339)")))
}

/// Escapes `%`, `_` and `\` for a LIKE pattern.
fn like(v: &str) -> String {
    v.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub async fn audit_log(
    State(st): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<AuditQuery>,
) -> ApiResult<Json<Value>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let non_empty = |v: &Option<String>| {
        v.as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let action = non_empty(&q.action).map(|a| format!("{}%", like(&a)));
    let actor = non_empty(&q.actor).map(|a| format!("%{}%", like(&a)));
    let target = non_empty(&q.target);
    let target_like = target.as_deref().map(|t| format!("%{}%", like(t)));
    let from = non_empty(&q.from).map(|v| parse_time(&v)).transpose()?;
    let to = non_empty(&q.to).map(|v| parse_time(&v)).transpose()?;
    type Row = (
        i64,
        time::OffsetDateTime,
        Option<Uuid>,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Value,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, at, actor_id, actor_email, action, target_type, target_id, detail FROM audit_log
         WHERE ($1::text IS NULL OR action LIKE $1)
           AND ($2::text IS NULL OR actor_email ILIKE $2)
           AND ($3::text IS NULL OR target_id = $3 OR detail ->> 'email' ILIKE $4)
           AND ($5::timestamptz IS NULL OR at >= $5)
           AND ($6::timestamptz IS NULL OR at < $6)
           AND ($7::bigint IS NULL OR id < $7)
         ORDER BY id DESC LIMIT $8",
    )
    .bind(action)
    .bind(actor)
    .bind(&target)
    .bind(target_like)
    .bind(from)
    .bind(to)
    .bind(q.before)
    .bind(limit + 1)
    .fetch_all(&st.db)
    .await?;
    let more = rows.len() as i64 > limit;
    let items: Vec<Value> = rows
        .into_iter()
        .take(limit as usize)
        .map(
            |(id, at, actor_id, actor_email, action, target_type, target_id, detail)| {
                json!({
                    "id": id, "at": rfc3339(at), "actor_id": actor_id, "actor_email": actor_email, "action": action,
                    "target_type": target_type, "target_id": target_id, "detail": detail,
                })
            },
        )
        .collect();
    let next_before = if more {
        items.last().and_then(|i| i["id"].as_i64())
    } else {
        None
    };
    let actions: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT action FROM audit_log ORDER BY action")
        .fetch_all(&st.db)
        .await?;
    Ok(Json(json!({
        "items": items,
        "next_before": next_before,
        "actions": actions.into_iter().map(|a| a.0).collect::<Vec<_>>(),
    })))
}
