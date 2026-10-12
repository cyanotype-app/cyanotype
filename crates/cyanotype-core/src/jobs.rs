//! Postgres job queue. Enqueue is idempotent per (kind, asset, version);
//! claiming uses `FOR UPDATE SKIP LOCKED` so any number of workers can run.
use anyhow::Result;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

pub const KIND_METADATA: &str = "metadata";
pub const KIND_THUMBNAIL: &str = "thumbnail";
pub const KIND_GEOCODE: &str = "geocode";
pub const KIND_EMBED: &str = "embed";

/// Jobs enqueued for every new asset. Later pipeline stages are enqueued by earlier ones
/// or by the milestones that implement them.
pub const DEFAULT_PIPELINE: &[&str] = &[KIND_METADATA, KIND_THUMBNAIL];

/// A running job older than this is assumed to belong to a dead worker.
const STALE_AFTER_SECS: i64 = 600;
const MAX_ATTEMPTS: i32 = 5;

#[derive(Debug, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub asset_id: Uuid,
    pub version: i32,
    pub attempts: i32,
}

pub async fn enqueue(db: impl PgExecutor<'_>, kind: &str, asset_id: Uuid, version: i32) -> Result<()> {
    sqlx::query("INSERT INTO jobs (kind, asset_id, version) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(kind)
        .bind(asset_id)
        .bind(version)
        .execute(db)
        .await?;
    Ok(())
}

/// Claim one job of `kind`, also reclaiming jobs whose worker died mid-run.
pub async fn claim(pool: &PgPool, kind: &str) -> Result<Option<Job>> {
    Ok(sqlx::query_as::<_, Job>(
        "UPDATE jobs SET state = 'running', locked_at = now(), attempts = attempts + 1
         WHERE id = (
             SELECT id FROM jobs
             WHERE kind = $1 AND attempts < $3 AND run_after <= now()
               AND (state = 'queued' OR (state = 'running' AND locked_at < now() - make_interval(secs => $2)))
             ORDER BY run_after, id
             FOR UPDATE SKIP LOCKED
             LIMIT 1)
         RETURNING id, kind, asset_id, version, attempts",
    )
    .bind(kind)
    .bind(STALE_AFTER_SECS as f64)
    .bind(MAX_ATTEMPTS)
    .fetch_optional(pool)
    .await?)
}

pub async fn complete(pool: &PgPool, id: i64) -> Result<()> {
    sqlx::query("UPDATE jobs SET state = 'done', finished_at = now(), last_error = NULL WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Requeue with exponential backoff, or mark failed once attempts are exhausted.
pub async fn fail(pool: &PgPool, job: &Job, error: &str) -> Result<()> {
    let exhausted = job.attempts >= MAX_ATTEMPTS;
    sqlx::query(
        "UPDATE jobs SET state = CASE WHEN $2 THEN 'failed' ELSE 'queued' END,
             last_error = $3, locked_at = NULL,
             run_after = now() + make_interval(secs => $4)
         WHERE id = $1",
    )
    .bind(job.id)
    .bind(exhausted)
    .bind(error)
    .bind(f64::from(2_i32.pow(job.attempts.clamp(1, 10) as u32)) * 5.0)
    .execute(pool)
    .await?;
    Ok(())
}
