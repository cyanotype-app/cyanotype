//! Invites: single-use registration links made by an admin (`REGISTRATION_MODE=invite`, docs/admin.md).
//!
//! The token (32 random bytes as hex) exists in clear only in the response that creates or re-issues it; the database
//! holds its sha256. "Show the link again" therefore means re-issuing: the old link stops working and a new one is shown.
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use cyanotype_core::limits::Limits;
use cyanotype_service::{AdminUser, ApiError, ApiResult, audit, auth::sha256};

use crate::routes::{AppState, random_token, rfc3339};

pub const DEFAULT_EXPIRY_DAYS: i64 = 7;
pub const MAX_EXPIRY_DAYS: i64 = 90;
const MAX_NOTE_CHARS: usize = 200;
/// Unused, unexpired invites at once: a guard against a runaway script, not a product limit.
const MAX_PENDING: i64 = 100;

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/admin/invites", get(list).post(create))
        .route("/auth/admin/invites/{id}", delete(revoke))
        .route("/auth/admin/invites/{id}/reissue", post(reissue))
}

/// The columns of one invite, with who made it and who used it.
const SELECT: &str = "SELECT i.id, i.note, i.created_at, i.expires_at, i.revoked_at, i.used_at, \
    COALESCE(u.email, i.used_email) AS used_email, c.email AS created_by_email, \
    i.quota_bytes, i.daily_upload_bytes, i.max_file_bytes, \
    CASE WHEN i.used_at IS NOT NULL THEN 'used' WHEN i.revoked_at IS NOT NULL THEN 'revoked' \
         WHEN i.expires_at <= now() THEN 'expired' ELSE 'pending' END AS status \
    FROM invites i LEFT JOIN users u ON u.id = i.used_by LEFT JOIN users c ON c.id = i.created_by";

#[derive(sqlx::FromRow)]
struct InviteRow {
    id: Uuid,
    note: Option<String>,
    created_at: time::OffsetDateTime,
    expires_at: time::OffsetDateTime,
    revoked_at: Option<time::OffsetDateTime>,
    used_at: Option<time::OffsetDateTime>,
    used_email: Option<String>,
    created_by_email: Option<String>,
    quota_bytes: Option<i64>,
    daily_upload_bytes: Option<i64>,
    max_file_bytes: Option<i64>,
    status: String,
}

impl InviteRow {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "note": self.note,
            "status": self.status,
            "created_at": rfc3339(self.created_at),
            "created_by": self.created_by_email,
            "expires_at": rfc3339(self.expires_at),
            "revoked_at": self.revoked_at.map(rfc3339),
            "used_at": self.used_at.map(rfc3339),
            "used_by": self.used_email,
            "limits": {
                "quota_bytes": self.quota_bytes,
                "daily_upload_bytes": self.daily_upload_bytes,
                "max_file_bytes": self.max_file_bytes,
            },
        })
    }
}

async fn fetch(db: &PgPool, id: Uuid) -> ApiResult<InviteRow> {
    sqlx::query_as::<_, InviteRow>(&format!("{SELECT} WHERE i.id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

async fn list(State(st): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Value>> {
    let rows = sqlx::query_as::<_, InviteRow>(&format!("{SELECT} ORDER BY i.created_at DESC, i.id LIMIT 500"))
        .fetch_all(&st.db)
        .await?;
    Ok(Json(
        json!({"items": rows.iter().map(InviteRow::json).collect::<Vec<_>>()}),
    ))
}

#[derive(Deserialize)]
struct NewInvite {
    note: Option<String>,
    expires_in_days: Option<i64>,
    quota_bytes: Option<i64>,
    daily_upload_bytes: Option<i64>,
    max_file_bytes: Option<i64>,
}

fn positive(name: &str, v: Option<i64>) -> ApiResult<Option<i64>> {
    match v {
        Some(n) if n <= 0 => Err(ApiError::BadRequest(format!(
            "{name} must be greater than zero (or left empty)"
        ))),
        other => Ok(other),
    }
}

fn expiry_days(v: Option<i64>) -> ApiResult<i64> {
    let d = v.unwrap_or(DEFAULT_EXPIRY_DAYS);
    if !(1..=MAX_EXPIRY_DAYS).contains(&d) {
        return Err(ApiError::BadRequest(format!(
            "expires_in_days must be between 1 and {MAX_EXPIRY_DAYS}"
        )));
    }
    Ok(d)
}

/// A new invite and its link. The token is in this response only.
fn issued(row: &InviteRow, token: &str) -> Value {
    json!({"invite": row.json(), "token": token, "path": format!("/register?invite={token}")})
}

async fn create(
    State(st): State<AppState>,
    admin: AdminUser,
    Json(req): Json<NewInvite>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let days = expiry_days(req.expires_in_days)?;
    let note = req.note.as_deref().map(str::trim).filter(|n| !n.is_empty());
    if note.is_some_and(|n| n.chars().count() > MAX_NOTE_CHARS) {
        return Err(ApiError::BadRequest(format!(
            "the note can be at most {MAX_NOTE_CHARS} characters"
        )));
    }
    let quota = positive("quota_bytes", req.quota_bytes)?;
    let daily = positive("daily_upload_bytes", req.daily_upload_bytes)?;
    let max_file = positive("max_file_bytes", req.max_file_bytes)?;
    let (pending,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM invites WHERE used_at IS NULL AND revoked_at IS NULL AND expires_at > now()",
    )
    .fetch_one(&st.db)
    .await?;
    if pending >= MAX_PENDING {
        return Err(ApiError::Conflict(format!(
            "{MAX_PENDING} invites are waiting to be used; revoke some first"
        )));
    }
    let token = random_token();
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO invites (token_hash, created_by, note, expires_at, quota_bytes, daily_upload_bytes, max_file_bytes) \
         VALUES ($1, $2, $3, now() + make_interval(days => $4), $5, $6, $7) RETURNING id",
    )
    .bind(sha256(token.as_bytes()))
    .bind(admin.id)
    .bind(note)
    .bind(days as i32)
    .bind(quota)
    .bind(daily)
    .bind(max_file)
    .fetch_one(&st.db)
    .await?;
    audit::record(
        &st.db,
        Some(admin.id),
        "invite.create",
        Some("invite"),
        Some(&id.to_string()),
        json!({"expires_in_days": days, "note": note, "quota_bytes": quota, "daily_upload_bytes": daily, "max_file_bytes": max_file}),
    )
    .await?;
    let row = fetch(&st.db, id).await?;
    Ok((StatusCode::CREATED, Json(issued(&row, &token))))
}

#[derive(Deserialize, Default)]
struct Reissue {
    expires_in_days: Option<i64>,
}

/// A new link for an invite that has not been used or revoked: the old token stops working. The expiry is kept unless the
/// invite has expired or `expires_in_days` is given, which restarts it from now.
async fn reissue(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<Uuid>,
    body: Option<Json<Reissue>>,
) -> ApiResult<Json<Value>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let restart = req.expires_in_days.is_some();
    let days = expiry_days(req.expires_in_days)?;
    let token = random_token();
    let n = sqlx::query(
        "UPDATE invites SET token_hash = $2, \
           expires_at = CASE WHEN $3 OR expires_at <= now() THEN now() + make_interval(days => $4) ELSE expires_at END \
         WHERE id = $1 AND used_at IS NULL AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(sha256(token.as_bytes()))
    .bind(restart)
    .bind(days as i32)
    .execute(&st.db)
    .await?
    .rows_affected();
    if n == 0 {
        fetch(&st.db, id).await?; // 404 for an unknown id
        return Err(ApiError::Conflict("this invite was already used or revoked".into()));
    }
    audit::record(
        &st.db,
        Some(admin.id),
        "invite.reissue",
        Some("invite"),
        Some(&id.to_string()),
        json!({}),
    )
    .await?;
    Ok(Json(issued(&fetch(&st.db, id).await?, &token)))
}

/// Revoking is final and idempotent; a used invite cannot be revoked (the account exists now).
async fn revoke(State(st): State<AppState>, admin: AdminUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let row = fetch(&st.db, id).await?;
    if row.used_at.is_some() {
        return Err(ApiError::Conflict("this invite was already used".into()));
    }
    let n =
        sqlx::query("UPDATE invites SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL AND used_at IS NULL")
            .bind(id)
            .execute(&st.db)
            .await?
            .rows_affected();
    if n == 1 {
        audit::record(
            &st.db,
            Some(admin.id),
            "invite.revoke",
            Some("invite"),
            Some(&id.to_string()),
            json!({}),
        )
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// What a registration takes from an invite.
pub struct Consumed {
    pub id: Uuid,
    /// The starting limits the invite presets (unset fields: the server default applies).
    pub limits: Limits,
}

/// Why a token cannot be used, for the message shown to the person holding the link.
pub async fn refusal(tx: &mut Transaction<'_, Postgres>, token: &str) -> ApiResult<ApiError> {
    type Row = (Option<time::OffsetDateTime>, Option<time::OffsetDateTime>);
    let row: Option<Row> = sqlx::query_as("SELECT used_at, revoked_at FROM invites WHERE token_hash = $1")
        .bind(sha256(token.trim().as_bytes()))
        .fetch_optional(&mut **tx)
        .await?;
    let why = match row {
        None => "this invite link is not valid",
        Some((Some(_), _)) => "this invite link was already used",
        Some((None, Some(_))) => "this invite link was revoked",
        Some(_) => "this invite link has expired",
    };
    Ok(ApiError::Forbidden(why.into()))
}

/// Claims the invite for the registration in `tx`: marks it used if it is unused, unrevoked and unexpired. The row lock
/// makes two registrations with one token race safely (the second finds it used). `None` if it cannot be used.
pub async fn consume(tx: &mut Transaction<'_, Postgres>, token: &str) -> ApiResult<Option<Consumed>> {
    #[derive(sqlx::FromRow)]
    struct Claimed {
        id: Uuid,
        quota_bytes: Option<i64>,
        daily_upload_bytes: Option<i64>,
        max_file_bytes: Option<i64>,
    }
    let row: Option<Claimed> = sqlx::query_as(
        "UPDATE invites SET used_at = now() \
         WHERE token_hash = $1 AND used_at IS NULL AND revoked_at IS NULL AND expires_at > now() \
         RETURNING id, quota_bytes, daily_upload_bytes, max_file_bytes",
    )
    .bind(sha256(token.trim().as_bytes()))
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|c| Consumed {
        id: c.id,
        limits: Limits {
            quota_bytes: c.quota_bytes,
            daily_upload_bytes: c.daily_upload_bytes,
            max_file_bytes: c.max_file_bytes,
        },
    }))
}

/// `POST /auth/invites/check` body.
#[derive(Deserialize)]
pub struct CheckReq {
    pub token: String,
}

/// Whether a token can still be used, so the register page can say so before the form is filled in. Reveals nothing but
/// the reason (the note and limits are the admin's).
pub async fn check(State(st): State<AppState>, Json(req): Json<CheckReq>) -> ApiResult<Json<Value>> {
    let row: Option<(String, time::OffsetDateTime)> = sqlx::query_as(
        "SELECT CASE WHEN used_at IS NOT NULL THEN 'used' WHEN revoked_at IS NOT NULL THEN 'revoked' \
                     WHEN expires_at <= now() THEN 'expired' ELSE 'pending' END, expires_at \
         FROM invites WHERE token_hash = $1",
    )
    .bind(sha256(req.token.trim().as_bytes()))
    .fetch_optional(&st.db)
    .await?;
    Ok(Json(match row {
        None => json!({"valid": false, "reason": "unknown"}),
        Some((s, exp)) if s == "pending" => json!({"valid": true, "expires_at": rfc3339(exp)}),
        Some((s, _)) => json!({"valid": false, "reason": s}),
    }))
}
