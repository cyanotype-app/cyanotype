//! Image embeddings in `asset_embeddings` and the jobs that produce them. The model itself lives in the
//! `cyanotype-clip` crate (this crate must not link ONNX Runtime), so callers pass the model id and version in.
use anyhow::Result;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::jobs;

/// pgvector's text input format (`[0.1,0.2,...]`), for `vector` and `halfvec` alike: bind as text, cast in SQL.
pub fn pg_literal(v: &[f32]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(v.len() * 10 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "{x}");
    }
    s.push(']');
    s
}

/// Store a photo's vector for `(model, version)` and drop what an older model left for it, in one statement, so a swap
/// never leaves two generations of vectors for one photo (the table is one HNSW index over all rows).
pub async fn store(db: &PgPool, asset_id: Uuid, model: &str, version: i32, embedding: &[f32]) -> Result<()> {
    sqlx::query(
        "WITH gone AS (DELETE FROM asset_embeddings WHERE asset_id = $1 AND model <> $2)
         INSERT INTO asset_embeddings (asset_id, model, version, embedding)
         VALUES ($1, $2, $3, $4::text::halfvec)
         ON CONFLICT (asset_id, model) DO UPDATE SET version = EXCLUDED.version, embedding = EXCLUDED.embedding,
             created_at = now()",
    )
    .bind(asset_id)
    .bind(model)
    .bind(version)
    .bind(pg_literal(embedding))
    .execute(db)
    .await?;
    Ok(())
}

/// Queue an embed job for every visible photo that has derived images and no vector for `(model, version)`: the
/// catch-up after a model swap, after a photo's hidden-RAW status ended (unpair), and for jobs that were lost. A job
/// already queued, running or failed is left alone; one that finished without producing a vector is requeued.
/// Returns how many jobs were queued.
pub async fn enqueue_missing(db: impl PgExecutor<'_>, model: &str, version: i32) -> Result<u64> {
    let done = sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version, priority)
         SELECT 'embed', a.id, $2, $3 FROM assets a
         WHERE a.thumbhash IS NOT NULL AND a.primary_id IS NULL AND a.purging_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM asset_embeddings e
                           WHERE e.asset_id = a.id AND e.model = $1 AND e.version = $2)
         ON CONFLICT (kind, asset_id, version) DO UPDATE
             SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL,
                 last_error = NULL, priority = EXCLUDED.priority
             WHERE jobs.state = 'done'",
    )
    .bind(model)
    .bind(version)
    .bind(jobs::PRIORITY_BACKFILL)
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

/// Embedding progress for one owner's visible, live photos that have derived images (what search can cover).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub indexed: i64,
    pub total: i64,
}

pub async fn progress(db: impl PgExecutor<'_>, owner: Uuid, model: &str, version: i32) -> Result<Progress> {
    let (indexed, total): (i64, i64) = sqlx::query_as(
        "SELECT count(e.asset_id), count(*) FROM assets a
         LEFT JOIN asset_embeddings e ON e.asset_id = a.id AND e.model = $2 AND e.version = $3
         WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.primary_id IS NULL AND a.thumbhash IS NOT NULL",
    )
    .bind(owner)
    .bind(model)
    .bind(version)
    .fetch_one(db)
    .await?;
    Ok(Progress { indexed, total })
}
