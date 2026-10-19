//! What each account stores (docs/admin.md, "Usage").
//!
//! `user_usage` (migration 0017) is kept current by triggers on `assets`. This module holds what checks and completes
//! it: the truth computed straight from `assets` ([`truth`]), the nightly [`reconcile`] that reports and repairs drift,
//! [`measure_derived`] (sizes of derived objects written before they were recorded) and [`bucket_check`] (the bucket's
//! real contents against the database, an expensive listing the admin asks for).
use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{keys, metrics, storage::Storage};

/// One account's numbers. All of them are exact except `derived_bytes`, which covers `derived_files` of the account's
/// files (the rest have not been measured yet).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Usage {
    pub originals_bytes: i64,
    pub trash_bytes: i64,
    pub derived_bytes: i64,
    pub photo_count: i64,
    pub raw_count: i64,
    pub file_count: i64,
    pub trash_count: i64,
    pub derived_files: i64,
}

impl Usage {
    /// What counts toward a quota: originals, trash included until it is purged. Derived images do not.
    pub fn stored_bytes(&self) -> i64 {
        self.originals_bytes + self.trash_bytes
    }
}

/// The columns of [`Usage`], as the grouped query over `assets` computes them (the independent check of the triggers).
const TRUTH: &str = "SELECT owner_id AS user_id,
    coalesce(sum(size_bytes) FILTER (WHERE deleted_at IS NULL), 0)::bigint AS originals_bytes,
    coalesce(sum(size_bytes) FILTER (WHERE deleted_at IS NOT NULL), 0)::bigint AS trash_bytes,
    coalesce(sum(coalesce(derived_bytes, 0) + coalesce(share_copy_bytes, 0)), 0)::bigint AS derived_bytes,
    count(*) FILTER (WHERE deleted_at IS NULL AND primary_id IS NULL) AS photo_count,
    count(*) FILTER (WHERE deleted_at IS NULL AND asset_is_raw(file_ext)) AS raw_count,
    count(*) FILTER (WHERE deleted_at IS NULL) AS file_count,
    count(*) FILTER (WHERE deleted_at IS NOT NULL) AS trash_count,
    count(*) FILTER (WHERE derived_bytes IS NOT NULL) AS derived_files
  FROM assets";

#[derive(sqlx::FromRow)]
struct Row {
    user_id: Uuid,
    #[sqlx(flatten)]
    usage: Usage,
}

/// The numbers recomputed from `assets` for one account (zeros for one with no files).
pub async fn truth_of(db: &PgPool, user: Uuid) -> Result<Usage> {
    let row: Option<Row> = sqlx::query_as(&format!("{TRUTH} WHERE owner_id = $1 GROUP BY owner_id"))
        .bind(user)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|r| r.usage).unwrap_or_default())
}

/// The maintained numbers of one account (zeros before its first file).
pub async fn of(db: &PgPool, user: Uuid) -> Result<Usage> {
    let row: Option<Usage> = sqlx::query_as(
        "SELECT originals_bytes, trash_bytes, derived_bytes, photo_count, raw_count, file_count, trash_count, derived_files
         FROM user_usage WHERE user_id = $1",
    )
    .bind(user)
    .fetch_optional(db)
    .await?;
    Ok(row.unwrap_or_default())
}

/// One account whose maintained numbers differed from the truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Drift {
    pub user_id: Uuid,
    pub stored: Usage,
    pub truth: Usage,
}

impl Drift {
    /// Bytes the maintained originals + trash were off by (signed: stored minus truth).
    pub fn stored_bytes_off(&self) -> i64 {
        self.stored.stored_bytes() - self.truth.stored_bytes()
    }
}

#[derive(Debug, Default, Serialize)]
pub struct ReconcileReport {
    pub users_checked: usize,
    pub drifted: Vec<Drift>,
    /// Upload-log rows older than the retention that were removed.
    pub log_rows_pruned: u64,
}

/// How long `upload_log` rows are kept (the longest window shown is 30 days).
pub const UPLOAD_LOG_DAYS: i32 = 90;

/// Recomputes every account's usage from `assets`, repairs any row that differs and returns what differed. A drifted
/// account is fixed under a lock on its usage row (the trigger's updates wait for it) and re-read there, so a photo
/// arriving while this runs is neither lost nor reported. Also gives accounts without a row one, and prunes the upload log.
pub async fn reconcile(db: &PgPool) -> Result<ReconcileReport> {
    let truth: Vec<Row> = sqlx::query_as(&format!("{TRUTH} GROUP BY owner_id"))
        .fetch_all(db)
        .await?;
    let truth: HashMap<Uuid, Usage> = truth.into_iter().map(|r| (r.user_id, r.usage)).collect();
    let stored: Vec<(Uuid, Usage)> = {
        #[derive(sqlx::FromRow)]
        struct S {
            user_id: Uuid,
            #[sqlx(flatten)]
            usage: Usage,
        }
        let rows: Vec<S> = sqlx::query_as(
            "SELECT user_id, originals_bytes, trash_bytes, derived_bytes, photo_count, raw_count, file_count, trash_count,
                    derived_files FROM user_usage",
        )
        .fetch_all(db)
        .await?;
        rows.into_iter().map(|s| (s.user_id, s.usage)).collect()
    };
    let users: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM users").fetch_all(db).await?;
    let stored: HashMap<Uuid, Usage> = stored.into_iter().collect();

    let mut report = ReconcileReport {
        users_checked: users.len(),
        ..Default::default()
    };
    for (user,) in users {
        let want = truth.get(&user).copied().unwrap_or_default();
        if stored.get(&user).copied().unwrap_or_default() == want && stored.contains_key(&user) {
            continue;
        }
        // Possible drift: settle it with the row locked.
        let mut tx = db.begin().await?;
        sqlx::query("INSERT INTO user_usage (user_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(user)
            .execute(&mut *tx)
            .await?;
        let now: Usage = sqlx::query_as(
            "SELECT originals_bytes, trash_bytes, derived_bytes, photo_count, raw_count, file_count, trash_count,
                    derived_files FROM user_usage WHERE user_id = $1 FOR UPDATE",
        )
        .bind(user)
        .fetch_one(&mut *tx)
        .await?;
        let row: Option<Row> = sqlx::query_as(&format!("{TRUTH} WHERE owner_id = $1 GROUP BY owner_id"))
            .bind(user)
            .fetch_optional(&mut *tx)
            .await?;
        let want = row.map(|r| r.usage).unwrap_or_default();
        if now != want {
            sqlx::query(
                "UPDATE user_usage SET originals_bytes = $2, trash_bytes = $3, derived_bytes = $4, photo_count = $5,
                        raw_count = $6, file_count = $7, trash_count = $8, derived_files = $9, updated_at = now()
                 WHERE user_id = $1",
            )
            .bind(user)
            .bind(want.originals_bytes)
            .bind(want.trash_bytes)
            .bind(want.derived_bytes)
            .bind(want.photo_count)
            .bind(want.raw_count)
            .bind(want.file_count)
            .bind(want.trash_count)
            .bind(want.derived_files)
            .execute(&mut *tx)
            .await?;
            report.drifted.push(Drift {
                user_id: user,
                stored: now,
                truth: want,
            });
        }
        tx.commit().await?;
    }
    report.log_rows_pruned = sqlx::query("DELETE FROM upload_log WHERE at < now() - make_interval(days => $1)")
        .bind(UPLOAD_LOG_DAYS)
        .execute(db)
        .await?
        .rows_affected();
    Ok(report)
}

/// Runs [`reconcile`], logs what it found and sends the metrics: `usage.reconcile.drifted_users` and `.drift_bytes`
/// (gauges: 0 is healthy), and the server totals `usage.bytes{kind}`.
pub async fn reconcile_and_report(db: &PgPool) -> Result<ReconcileReport> {
    let report = reconcile(db).await?;
    for d in &report.drifted {
        tracing::warn!(
            user_id = %d.user_id,
            stored_bytes_off = d.stored_bytes_off(),
            stored = ?d.stored,
            truth = ?d.truth,
            "usage counters had drifted from the files; corrected"
        );
    }
    let m = metrics::global();
    m.gauge("usage.reconcile.drifted_users", report.drifted.len() as f64, &[]);
    m.gauge(
        "usage.reconcile.drift_bytes",
        report
            .drifted
            .iter()
            .map(|d| d.stored_bytes_off().unsigned_abs())
            .sum::<u64>() as f64,
        &[],
    );
    let t: (Option<i64>, Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT sum(originals_bytes)::bigint, sum(trash_bytes)::bigint, sum(derived_bytes)::bigint, sum(photo_count)::bigint
         FROM user_usage",
    )
    .fetch_one(db)
    .await?;
    for (kind, v) in [("originals", t.0), ("trash", t.1), ("derived", t.2)] {
        m.gauge("usage.bytes", v.unwrap_or(0) as f64, &[("kind", kind)]);
    }
    m.gauge("usage.photos", t.3.unwrap_or(0) as f64, &[]);
    tracing::info!(
        users = report.users_checked,
        drifted = report.drifted.len(),
        log_rows_pruned = report.log_rows_pruned,
        "usage reconcile finished"
    );
    sqlx::query(
        "INSERT INTO server_state (key, value) VALUES ('usage_reconcile', $1)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(serde_json::json!({
        "users_checked": report.users_checked,
        "drifted_users": report.drifted.len(),
        "drift_bytes": report.drifted.iter().map(|d| d.stored_bytes_off().unsigned_abs()).sum::<u64>(),
    }))
    .execute(db)
    .await?;
    Ok(report)
}

/// Records the sizes of derived objects for up to `limit` files that were rendered before sizes were recorded, by one
/// HEAD per object. Returns how many files were measured. A file whose objects are missing is left alone (and tried
/// again next time), so a catch-up over a large library takes `files / limit` runs.
pub async fn measure_derived(db: &PgPool, storage: &Storage, limit: i64) -> Result<usize> {
    let rows: Vec<(Uuid, Uuid, bool)> = sqlx::query_as(
        "SELECT id, owner_id, share_copy_at IS NOT NULL FROM assets
         WHERE derived_bytes IS NULL AND thumbhash IS NOT NULL AND purging_at IS NULL
         ORDER BY id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await?;
    let mut measured = 0;
    for chunk in rows.chunks(8) {
        let mut set = tokio::task::JoinSet::new();
        for &(id, owner, has_share) in chunk {
            let storage = storage.clone();
            set.spawn(async move {
                let thumb = storage.head_size(&keys::thumb(owner, id)).await?;
                let preview = storage.head_size(&keys::preview(owner, id)).await?;
                let share = if has_share {
                    storage.head_size(&keys::share_copy(owner, id)).await?
                } else {
                    None
                };
                anyhow::Ok((id, thumb, preview, share))
            });
        }
        while let Some(joined) = set.join_next().await {
            let (id, thumb, preview, share) = joined.context("measure task")??;
            let (Some(t), Some(p)) = (thumb, preview) else { continue };
            // Only a file nobody measured meanwhile (the worker records the size when it renders).
            let n = sqlx::query(
                "UPDATE assets SET derived_bytes = $2, share_copy_bytes = coalesce(share_copy_bytes, $3)
                 WHERE id = $1 AND derived_bytes IS NULL",
            )
            .bind(id)
            .bind(t + p)
            .bind(share)
            .execute(db)
            .await?
            .rows_affected();
            measured += n as usize;
        }
    }
    Ok(measured)
}

/// The bucket against the database (`bucket_check` admin task). Sizes are of the latest versions unless a name says
/// otherwise; `noncurrent_*` are old versions the versioned bucket keeps after an overwrite or a delete.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BucketReport {
    pub finished_at: String,
    /// Whether the store listed versions (otherwise the noncurrent numbers are 0).
    pub versioned: bool,
    pub objects: u64,
    pub current_bytes: i64,
    pub originals_bytes: i64,
    pub derived_bytes: i64,
    pub manifests_bytes: i64,
    pub edits_bytes: i64,
    pub other_bytes: i64,
    pub noncurrent_versions: u64,
    pub noncurrent_bytes: i64,
    pub delete_markers: u64,
    /// The database's own numbers, summed over accounts: originals plus trash, and measured derived bytes.
    pub db_originals_bytes: i64,
    pub db_derived_bytes: i64,
    pub db_derived_files: i64,
    pub db_files: i64,
    /// Originals under an account id the database does not know.
    pub unknown_account_bytes: i64,
    /// Accounts whose originals in the bucket differ from the database's originals + trash.
    pub mismatched: Vec<AccountMismatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountMismatch {
    pub user_id: Uuid,
    pub email: String,
    pub bucket_originals_bytes: i64,
    pub db_originals_bytes: i64,
}

/// Lists the whole bucket (expensive: a request per thousand objects) and compares it with `user_usage`. Run it from the
/// worker as an admin task, not in a request.
pub async fn bucket_check(db: &PgPool, storage: &Storage) -> Result<BucketReport> {
    let mut r = BucketReport::default();
    let mut originals: HashMap<Uuid, i64> = HashMap::new();
    r.versioned = storage
        .walk_versions("", |e| {
            if e.delete_marker {
                r.delete_markers += 1;
                return;
            }
            if !e.is_latest {
                r.noncurrent_versions += 1;
                r.noncurrent_bytes += e.size;
                return;
            }
            r.objects += 1;
            r.current_bytes += e.size;
            let mut parts = e.key.splitn(3, '/');
            let (top, who) = (
                parts.next().unwrap_or(""),
                parts.next().and_then(|u| Uuid::parse_str(u).ok()),
            );
            match (top, who) {
                ("originals", Some(u)) => {
                    r.originals_bytes += e.size;
                    *originals.entry(u).or_default() += e.size;
                }
                ("derived", Some(_)) => r.derived_bytes += e.size,
                ("manifests", _) => r.manifests_bytes += e.size,
                ("edits", _) => r.edits_bytes += e.size,
                _ => r.other_bytes += e.size,
            }
        })
        .await?;

    #[derive(sqlx::FromRow)]
    struct A {
        user_id: Uuid,
        email: String,
        stored: i64,
        derived_bytes: i64,
        derived_files: i64,
        file_count: i64,
        trash_count: i64,
    }
    let accounts: Vec<A> = sqlx::query_as(
        "SELECT u.id AS user_id, u.email, coalesce(m.originals_bytes + m.trash_bytes, 0) AS stored,
                coalesce(m.derived_bytes, 0) AS derived_bytes, coalesce(m.derived_files, 0) AS derived_files,
                coalesce(m.file_count, 0) AS file_count, coalesce(m.trash_count, 0) AS trash_count
         FROM users u LEFT JOIN user_usage m ON m.user_id = u.id ORDER BY u.created_at, u.id",
    )
    .fetch_all(db)
    .await?;
    for a in &accounts {
        r.db_originals_bytes += a.stored;
        r.db_derived_bytes += a.derived_bytes;
        r.db_derived_files += a.derived_files;
        r.db_files += a.file_count + a.trash_count;
        let in_bucket = originals.remove(&a.user_id).unwrap_or(0);
        if in_bucket != a.stored {
            r.mismatched.push(AccountMismatch {
                user_id: a.user_id,
                email: a.email.clone(),
                bucket_originals_bytes: in_bucket,
                db_originals_bytes: a.stored,
            });
        }
    }
    r.unknown_account_bytes = originals.values().sum();
    r.finished_at = crate::admin_tasks::now_rfc3339();
    Ok(r)
}
