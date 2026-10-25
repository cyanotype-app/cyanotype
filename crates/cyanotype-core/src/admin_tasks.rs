//! Background work an admin asked for (`admin_tasks`, migration 0017): the API enqueues a row, the worker claims and runs
//! it, and the page reads the state and result. One row per request, kept as the history.
use anyhow::Result;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

pub const KIND_BUCKET_CHECK: &str = "bucket_check";
/// The storage integrity check (`integrity`): the nightly schedule queues it, Admin > Health queues it on demand; `params` are
/// `{"trigger": "schedule"}` or `{"trigger": "manual"}`.
pub const KIND_INTEGRITY_CHECK: &str = "integrity_check";
/// Deleting an account for good (`accounts::delete_account`); `params` are `{"user_id", "email"}`.
pub const KIND_DELETE_ACCOUNT: &str = "delete_account";

/// A task `running` for longer than this is taken to belong to a worker that died, and is claimed again.
pub const STALE_AFTER_MINUTES: i32 = 120;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Task {
    pub id: i64,
    pub kind: String,
    pub state: String,
    pub params: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub requested_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<time::OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<time::OffsetDateTime>,
}

const COLS: &str = "id, kind, state, params, result, error, requested_by, created_at, started_at, finished_at";

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Queues a task. For a kind that allows one at a time (the bucket check) a second request while one is open returns
/// the open one instead (`false` in the pair: nothing new was queued).
pub async fn enqueue(
    db: &PgPool,
    kind: &str,
    requested_by: Option<Uuid>,
    params: serde_json::Value,
) -> Result<(Task, bool)> {
    let inserted: Option<Task> = sqlx::query_as(&format!(
        "INSERT INTO admin_tasks (kind, requested_by, params) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING RETURNING {COLS}"
    ))
    .bind(kind)
    .bind(requested_by)
    .bind(params)
    .fetch_optional(db)
    .await?;
    if let Some(t) = inserted {
        return Ok((t, true));
    }
    let open: Task = sqlx::query_as(&format!(
        "SELECT {COLS} FROM admin_tasks WHERE kind = $1 AND state IN ('queued', 'running') ORDER BY id DESC LIMIT 1"
    ))
    .bind(kind)
    .fetch_one(db)
    .await?;
    Ok((open, false))
}

/// The most recent task of a kind.
pub async fn latest(db: &PgPool, kind: &str) -> Result<Option<Task>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM admin_tasks WHERE kind = $1 ORDER BY id DESC LIMIT 1"
    ))
    .bind(kind)
    .fetch_optional(db)
    .await?)
}

/// Takes the oldest queued task (or an abandoned running one) of one of `kinds` and marks it running.
pub async fn claim(db: &PgPool, kinds: &[&str]) -> Result<Option<Task>> {
    Ok(sqlx::query_as(&format!(
        "UPDATE admin_tasks SET state = 'running', started_at = now()
         WHERE id = (SELECT id FROM admin_tasks
                     WHERE kind = ANY($1)
                       AND (state = 'queued' OR (state = 'running' AND started_at < now() - make_interval(mins => $2)))
                     ORDER BY id FOR UPDATE SKIP LOCKED LIMIT 1)
         RETURNING {COLS}"
    ))
    .bind(kinds)
    .bind(STALE_AFTER_MINUTES)
    .fetch_optional(db)
    .await?)
}

pub async fn finish(db: &PgPool, id: i64, outcome: Result<serde_json::Value, String>) -> Result<()> {
    let (state, result, error) = match outcome {
        Ok(v) => ("done", Some(v), None),
        Err(e) => ("failed", None, Some(e)),
    };
    sqlx::query("UPDATE admin_tasks SET state = $2, result = $3, error = $4, finished_at = now() WHERE id = $1")
        .bind(id)
        .bind(state)
        .bind(result)
        .bind(error)
        .execute(db)
        .await?;
    Ok(())
}
