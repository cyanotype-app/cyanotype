//! Storage usage: the caller's own numbers (`GET /assets/usage`, Settings > Storage) and the admin's view of every account
//! (`GET /assets/admin/accounts`, the bucket check). Admins get counts and sizes, never anyone's photos or file names.
use axum::extract::Path;
use axum::{Json, extract::State};
use cyanotype_core::{
    admin_tasks,
    limits::{self, Limits},
    usage,
};
use cyanotype_service::{AdminUser, ApiError, ApiResult, AuthUser, audit};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;

#[derive(Serialize, sqlx::FromRow)]
struct Uploads {
    count_24h: i64,
    bytes_24h: i64,
    count_30d: i64,
    bytes_30d: i64,
}

const UPLOADS: &str = "SELECT
    count(*) FILTER (WHERE at > now() - interval '24 hours') AS count_24h,
    coalesce(sum(bytes) FILTER (WHERE at > now() - interval '24 hours'), 0)::bigint AS bytes_24h,
    count(*) FILTER (WHERE at > now() - interval '30 days') AS count_30d,
    coalesce(sum(bytes) FILTER (WHERE at > now() - interval '30 days'), 0)::bigint AS bytes_30d
    FROM upload_log WHERE owner_id = $1";

async fn uploads_of(db: &PgPool, user: Uuid) -> ApiResult<Uploads> {
    Ok(sqlx::query_as(UPLOADS).bind(user).fetch_one(db).await?)
}

/// The usage of one account as the API shows it. `derived_complete` is false while some files' thumbnails have not been
/// measured yet, which makes `derived_bytes` a lower bound.
fn usage_json(u: &usage::Usage) -> Value {
    json!({
        "originals_bytes": u.originals_bytes,
        "trash_bytes": u.trash_bytes,
        "stored_bytes": u.stored_bytes(),
        "derived_bytes": u.derived_bytes,
        "derived_complete": u.derived_files >= u.file_count + u.trash_count,
        "photo_count": u.photo_count,
        "raw_count": u.raw_count,
        "file_count": u.file_count,
        "trash_count": u.trash_count,
    })
}

/// `GET /assets/usage`: what the caller stores and uploaded lately. Only their own rows.
pub(super) async fn mine(State(s): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let u = usage::of(&s.db, user.id).await?;
    let up = uploads_of(&s.db, user.id).await?;
    let mut body = usage_json(&u);
    body["uploads"] = serde_json::to_value(up).map_err(anyhow::Error::from)?;
    // The account's limits (null: none) and the rolling day as the daily cap counts it.
    body["limits"] = serde_json::to_value(limits::of(&s.db, user.id).await?).map_err(anyhow::Error::from)?;
    let (used, resumes) = limits::daily_state(&s.db, user.id).await?;
    body["daily"] = json!({
        "used_bytes": used,
        "oldest_rolls_out_at": resumes.map(|t| t.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()),
    });
    Ok(Json(body))
}

#[derive(Serialize, sqlx::FromRow)]
struct Account {
    id: Uuid,
    email: String,
    role: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    /// The latest login, API key use or upload (there is no per-request "last seen").
    #[serde(with = "time::serde::rfc3339::option")]
    last_active: Option<OffsetDateTime>,
    originals_bytes: i64,
    trash_bytes: i64,
    derived_bytes: i64,
    photo_count: i64,
    raw_count: i64,
    file_count: i64,
    trash_count: i64,
    derived_files: i64,
    uploads_24h_count: i64,
    uploads_24h_bytes: i64,
    uploads_30d_count: i64,
    uploads_30d_bytes: i64,
    albums: i64,
    shares: i64,
    api_keys: i64,
    quota_bytes: Option<i64>,
    daily_upload_bytes: Option<i64>,
    max_file_bytes: Option<i64>,
    /// Disabled accounts cannot sign in and their API keys are refused.
    #[serde(with = "time::serde::rfc3339::option")]
    disabled_at: Option<OffsetDateTime>,
    /// The state of the latest deletion of this account (`queued`, `running`, `failed`), if one was requested.
    deletion: Option<String>,
    deletion_error: Option<String>,
}

/// `GET /assets/admin/accounts`: every account with its numbers, the server totals, the last reconcile and the last bucket check.
pub(super) async fn accounts(State(s): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Value>> {
    let rows: Vec<Account> = sqlx::query_as(
        "SELECT u.id, u.email, u.role, u.created_at,
                greatest((SELECT max(created_at) FROM sessions WHERE user_id = u.id),
                         (SELECT max(last_used_at) FROM api_keys WHERE user_id = u.id),
                         (SELECT max(at) FROM upload_log WHERE owner_id = u.id)) AS last_active,
                coalesce(m.originals_bytes, 0) AS originals_bytes, coalesce(m.trash_bytes, 0) AS trash_bytes,
                coalesce(m.derived_bytes, 0) AS derived_bytes, coalesce(m.photo_count, 0) AS photo_count,
                coalesce(m.raw_count, 0) AS raw_count, coalesce(m.file_count, 0) AS file_count,
                coalesce(m.trash_count, 0) AS trash_count, coalesce(m.derived_files, 0) AS derived_files,
                (SELECT count(*) FROM upload_log WHERE owner_id = u.id AND at > now() - interval '24 hours') AS uploads_24h_count,
                (SELECT coalesce(sum(bytes), 0)::bigint FROM upload_log WHERE owner_id = u.id AND at > now() - interval '24 hours') AS uploads_24h_bytes,
                (SELECT count(*) FROM upload_log WHERE owner_id = u.id AND at > now() - interval '30 days') AS uploads_30d_count,
                (SELECT coalesce(sum(bytes), 0)::bigint FROM upload_log WHERE owner_id = u.id AND at > now() - interval '30 days') AS uploads_30d_bytes,
                (SELECT count(*) FROM albums WHERE owner_id = u.id) AS albums,
                (SELECT count(*) FROM shares WHERE owner_id = u.id) AS shares,
                (SELECT count(*) FROM api_keys WHERE user_id = u.id) AS api_keys,
                u.quota_bytes, u.daily_upload_bytes, u.max_file_bytes, u.disabled_at,
                t.state AS deletion, t.error AS deletion_error
         FROM users u
         LEFT JOIN LATERAL (SELECT state, error FROM admin_tasks
                            WHERE kind = 'delete_account' AND params ->> 'user_id' = u.id::text
                            ORDER BY id DESC LIMIT 1) t ON true LEFT JOIN user_usage m ON m.user_id = u.id
         ORDER BY u.created_at, u.id",
    )
    .fetch_all(&s.db)
    .await?;
    let sum = |f: fn(&Account) -> i64| rows.iter().map(f).sum::<i64>();
    let totals = json!({
        "accounts": rows.len(),
        "originals_bytes": sum(|a| a.originals_bytes),
        "trash_bytes": sum(|a| a.trash_bytes),
        "derived_bytes": sum(|a| a.derived_bytes),
        "derived_complete": sum(|a| a.derived_files) >= sum(|a| a.file_count + a.trash_count),
        "photo_count": sum(|a| a.photo_count),
        "raw_count": sum(|a| a.raw_count),
        "uploads_24h_count": sum(|a| a.uploads_24h_count),
        "uploads_24h_bytes": sum(|a| a.uploads_24h_bytes),
        "uploads_30d_count": sum(|a| a.uploads_30d_count),
        "uploads_30d_bytes": sum(|a| a.uploads_30d_bytes),
    });
    let reconcile: Option<(Value, OffsetDateTime)> =
        sqlx::query_as("SELECT value, updated_at FROM server_state WHERE key = 'usage_reconcile'")
            .fetch_optional(&s.db)
            .await?;
    let reconcile = reconcile.map(|(mut v, at)| {
        v["at"] = json!(
            at.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        );
        v
    });
    let bucket = admin_tasks::latest(&s.db, admin_tasks::KIND_BUCKET_CHECK).await?;
    Ok(Json(json!({
        "items": rows,
        "totals": totals,
        "reconcile": reconcile,
        "bucket_check": bucket,
        "default_limits": limits::server_default(&s.db).await?,
    })))
}

fn bad(e: anyhow::Error) -> ApiError {
    ApiError::BadRequest(e.to_string())
}

/// `PUT /assets/admin/accounts/{id}/limits`: set an account's limits (a field left out or null is unlimited). Audited
/// with the old and new values.
pub(super) async fn set_limits(
    State(s): State<AppState>,
    admin: AdminUser,
    Path(id): Path<Uuid>,
    Json(new): Json<Limits>,
) -> ApiResult<Json<Limits>> {
    new.validate().map_err(bad)?;
    let before: Option<(String,)> = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&s.db)
        .await?;
    let Some((email,)) = before else {
        return Err(ApiError::NotFound);
    };
    let old = limits::of(&s.db, id).await?;
    limits::set(&s.db, id, new).await?;
    audit::record(
        &s.db,
        Some(admin.id),
        "account.limits",
        Some("user"),
        Some(&id.to_string()),
        json!({"email": email, "before": old, "after": new}),
    )
    .await?;
    Ok(Json(new))
}

/// `GET /assets/admin/default-limits`.
pub(super) async fn default_limits(State(s): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Limits>> {
    Ok(Json(limits::server_default(&s.db).await?))
}

/// `PUT /assets/admin/default-limits`: the limits accounts created from now on start with (invites may preset their own in
/// front of it; the first account of an instance gets none). Existing accounts are not changed. Audited.
pub(super) async fn set_default_limits(
    State(s): State<AppState>,
    admin: AdminUser,
    Json(new): Json<Limits>,
) -> ApiResult<Json<Limits>> {
    new.validate().map_err(bad)?;
    let old = limits::server_default(&s.db).await?;
    limits::set_server_default(&s.db, new).await?;
    audit::record(
        &s.db,
        Some(admin.id),
        "settings.default_limits",
        None,
        None,
        json!({"before": old, "after": new}),
    )
    .await?;
    Ok(Json(new))
}

/// `POST /assets/admin/bucket-check`: queue the bucket listing (the worker runs it). Returns the task; if one is already
/// queued or running, that one (202 either way, the page polls `GET`).
pub(super) async fn start_bucket_check(
    State(s): State<AppState>,
    admin: AdminUser,
) -> ApiResult<(axum::http::StatusCode, Json<Value>)> {
    let (task, created) =
        admin_tasks::enqueue(&s.db, admin_tasks::KIND_BUCKET_CHECK, Some(admin.id), json!({})).await?;
    if created {
        audit::record(
            &s.db,
            Some(admin.id),
            "storage.bucket_check",
            Some("task"),
            Some(&task.id.to_string()),
            json!({}),
        )
        .await?;
    }
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(json!({"task": task, "already_running": !created})),
    ))
}

/// `GET /assets/admin/bucket-check`: the latest bucket check (null before the first).
pub(super) async fn bucket_check(State(s): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Value>> {
    let task = admin_tasks::latest(&s.db, admin_tasks::KIND_BUCKET_CHECK).await?;
    Ok(Json(json!({"task": task})))
}
