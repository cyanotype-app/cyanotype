//! Aborts abandoned multipart uploads. An interrupted client leaves a `uploads` row and S3 parts behind
//! (parts are billed until aborted). `check` resumes a pending upload and refreshes its `created_at`, so
//! anything still old here has been idle for the whole retention period.
use std::time::Duration;

use cyanotype_core::storage::Storage;
use sqlx::PgPool;

const RETENTION_DAYS: i32 = 7;
const BATCH: i64 = 200;
const EVERY: Duration = Duration::from_secs(3600);

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

/// Runs [`sweep`] hourly until the process exits.
pub fn spawn(db: PgPool, storage: Storage) {
    tokio::spawn(async move {
        loop {
            match sweep(&db, &storage, RETENTION_DAYS).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(removed = n, "aborted stale uploads"),
                Err(e) => tracing::warn!(error = ?e, "stale upload sweep failed"),
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}
