//! Disaster recovery: reconcile the `assets` table with the bucket. See `docs/recovery.md`.
//!
//! The bucket is the source of truth. `run` scans `originals/` page by page (current versions only), looks each page
//! up in Postgres and sorts every object into a category:
//!
//! * present: a row exists for this owner and content hash (the usual case), counted per user;
//! * orphan: no row. Recreated (`apply`) from the key (owner, hash, extension), the object's own metadata
//!   (`objmeta`) and, when present, the user's daily manifest (`manifest`), then the usual metadata + thumbnail jobs
//!   are queued so EXIF, dimensions, thumbnails and RAW/JPEG pairing regenerate. Owners that no longer exist are
//!   skipped unless `assign_to` names an existing user;
//! * missing: a row whose original is gone. Reported; `mark_missing` moves live rows to the trash (rows are never
//!   deleted here);
//! * thumbnails: live rows whose `derived/` thumbnail or preview is gone (and have no thumbnail job pending):
//!   reported; `rethumb` re-queues the job.
//!
//! Nothing in the bucket is deleted or modified, except by the opt-in `backfill_metadata`, which copies an original
//! onto itself (a new version on a versioned bucket). Without `apply` nothing is written anywhere. Every write is
//! idempotent (`ON CONFLICT DO NOTHING`, idempotent job enqueue), so an interrupted run is simply run again.
//!
//! Rows queued for permanent deletion (`purging_at`) are never touched and their objects are never resurrected:
//! an object that still has such a row is counted as `purging` and skipped.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::{
    edits, identity, jobs, keys,
    manifest::{self, UserManifest},
    objmeta,
    scan::{Head, Listed, MAX_COPY_BYTES},
    storage::Storage,
    tags,
};

const SAMPLE_CAP: usize = 10;

#[derive(Debug, Clone)]
pub struct Options {
    /// Write to the database (and, for `backfill_metadata`, the bucket). False = dry run.
    pub apply: bool,
    /// Give orphans whose owner no longer exists to this (existing) user instead of skipping them.
    pub assign_to: Option<Uuid>,
    pub mark_missing: bool,
    pub rethumb: bool,
    pub backfill_metadata: bool,
    /// Read `manifests/` to restore names, trash state, favorites and albums.
    pub use_manifests: bool,
    /// Parallel HEAD requests.
    pub concurrency: usize,
    /// Leave alone objects modified more recently than this: an upload may be completing right now.
    pub min_age: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            apply: false,
            assign_to: None,
            mark_missing: false,
            rethumb: false,
            backfill_metadata: false,
            use_manifests: true,
            concurrency: 16,
            min_age: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct UserReport {
    /// Objects under `originals/{user}/`.
    pub objects: u64,
    /// Objects that already have a row.
    pub present: u64,
    /// Orphans recreated (apply) or that would be (dry run).
    pub orphans: u64,
    /// Orphans left alone because the owner does not exist (and no `assign_to`).
    pub orphans_skipped: u64,
    /// Rows whose original is gone.
    pub missing_originals: u64,
    /// Live rows without thumbnail or preview.
    pub thumbs_missing: u64,
    /// The user id has no `users` row.
    pub unknown_user: bool,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Report {
    pub mode: &'static str,
    pub scanned_objects: u64,
    /// Keys under `originals/` that are not `originals/{user}/{xx}/{sha256}.{ext}`; never touched.
    pub unparseable_keys: u64,
    pub too_recent: u64,
    pub present: u64,
    /// Present objects whose row is in the trash.
    pub trashed_rows: u64,
    /// Present objects whose row is queued for permanent deletion (never resurrected, never modified).
    pub purging_rows: u64,
    /// Objects whose content already has a row under another key (e.g. a different extension).
    pub alias_objects: u64,
    pub orphans: u64,
    pub orphans_recreated: u64,
    pub orphans_unknown_user: u64,
    pub orphans_reassigned: u64,
    pub orphans_vanished: u64,
    pub missing_originals: u64,
    pub missing_marked_trashed: u64,
    pub rows_checked: u64,
    pub thumbs_missing: u64,
    pub thumbs_requeued: u64,
    pub backfill_needed: u64,
    pub backfilled: u64,
    pub backfill_too_large: u64,
    pub manifest_users: u64,
    pub manifest_files: u64,
    pub manifest_bad_lines: u64,
    /// Users listed in the newest account manifest.
    pub users_in_manifest: u64,
    /// Recreated (or that would be): the database lacked them. Their passwords are locked: `cyanotype reset-link`.
    pub users_restored: u64,
    /// Already in the database (same id).
    pub users_present: u64,
    /// The email is taken by an account with another id: left alone (their photos need `--assign-to`).
    pub users_conflict: u64,
    /// Server settings (default limits) put back.
    pub settings_restored: u64,
    pub albums_restored: u64,
    /// Of those, smart albums (their saved rules).
    pub smart_albums_restored: u64,
    pub album_items_restored: u64,
    /// Share links recreated (or that would be). Password-protected ones come back locked.
    pub shares_restored: u64,
    /// Share links the manifest lists but whose album or photos are gone.
    pub shares_skipped: u64,
    pub pairs_restored: u64,
    pub favorites_restored: u64,
    pub trashed_restored: u64,
    /// Photos put back as hidden / archived (or that would be): a hidden photo must come back hidden.
    pub hidden_restored: u64,
    pub archived_restored: u64,
    /// Photos put back with their star rating (or that would be).
    pub ratings_restored: u64,
    /// Tags recreated (the owner had none by that name) and photo-tag links put back (or that would be).
    pub tags_restored: u64,
    pub tag_links_restored: u64,
    /// Objects under `edits/` (a photo's corrected date, location and caption).
    pub edits_objects: u64,
    /// Applied to a row that had none or older ones (or would be, in a dry run).
    pub edits_restored: u64,
    /// Already what the row says.
    pub edits_current: u64,
    /// For a photo that has no row (in a dry run, an orphan that `--apply` would recreate first), or a user that does not exist.
    pub edits_without_photo: u64,
    /// Not valid objects (left alone).
    pub edits_bad: u64,
    /// Requests or writes that failed; the run continued. Non-zero means re-run.
    pub errors: u64,
    pub users: BTreeMap<Uuid, UserReport>,
    /// A few example keys per category.
    pub samples: BTreeMap<&'static str, Vec<String>>,
}

impl Report {
    fn sample(&mut self, cat: &'static str, key: &str) {
        let v = self.samples.entry(cat).or_default();
        if v.len() < SAMPLE_CAP {
            v.push(key.to_string());
        }
    }

    fn user(&mut self, id: Uuid) -> &mut UserReport {
        self.users.entry(id).or_default()
    }

    /// Human-readable summary (what the CLI prints; also fine for a GitHub step summary).
    pub fn text(&self) -> String {
        use std::fmt::Write;
        let apply = self.mode == "apply";
        let w = |apply_word: &str, dry_word: &str| {
            if apply {
                apply_word.to_string()
            } else {
                dry_word.to_string()
            }
        };
        let mut s = String::new();
        let _ = writeln!(
            s,
            "reindex ({})",
            if apply {
                "APPLIED"
            } else {
                "DRY RUN: nothing was changed"
            }
        );
        let _ = writeln!(s, "  objects scanned in originals/      {}", self.scanned_objects);
        let _ = writeln!(s, "  already indexed                    {}", self.present);
        let _ = writeln!(s, "    of which in the trash            {}", self.trashed_rows);
        let _ = writeln!(s, "    of which being purged (skipped)  {}", self.purging_rows);
        let _ = writeln!(s, "  not in the index (orphans)         {}", self.orphans);
        let _ = writeln!(
            s,
            "    {:<33}{}",
            w("recreated", "would be recreated"),
            self.orphans_recreated
        );
        let _ = writeln!(s, "    skipped, owner does not exist    {}", self.orphans_unknown_user);
        let _ = writeln!(s, "    assigned to --assign-to user     {}", self.orphans_reassigned);
        let _ = writeln!(s, "    too recent (upload in flight?)   {}", self.too_recent);
        let _ = writeln!(s, "  extra copies of indexed content    {}", self.alias_objects);
        let _ = writeln!(s, "  keys not shaped like originals     {}", self.unparseable_keys);
        let _ = writeln!(s, "  rows checked                       {}", self.rows_checked);
        let _ = writeln!(s, "  rows whose original is missing     {}", self.missing_originals);
        let _ = writeln!(
            s,
            "    {:<33}{}",
            w("moved to trash", "would move to trash (--mark-missing)"),
            self.missing_marked_trashed
        );
        let _ = writeln!(s, "  rows missing thumbnail/preview     {}", self.thumbs_missing);
        let _ = writeln!(
            s,
            "    {:<33}{}",
            w("jobs re-queued", "jobs would be queued (--rethumb)"),
            self.thumbs_requeued
        );
        let _ = writeln!(s, "  originals without metadata         {}", self.backfill_needed);
        let _ = writeln!(
            s,
            "    {:<33}{}",
            w("backfilled", "would backfill (--backfill-metadata)"),
            self.backfilled
        );
        let _ = writeln!(s, "    too large to copy (> 5 GiB)      {}", self.backfill_too_large);
        let _ = writeln!(
            s,
            "  manifests: users / files / bad     {} / {} / {}",
            self.manifest_users, self.manifest_files, self.manifest_bad_lines
        );
        let _ = writeln!(
            s,
            "    albums / items / pairs restored  {} / {} / {}",
            self.albums_restored, self.album_items_restored, self.pairs_restored
        );
        let _ = writeln!(s, "    smart albums (saved rules)       {}", self.smart_albums_restored);
        let _ = writeln!(
            s,
            "    share links restored / skipped   {} / {}",
            self.shares_restored, self.shares_skipped
        );
        let _ = writeln!(
            s,
            "  accounts in manifest / {} / have  {} / {} / {}",
            w("restored", "to restore"),
            self.users_in_manifest,
            self.users_restored,
            self.users_present
        );
        let _ = writeln!(
            s,
            "    email taken by another account   {}  (settings restored: {})",
            self.users_conflict, self.settings_restored
        );
        if self.users_restored > 0 && apply {
            let _ = writeln!(
                s,
                "    restored accounts have LOCKED passwords: run `cyanotype reset-link --email ...` for each owner"
            );
        }
        let _ = writeln!(
            s,
            "    trash state / favorites          {} / {}",
            self.trashed_restored, self.favorites_restored
        );
        let _ = writeln!(
            s,
            "    hidden / archived                {} / {}",
            self.hidden_restored, self.archived_restored
        );
        let _ = writeln!(s, "    star ratings                     {}", self.ratings_restored);
        let _ = writeln!(
            s,
            "    tags / photo-tag links           {} / {}",
            self.tags_restored, self.tag_links_restored
        );
        let _ = writeln!(
            s,
            "  edits: objects / {} / current     {} / {} / {}",
            w("restored", "would restore"),
            self.edits_objects,
            self.edits_restored,
            self.edits_current
        );
        let _ = writeln!(
            s,
            "    no photo for them / not valid    {} / {}",
            self.edits_without_photo, self.edits_bad
        );
        let _ = writeln!(s, "  errors                             {}", self.errors);
        let _ = writeln!(s, "per user:");
        for (id, u) in &self.users {
            let _ = writeln!(
                s,
                "  {id}{}: objects {}, indexed {}, orphans {}, skipped {}, missing originals {}, no thumbs {}",
                if u.unknown_user { " (no such user)" } else { "" },
                u.objects,
                u.present,
                u.orphans,
                u.orphans_skipped,
                u.missing_originals,
                u.thumbs_missing
            );
        }
        for (cat, keys) in &self.samples {
            let _ = writeln!(s, "examples, {cat}:");
            for k in keys {
                let _ = writeln!(s, "  {k}");
            }
        }
        if self.errors > 0 {
            let _ = writeln!(s, "INCOMPLETE: {} error(s); see the logs and run again.", self.errors);
        }
        s
    }
}

fn hash_str(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[derive(sqlx::FromRow)]
struct RowInfo {
    owner_id: Uuid,
    sha256: Vec<u8>,
    original_key: String,
    original_name: String,
    content_type: String,
    created_at: OffsetDateTime,
    file_modified_at: Option<OffsetDateTime>,
    deleted_at: Option<OffsetDateTime>,
    purging_at: Option<OffsetDateTime>,
}

/// HEAD many keys with bounded parallelism.
pub(crate) async fn head_many(
    storage: &Storage,
    keys: Vec<String>,
    concurrency: usize,
) -> Vec<(String, Result<Option<Head>>)> {
    let mut out = Vec::with_capacity(keys.len());
    for chunk in keys.chunks(concurrency.max(1)) {
        let mut set = JoinSet::new();
        for k in chunk {
            let (s, k) = (storage.clone(), k.clone());
            set.spawn(async move {
                let r = s.head(&k).await;
                (k, r)
            });
        }
        while let Some(j) = set.join_next().await {
            match j {
                Ok(v) => out.push(v),
                Err(e) => out.push((String::new(), Err(anyhow::anyhow!("head task failed: {e}")))),
            }
        }
    }
    out
}

struct Ctx<'a> {
    db: &'a PgPool,
    storage: &'a Storage,
    opts: &'a Options,
    has_favorite: bool,
    /// The schema has `hidden` and `archived` (migration 0020).
    has_state: bool,
    /// The schema has `rating` and `rating_source` (migration 0024).
    has_rating: bool,
    known_users: HashMap<Uuid, bool>,
    manifests: HashMap<Uuid, Arc<UserManifest>>,
    manifest_keys: BTreeMap<Uuid, Vec<String>>,
    /// (owner, hash) of every orphan recreated (or that would be), for album and pair restoration.
    recreated: Vec<(Uuid, [u8; 32])>,
    report: Report,
}

/// Which of the optional `assets` columns this schema has: `(favorite, hidden + archived, rating + rating_source)`.
async fn schema_flags(db: &PgPool) -> Result<(bool, bool, bool)> {
    let has = |column: &'static str| async move {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM information_schema.columns
                            WHERE table_schema = current_schema() AND table_name = 'assets' AND column_name = $1)",
        )
        .bind(column)
        .fetch_one(db)
        .await
    };
    Ok((
        has("favorite").await?,
        has("hidden").await?,
        has("rating_source").await?,
    ))
}

pub async fn run(db: &PgPool, storage: &Storage, opts: &Options) -> Result<Report> {
    anyhow::ensure!(opts.concurrency >= 1, "concurrency must be at least 1");
    let (has_favorite, has_state, has_rating) = schema_flags(db).await?;
    if let Some(u) = opts.assign_to {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
            .bind(u)
            .fetch_one(db)
            .await?;
        anyhow::ensure!(ok, "--assign-to user {u} does not exist");
    }
    let mut cx = Ctx {
        db,
        storage,
        opts,
        has_favorite,
        has_state,
        has_rating,
        known_users: HashMap::new(),
        manifests: HashMap::new(),
        manifest_keys: if opts.use_manifests {
            manifest::list_all(storage).await.context("list manifests")?
        } else {
            BTreeMap::new()
        },
        recreated: Vec::new(),
        report: Report {
            mode: if opts.apply { "apply" } else { "dry-run" },
            ..Default::default()
        },
    };
    tracing::info!(
        apply = opts.apply,
        manifests_users = cx.manifest_keys.len(),
        "reindex starting"
    );

    // First: the accounts, because the photos' keys name their owners and an owner without an account is skipped.
    restore_users(&mut cx).await?;
    let derived = scan_derived(storage).await?;
    let seen = scan_originals(&mut cx).await?;
    check_rows(&mut cx, &seen, &derived).await?;
    restore_state(&mut cx).await?;
    restore_edits(&mut cx).await?;

    tracing::info!(
        orphans = cx.report.orphans,
        recreated = cx.report.orphans_recreated,
        missing = cx.report.missing_originals,
        thumbs_missing = cx.report.thumbs_missing,
        errors = cx.report.errors,
        apply = opts.apply,
        "reindex finished"
    );
    Ok(cx.report)
}

/// The outcome of [`import_object`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Imported {
    /// A row was made for the object (with its metadata and thumbnail jobs queued); its id.
    Restored(Uuid),
    /// The owner already has a row for this content (so nothing was made); its id.
    AlreadyIndexed(Uuid),
    /// The key is not an original's, or its owner has no account here, or the photo is being deleted for good.
    Refused(&'static str),
    /// The object is not in the bucket (any more).
    Gone,
}

/// Make a photo of one object under `originals/` that has no row, the way `run --apply` does for an orphan: the same row (name,
/// time and content type from the object's own metadata and the owner's newest manifest, with its trash, favourite, hidden,
/// archived and rating state), the same `metadata` + `thumbnail` jobs, and the photo's `edits/` object applied when there is one.
/// Albums, pairs, tags and share links are not restored here (they belong to a whole-library `reindex`). Idempotent: an owner who
/// already has the content gets that row's id back.
pub async fn import_object(db: &PgPool, storage: &Storage, key: &str) -> Result<Imported> {
    let Some(p) = keys::parse_original(key) else {
        return Ok(Imported::Refused("the key is not the key of an original"));
    };
    let owner_exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(p.user)
        .fetch_one(db)
        .await?;
    if !owner_exists {
        return Ok(Imported::Refused("the owner of the object has no account here"));
    }
    let existing: Option<(Uuid, bool)> =
        sqlx::query_as("SELECT id, purging_at IS NOT NULL FROM assets WHERE owner_id = $1 AND sha256 = $2")
            .bind(p.user)
            .bind(&p.sha256[..])
            .fetch_optional(db)
            .await?;
    match existing {
        Some((_, true)) => return Ok(Imported::Refused("the photo is being deleted for good")),
        Some((id, false)) => return Ok(Imported::AlreadyIndexed(id)),
        None => {}
    }
    if storage.head(key).await?.is_none() {
        return Ok(Imported::Gone);
    }
    let opts = Options {
        apply: true,
        min_age: Duration::ZERO,
        concurrency: 1,
        ..Options::default()
    };
    let (has_favorite, has_state, has_rating) = schema_flags(db).await?;
    // Only this owner's manifests.
    let mut found = Vec::new();
    let mut token = None;
    loop {
        let page = storage.list_page(&keys::manifest_prefix(p.user), token).await?;
        found.extend(
            page.objects
                .into_iter()
                .map(|o| o.key)
                .filter(|k| manifest::key_date(k).is_some()),
        );
        match page.next {
            Some(t) => token = Some(t),
            None => break,
        }
    }
    found.sort_by(|a, b| b.cmp(a)); // YYYYMMDD sorts lexically, newest first
    let mut manifest_keys = BTreeMap::new();
    if !found.is_empty() {
        manifest_keys.insert(p.user, found);
    }
    let mut cx = Ctx {
        db,
        storage,
        opts: &opts,
        has_favorite,
        has_state,
        has_rating,
        known_users: HashMap::from([(p.user, true)]),
        manifests: HashMap::new(),
        manifest_keys,
        recreated: Vec::new(),
        report: Report {
            mode: "apply",
            ..Default::default()
        },
    };
    let listed = Listed {
        key: key.to_string(),
        size: 0,
        last_modified: None,
    };
    process_orphans(&mut cx, vec![(listed, p.clone())]).await?;
    anyhow::ensure!(cx.report.errors == 0, "could not make a photo of the object");
    let id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM assets WHERE owner_id = $1 AND sha256 = $2")
        .bind(p.user)
        .bind(&p.sha256[..])
        .fetch_optional(db)
        .await?;
    let Some(id) = id else {
        return Ok(if cx.report.orphans_vanished > 0 {
            Imported::Gone
        } else {
            Imported::Refused("the object could not be made a photo")
        });
    };
    let edits_key = keys::edits(p.user, &p.sha256);
    if storage.head(&edits_key).await?.is_some()
        && let Err(e) = restore_edit(&mut cx, &edits_key).await
    {
        tracing::warn!(error = ?e, "could not restore the edits of an imported photo");
    }
    Ok(if cx.report.orphans_recreated > 0 {
        Imported::Restored(id)
    } else {
        Imported::AlreadyIndexed(id)
    })
}

/// Users and server settings from the newest account manifest (`identity`): recreate the accounts the database lacks (with a
/// locked password) so their photos have owners. Nothing that exists is changed. Skipped with `--no-manifest`.
async fn restore_users(cx: &mut Ctx<'_>) -> Result<()> {
    if !cx.opts.use_manifests {
        return Ok(());
    }
    let roster = match identity::load_newest(cx.storage).await {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(()),
        Err(e) => {
            cx.report.errors += 1;
            tracing::warn!(error = ?e, "could not read the account manifest; continuing without");
            return Ok(());
        }
    };
    cx.report.users_in_manifest = roster.users.len() as u64;
    cx.report.manifest_bad_lines += roster.bad_lines as u64;
    let out = match identity::restore(cx.db, &roster, cx.opts.apply).await {
        Ok(o) => o,
        Err(e) => {
            cx.report.errors += 1;
            tracing::warn!(error = ?e, "could not restore accounts from the manifest");
            return Ok(());
        }
    };
    cx.report.users_restored = out.restored.len() as u64;
    cx.report.users_present = out.present;
    cx.report.users_conflict = out.conflicts.len() as u64;
    cx.report.settings_restored = out.settings_restored;
    for id in &out.restored {
        // In a dry run too: the photos of an account that would be recreated are not "owner unknown".
        cx.known_users.insert(*id, true);
    }
    for id in &out.conflicts {
        cx.report.sample("accounts_email_taken", &id.to_string());
    }
    tracing::info!(
        restored = out.restored.len(),
        present = out.present,
        conflicts = out.conflicts.len(),
        apply = cx.opts.apply,
        "accounts from the manifest"
    );
    Ok(())
}

/// Hashes of the `thumb.webp` / `preview.webp` keys that exist.
async fn scan_derived(storage: &Storage) -> Result<HashSet<u64>> {
    let mut set = HashSet::new();
    let mut token = None;
    loop {
        let page = storage
            .list_page(keys::DERIVED_PREFIX, token)
            .await
            .context("list derived/")?;
        for o in &page.objects {
            if o.key.ends_with("/thumb.webp") || o.key.ends_with("/preview.webp") {
                set.insert(hash_str(&o.key));
            }
        }
        match page.next {
            Some(t) => token = Some(t),
            None => return Ok(set),
        }
    }
}

async fn known_users(cx: &mut Ctx<'_>, ids: &[Uuid]) -> Result<()> {
    let unknown: Vec<Uuid> = ids
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|u| !cx.known_users.contains_key(u))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let found: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE id = ANY($1)")
        .bind(&unknown)
        .fetch_all(cx.db)
        .await?;
    let found: HashSet<Uuid> = found.into_iter().map(|r| r.0).collect();
    for u in unknown {
        cx.known_users.insert(u, found.contains(&u));
        cx.report.user(u).unknown_user = !found.contains(&u);
    }
    Ok(())
}

async fn manifest_for(cx: &mut Ctx<'_>, user: Uuid) -> Option<Arc<UserManifest>> {
    if let Some(m) = cx.manifests.get(&user) {
        return Some(m.clone());
    }
    let keys = cx.manifest_keys.get(&user)?;
    match manifest::load(cx.storage, user, keys).await {
        Ok(m) => {
            cx.report.manifest_users += 1;
            cx.report.manifest_files += m.files as u64;
            cx.report.manifest_bad_lines += m.bad_lines as u64;
            let m = Arc::new(m);
            cx.manifests.insert(user, m.clone());
            Some(m)
        }
        Err(e) => {
            cx.report.errors += 1;
            tracing::warn!(user_id = %user, error = ?e, "could not read the user's manifests; continuing without");
            None
        }
    }
}

/// Scan `originals/`; returns the hashes of every key seen (so rows can be checked without holding the keys).
async fn scan_originals(cx: &mut Ctx<'_>) -> Result<HashSet<u64>> {
    let mut seen = HashSet::new();
    let mut token = None;
    let now = OffsetDateTime::now_utc();
    loop {
        let page = cx
            .storage
            .list_page(keys::ORIGINALS_PREFIX, token)
            .await
            .context("list originals/")?;
        let mut parsed: Vec<(Listed, keys::ParsedOriginal)> = Vec::new();
        for o in page.objects {
            cx.report.scanned_objects += 1;
            let Some(p) = keys::parse_original(&o.key) else {
                cx.report.unparseable_keys += 1;
                cx.report.sample("unparseable_keys", &o.key);
                continue;
            };
            seen.insert(hash_str(&o.key));
            cx.report.user(p.user).objects += 1;
            parsed.push((o, p));
        }
        let owners: Vec<Uuid> = parsed.iter().map(|(_, p)| p.user).collect();
        known_users(cx, &owners).await?;

        let rows = sqlx::query_as::<_, RowInfo>(
            "SELECT a.owner_id, a.sha256, a.original_key, a.original_name, a.content_type, a.created_at,
                    a.file_modified_at, a.deleted_at, a.purging_at
             FROM assets a JOIN unnest($1::uuid[], $2::bytea[]) AS t(o, s) ON a.owner_id = t.o AND a.sha256 = t.s",
        )
        .bind(&owners)
        .bind(parsed.iter().map(|(_, p)| p.sha256.to_vec()).collect::<Vec<_>>())
        .fetch_all(cx.db)
        .await?;
        let by_content: HashMap<(Uuid, Vec<u8>), RowInfo> =
            rows.into_iter().map(|r| ((r.owner_id, r.sha256.clone()), r)).collect();

        let mut orphans: Vec<(Listed, keys::ParsedOriginal)> = Vec::new();
        let mut to_head: Vec<String> = Vec::new();
        for (o, p) in parsed {
            match by_content.get(&(p.user, p.sha256.to_vec())) {
                Some(row) if row.original_key == o.key => {
                    cx.report.present += 1;
                    cx.report.user(p.user).present += 1;
                    if row.purging_at.is_some() {
                        cx.report.purging_rows += 1;
                    } else {
                        if row.deleted_at.is_some() {
                            cx.report.trashed_rows += 1;
                        }
                        if cx.opts.backfill_metadata {
                            to_head.push(o.key.clone());
                        }
                    }
                }
                Some(_) => {
                    cx.report.alias_objects += 1;
                    cx.report.sample("alias_objects", &o.key);
                }
                None => {
                    let age_ok = o
                        .last_modified
                        .is_none_or(|t| now - t >= time::Duration::try_from(cx.opts.min_age).unwrap_or_default());
                    if age_ok {
                        orphans.push((o, p));
                    } else {
                        cx.report.too_recent += 1;
                        cx.report.sample("too_recent", &o.key);
                    }
                }
            }
        }

        // Backfill: present objects without the self-describing metadata.
        if !to_head.is_empty() {
            let rows_by_key: HashMap<&str, &RowInfo> =
                by_content.values().map(|r| (r.original_key.as_str(), r)).collect();
            for (key, head) in head_many(cx.storage, to_head, cx.opts.concurrency).await {
                let head = match head {
                    Ok(Some(h)) => h,
                    Ok(None) => continue,
                    Err(e) => {
                        cx.report.errors += 1;
                        tracing::warn!(key, error = ?e, "HEAD failed");
                        continue;
                    }
                };
                if objmeta::is_described(&head.metadata) {
                    continue;
                }
                let Some(row) = rows_by_key.get(key.as_str()) else {
                    continue;
                };
                cx.report.backfill_needed += 1;
                cx.report.sample("backfill_needed", &key);
                if head.size > MAX_COPY_BYTES {
                    cx.report.backfill_too_large += 1;
                    continue;
                }
                if !cx.opts.apply {
                    continue;
                }
                let meta = objmeta::encode(&objmeta::OriginalMeta {
                    name: Some(row.original_name.clone()),
                    owner: Some(row.owner_id),
                    created_at: Some(row.created_at),
                    file_modified_at: row.file_modified_at,
                });
                let ct = head.content_type.unwrap_or_else(|| row.content_type.clone());
                match cx.storage.copy_onto_self(&key, &ct, meta).await {
                    Ok(()) => cx.report.backfilled += 1,
                    Err(e) => {
                        cx.report.errors += 1;
                        tracing::warn!(key, error = ?e, "metadata backfill failed");
                    }
                }
            }
        }
        if !cx.opts.apply {
            cx.report.backfilled = 0;
        }

        process_orphans(cx, orphans).await?;
        tracing::info!(
            scanned = cx.report.scanned_objects,
            orphans = cx.report.orphans,
            "reindex: originals scanned"
        );
        match page.next {
            Some(t) => token = Some(t),
            None => return Ok(seen),
        }
    }
}

async fn process_orphans(cx: &mut Ctx<'_>, orphans: Vec<(Listed, keys::ParsedOriginal)>) -> Result<()> {
    if orphans.is_empty() {
        return Ok(());
    }
    let heads = head_many(
        cx.storage,
        orphans.iter().map(|(o, _)| o.key.clone()).collect(),
        cx.opts.concurrency,
    )
    .await;
    let mut heads: HashMap<String, Result<Option<Head>>> = heads.into_iter().collect();
    for (o, p) in orphans {
        let head = match heads.remove(&o.key) {
            Some(Ok(Some(h))) => h,
            Some(Ok(None)) | None => {
                // Deleted between listing and HEAD (e.g. a purge in flight).
                cx.report.orphans_vanished += 1;
                continue;
            }
            Some(Err(e)) => {
                cx.report.errors += 1;
                tracing::warn!(key = o.key, error = ?e, "HEAD failed");
                continue;
            }
        };
        cx.report.orphans += 1;
        cx.report.user(p.user).orphans += 1;
        cx.report.sample("orphans", &o.key);

        let owner_exists = cx.known_users.get(&p.user).copied().unwrap_or(false);
        let owner = if owner_exists {
            p.user
        } else if let Some(a) = cx.opts.assign_to {
            cx.report.orphans_reassigned += 1;
            a
        } else {
            cx.report.orphans_unknown_user += 1;
            cx.report.user(p.user).orphans_skipped += 1;
            cx.report.user(p.user).orphans -= 1;
            cx.report.sample("orphans_unknown_user", &o.key);
            continue;
        };

        let m = objmeta::decode(&head.metadata);
        let manifest = manifest_for(cx, p.user).await;
        let entry = manifest.as_ref().and_then(|m| m.assets.get(&p.sha256));
        let sha_hex = keys::hex(&p.sha256);
        let name = m
            .name
            .clone()
            .or_else(|| entry.map(|e| e.name.clone()))
            .unwrap_or_else(|| format!("{sha_hex}.{}", p.ext));
        let created_at = m
            .created_at
            .or_else(|| entry.and_then(|e| e.created_at))
            .or(head.last_modified)
            .unwrap_or_else(OffsetDateTime::now_utc);
        let mtime = m.file_modified_at.or_else(|| entry.and_then(|e| e.file_modified_at));
        let content_type = head
            .content_type
            .clone()
            .or_else(|| entry.map(|e| e.content_type.clone()))
            .unwrap_or_else(|| "application/octet-stream".into());
        let trashed = entry.is_some_and(|e| e.deleted_at.is_some());
        let favorite = entry.and_then(|e| e.favorite).unwrap_or(false);
        let hidden = entry.and_then(|e| e.hidden).unwrap_or(false);
        let archived = entry.and_then(|e| e.archived).unwrap_or(false);
        // A manifest that knows tags says exactly which a photo has: its keywords are not read from the file again (a tag the
        // owner removed must not come back).
        let tags_known = entry.is_some() && manifest.as_ref().is_some_and(|m| m.tags_known);
        let rating = entry
            .and_then(|e| {
                Some((
                    e.rating?.min(5),
                    e.rating_source.clone().unwrap_or_else(|| "user".into()),
                ))
            })
            .filter(|_| cx.has_rating);
        let wanted_id = entry.map(|e| e.id);

        if !cx.opts.apply {
            cx.report.orphans_recreated += 1;
            cx.recreated.push((p.user, p.sha256));
            if trashed {
                cx.report.trashed_restored += 1;
            }
            if favorite && cx.has_favorite {
                cx.report.favorites_restored += 1;
            }
            if cx.has_state {
                cx.report.hidden_restored += u64::from(hidden);
                cx.report.archived_restored += u64::from(archived);
            }
            cx.report.ratings_restored += u64::from(rating.is_some());
            continue;
        }
        let r = insert_orphan(
            cx,
            Insert {
                id: wanted_id,
                owner,
                sha: p.sha256,
                key: &o.key,
                name: &name,
                content_type: &content_type,
                size: head.size,
                mtime,
                created_at,
                trashed,
                favorite,
                hidden,
                archived,
                rating: rating.clone(),
                tags_known,
            },
        )
        .await;
        match r {
            Ok(true) => {
                cx.report.orphans_recreated += 1;
                cx.recreated.push((p.user, p.sha256));
                if trashed {
                    cx.report.trashed_restored += 1;
                }
                if favorite && cx.has_favorite {
                    cx.report.favorites_restored += 1;
                }
                if cx.has_state {
                    cx.report.hidden_restored += u64::from(hidden);
                    cx.report.archived_restored += u64::from(archived);
                }
                cx.report.ratings_restored += u64::from(rating.is_some());
            }
            Ok(false) => {
                cx.report.alias_objects += 1;
                cx.report.orphans -= 1;
                cx.report.user(p.user).orphans -= 1;
            }
            Err(e) => {
                cx.report.errors += 1;
                tracing::warn!(key = o.key, error = ?e, "could not recreate asset row");
            }
        }
    }
    Ok(())
}

struct Insert<'a> {
    id: Option<Uuid>,
    owner: Uuid,
    sha: [u8; 32],
    key: &'a str,
    name: &'a str,
    content_type: &'a str,
    size: i64,
    mtime: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    trashed: bool,
    favorite: bool,
    hidden: bool,
    archived: bool,
    /// Stars and who set them, from the manifest (`None`: not rated, or the schema has no ratings).
    rating: Option<(u8, String)>,
    /// The photo's tags come from the manifest: mark its keywords as read.
    tags_known: bool,
}

/// Insert one row and queue its jobs, atomically. False if the owner already has this content (a race or an alias).
async fn insert_orphan(cx: &Ctx<'_>, i: Insert<'_>) -> Result<bool> {
    let mut tx = cx.db.begin().await?;
    let mut ids = vec![i.id.unwrap_or_else(Uuid::new_v4)];
    if i.id.is_some() {
        ids.push(Uuid::new_v4()); // the remembered id may be taken; fall back to a fresh one
    }
    let mut inserted = None;
    for id in ids {
        // A savepoint-free retry is safe: ON CONFLICT DO NOTHING does not abort the transaction.
        inserted = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO assets (id, owner_id, sha256, original_key, original_name, content_type, size_bytes,
                                 file_modified_at, created_at, deleted_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9, CASE WHEN $10 THEN now() END)
             ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(id)
        .bind(i.owner)
        .bind(&i.sha[..])
        .bind(i.key)
        .bind(i.name)
        .bind(i.content_type)
        .bind(i.size)
        .bind(i.mtime)
        .bind(i.created_at)
        .bind(i.trashed)
        .fetch_optional(&mut *tx)
        .await?;
        if inserted.is_some() {
            break;
        }
        // Same owner and content already present: no point retrying with another id.
        let dup: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM assets WHERE owner_id = $1 AND sha256 = $2)")
            .bind(i.owner)
            .bind(&i.sha[..])
            .fetch_one(&mut *tx)
            .await?;
        if dup {
            break;
        }
    }
    let Some(id) = inserted else {
        tx.rollback().await?;
        return Ok(false);
    };
    for kind in jobs::DEFAULT_PIPELINE {
        jobs::enqueue(&mut *tx, kind, id, 1).await?;
    }
    if i.favorite && cx.has_favorite {
        sqlx::query("UPDATE assets SET favorite = true WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    // In the same transaction as the insert: there is no moment at which the row exists and is visible.
    if (i.hidden || i.archived) && cx.has_state {
        sqlx::query(
            "UPDATE assets SET hidden = $2, archived = $3, archived_at = CASE WHEN $3 THEN now() END WHERE id = $1",
        )
        .bind(id)
        .bind(i.hidden)
        .bind(i.archived)
        .execute(&mut *tx)
        .await?;
    }
    if i.tags_known {
        sqlx::query("UPDATE assets SET tags_imported_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some((stars, source)) = &i.rating {
        let source = if source == "file" { "file" } else { "user" };
        sqlx::query("UPDATE assets SET rating = $2, rating_source = $3 WHERE id = $1")
            .bind(id)
            .bind(i16::from(*stars))
            .bind(source)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(true)
}

#[derive(sqlx::FromRow)]
struct RowCheck {
    id: Uuid,
    owner_id: Uuid,
    original_key: String,
    deleted_at: Option<OffsetDateTime>,
    primary_id: Option<Uuid>,
}

/// Rows against the scan: originals that are gone, and derived images that are gone.
async fn check_rows(cx: &mut Ctx<'_>, seen: &HashSet<u64>, derived: &HashSet<u64>) -> Result<()> {
    let mut after = Uuid::nil();
    loop {
        let rows: Vec<RowCheck> = sqlx::query_as(
            "SELECT id, owner_id, original_key, deleted_at, primary_id
             FROM assets WHERE purging_at IS NULL AND id > $1 ORDER BY id LIMIT 1000",
        )
        .bind(after)
        .fetch_all(cx.db)
        .await?;
        let Some(last) = rows.last() else { return Ok(()) };
        after = last.id;
        cx.report.rows_checked += rows.len() as u64;

        // Rows whose key the listing did not show: confirm with a HEAD before calling the original missing.
        let unseen: Vec<String> = rows
            .iter()
            .filter(|r| !seen.contains(&hash_str(&r.original_key)))
            .map(|r| r.original_key.clone())
            .collect();
        let mut gone: HashSet<String> = HashSet::new();
        for (key, head) in head_many(cx.storage, unseen, cx.opts.concurrency).await {
            match head {
                Ok(None) => {
                    gone.insert(key);
                }
                Ok(Some(_)) => {}
                Err(e) => {
                    cx.report.errors += 1;
                    tracing::warn!(key, error = ?e, "HEAD failed");
                }
            }
        }

        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let pending: HashSet<Uuid> = sqlx::query_scalar::<_, Uuid>(
            "SELECT asset_id FROM jobs WHERE kind = $1 AND asset_id = ANY($2) AND state IN ('queued','running')",
        )
        .bind(jobs::KIND_THUMBNAIL)
        .bind(&ids)
        .fetch_all(cx.db)
        .await?
        .into_iter()
        .collect();

        let (mut to_trash, mut to_rethumb) = (Vec::new(), Vec::new());
        for r in &rows {
            if gone.contains(&r.original_key) {
                cx.report.missing_originals += 1;
                cx.report.user(r.owner_id).missing_originals += 1;
                cx.report.sample("missing_originals", &r.original_key);
                if r.deleted_at.is_none() {
                    to_trash.push(r);
                }
                continue;
            }
            let (t, p) = (keys::thumb(r.owner_id, r.id), keys::preview(r.owner_id, r.id));
            let has_derived = derived.contains(&hash_str(&t)) && derived.contains(&hash_str(&p));
            if !has_derived && !pending.contains(&r.id) {
                cx.report.thumbs_missing += 1;
                cx.report.user(r.owner_id).thumbs_missing += 1;
                cx.report.sample("thumbs_missing", &r.original_key);
                to_rethumb.push(r.id);
            }
        }

        if cx.opts.mark_missing {
            for r in to_trash {
                if cx.opts.apply {
                    match trash_missing(cx.db, r.id, r.primary_id).await {
                        Ok(()) => cx.report.missing_marked_trashed += 1,
                        Err(e) => {
                            cx.report.errors += 1;
                            tracing::warn!(asset_id = %r.id, error = ?e, "could not trash row");
                        }
                    }
                } else {
                    cx.report.missing_marked_trashed += 1;
                }
            }
        }
        if cx.opts.rethumb {
            for id in to_rethumb {
                if cx.opts.apply {
                    match requeue_thumbnail(cx.db, id).await {
                        Ok(()) => cx.report.thumbs_requeued += 1,
                        Err(e) => {
                            cx.report.errors += 1;
                            tracing::warn!(asset_id = %id, error = ?e, "could not queue thumbnail job");
                        }
                    }
                } else {
                    cx.report.thumbs_requeued += 1;
                }
            }
        }
        tracing::info!(
            rows = cx.report.rows_checked,
            missing = cx.report.missing_originals,
            "reindex: rows checked"
        );
    }
}

/// Move a row whose original is gone to the trash. A RAW stacked behind it goes along (a pair shares its trash
/// state); a RAW that is itself the missing file is detached first so the JPEG it hid behind stays visible.
pub(crate) async fn trash_missing(db: &PgPool, id: Uuid, primary_id: Option<Uuid>) -> Result<()> {
    let mut tx = db.begin().await?;
    if primary_id.is_some() {
        sqlx::query("UPDATE assets SET primary_id = NULL WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE (id = $1 OR primary_id = $1) AND deleted_at IS NULL AND purging_at IS NULL")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Queue a thumbnail job, resetting a finished or failed one. Queued and running jobs are left alone.
async fn requeue_thumbnail(db: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version) VALUES ($1, $2, 1)
         ON CONFLICT (kind, asset_id, version) DO UPDATE
            SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL, last_error = NULL
            WHERE jobs.state IN ('done', 'failed')",
    )
    .bind(jobs::KIND_THUMBNAIL)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// Albums and RAW/JPEG pairs from the manifests: whole albums that no longer exist, and the memberships and pairs
/// of photos recreated by this run. Existing albums are never changed otherwise.
async fn restore_state(cx: &mut Ctx<'_>) -> Result<()> {
    let users: Vec<Uuid> = cx.manifest_keys.keys().copied().collect();
    let recreated: HashSet<(Uuid, [u8; 32])> = cx.recreated.iter().copied().collect();
    for user in users {
        if !cx.known_users.get(&user).copied().unwrap_or(false) {
            let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
                .bind(user)
                .fetch_one(cx.db)
                .await?;
            cx.known_users.insert(user, ok);
        }
        if !cx.known_users[&user] {
            continue;
        }
        let Some(m) = manifest_for(cx, user).await else {
            continue;
        };
        if let Err(e) = restore_user(cx, user, &m, &recreated).await {
            cx.report.errors += 1;
            tracing::warn!(user_id = %user, error = ?e, "could not restore albums/pairs from the manifest");
        }
    }
    Ok(())
}

/// Photo edits from `edits/`: for every object whose photo has a row, apply it when the row has no edits or older ones
/// (`edited_at`). This is the one place reindex rewrites a row that already exists, on purpose: an edit is what the user
/// asked for, and the bucket's copy is written before the database changes, so it is never older than the truth. A photo
/// whose edits were reverted keeps an empty object, which puts a restored dump's older edit back to the file's values.
/// Edits of an orphan are applied after the orphan was recreated above; in a dry run they count as "no photo yet".
async fn restore_edits(cx: &mut Ctx<'_>) -> Result<()> {
    let mut token = None;
    loop {
        let page = cx
            .storage
            .list_page(keys::EDITS_PREFIX, token)
            .await
            .context("list edits/")?;
        for o in &page.objects {
            if let Err(e) = restore_edit(cx, &o.key).await {
                cx.report.errors += 1;
                tracing::warn!(key = %o.key, error = ?e, "could not restore a photo's edits");
            }
        }
        match page.next {
            Some(t) => token = Some(t),
            None => return Ok(()),
        }
    }
}

async fn restore_edit(cx: &mut Ctx<'_>, key: &str) -> Result<()> {
    cx.report.edits_objects += 1;
    let Some((key_owner, sha)) = keys::parse_edits(key) else {
        cx.report.edits_bad += 1;
        cx.report.sample("edits keys not recognised", key);
        return Ok(());
    };
    known_users(cx, &[key_owner]).await?;
    // Photos of a user that no longer exists were given to `--assign-to`, whose rows keep the old owner in the key's path.
    let owner = match (
        cx.known_users.get(&key_owner).copied().unwrap_or(false),
        cx.opts.assign_to,
    ) {
        (true, _) => key_owner,
        (false, Some(to)) => to,
        (false, None) => {
            cx.report.edits_without_photo += 1;
            return Ok(());
        }
    };
    let row: Option<(Uuid, Option<OffsetDateTime>)> =
        sqlx::query_as("SELECT id, edited_at FROM assets WHERE owner_id = $1 AND sha256 = $2 AND purging_at IS NULL")
            .bind(owner)
            .bind(&sha[..])
            .fetch_optional(cx.db)
            .await?;
    let Some((id, edited_at)) = row else {
        cx.report.edits_without_photo += 1;
        return Ok(());
    };
    let bytes = cx.storage.get_bytes_limited(key, 64 * 1024).await?;
    let edits = match edits::parse(&bytes) {
        Ok(e) => e,
        Err(e) => {
            cx.report.edits_bad += 1;
            cx.report.sample("edits not valid", key);
            tracing::warn!(key, error = ?e, "an edits object is not valid; left alone");
            return Ok(());
        }
    };
    let current = match edited_at {
        Some(at) => at >= edits.updated_at,
        // Never edited: nothing to put back for "no edits".
        None => edits.is_empty(),
    };
    if current {
        cx.report.edits_current += 1;
        return Ok(());
    }
    if cx.opts.apply {
        edits::apply(cx.db, owner, id, &edits).await?;
    }
    cx.report.edits_restored += 1;
    Ok(())
}

async fn asset_ids(db: &PgPool, user: Uuid, shas: &[Vec<u8>]) -> Result<HashMap<Vec<u8>, Uuid>> {
    let rows: Vec<(Vec<u8>, Uuid)> =
        sqlx::query_as("SELECT sha256, id FROM assets WHERE owner_id = $1 AND sha256 = ANY($2) AND purging_at IS NULL")
            .bind(user)
            .bind(shas)
            .fetch_all(db)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Tags from the user's newest manifest: a tag the owner has no tag of that name for is recreated with all its photos that exist
/// (an empty tag too), and for a tag that exists only the photos recreated by this run are tagged again, so a tag the owner
/// changed since the manifest is never overwritten. Imported tags keep their mark.
async fn restore_tags(
    cx: &mut Ctx<'_>,
    user: Uuid,
    m: &UserManifest,
    recreated: &HashSet<(Uuid, [u8; 32])>,
) -> Result<()> {
    if m.tags.is_empty() {
        return Ok(());
    }
    let apply = cx.opts.apply;
    let existing: HashMap<String, Uuid> =
        sqlx::query_as::<_, (String, Uuid)>("SELECT lower(name), id FROM tags WHERE owner_id = $1")
            .bind(user)
            .fetch_all(cx.db)
            .await?
            .into_iter()
            .collect();
    let shas: Vec<Vec<u8>> = m
        .tags
        .values()
        .flat_map(|t| t.items.iter().chain(t.imported.iter()))
        .filter_map(|h| keys::parse_sha256(h))
        .map(|s| s.to_vec())
        .collect();
    let ids = asset_ids(cx.db, user, &shas).await?;
    for tag in m.tags.values() {
        let Ok(name) = tags::normalize(&tag.name) else {
            cx.report.errors += 1;
            tracing::warn!(tag = %tag.id, "a tag in the manifest has no valid name; skipped");
            continue;
        };
        let known = existing.get(&name.to_lowercase()).copied();
        let mut links: Vec<(Uuid, bool)> = Vec::new();
        let mut n_links = 0u64;
        for (hashes, imported) in [(&tag.items, false), (&tag.imported, true)] {
            for h in hashes {
                let Some(sha) = keys::parse_sha256(h) else { continue };
                if known.is_some() && !recreated.contains(&(user, sha)) {
                    continue;
                }
                match ids.get(&sha[..]) {
                    Some(id) => links.push((*id, imported)),
                    None if !apply && recreated.contains(&(user, sha)) => {}
                    None => continue,
                }
                n_links += 1;
            }
        }
        if known.is_none() {
            cx.report.tags_restored += 1;
        }
        cx.report.tag_links_restored += n_links;
        if !apply || (known.is_some() && links.is_empty()) {
            continue;
        }
        let mut tx = cx.db.begin().await?;
        let tag_id = match known {
            Some(id) => id,
            None => {
                sqlx::query(
                    "INSERT INTO tags (id, owner_id, name, created_at) VALUES ($1,$2,$3, coalesce($4, now()))
                     ON CONFLICT DO NOTHING",
                )
                .bind(tag.id)
                .bind(user)
                .bind(&name)
                .bind(tag.created_at)
                .execute(&mut *tx)
                .await?;
                // The remembered id may be taken by something else: whatever tag now has the name is the one.
                tags::ensure(&mut tx, user, &name).await?
            }
        };
        for (asset, imported) in links {
            tags::attach(&mut tx, asset, tag_id, imported).await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

/// Share links of a user that has none, from the user's newest manifest: same id, same token (so links already handed out
/// work again), same settings. An album link needs its album (restored just before) and a selection link at least one of its
/// photos. A link that had a password comes back with a locked one: closed until its owner sets a new password, never open.
async fn restore_shares(cx: &mut Ctx<'_>, user: Uuid, m: &UserManifest) -> Result<()> {
    if m.shares.is_empty() {
        return Ok(());
    }
    let apply = cx.opts.apply;
    // Only into an account that has no share links at all (a rebuilt database). An account whose database already has links
    // (a restored dump) is left alone: a link revoked after the manifest was written must never come back, and a link that
    // is only missing (made since the dump) is cheaper to make again than a revoked one is to notice.
    let any: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM shares WHERE owner_id = $1)")
        .bind(user)
        .fetch_one(cx.db)
        .await?;
    if any {
        return Ok(());
    }
    let albums: HashSet<Uuid> = sqlx::query_scalar::<_, Uuid>("SELECT id FROM albums WHERE owner_id = $1")
        .bind(user)
        .fetch_all(cx.db)
        .await?
        .into_iter()
        .collect();
    let shas: Vec<Vec<u8>> = m
        .shares
        .values()
        .flat_map(|s| s.items.iter())
        .filter_map(|h| keys::parse_sha256(h))
        .map(|s| s.to_vec())
        .collect();
    let ids = asset_ids(cx.db, user, &shas).await?;
    for sh in m.shares.values() {
        let members: Vec<Uuid> = sh
            .items
            .iter()
            .filter_map(|h| keys::parse_sha256(h))
            .filter_map(|s| ids.get(&s[..]).copied())
            .collect();
        let possible = match sh.kind.as_str() {
            "album" => sh
                .album
                .is_some_and(|a| albums.contains(&a) || (!apply && m.albums.contains_key(&a))),
            // In a dry run the photos may be orphans that --apply recreates first.
            "selection" => !sh.items.is_empty() && (!members.is_empty() || !apply),
            _ => false,
        };
        if !possible {
            cx.report.shares_skipped += 1;
            continue;
        }
        if !apply {
            cx.report.shares_restored += 1;
            continue;
        }
        let r: Result<bool> = async {
            let hash = if sh.password_protected {
                Some(identity::locked_password_hash()?)
            } else {
                None
            };
            let mut tx = cx.db.begin().await?;
            let inserted: Option<Uuid> = sqlx::query_scalar(
                "INSERT INTO shares (id, owner_id, token, kind, album_id, title, password_hash, allow_download,
                                     include_originals, expires_at, created_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10, coalesce($11, now())) ON CONFLICT DO NOTHING RETURNING id",
            )
            .bind(sh.id)
            .bind(user)
            .bind(&sh.token)
            .bind(&sh.kind)
            .bind(if sh.kind == "album" { sh.album } else { None })
            .bind(&sh.title)
            .bind(hash)
            .bind(sh.allow_download)
            .bind(sh.include_originals)
            .bind(sh.expires_at)
            .bind(sh.created_at)
            .fetch_optional(&mut *tx)
            .await?;
            if inserted.is_none() {
                tx.rollback().await?;
                return Ok(false);
            }
            if sh.kind == "selection" {
                for a in &members {
                    sqlx::query("INSERT INTO share_assets (share_id, asset_id) VALUES ($1,$2) ON CONFLICT DO NOTHING")
                        .bind(sh.id)
                        .bind(a)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            tx.commit().await?;
            Ok(true)
        }
        .await;
        match r {
            Ok(true) => cx.report.shares_restored += 1,
            Ok(false) => cx.report.shares_skipped += 1,
            Err(e) => {
                cx.report.errors += 1;
                tracing::warn!(share_id = %sh.id, error = ?e, "could not restore a share link");
            }
        }
    }
    Ok(())
}

async fn restore_user(
    cx: &mut Ctx<'_>,
    user: Uuid,
    m: &UserManifest,
    recreated: &HashSet<(Uuid, [u8; 32])>,
) -> Result<()> {
    let apply = cx.opts.apply;
    let existing: HashSet<Uuid> = sqlx::query_scalar::<_, Uuid>("SELECT id FROM albums WHERE owner_id = $1")
        .bind(user)
        .fetch_all(cx.db)
        .await?
        .into_iter()
        .collect();
    let all_shas: Vec<Vec<u8>> = m
        .albums
        .values()
        .flat_map(|a| a.items.iter().chain(a.cover.iter()))
        .filter_map(|h| keys::parse_sha256(h))
        .map(|s| s.to_vec())
        .collect();
    let ids = asset_ids(cx.db, user, &all_shas).await?;
    for album in m.albums.values() {
        let is_new = !existing.contains(&album.id);
        let mut members: Vec<(Uuid, Option<Uuid>)> = Vec::new(); // (asset id if it exists, ...)
        let mut n_items = 0u64;
        for h in &album.items {
            let Some(sha) = keys::parse_sha256(h) else { continue };
            let wanted = is_new || recreated.contains(&(user, sha));
            if !wanted {
                continue;
            }
            match ids.get(&sha[..]) {
                Some(id) => members.push((*id, None)),
                None if !apply && recreated.contains(&(user, sha)) => {}
                None => continue,
            }
            n_items += 1;
        }
        if is_new {
            cx.report.albums_restored += 1;
            cx.report.smart_albums_restored += u64::from(album.rules.is_some());
        }
        if !is_new && n_items == 0 {
            continue;
        }
        cx.report.album_items_restored += n_items;
        if !apply {
            continue;
        }
        let mut tx = cx.db.begin().await?;
        if is_new {
            let cover = album
                .cover
                .as_deref()
                .and_then(keys::parse_sha256)
                .and_then(|s| ids.get(&s[..]).copied());
            sqlx::query(
                "INSERT INTO albums (id, owner_id, title, description, cover_asset_id, created_at, updated_at, rules)
                 VALUES ($1,$2,$3,$4,$5, coalesce($6, now()), coalesce($7, now()), $8) ON CONFLICT DO NOTHING",
            )
            .bind(album.id)
            .bind(user)
            .bind(&album.title)
            .bind(&album.description)
            .bind(cover)
            .bind(album.created_at)
            .bind(album.updated_at)
            .bind(&album.rules)
            .execute(&mut *tx)
            .await?;
        }
        for (id, _) in members {
            sqlx::query("INSERT INTO album_assets (album_id, asset_id) VALUES ($1,$2) ON CONFLICT DO NOTHING")
                .bind(album.id)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
    }

    restore_shares(cx, user, m).await?;
    restore_tags(cx, user, m, recreated).await?;

    // Pairs: only for RAWs recreated in this run.
    let pairs: Vec<([u8; 32], [u8; 32])> = m
        .assets
        .iter()
        .filter(|(sha, _)| recreated.contains(&(user, **sha)))
        .filter_map(|(sha, a)| Some((*sha, keys::parse_sha256(a.pair_primary.as_deref()?)?)))
        .collect();
    if pairs.is_empty() {
        return Ok(());
    }
    if !apply {
        cx.report.pairs_restored += pairs.len() as u64;
        return Ok(());
    }
    let shas: Vec<Vec<u8>> = pairs.iter().flat_map(|(a, b)| [a.to_vec(), b.to_vec()]).collect();
    let ids = asset_ids(cx.db, user, &shas).await?;
    for (raw, primary) in pairs {
        let (Some(raw), Some(primary)) = (ids.get(&raw[..]), ids.get(&primary[..])) else {
            continue;
        };
        let done = sqlx::query(
            "UPDATE assets SET primary_id = $2
             WHERE id = $1 AND id <> $2 AND primary_id IS NULL
               AND NOT EXISTS (SELECT 1 FROM assets r WHERE r.primary_id = $2)
               AND EXISTS (SELECT 1 FROM assets p WHERE p.id = $2 AND p.primary_id IS NULL)",
        )
        .bind(raw)
        .bind(primary)
        .execute(cx.db)
        .await?
        .rows_affected();
        cx.report.pairs_restored += done;
    }
    Ok(())
}
