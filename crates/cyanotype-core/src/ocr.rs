//! Text read off photos (`asset_text`) and the jobs that read it. The model itself lives in the `cyanotype-ocr` crate (this
//! crate must not link ONNX Runtime), so callers pass the model id and version in, as they do for embeddings.
use anyhow::Result;
use serde_json::Value;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::jobs;

/// The model that reads text: PP-OCRv6 small (detection and recognition), `models/ocr.lock`. A new model gets a new id, a
/// change of how the text is made (a different cut-off, a fixed bug) a new version: either re-reads the library, at low
/// priority, and the old text keeps answering searches until its photo's new text replaces it.
pub const MODEL: &str = "ppocr-v6-small";
pub const VERSION: i32 = 1;

/// The longest text kept per photo (a page of a book is about 3,000 characters; a screenshot of a spreadsheet can be more).
pub const MAX_TEXT_CHARS: usize = 20_000;

/// What the worker found on one photo.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    /// The lines, joined with newlines.
    pub text: String,
    /// `[{"t": line, "s": score, "q": [x0,y0,...,x3,y3], "w": [{"t": word, "q": [...]}]}]`, coordinates 0..1.
    pub lines: Value,
    /// `[{"t": payload, "q": [...]}]`.
    pub qr: Value,
}

/// Store what was read from a photo for `(model, version)`, replacing the previous row.
pub async fn store(db: &PgPool, asset_id: Uuid, model: &str, version: i32, found: &Found) -> Result<()> {
    let text: String = found.text.chars().take(MAX_TEXT_CHARS).collect();
    // Postgres text cannot hold NUL.
    let text = text.replace('\0', "");
    sqlx::query(
        "INSERT INTO asset_text (asset_id, model, version, text, lines, qr)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (asset_id) DO UPDATE SET model = EXCLUDED.model, version = EXCLUDED.version, text = EXCLUDED.text,
             lines = EXCLUDED.lines, qr = EXCLUDED.qr, created_at = now()",
    )
    .bind(asset_id)
    .bind(model)
    .bind(version)
    .bind(text)
    .bind(&found.lines)
    .bind(&found.qr)
    .execute(db)
    .await?;
    Ok(())
}

/// Queue an `ocr` job at backfill priority for every visible photo that has a preview and no text read by `(model, version)`:
/// the backfill of an existing library, the catch-up after a model switch, and jobs that were lost. A job already queued,
/// running or failed is left alone; one that finished without a row is requeued. Returns how many were queued.
pub async fn enqueue_missing(db: impl PgExecutor<'_>, model: &str, version: i32) -> Result<u64> {
    let done = sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version, priority)
         SELECT 'ocr', a.id, $2, $3 FROM assets a
         WHERE a.thumbhash IS NOT NULL AND a.primary_id IS NULL AND a.purging_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM asset_text t WHERE t.asset_id = a.id AND t.model = $1 AND t.version = $2)
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

/// Queue the `ocr` job of one photo (after its embedding, at backfill priority: text is the least urgent thing to know).
pub async fn enqueue_for_asset(db: impl PgExecutor<'_>, asset_id: Uuid, version: i32) -> Result<()> {
    jobs::enqueue_with_priority(db, jobs::KIND_OCR, asset_id, version, jobs::PRIORITY_BACKFILL).await
}

/// How many visible photos with a preview have no text read by `(model, version)`.
pub async fn missing_count(db: impl PgExecutor<'_>, model: &str, version: i32) -> Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM assets a
         WHERE a.thumbhash IS NOT NULL AND a.primary_id IS NULL AND a.purging_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM asset_text t WHERE t.asset_id = a.id AND t.model = $1 AND t.version = $2)",
    )
    .bind(model)
    .bind(version)
    .fetch_one(db)
    .await?;
    Ok(n)
}

/// Reading progress of one owner's library: photos read, and photos that can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub read: i64,
    pub total: i64,
}

pub async fn progress(db: impl PgExecutor<'_>, owner: Uuid, model: &str, version: i32) -> Result<Progress> {
    let (read, total): (i64, i64) = sqlx::query_as(
        "SELECT count(t.asset_id), count(*) FROM assets a
         LEFT JOIN asset_text t ON t.asset_id = a.id AND t.model = $2 AND t.version = $3
         WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.primary_id IS NULL AND a.thumbhash IS NOT NULL",
    )
    .bind(owner)
    .bind(model)
    .bind(version)
    .fetch_one(db)
    .await?;
    Ok(Progress { read, total })
}
