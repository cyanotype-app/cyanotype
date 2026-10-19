//! The admin health card (`GET /assets/admin/health`) and the retry of failed jobs (`POST /assets/admin/jobs/retry`).
//! Everything here comes from what the stack already has: the queue tables, what the worker publishes about itself every
//! minute (`server_state`), the database's own size, the bucket's `backups/` prefix, and the last bucket check. Nothing calls
//! Datadog. Admins see kinds, counts, sizes and error text, never file names or owners.
use axum::{Json, extract::State};
use cyanotype_core::{admin_tasks, jobs};
use cyanotype_service::{AdminUser, ApiResult, audit};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::AppState;

/// Where the backup service writes (`BACKUP_PREFIX` there; docs/deploy.md).
const DEFAULT_BACKUP_PREFIX: &str = "backups/postgres";
/// A worker that has not reported for this long is shown as not reporting.
const WORKER_STALE_MINUTES: i32 = 5;

/// `cyanotype-20261012T031700Z.dump` -> its time.
pub(crate) fn backup_time(name: &str) -> Option<OffsetDateTime> {
    let stamp = name.strip_prefix("cyanotype-")?.strip_suffix(".dump")?;
    let fmt = time::macros::format_description!("[year][month][day]T[hour][minute][second]Z");
    time::PrimitiveDateTime::parse(stamp, &fmt).ok().map(|t| t.assume_utc())
}

fn rfc(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// The newest dump under the backup prefix, from the bucket: `(count, newest time, newest size)`.
async fn backups(s: &AppState) -> Value {
    let prefix = std::env::var("BACKUP_PREFIX")
        .ok()
        .map(|p| p.trim().trim_matches('/').to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| DEFAULT_BACKUP_PREFIX.to_string());
    let mut found: Vec<(OffsetDateTime, i64)> = Vec::new();
    let walked = s
        .storage
        .walk_versions(&format!("{prefix}/"), |e| {
            if e.delete_marker || !e.is_latest {
                return;
            }
            if let Some(t) = e.key.rsplit('/').next().and_then(backup_time) {
                found.push((t, e.size));
            }
        })
        .await;
    if let Err(e) = walked {
        tracing::warn!(error = %format!("{e:#}"), "could not list backups");
        return json!({"prefix": prefix, "error": "could not list the bucket's backups"});
    }
    found.sort();
    match found.last() {
        None => json!({"prefix": prefix, "count": 0, "last_at": null, "last_size_bytes": null}),
        Some((t, size)) => json!({
            "prefix": prefix,
            "count": found.len(),
            "last_at": rfc(*t),
            "last_size_bytes": size,
        }),
    }
}

/// The deployed version: what the services are labelled with (`DD_VERSION`, the image tag in production, `sha-<commit>`).
fn version() -> String {
    std::env::var("DD_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

pub(super) async fn health(State(s): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Value>> {
    let queue = jobs::queue_stats(&s.db).await?;
    let stalled = jobs::stalled_seconds(&s.db).await.unwrap_or(0.0);
    type FailedJob = (i64, String, i32, Option<String>, Option<OffsetDateTime>);
    let failed: Vec<FailedJob> = sqlx::query_as(
        "SELECT id, kind, attempts, left(last_error, 300), finished_at FROM jobs
         WHERE state = 'failed' ORDER BY finished_at DESC NULLS LAST, id DESC LIMIT 20",
    )
    .fetch_all(&s.db)
    .await?;
    let workers: Vec<(String, Value, OffsetDateTime, bool)> = sqlx::query_as(
        "SELECT substring(key from 8), value, updated_at, updated_at > now() - make_interval(mins => $1)
         FROM server_state WHERE key LIKE 'worker:%' ORDER BY key",
    )
    .bind(WORKER_STALE_MINUTES)
    .fetch_all(&s.db)
    .await?;
    let (db_size,): (i64,) = sqlx::query_as("SELECT pg_database_size(current_database())")
        .fetch_one(&s.db)
        .await?;
    let (stored, derived): (Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT sum(originals_bytes + trash_bytes)::bigint, sum(derived_bytes)::bigint FROM user_usage")
            .fetch_one(&s.db)
            .await?;
    let check = admin_tasks::latest(&s.db, admin_tasks::KIND_BUCKET_CHECK).await?;
    let bucket_check = check.filter(|t| t.state == "done").and_then(|t| {
        let r = t.result?;
        Some(json!({
            "at": t.finished_at.map(rfc),
            "current_bytes": r.get("current_bytes"),
            "noncurrent_bytes": r.get("noncurrent_bytes"),
            "objects": r.get("objects"),
        }))
    });

    Ok(Json(json!({
        "version": {
            "assets": version(),
            "workers": workers.iter().map(|(host, v, _, fresh)| json!({
                "host": host, "version": v.get("version"), "reporting": fresh,
            })).collect::<Vec<_>>(),
        },
        "queue": queue.iter().map(|q| json!({
            "kind": q.kind, "queued": q.queued, "running": q.running, "failed": q.failed,
            "oldest_queued_age_secs": q.oldest_queued_age_secs,
        })).collect::<Vec<_>>(),
        "stalled_seconds": stalled,
        "failed_jobs": failed.into_iter().map(|(id, kind, attempts, error, at)| json!({
            "id": id, "kind": kind, "attempts": attempts, "last_error": error, "finished_at": at.map(rfc),
        })).collect::<Vec<_>>(),
        "workers": workers.into_iter().map(|(host, v, at, fresh)| json!({
            "host": host, "reporting": fresh, "at": rfc(at),
            "version": v.get("version"),
            "rss_bytes": v.get("rss_bytes"),
            "budget_used_bytes": v.get("budget_used_bytes"),
            "budget_total_bytes": v.get("budget_total_bytes"),
        })).collect::<Vec<_>>(),
        "backup": backups(&s).await,
        "storage": {
            "database_bytes": db_size,
            "stored_bytes": stored.unwrap_or(0),
            "derived_bytes": derived.unwrap_or(0),
            "bucket_check": bucket_check,
        },
    })))
}

#[derive(Deserialize, Default)]
pub(super) struct RetryReq {
    /// Only this kind (`thumbnail`, `metadata`, ...); every kind when left out.
    kind: Option<String>,
}

/// Puts failed jobs back in the queue with a fresh set of attempts, for every account (a file being purged is skipped).
/// The trigger on `jobs` wakes the worker.
pub(super) async fn retry_failed(
    State(s): State<AppState>,
    admin: AdminUser,
    body: Option<Json<RetryReq>>,
) -> ApiResult<Json<Value>> {
    let kind = body.and_then(|Json(b)| b.kind).filter(|k| !k.trim().is_empty());
    let n = sqlx::query(
        "UPDATE jobs SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL
         WHERE state = 'failed' AND ($1::text IS NULL OR kind = $1)
           AND asset_id IN (SELECT id FROM assets WHERE purging_at IS NULL)",
    )
    .bind(&kind)
    .execute(&s.db)
    .await?
    .rows_affected();
    audit::record(
        &s.db,
        Some(admin.id),
        "jobs.retry",
        None,
        None,
        json!({"kind": kind, "requeued": n}),
    )
    .await?;
    Ok(Json(json!({"requeued": n})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_names_carry_their_time() {
        let t = backup_time("cyanotype-20261012T031700Z.dump").unwrap();
        assert_eq!(rfc(t), "2026-10-12T03:17:00Z");
        for bad in [
            "cyanotype-20261012.dump",
            "other.dump",
            "cyanotype-20261012T031700Z.dump.tmp",
            "",
            "cyanotype-.dump",
        ] {
            assert!(backup_time(bad).is_none(), "{bad}");
        }
    }
}
