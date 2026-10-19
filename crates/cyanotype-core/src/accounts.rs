//! Deleting an account for good (the `delete_account` admin task, docs/admin.md): everything the account stored goes, in
//! the bucket (every version, like "delete forever") and in the database, and nothing of anybody else's is touched.
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{keys, storage::Storage};

/// The bucket prefixes that hold one account's objects (always ending with the account id and a slash, so one account's
/// prefix can never match another's).
pub fn prefixes(user: Uuid) -> [String; 4] {
    [
        format!("{}{user}/", keys::ORIGINALS_PREFIX),
        format!("{}{user}/", keys::DERIVED_PREFIX),
        format!("{}{user}/", keys::MANIFESTS_PREFIX),
        format!("{}{user}/", keys::EDITS_PREFIX),
    ]
}

#[derive(Debug, Default, Serialize)]
pub struct DeleteReport {
    /// The account existed (false: an earlier run already finished, nothing was left to do).
    pub existed: bool,
    pub files: i64,
    pub objects_deleted: usize,
    pub uploads_aborted: usize,
}

/// Why a deletion could not finish yet (the task fails and an admin retries it).
#[derive(Debug, thiserror::Error)]
#[error("jobs for this account's files are still running; try again in a few minutes")]
pub struct JobsStillRunning;

/// Deletes the account and everything it owns. Safe to run again after a failure (each step is idempotent):
///
/// 1. disable the account and end its sessions, so nothing new arrives;
/// 2. mark every file as being purged, which makes the worker refuse to (re)create derived images for it, and wait (up to
///    `wait_for_jobs`) for jobs already running on them to finish;
/// 3. abort the account's unfinished multipart uploads;
/// 4. delete every version of everything under the account's four prefixes in the bucket;
/// 5. delete the account row, which removes its files, albums, shares, keys, usage and so on by cascade.
///
/// The bucket is emptied before the rows go, so a failure in between leaves rows pointing at nothing (and a retry), never
/// objects with no row to say whose they are.
pub async fn delete_account(
    db: &PgPool,
    storage: &Storage,
    user: Uuid,
    wait_for_jobs: Duration,
) -> Result<DeleteReport> {
    let mut report = DeleteReport::default();
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(user)
        .fetch_one(db)
        .await?;
    if !exists {
        // Finished before (or never was): the objects are removed all the same, in case that is what failed.
        for prefix in prefixes(user) {
            report.objects_deleted += storage.delete_prefix_all_versions(&prefix).await?;
        }
        return Ok(report);
    }
    report.existed = true;

    sqlx::query("UPDATE users SET disabled_at = coalesce(disabled_at, now()) WHERE id = $1")
        .bind(user)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user)
        .execute(db)
        .await?;

    report.files = sqlx::query_scalar("SELECT count(*) FROM assets WHERE owner_id = $1")
        .bind(user)
        .fetch_one(db)
        .await?;
    sqlx::query(
        "UPDATE assets SET purging_at = coalesce(purging_at, now()), deleted_at = coalesce(deleted_at, now())
         WHERE owner_id = $1 AND (purging_at IS NULL OR deleted_at IS NULL)",
    )
    .bind(user)
    .execute(db)
    .await?;
    let deadline = tokio::time::Instant::now() + wait_for_jobs;
    loop {
        let running: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs j JOIN assets a ON a.id = j.asset_id WHERE a.owner_id = $1 AND j.state = 'running'",
        )
        .bind(user)
        .fetch_one(db)
        .await?;
        if running == 0 {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(JobsStillRunning.into());
        }
        tokio::time::sleep(Duration::from_secs(5).min(wait_for_jobs.max(Duration::from_millis(50)))).await;
    }

    let pending: Vec<(String, String)> = sqlx::query_as("SELECT upload_id, key FROM uploads WHERE owner_id = $1")
        .bind(user)
        .fetch_all(db)
        .await?;
    for (upload_id, key) in pending {
        // A multipart upload that S3 already forgot is fine.
        if storage.abort_multipart(&key, &upload_id).await.is_ok() {
            report.uploads_aborted += 1;
        }
    }

    for prefix in prefixes(user) {
        report.objects_deleted += storage.delete_prefix_all_versions(&prefix).await?;
    }
    // Objects an upload completed while this ran (a part-sending client is refused by now: the account is disabled).
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user)
        .execute(db)
        .await?;
    Ok(report)
}
