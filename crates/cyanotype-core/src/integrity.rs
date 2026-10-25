//! The storage integrity check (docs/admin.md, "Storage integrity"): a scan that compares the database with the bucket and
//! reports what does not agree, run nightly by the worker (`INTEGRITY_CHECK_HOUR`) and on demand from Admin > Health.
//!
//! What it looks for, and what it does about it:
//!
//! * **missing original**: a live row whose `originals/` object is gone (HEAD-confirmed, since the listing can race an upload).
//!   A finding; the admin restores an earlier version, moves the photo to the trash or marks it lost.
//! * **untracked object**: an object under `originals/` that no row points at. Objects younger than `min_age`, objects of an
//!   upload in flight (`uploads`), rows being purged, accounts being deleted, and everything outside `originals/` (manifests,
//!   `edits/`, `derived/`) are left alone. A finding; the admin restores it as a photo (the same import `reindex` does) or
//!   deletes it.
//! * **checksum mismatch**: the object's size differs from the row's (free, from the listing), or its bytes no longer hash to
//!   the stored sha256. Hashing reads the whole file, so only a rolling sample is checked per run: the rows verified longest ago
//!   (`assets.verified_at`, never-verified first), until the run's byte budget is spent, streamed, one file at a time.
//! * **missing thumbnail or preview**: not a finding. The `thumbnail` job is queued again (a derived image can always be made
//!   again) and the run counts it. A photo whose thumbnail job failed is left to the failed-jobs list.
//!
//! Nothing here deletes or rewrites an object by itself: only an admin's action does ([`act`]). Findings are upserted, so a run
//! that sees a problem again updates it; a run that completes resolves what it no longer sees. The scan is read-only and
//! idempotent: if the worker stops in the middle, the next run starts over (the sample's progress is kept in `verified_at`).
//! Trashed rows and photos queued for permanent deletion (`purging_at`) are not checked for a missing original, and nothing
//! the purge sweep is removing is ever reported.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    admin_tasks, jobs, keys, metrics, reindex,
    scan::{Head, Listed},
    storage::Storage,
};

pub const KIND_MISSING_ORIGINAL: &str = "missing_original";
pub const KIND_UNTRACKED: &str = "untracked_object";
pub const KIND_CHECKSUM: &str = "checksum_mismatch";
pub const KINDS: [&str; 3] = [KIND_MISSING_ORIGINAL, KIND_UNTRACKED, KIND_CHECKSUM];

pub const DEFAULT_HOUR: u8 = 3;
const DEFAULT_VERIFY_MB: u64 = 2048;
const DEFAULT_MAX_MINUTES: u64 = 60;
const DEFAULT_MIN_AGE_MINUTES: u64 = 10;
/// How many missing originals get a lookup of their earlier versions per run (one request each); the rest are shown without.
const VERSION_LOOKUPS_PER_RUN: u32 = 200;
/// Most a run waits in total for the upload pipeline to go quiet before hashing the next file.
const MAX_YIELD: Duration = Duration::from_secs(15 * 60);
/// Rows read from the database per round.
const ROWS_PER_PAGE: i64 = 500;
/// Files hashed per round of the sample, between two writes of `verified_at`.
const VERIFY_BATCH: i64 = 20;
/// Earlier versions of one object that [`act`] reads in full to find the one with the stored checksum.
const VERSIONS_TRIED: usize = 10;

#[derive(Debug, Clone)]
pub struct Options {
    /// Objects modified more recently than this are not judged: an upload may be completing right now.
    pub min_age: Duration,
    /// Bytes the checksum sample may read per run; 0 turns the sample off. A file that alone exceeds it is still read when it
    /// is the first of the run, so the oldest-verified file can never be skipped for good.
    pub verify_bytes: u64,
    /// The sample stops starting new files after this long (the scan itself always completes).
    pub max_runtime: Duration,
    /// Parallel HEAD requests.
    pub concurrency: usize,
    /// Pause the sample while thumbnail or metadata jobs are waiting, so uploads go first.
    pub yield_to_jobs: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            min_age: Duration::from_secs(DEFAULT_MIN_AGE_MINUTES * 60),
            verify_bytes: DEFAULT_VERIFY_MB << 20,
            max_runtime: Duration::from_secs(DEFAULT_MAX_MINUTES * 60),
            concurrency: 8,
            yield_to_jobs: true,
        }
    }
}

fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

impl Options {
    /// `INTEGRITY_VERIFY_MB` (default 2048; 0 = no checksum sample), `INTEGRITY_MAX_MINUTES` (60) and
    /// `INTEGRITY_MIN_AGE_MINUTES` (10).
    pub fn from_env() -> Self {
        let d = Self::default();
        Self {
            verify_bytes: env_parse::<u64>("INTEGRITY_VERIFY_MB")
                .map_or(d.verify_bytes, |mb| mb.saturating_mul(1 << 20)),
            max_runtime: env_parse::<u64>("INTEGRITY_MAX_MINUTES")
                .map_or(d.max_runtime, |m| Duration::from_secs(m.max(1) * 60)),
            min_age: env_parse::<u64>("INTEGRITY_MIN_AGE_MINUTES").map_or(d.min_age, |m| Duration::from_secs(m * 60)),
            ..d
        }
    }
}

/// The nightly schedule from the environment: `None` when `INTEGRITY_CHECK_ENABLED=0`, else the UTC hour
/// (`INTEGRITY_CHECK_HOUR`, 0 to 23, default 3).
pub fn schedule_from_env() -> Option<u8> {
    if std::env::var("INTEGRITY_CHECK_ENABLED").is_ok_and(|v| matches!(v.trim(), "0" | "false" | "off" | "no")) {
        return None;
    }
    Some(
        env_parse::<u8>("INTEGRITY_CHECK_HOUR")
            .filter(|h| *h < 24)
            .unwrap_or(DEFAULT_HOUR),
    )
}

/// The most recent `hour`:00 UTC at or before `now`: a run that started after it counts as tonight's.
pub fn last_boundary(now: OffsetDateTime, hour: u8) -> OffsetDateTime {
    let today = now.replace_time(time::Time::from_hms(hour.min(23), 0, 0).unwrap_or(time::Time::MIDNIGHT));
    if today <= now {
        today
    } else {
        today - time::Duration::days(1)
    }
}

/// Queues tonight's run when none has started since the schedule's hour came round (a worker that was down at the hour runs it
/// when it is back). Returns whether a task was queued. Safe from several workers: one open task at a time.
pub async fn schedule_if_due(db: &PgPool, hour: u8) -> Result<bool> {
    let since = last_boundary(OffsetDateTime::now_utc(), hour);
    let ran: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM integrity_runs WHERE started_at >= $1)
             OR EXISTS (SELECT 1 FROM admin_tasks WHERE kind = $2 AND created_at >= $1)",
    )
    .bind(since)
    .bind(admin_tasks::KIND_INTEGRITY_CHECK)
    .fetch_one(db)
    .await?;
    if ran {
        return Ok(false);
    }
    let (_, created) = admin_tasks::enqueue(
        db,
        admin_tasks::KIND_INTEGRITY_CHECK,
        None,
        json!({"trigger": "schedule"}),
    )
    .await?;
    Ok(created)
}

/// What a finished run reports (also the task's result).
#[derive(Debug, Default, Clone, Serialize)]
pub struct Summary {
    pub run_id: i64,
    pub objects_listed: u64,
    pub assets_checked: u64,
    pub assets_verified: u64,
    pub bytes_hashed: u64,
    pub derived_requeued: u64,
    pub findings_resolved: u64,
    /// Open findings when the run ended, per kind.
    pub open: BTreeMap<String, i64>,
    /// Requests that failed and were skipped (the next run looks again).
    pub errors: u64,
}

/// A 16-byte digest of a key, so the listing can be remembered without keeping every key.
type KeyHash = [u8; 16];

fn key_hash(key: &str) -> KeyHash {
    let d = Sha256::digest(key.as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&d[..16]);
    out
}

/// Start a run record. A row still `running` belongs to a worker that stopped; it is closed as failed.
pub async fn begin(db: &PgPool, trigger: &str, requested_by: Option<Uuid>) -> Result<i64> {
    sqlx::query(
        "UPDATE integrity_runs SET state = 'failed', finished_at = now(), error = 'the worker stopped before the run finished'
         WHERE state = 'running'",
    )
    .execute(db)
    .await?;
    Ok(
        sqlx::query_scalar("INSERT INTO integrity_runs (trigger, requested_by) VALUES ($1, $2) RETURNING id")
            .bind(if trigger == "manual" { "manual" } else { "schedule" })
            .bind(requested_by)
            .fetch_one(db)
            .await?,
    )
}

struct Run<'a> {
    db: &'a PgPool,
    storage: &'a Storage,
    opts: &'a Options,
    id: i64,
    started_at: OffsetDateTime,
    clock: Instant,
    stats: Summary,
    /// Assets already known to have a problem this run: not hashed.
    bad: HashSet<Uuid>,
    yielded: Duration,
    version_lookups: u32,
}

/// Run the scan for the run `run_id` (from [`begin`]) and close the record. On an error the record is closed as failed.
pub async fn run(db: &PgPool, storage: &Storage, run_id: i64, opts: &Options) -> Result<Summary> {
    let started_at: OffsetDateTime = sqlx::query_scalar("SELECT started_at FROM integrity_runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(db)
        .await?;
    let mut r = Run {
        db,
        storage,
        opts,
        id: run_id,
        started_at,
        clock: Instant::now(),
        stats: Summary {
            run_id,
            ..Default::default()
        },
        bad: HashSet::new(),
        yielded: Duration::ZERO,
        version_lookups: 0,
    };
    let m = metrics::global();
    match r.scan().await {
        Ok(()) => {
            let open = open_counts(db).await?;
            r.stats.open = open.clone();
            sqlx::query(
                "UPDATE integrity_runs SET state = 'done', finished_at = now(), objects_listed = $2, assets_checked = $3,
                        assets_verified = $4, bytes_hashed = $5, missing_originals = $6, untracked_objects = $7,
                        checksum_mismatches = $8, derived_requeued = $9, findings_resolved = $10
                 WHERE id = $1",
            )
            .bind(run_id)
            .bind(r.stats.objects_listed as i64)
            .bind(r.stats.assets_checked as i64)
            .bind(r.stats.assets_verified as i64)
            .bind(r.stats.bytes_hashed as i64)
            .bind(open.get(KIND_MISSING_ORIGINAL).copied().unwrap_or(0) as i32)
            .bind(open.get(KIND_UNTRACKED).copied().unwrap_or(0) as i32)
            .bind(open.get(KIND_CHECKSUM).copied().unwrap_or(0) as i32)
            .bind(r.stats.derived_requeued as i32)
            .bind(r.stats.findings_resolved as i32)
            .execute(db)
            .await?;
            m.count("integrity.runs", 1, &[("result", "done")]);
            m.count("integrity.bytes_hashed", r.stats.bytes_hashed, &[]);
            m.count("integrity.derived_requeued", r.stats.derived_requeued, &[]);
            m.histogram("integrity.run.duration_seconds", r.clock.elapsed().as_secs_f64(), &[]);
            let _ = emit_gauges(db).await;
            tracing::info!(
                run = run_id,
                objects = r.stats.objects_listed,
                assets = r.stats.assets_checked,
                verified = r.stats.assets_verified,
                bytes_hashed = r.stats.bytes_hashed,
                requeued = r.stats.derived_requeued,
                open = ?open,
                "integrity check finished"
            );
            Ok(r.stats)
        }
        Err(e) => {
            let msg: String = format!("{e:#}").chars().take(500).collect();
            let _ = sqlx::query(
                "UPDATE integrity_runs SET state = 'failed', finished_at = now(), error = $2 WHERE id = $1",
            )
            .bind(run_id)
            .bind(&msg)
            .execute(db)
            .await;
            m.count("integrity.runs", 1, &[("result", "failed")]);
            Err(e)
        }
    }
}

/// Open findings per kind (every kind present, zeros included).
pub async fn open_counts(db: &PgPool) -> Result<BTreeMap<String, i64>> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT kind, count(*) FROM integrity_findings WHERE state = 'open' GROUP BY kind")
            .fetch_all(db)
            .await?;
    let mut out: BTreeMap<String, i64> = KINDS.iter().map(|k| ((*k).to_string(), 0)).collect();
    out.extend(rows);
    Ok(out)
}

/// The gauges the monitors watch: open findings per kind, and the age of the last finished run (absent before the first).
pub async fn emit_gauges(db: &PgPool) -> Result<()> {
    let m = metrics::global();
    if !m.is_enabled() {
        return Ok(());
    }
    for (kind, n) in open_counts(db).await? {
        m.gauge("integrity.findings_open", n as f64, &[("kind", &kind)]);
    }
    let age: Option<f64> = sqlx::query_scalar(
        "SELECT extract(epoch FROM now() - max(finished_at))::float8 FROM integrity_runs WHERE state = 'done'",
    )
    .fetch_one(db)
    .await?;
    if let Some(age) = age {
        m.gauge("integrity.last_run_age_seconds", age.max(0.0), &[]);
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct RowInfo {
    id: Uuid,
    owner_id: Uuid,
    sha256: Vec<u8>,
    original_key: String,
    size_bytes: i64,
    deleted_at: Option<OffsetDateTime>,
}

impl Run<'_> {
    async fn scan(&mut self) -> Result<()> {
        let seen = self.scan_originals().await?;
        let derived = self.scan_derived().await?;
        self.check_rows(&seen, &derived).await?;
        self.verify_sample().await?;
        self.resolve_gone().await?;
        Ok(())
    }

    /// Insert a finding or touch the live one of the same kind and key. `asset` ties it to a row that must still exist and not
    /// be purging (a purge in flight is not a problem). Returns whether it is new.
    async fn finding(
        &mut self,
        kind: &str,
        owner: Option<Uuid>,
        asset: Option<Uuid>,
        key: &str,
        detail: Value,
    ) -> Result<bool> {
        let new: Option<bool> = sqlx::query_scalar(
            "INSERT INTO integrity_findings (kind, owner_id, asset_id, key, detail, first_run, last_run)
             SELECT $1, $2, $3, $4, $5, $6, $6
             WHERE $3::uuid IS NULL OR EXISTS (SELECT 1 FROM assets WHERE id = $3 AND purging_at IS NULL)
             ON CONFLICT (kind, key) WHERE state <> 'resolved' DO UPDATE
                 SET last_run = EXCLUDED.last_run, last_seen_at = now(), detail = EXCLUDED.detail, asset_id = EXCLUDED.asset_id
             RETURNING (xmax = 0)",
        )
        .bind(kind)
        .bind(owner)
        .bind(asset)
        .bind(key)
        .bind(detail)
        .bind(self.id)
        .fetch_optional(self.db)
        .await?;
        Ok(new.unwrap_or(false))
    }

    /// Objects under `originals/` against the rows, page by page. Returns each key's digest and size for the row check.
    async fn scan_originals(&mut self) -> Result<HashMap<KeyHash, i64>> {
        let mut seen = HashMap::new();
        let mut token = None;
        let now = OffsetDateTime::now_utc();
        let min_age = time::Duration::try_from(self.opts.min_age).unwrap_or_default();
        loop {
            let page = self
                .storage
                .list_page(keys::ORIGINALS_PREFIX, token)
                .await
                .context("list originals/")?;
            let mut parsed: Vec<(Listed, keys::ParsedOriginal)> = Vec::new();
            let mut strays: Vec<Listed> = Vec::new();
            for o in page.objects {
                self.stats.objects_listed += 1;
                seen.insert(key_hash(&o.key), o.size);
                match keys::parse_original(&o.key) {
                    Some(p) => parsed.push((o, p)),
                    None => strays.push(o),
                }
            }
            // Rows for the parsed objects: by owner and content, as `reindex` finds them.
            let rows: Vec<(Uuid, Vec<u8>, Uuid, String, bool)> = sqlx::query_as(
                "SELECT a.owner_id, a.sha256, a.id, a.original_key, a.purging_at IS NOT NULL
                 FROM assets a JOIN unnest($1::uuid[], $2::bytea[]) AS t(o, s) ON a.owner_id = t.o AND a.sha256 = t.s",
            )
            .bind(parsed.iter().map(|(_, p)| p.user).collect::<Vec<_>>())
            .bind(parsed.iter().map(|(_, p)| p.sha256.to_vec()).collect::<Vec<_>>())
            .fetch_all(self.db)
            .await?;
            let by_content: HashMap<(Uuid, Vec<u8>), (Uuid, String, bool)> = rows
                .into_iter()
                .map(|(o, s, id, k, purging)| ((o, s), (id, k, purging)))
                .collect();

            // (object, owner, reason)
            let mut suspects: Vec<(Listed, Option<Uuid>, &'static str)> = Vec::new();
            for (o, p) in parsed {
                match by_content.get(&(p.user, p.sha256.to_vec())) {
                    // Being purged: the sweep is removing it.
                    Some((_, _, true)) => {}
                    Some((_, key, false)) if *key == o.key => {}
                    Some(_) => suspects.push((o, Some(p.user), "duplicate_content")),
                    None => suspects.push((o, Some(p.user), "no_row")),
                }
            }
            for o in strays {
                let owner = o
                    .key
                    .strip_prefix(keys::ORIGINALS_PREFIX)
                    .and_then(|r| r.split('/').next())
                    .and_then(|u| Uuid::parse_str(u).ok());
                suspects.push((o, owner, "unrecognised_key"));
            }
            suspects.retain(|(o, _, _)| o.last_modified.is_none_or(|t| now - t >= min_age));
            if !suspects.is_empty() {
                self.report_untracked(suspects).await?;
            }
            match page.next {
                Some(t) => token = Some(t),
                None => return Ok(seen),
            }
        }
    }

    async fn report_untracked(&mut self, suspects: Vec<(Listed, Option<Uuid>, &'static str)>) -> Result<()> {
        let object_keys: Vec<String> = suspects.iter().map(|(o, _, _)| o.key.clone()).collect();
        let in_flight: HashSet<String> = sqlx::query_scalar("SELECT key FROM uploads WHERE key = ANY($1)")
            .bind(&object_keys)
            .fetch_all(self.db)
            .await?
            .into_iter()
            .collect();
        let owners: Vec<Uuid> = suspects.iter().filter_map(|(_, o, _)| *o).collect();
        let known: HashSet<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = ANY($1)")
            .bind(&owners)
            .fetch_all(self.db)
            .await?
            .into_iter()
            .collect();
        // An account being deleted removes its objects before its rows: not strays.
        let deleting: HashSet<Uuid> = sqlx::query_scalar::<_, String>(
            "SELECT params->>'user_id' FROM admin_tasks WHERE kind = $1 AND state IN ('queued', 'running')",
        )
        .bind(admin_tasks::KIND_DELETE_ACCOUNT)
        .fetch_all(self.db)
        .await?
        .into_iter()
        .filter_map(|u| Uuid::parse_str(&u).ok())
        .collect();
        for (o, owner, reason) in suspects {
            if in_flight.contains(&o.key) || owner.is_some_and(|u| deleting.contains(&u)) {
                continue;
            }
            let detail = json!({
                "reason": reason,
                "size_bytes": o.size,
                "last_modified": o.last_modified.and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
                "owner_known": owner.is_some_and(|u| known.contains(&u)),
            });
            self.finding(KIND_UNTRACKED, owner, None, &o.key, detail).await?;
        }
        Ok(())
    }

    /// The `thumb.webp` / `preview.webp` objects that exist.
    async fn scan_derived(&mut self) -> Result<HashSet<KeyHash>> {
        let mut set = HashSet::new();
        let mut token = None;
        loop {
            let page = self
                .storage
                .list_page(keys::DERIVED_PREFIX, token)
                .await
                .context("list derived/")?;
            for o in &page.objects {
                if o.key.ends_with("/thumb.webp") || o.key.ends_with("/preview.webp") {
                    set.insert(key_hash(&o.key));
                }
            }
            match page.next {
                Some(t) => token = Some(t),
                None => return Ok(set),
            }
        }
    }

    /// Live rows against the listing: a missing original, a wrong size, a thumbnail to make again.
    async fn check_rows(&mut self, seen: &HashMap<KeyHash, i64>, derived: &HashSet<KeyHash>) -> Result<()> {
        let mut after = Uuid::nil();
        loop {
            let rows: Vec<RowInfo> = sqlx::query_as(
                "SELECT id, owner_id, sha256, original_key, size_bytes, deleted_at FROM assets
                 WHERE purging_at IS NULL AND deleted_at IS NULL AND id > $1 ORDER BY id LIMIT $2",
            )
            .bind(after)
            .bind(ROWS_PER_PAGE)
            .fetch_all(self.db)
            .await?;
            let Some(last) = rows.last() else { return Ok(()) };
            after = last.id;
            self.stats.assets_checked += rows.len() as u64;

            // Rows whose key the listing did not show: ask the store before calling the original missing.
            let unseen: Vec<String> = rows
                .iter()
                .filter(|r| !seen.contains_key(&key_hash(&r.original_key)))
                .map(|r| r.original_key.clone())
                .collect();
            let mut heads: HashMap<String, Result<Option<Head>>> =
                reindex::head_many(self.storage, unseen, self.opts.concurrency)
                    .await
                    .into_iter()
                    .collect();

            let mut remake: Vec<&RowInfo> = Vec::new();
            for r in &rows {
                // Some(size): there; None: gone; error: unknown (left for the next run).
                let size: Result<Option<i64>> = match seen.get(&key_hash(&r.original_key)) {
                    Some(s) => Ok(Some(*s)),
                    None => match heads.remove(&r.original_key) {
                        Some(Ok(h)) => Ok(h.map(|h| h.size)),
                        Some(Err(e)) => Err(e),
                        None => Ok(None),
                    },
                };
                match size {
                    Err(e) => {
                        self.stats.errors += 1;
                        tracing::warn!(error = ?e, "integrity check: HEAD failed");
                    }
                    Ok(None) => self.missing_original(r).await?,
                    Ok(Some(actual)) if actual != r.size_bytes => {
                        self.bad.insert(r.id);
                        let detail = json!({
                            "reason": "size",
                            "expected_sha256": keys::hex(&r.sha256),
                            "size_bytes": r.size_bytes,
                            "actual_bytes": actual,
                            "previous_versions": self.previous_versions(&r.original_key).await,
                        });
                        self.finding(KIND_CHECKSUM, Some(r.owner_id), Some(r.id), &r.original_key, detail)
                            .await?;
                    }
                    Ok(Some(_)) => {
                        let (t, p) = (keys::thumb(r.owner_id, r.id), keys::preview(r.owner_id, r.id));
                        if !(derived.contains(&key_hash(&t)) && derived.contains(&key_hash(&p))) {
                            remake.push(r);
                        }
                    }
                }
            }
            self.remake_derived(remake).await?;
        }
    }

    async fn missing_original(&mut self, r: &RowInfo) -> Result<()> {
        self.bad.insert(r.id);
        let detail = json!({
            "size_bytes": r.size_bytes,
            "sha256": keys::hex(&r.sha256),
            "previous_versions": self.previous_versions(&r.original_key).await,
        });
        self.finding(
            KIND_MISSING_ORIGINAL,
            Some(r.owner_id),
            Some(r.id),
            &r.original_key,
            detail,
        )
        .await?;
        Ok(())
    }

    /// How many earlier (non-delete-marker) versions the object has: lets the page offer "Restore a previous version". Null
    /// when not looked up (a cap per run) or the store could not say.
    async fn previous_versions(&mut self, key: &str) -> Value {
        if self.version_lookups >= VERSION_LOOKUPS_PER_RUN {
            return Value::Null;
        }
        self.version_lookups += 1;
        match self.storage.key_versions(key).await {
            Ok(v) => json!(v.iter().filter(|v| !v.delete_marker && !v.is_latest).count()),
            Err(_) => Value::Null,
        }
    }

    /// Self-heal: queue the thumbnail job again for rows whose thumbnail or preview is gone. A row whose job is queued, running
    /// or failed is left to the queue; the store is asked once more before queueing (the listing may be older than the job).
    async fn remake_derived(&mut self, candidates: Vec<&RowInfo>) -> Result<()> {
        if candidates.is_empty() {
            return Ok(());
        }
        let ids: Vec<Uuid> = candidates.iter().map(|r| r.id).collect();
        let blocked: HashSet<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT asset_id FROM jobs
             WHERE kind = $1 AND asset_id = ANY($2) AND state IN ('queued', 'running', 'failed')",
        )
        .bind(jobs::KIND_THUMBNAIL)
        .bind(&ids)
        .fetch_all(self.db)
        .await?
        .into_iter()
        .collect();
        let todo: Vec<&RowInfo> = candidates.into_iter().filter(|r| !blocked.contains(&r.id)).collect();
        let derived_keys: Vec<String> = todo
            .iter()
            .flat_map(|r| [keys::thumb(r.owner_id, r.id), keys::preview(r.owner_id, r.id)])
            .collect();
        let heads: HashMap<String, Result<Option<Head>>> =
            reindex::head_many(self.storage, derived_keys, self.opts.concurrency)
                .await
                .into_iter()
                .collect();
        for r in todo {
            let absent = |k: String| matches!(heads.get(&k), Some(Ok(None)));
            if absent(keys::thumb(r.owner_id, r.id)) || absent(keys::preview(r.owner_id, r.id)) {
                jobs::requeue(self.db, jobs::KIND_THUMBNAIL, r.id, 1).await?;
                self.stats.derived_requeued += 1;
            }
        }
        Ok(())
    }

    /// Wait (bounded) for the upload pipeline to go quiet: uploads come before a background read of the library.
    async fn wait_for_quiet(&mut self) {
        if !self.opts.yield_to_jobs {
            return;
        }
        while self.yielded < MAX_YIELD
            && jobs::runnable_waiting(self.db, &[jobs::KIND_THUMBNAIL, jobs::KIND_METADATA])
                .await
                .unwrap_or(false)
        {
            tokio::time::sleep(Duration::from_secs(2)).await;
            self.yielded += Duration::from_secs(2);
        }
    }

    /// The rolling checksum sample: open mismatches first (are they still wrong?), then the rows verified longest ago.
    async fn verify_sample(&mut self) -> Result<()> {
        if self.opts.verify_bytes == 0 {
            return Ok(());
        }
        let cols = "id, owner_id, sha256, original_key, size_bytes, deleted_at";
        let skip = |bad: &HashSet<Uuid>| bad.iter().copied().collect::<Vec<_>>();
        let again: Vec<RowInfo> = sqlx::query_as(&format!(
            "SELECT {cols} FROM assets a WHERE purging_at IS NULL AND id <> ALL($1) AND EXISTS (
                 SELECT 1 FROM integrity_findings f
                 WHERE f.asset_id = a.id AND f.kind = $2 AND f.state = 'open' AND f.detail->>'reason' = 'hash')
             ORDER BY id LIMIT 100"
        ))
        .bind(skip(&self.bad))
        .bind(KIND_CHECKSUM)
        .fetch_all(self.db)
        .await?;
        let mut hashed = 0u64;
        if !self.verify(again, &mut hashed).await? {
            return Ok(());
        }
        loop {
            let batch: Vec<RowInfo> = sqlx::query_as(&format!(
                "SELECT {cols} FROM assets
                 WHERE purging_at IS NULL AND id <> ALL($1) AND (verified_at IS NULL OR verified_at < $2)
                 ORDER BY verified_at NULLS FIRST, created_at, id LIMIT $3"
            ))
            .bind(skip(&self.bad))
            .bind(self.started_at)
            .bind(VERIFY_BATCH)
            .fetch_all(self.db)
            .await?;
            if batch.is_empty() || !self.verify(batch, &mut hashed).await? {
                return Ok(());
            }
        }
    }

    /// Hash these rows' originals one after the other. Returns false once the budget or the time is spent.
    async fn verify(&mut self, rows: Vec<RowInfo>, hashed: &mut u64) -> Result<bool> {
        let mut verified: Vec<Uuid> = Vec::new();
        let mut more = true;
        for r in rows {
            let size = r.size_bytes.max(0) as u64;
            // The first file of a run is read whatever its size; after that the budget is a ceiling.
            if (*hashed > 0 && *hashed + size > self.opts.verify_bytes) || self.clock.elapsed() >= self.opts.max_runtime
            {
                more = false;
                break;
            }
            self.wait_for_quiet().await;
            match self.storage.hash_object(&r.original_key, None, size).await {
                Ok(Some(h)) => {
                    *hashed += h.bytes;
                    self.stats.bytes_hashed += h.bytes;
                    self.stats.assets_verified += 1;
                    // Checked, right or wrong: a bad file must not be first in line every night (it stays a finding).
                    verified.push(r.id);
                    if h.sha256[..] != r.sha256[..] || h.bytes != size {
                        let detail = json!({
                            "reason": "hash",
                            "expected_sha256": keys::hex(&r.sha256),
                            "actual_sha256": keys::hex(&h.sha256),
                            "size_bytes": r.size_bytes,
                            "actual_bytes": h.bytes,
                            "previous_versions": self.previous_versions(&r.original_key).await,
                        });
                        self.bad.insert(r.id);
                        self.finding(KIND_CHECKSUM, Some(r.owner_id), Some(r.id), &r.original_key, detail)
                            .await?;
                    }
                }
                // Gone since the listing.
                Ok(None) => {
                    // Not asked for again this run, whatever the row is.
                    self.bad.insert(r.id);
                    if r.deleted_at.is_none() {
                        self.missing_original(&r).await?;
                    }
                }
                Err(e) => {
                    self.stats.errors += 1;
                    self.bad.insert(r.id);
                    tracing::warn!(error = ?e, "integrity check: could not read an original");
                }
            }
        }
        if !verified.is_empty() {
            sqlx::query("UPDATE assets SET verified_at = now() WHERE id = ANY($1)")
                .bind(&verified)
                .execute(self.db)
                .await?;
            // A file that hashes right again is no longer a finding.
            let fixed: Vec<Uuid> = verified.iter().copied().filter(|id| !self.bad.contains(id)).collect();
            let n = sqlx::query(
                "UPDATE integrity_findings SET state = 'resolved', resolution = 'verified', resolved_at = now()
                 WHERE kind = $1 AND asset_id = ANY($2) AND state <> 'resolved'",
            )
            .bind(KIND_CHECKSUM)
            .bind(&fixed)
            .execute(self.db)
            .await?
            .rows_affected();
            self.stats.findings_resolved += n;
        }
        Ok(more)
    }

    /// Close the findings this run did not see again (the object came back, the row went, the file is the right size again).
    /// Checksum findings from a hash are closed only by hashing the file again.
    async fn resolve_gone(&mut self) -> Result<()> {
        let n = sqlx::query(
            "UPDATE integrity_findings SET state = 'resolved', resolution = 'no longer found', resolved_at = now()
             WHERE state <> 'resolved' AND last_run IS DISTINCT FROM $1
               AND (kind <> $2 OR detail->>'reason' = 'size')",
        )
        .bind(self.id)
        .bind(KIND_CHECKSUM)
        .execute(self.db)
        .await?
        .rows_affected();
        self.stats.findings_resolved += n;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Reading results

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RunRow {
    pub id: i64,
    pub trigger: String,
    pub state: String,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    pub error: Option<String>,
    pub objects_listed: i64,
    pub assets_checked: i64,
    pub assets_verified: i64,
    pub bytes_hashed: i64,
    pub missing_originals: i32,
    pub untracked_objects: i32,
    pub checksum_mismatches: i32,
    pub derived_requeued: i32,
    pub findings_resolved: i32,
}

/// The newest run (any state).
pub async fn last_run(db: &PgPool) -> Result<Option<RunRow>> {
    Ok(sqlx::query_as(
        "SELECT id, trigger, state, started_at, finished_at, error, objects_listed, assets_checked, assets_verified,
                bytes_hashed, missing_originals, untracked_objects, checksum_mismatches, derived_requeued, findings_resolved
         FROM integrity_runs ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(db)
    .await?)
}

/// How much of the library the rolling sample has covered.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Coverage {
    pub assets: i64,
    pub verified: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub oldest_verified_at: Option<OffsetDateTime>,
}

pub async fn coverage(db: &PgPool) -> Result<Coverage> {
    Ok(sqlx::query_as(
        "SELECT count(*) AS assets, count(verified_at) AS verified, min(verified_at) AS oldest_verified_at
         FROM assets WHERE purging_at IS NULL",
    )
    .fetch_one(db)
    .await?)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Finding {
    pub id: i64,
    pub kind: String,
    pub state: String,
    pub owner_id: Option<Uuid>,
    pub owner_email: Option<String>,
    pub asset_id: Option<Uuid>,
    pub key: String,
    pub detail: Value,
    pub resolution: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub first_seen_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<OffsetDateTime>,
}

const FINDING_COLS: &str = "f.id, f.kind, f.state, f.owner_id, u.email AS owner_email, f.asset_id, f.key, f.detail,
    f.resolution, f.first_seen_at, f.last_seen_at, f.resolved_at";

/// A page of findings, newest first. `kind` and `state` filter when given; `before` is the last id of the previous page.
pub async fn list_findings(
    db: &PgPool,
    kind: Option<&str>,
    state: &str,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<Finding>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {FINDING_COLS} FROM integrity_findings f LEFT JOIN users u ON u.id = f.owner_id
         WHERE f.state = $1 AND ($2::text IS NULL OR f.kind = $2) AND ($3::bigint IS NULL OR f.id < $3)
         ORDER BY f.id DESC LIMIT $4"
    ))
    .bind(state)
    .bind(kind)
    .bind(before)
    .bind(limit.clamp(1, 200))
    .fetch_all(db)
    .await?)
}

pub async fn get_finding(db: &PgPool, id: i64) -> Result<Option<Finding>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {FINDING_COLS} FROM integrity_findings f LEFT JOIN users u ON u.id = f.owner_id WHERE f.id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

// ---------------------------------------------------------------------------------------------------------------------
// Actions (admin only; the route checks the role and writes the audit entry)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Missing original or checksum mismatch: make an earlier version with the stored checksum the current one again.
    RestoreVersion,
    /// Missing original: move the photo to the trash.
    Trash,
    /// Leave it: it stays true but is no longer counted and is not raised again.
    Acknowledge,
    /// Untracked object: make a photo of it (`reindex::import_object`).
    RestorePhoto,
    /// Untracked object: delete every version of it for good.
    DeleteObject,
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "restore_version" => Self::RestoreVersion,
            "trash" => Self::Trash,
            "acknowledge" => Self::Acknowledge,
            "restore_photo" => Self::RestorePhoto,
            "delete_object" => Self::DeleteObject,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::RestoreVersion => "restore_version",
            Self::Trash => "trash",
            Self::Acknowledge => "acknowledge",
            Self::RestorePhoto => "restore_photo",
            Self::DeleteObject => "delete_object",
        }
    }

    /// What a finding resolved by this action records.
    fn resolution(self) -> &'static str {
        match self {
            Self::RestoreVersion => "restored a previous version",
            Self::Trash => "moved to the trash",
            Self::Acknowledge => "acknowledged",
            Self::RestorePhoto => "restored as a photo",
            Self::DeleteObject => "object deleted",
        }
    }

    fn applies_to(self, kind: &str) -> bool {
        match self {
            Self::RestoreVersion => kind == KIND_MISSING_ORIGINAL || kind == KIND_CHECKSUM,
            Self::Trash => kind == KIND_MISSING_ORIGINAL,
            Self::Acknowledge => true,
            Self::RestorePhoto | Self::DeleteObject => kind == KIND_UNTRACKED,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ActError {
    #[error("no such finding")]
    NotFound,
    /// The action cannot be done now (wrong kind, already resolved another way, nothing to restore); the text is for the admin.
    #[error("{0}")]
    Conflict(String),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

impl From<sqlx::Error> for ActError {
    fn from(e: sqlx::Error) -> Self {
        Self::Failed(e.into())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Acted {
    /// False when the finding had already been resolved by this same action (nothing was done now).
    pub changed: bool,
    pub finding: Finding,
    /// Small facts for the audit entry.
    pub detail: Value,
}

/// Do `action` on finding `id` as admin `actor`. Idempotent: repeating an action that already resolved the finding changes
/// nothing; a different action on a resolved finding is refused. `confirmed` must be true for [`Action::DeleteObject`].
pub async fn act(
    db: &PgPool,
    storage: &Storage,
    id: i64,
    action: Action,
    actor: Uuid,
    confirmed: bool,
) -> std::result::Result<Acted, ActError> {
    let f = get_finding(db, id).await?.ok_or(ActError::NotFound)?;
    if !action.applies_to(&f.kind) {
        return Err(ActError::Conflict(
            "that action does not apply to this kind of finding".into(),
        ));
    }
    let done = |changed: bool, finding: Finding, detail: Value| Acted {
        changed,
        finding,
        detail,
    };
    match f.state.as_str() {
        "resolved" if f.resolution.as_deref() == Some(action.resolution()) => return Ok(done(false, f, json!({}))),
        "resolved" => {
            return Err(ActError::Conflict(format!(
                "this finding is already resolved ({})",
                f.resolution.as_deref().unwrap_or("no longer found")
            )));
        }
        "acknowledged" if action == Action::Acknowledge => return Ok(done(false, f, json!({}))),
        _ => {}
    }
    if action == Action::DeleteObject && !confirmed {
        return Err(ActError::Conflict("deleting an object needs a confirmation".into()));
    }
    let detail = match action {
        Action::Acknowledge => json!({}),
        Action::RestoreVersion => restore_version(db, storage, &f).await?,
        Action::Trash => trash(db, &f).await?,
        Action::RestorePhoto => restore_photo(db, storage, &f).await?,
        Action::DeleteObject => delete_object(db, storage, &f).await?,
    };
    let state = if action == Action::Acknowledge {
        "acknowledged"
    } else {
        "resolved"
    };
    sqlx::query(
        "UPDATE integrity_findings SET state = $2, resolution = $3, resolved_at = now(), resolved_by = $4
         WHERE id = $1 AND state <> 'resolved'",
    )
    .bind(id)
    .bind(state)
    .bind(action.resolution())
    .bind(actor)
    .execute(db)
    .await?;
    let finding = get_finding(db, id).await?.ok_or(ActError::NotFound)?;
    Ok(done(true, finding, detail))
}

/// The row a finding is about, if it is still there and not being purged.
async fn live_asset(db: &PgPool, f: &Finding) -> std::result::Result<RowInfo, ActError> {
    let Some(asset) = f.asset_id else {
        return Err(ActError::Conflict("the finding has no photo".into()));
    };
    sqlx::query_as::<_, RowInfo>(
        "SELECT id, owner_id, sha256, original_key, size_bytes, deleted_at FROM assets WHERE id = $1 AND purging_at IS NULL",
    )
    .bind(asset)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ActError::Conflict("the photo no longer exists".into()))
}

async fn restore_version(db: &PgPool, storage: &Storage, f: &Finding) -> std::result::Result<Value, ActError> {
    let a = live_asset(db, f).await?;
    let size = a.size_bytes.max(0) as u64;
    // Perhaps it is fine now (another admin, or the object was put back): then there is nothing to restore.
    if let Some(h) = storage.hash_object(&a.original_key, None, size).await?
        && h.sha256[..] == a.sha256[..]
        && h.bytes == size
    {
        sqlx::query("UPDATE assets SET verified_at = now() WHERE id = $1")
            .bind(a.id)
            .execute(db)
            .await?;
        return Ok(json!({"already_intact": true}));
    }
    let mut tried = 0;
    for v in storage.key_versions(&a.original_key).await? {
        let Some(vid) = v.version_id.as_deref() else { continue };
        if v.delete_marker || v.is_latest || v.size != a.size_bytes {
            continue;
        }
        if tried == VERSIONS_TRIED {
            break;
        }
        tried += 1;
        let Some(h) = storage.hash_object(&a.original_key, Some(vid), size).await? else {
            continue;
        };
        if h.sha256[..] != a.sha256[..] || h.bytes != size {
            continue;
        }
        if v.size > crate::scan::MAX_COPY_BYTES {
            return Err(ActError::Conflict(
                "the earlier version is too large to copy (over 5 GiB)".into(),
            ));
        }
        storage.restore_version(&a.original_key, vid).await?;
        let now = storage.head(&a.original_key).await?;
        if now.map(|h| h.size) != Some(a.size_bytes) {
            return Err(ActError::Failed(anyhow::anyhow!(
                "the restored object is not the size it should be"
            )));
        }
        sqlx::query("UPDATE assets SET verified_at = now() WHERE id = $1")
            .bind(a.id)
            .execute(db)
            .await?;
        return Ok(json!({"version_restored": true}));
    }
    Err(ActError::Conflict(
        "no earlier version of the object has the stored checksum".into(),
    ))
}

async fn trash(db: &PgPool, f: &Finding) -> std::result::Result<Value, ActError> {
    let a = live_asset(db, f).await?;
    let primary: Option<Uuid> = sqlx::query_scalar("SELECT primary_id FROM assets WHERE id = $1")
        .bind(a.id)
        .fetch_one(db)
        .await?;
    if a.deleted_at.is_none() {
        reindex::trash_missing(db, a.id, primary).await?;
    }
    Ok(json!({}))
}

async fn restore_photo(db: &PgPool, storage: &Storage, f: &Finding) -> std::result::Result<Value, ActError> {
    if f.detail.get("reason").and_then(Value::as_str) != Some("no_row") {
        return Err(ActError::Conflict(
            "this object cannot be made a photo; delete it or leave it".into(),
        ));
    }
    match reindex::import_object(db, storage, &f.key).await? {
        reindex::Imported::Restored(id) | reindex::Imported::AlreadyIndexed(id) => Ok(json!({"asset_id": id})),
        reindex::Imported::Refused(why) => Err(ActError::Conflict(why.into())),
        reindex::Imported::Gone => Ok(json!({"gone": true})),
    }
}

async fn delete_object(db: &PgPool, storage: &Storage, f: &Finding) -> std::result::Result<Value, ActError> {
    if !f.key.starts_with(keys::ORIGINALS_PREFIX) {
        return Err(ActError::Conflict(
            "only objects under originals/ can be deleted here".into(),
        ));
    }
    // Something may have adopted the object since the run: never delete a row's original.
    let used: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM assets WHERE original_key = $1)")
        .bind(&f.key)
        .fetch_one(db)
        .await?;
    if used {
        return Err(ActError::Conflict(
            "a photo now uses this object; it was not deleted".into(),
        ));
    }
    let n = storage.delete_all_versions(std::slice::from_ref(&f.key)).await?;
    Ok(json!({"versions_deleted": n}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn the_boundary_is_the_most_recent_scheduled_hour() {
        let now = datetime!(2026-06-10 14:30 UTC);
        assert_eq!(last_boundary(now, 3), datetime!(2026-06-10 03:00 UTC));
        assert_eq!(last_boundary(now, 14), datetime!(2026-06-10 14:00 UTC));
        assert_eq!(last_boundary(now, 15), datetime!(2026-06-09 15:00 UTC));
        assert_eq!(
            last_boundary(datetime!(2026-06-10 00:10 UTC), 0),
            datetime!(2026-06-10 00:00 UTC)
        );
    }

    #[test]
    fn the_key_digest_tells_keys_apart() {
        assert_eq!(key_hash("originals/a"), key_hash("originals/a"));
        assert_ne!(key_hash("originals/a"), key_hash("originals/b"));
    }
}
