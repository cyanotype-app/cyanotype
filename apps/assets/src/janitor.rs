//! Periodic housekeeping for the assets service.
//!
//! - Aborts abandoned multipart uploads. An interrupted client leaves a `uploads` row and S3 parts behind
//!   (parts are billed until aborted). `check` resumes a pending upload and refreshes its `created_at`, so
//!   anything still old here has been idle for the whole retention period.
//! - Permanently deletes photos that have been in the trash longer than `TRASH_RETENTION_DAYS`, and finishes
//!   purges the API queued ("delete forever", "empty trash"); see `purge`.
use std::{sync::Arc, time::Duration};

use cyanotype_core::storage::Storage;
use sqlx::PgPool;
use tokio::sync::Notify;

use crate::purge;

const RETENTION_DAYS: i32 = 7;
const BATCH: i64 = 200;
const EVERY: Duration = Duration::from_secs(3600);
/// When a purge could not finish (storage down), try again sooner than the hourly cadence.
const RETRY_PURGE_AFTER: Duration = Duration::from_secs(300);

/// Abort and forget uploads idle for more than `retention_days`. Returns how many were removed.
pub async fn sweep(db: &PgPool, storage: &Storage, retention_days: i32) -> anyhow::Result<usize> {
    let mut removed = 0;
    loop {
        let stale: Vec<(String, String)> = sqlx::query_as(
            "SELECT upload_id, key FROM uploads WHERE created_at < now() - make_interval(days => $1) LIMIT $2",
        )
        .bind(retention_days)
        .bind(BATCH)
        .fetch_all(db)
        .await?;
        if stale.is_empty() {
            return Ok(removed);
        }
        let mut progressed = false;
        for (upload_id, key) in &stale {
            match storage.abort_multipart(key, upload_id).await {
                Ok(()) => {
                    sqlx::query("DELETE FROM uploads WHERE upload_id = $1")
                        .bind(upload_id)
                        .execute(db)
                        .await?;
                    removed += 1;
                    progressed = true;
                }
                Err(e) => tracing::warn!(error = ?e, upload_id, "could not abort stale upload"),
            }
        }
        if !progressed {
            return Ok(removed);
        }
    }
}

/// One trash pass: mark rows past retention, then remove everything marked. Returns the sweep result.
pub async fn purge_sweep(db: &PgPool, storage: &Storage, trash_retention_days: i32) -> anyhow::Result<purge::Swept> {
    let marked = purge::mark_expired(db, trash_retention_days).await?;
    if marked > 0 {
        tracing::info!(marked, trash_retention_days, "trash past retention marked for deletion");
    }
    let swept = purge::sweep(db, storage).await?;
    if swept.purged > 0 || swept.failed > 0 {
        tracing::info!(
            purged = swept.purged,
            failed = swept.failed,
            "permanently deleted photos"
        );
    }
    Ok(swept)
}

/// Runs the sweeps hourly until the process exits. `purge_wake` runs the trash pass immediately (the API calls it
/// after queueing a purge); a pass that leaves failures behind is retried after a few minutes.
pub fn spawn(db: PgPool, storage: Storage, trash_retention_days: i32, purge_wake: Arc<Notify>) {
    tokio::spawn(async move {
        let mut stale_due = tokio::time::Instant::now();
        loop {
            if tokio::time::Instant::now() >= stale_due {
                match sweep(&db, &storage, RETENTION_DAYS).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(removed = n, "aborted stale uploads"),
                    Err(e) => tracing::warn!(error = ?e, "stale upload sweep failed"),
                }
                stale_due = tokio::time::Instant::now() + EVERY;
            }
            let wait = match purge_sweep(&db, &storage, trash_retention_days).await {
                Ok(s) if s.failed == 0 => EVERY,
                Ok(_) => RETRY_PURGE_AFTER,
                Err(e) => {
                    tracing::warn!(error = ?e, "trash purge sweep failed");
                    RETRY_PURGE_AFTER
                }
            };
            let wait = wait.min(stale_due.saturating_duration_since(tokio::time::Instant::now()));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = purge_wake.notified() => {}
            }
        }
    });
}
