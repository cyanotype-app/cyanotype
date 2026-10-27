//! Dogs and cats found in photos (`pet_detections`, `pet_scans`) and the jobs that find them (docs/pets.md), and what the owner decided about
//! the animals found (named pets, `crate::named_pets`: this module carries those decisions over to a new scan). The detector lives in the
//! `cyanotype-pets` crate (this crate must not link ONNX Runtime), so the worker passes what it found in.
//!
//! Everything here is owner-scoped data, read only by the owner's own library: the Pets view, the `pets` search filter and facet, the
//! smart-album rule and the viewer's boxes. Nothing is ever on a share page, a download or a recovery manifest: detections are derived data,
//! rebuilt by a rescan. The feature is on unless `PETS_ENABLED` says otherwise ([`enabled`]); off, the worker neither queues nor runs the job
//! and the assets service answers as if it did not exist.
use anyhow::Result;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::{embeddings::pg_literal, jobs, people_math};

/// What finds the animals: the search model's reading of the photo (SigLIP 2, `cyanotype_clip::zeroshot`: which kinds are in it) and
/// D-FINE-N (where), `models/pets.lock`. A new model, new prompts or another search model get a new id, a change of how detections are
/// made (a different threshold, a fixed bug) a new version: either scans the library again, at low priority, and the old rows answer
/// until their photo's new ones replace them. The `pets` job's version is [`VERSION`], so a model's next version must be higher than every
/// version before it.
pub const MODEL: &str = "siglip2-b16-q4-check+dfine-n-coco";
pub const VERSION: i32 = 1;

/// The fingerprint of an animal's crop that telling individuals apart compares (`pet_detections.embedding`): which model made it (DINOv2-small,
/// 8-bit, the class token), with which crop recipe (a square around the box, 10 % bigger), and its length. A different model or recipe gets a new id, and the sweep makes the fingerprints again without finding the
/// animals again. The id is what two fingerprints must share to be compared.
pub const EMBED_MODEL: &str = "dinov2-small-int8-cls-sq10-v1";
pub const EMBED_DIM: usize = 384;

/// A new scan's animal is the same animal as an earlier one of its photo (so what the owner decided about it moves over) when their
/// fingerprints of one model are at least this alike, or else when their boxes overlap at least [`SAME_BOX_IOU`], or else (a Rotate moves
/// the box and changes the crop: DINOv2 says only 0.65 for the same animal turned) when each is its photo's only animal of the kind.
/// Measured in docs/pets.md ("A new scan keeps the decisions").
pub const SAME_CROP: f32 = 0.80;
pub const SAME_BOX_IOU: f32 = 0.5;

/// What is looked for. A kind is a plain word stored with each detection, so another animal is a line here and in the detector's
/// class table, not a new table or column.
pub const DOG: &str = "dog";
pub const CAT: &str = "cat";
pub const KINDS: [&str; 2] = [DOG, CAT];

/// A kind a client may ask for: `dog` or `cat`.
pub fn is_kind(s: &str) -> bool {
    KINDS.contains(&s)
}

/// What a filter, a view or a smart-album rule asks for: any pet, or one kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Any,
    Kind(&'static str),
}

impl Which {
    /// `any`, `dog` or `cat` (any case, trimmed).
    pub fn parse(s: &str) -> Option<Which> {
        let s = s.trim().to_ascii_lowercase();
        if s == "any" {
            return Some(Which::Any);
        }
        KINDS.iter().find(|k| **k == s).map(|k| Which::Kind(k))
    }

    pub fn name(self) -> &'static str {
        match self {
            Which::Any => "any",
            Which::Kind(k) => k,
        }
    }

    /// The kind to compare with, or `None` for any (the SQL is `($n::text IS NULL OR d.kind = $n)`).
    pub fn kind(self) -> Option<&'static str> {
        match self {
            Which::Any => None,
            Which::Kind(k) => Some(k),
        }
    }
}

/// Is the feature on? `PETS_ENABLED` is on unless it says `0`, `false`, `no` or `off` (any case). Unset, empty or anything else: on.
pub fn enabled() -> bool {
    parse_enabled(std::env::var("PETS_ENABLED").ok().as_deref())
}

pub fn parse_enabled(v: Option<&str>) -> bool {
    !matches!(
        v.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "no" | "off")
    )
}

/// One animal to store. Coordinates are fractions (0..1) of the photo's preview image.
#[derive(Debug, Clone, PartialEq)]
pub struct NewDetection {
    pub kind: String,
    /// Left, top, width, height.
    pub bbox: [f32; 4],
    pub score: f32,
    /// The fingerprint of the animal's crop ([`EMBED_MODEL`], [`EMBED_DIM`] values, unit length); `None` when it was not made (yet).
    pub embedding: Option<Vec<f32>>,
}

/// What the worker found in a photo, to store.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    /// What the search model said about the photo: how much of its attention the dog prompts and the cat prompts got (0..1).
    pub dog_share: f32,
    pub cat_share: f32,
    pub found: Vec<NewDetection>,
    /// The job tried to fingerprint the animals with [`EMBED_MODEL`]: a `None` embedding then means the crop was too small, not that it was
    /// never tried (which the sweep would fill in later).
    pub fingerprinted: bool,
}

/// A detection counts when its photo has a scan by the detection's model (after a rotate or crop the scan is forgotten, and with it
/// the detections, until the new scan). `d` is the `pet_detections` alias.
pub const SCANNED: &str = "EXISTS (SELECT 1 FROM pet_scans s WHERE s.asset_id = d.asset_id AND s.model = d.model)";

/// Store the animals found in a photo for `(model, version)`, replacing every earlier row of the photo (of every model: the new scan
/// supersedes them) so running the job again never duplicates one, and note that the photo has been looked at, also when it has no
/// pets. Nothing is stored for a photo that is gone or being deleted. Returns whether anything was stored.
///
/// What the owner decided about the photo's earlier animals is carried over to the ones that take their place ([`Earlier`]): the pet
/// each was put in, the pets it was refused for and being a pet's chosen cover. The new rows have new ids.
pub async fn store(db: &PgPool, asset_id: Uuid, model: &str, version: i32, scan: &Scan) -> Result<bool> {
    let found = &scan.found;
    let mut tx = db.begin().await?;
    // The scan row first: its lock serialises two workers that scan the same photo.
    let scanned = sqlx::query(
        "INSERT INTO pet_scans (asset_id, model, version, detections, dog_share, cat_share)
         SELECT id, $2, $3, $4, $5, $6 FROM assets WHERE id = $1 AND purging_at IS NULL
         ON CONFLICT (asset_id, model) DO UPDATE SET version = EXCLUDED.version, detections = EXCLUDED.detections,
             dog_share = EXCLUDED.dog_share, cat_share = EXCLUDED.cat_share, scanned_at = now()",
    )
    .bind(asset_id)
    .bind(model)
    .bind(version)
    .bind(found.len() as i32)
    .bind(scan.dog_share)
    .bind(scan.cat_share)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if scanned == 0 {
        return Ok(false);
    }
    let earlier = Earlier::load(&mut tx, asset_id).await?;
    sqlx::query("DELETE FROM pet_detections WHERE asset_id = $1")
        .bind(asset_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM pet_scans WHERE asset_id = $1 AND model <> $2")
        .bind(asset_id)
        .bind(model)
        .execute(&mut *tx)
        .await?;
    let mut ids: Vec<Uuid> = Vec::with_capacity(found.len());
    for (i, d) in found.iter().enumerate() {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO pet_detections (asset_id, owner_id, kind, model, version, idx, box_x, box_y, box_w, box_h, score, embedding, embed_model)
             SELECT id, owner_id, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::text::halfvec, $12 FROM assets WHERE id = $1
             RETURNING id",
        )
        .bind(asset_id)
        .bind(&d.kind)
        .bind(model)
        .bind(version)
        .bind(i as i16)
        .bind(d.bbox[0])
        .bind(d.bbox[1])
        .bind(d.bbox[2])
        .bind(d.bbox[3])
        .bind(d.score)
        .bind(d.embedding.as_deref().map(pg_literal))
        .bind((scan.fingerprinted || d.embedding.is_some()).then_some(EMBED_MODEL))
        .fetch_one(&mut *tx)
        .await?;
        ids.push(id);
    }
    earlier.carry(&mut tx, found, &ids).await?;
    tx.commit().await?;
    Ok(true)
}

/// A photo's animals before a new scan replaces them, and what was decided about them.
struct Earlier {
    rows: Vec<EarlierRow>,
    /// `(detection, pet)`: the pets each animal was refused for.
    rejections: Vec<(Uuid, Uuid)>,
    /// `(pet, detection)`: pets whose chosen cover is one of these animals.
    covers: Vec<(Uuid, Uuid)>,
}

#[derive(sqlx::FromRow)]
struct EarlierRow {
    id: Uuid,
    kind: String,
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
    embedding: Option<String>,
    embed_model: Option<String>,
    pet_id: Option<Uuid>,
}

impl Earlier {
    async fn load(db: &mut sqlx::PgConnection, asset_id: Uuid) -> Result<Self> {
        let rows: Vec<EarlierRow> = sqlx::query_as(
            "SELECT id, kind, box_x, box_y, box_w, box_h, embedding::text AS embedding, embed_model, pet_id
             FROM pet_detections WHERE asset_id = $1 ORDER BY model, idx",
        )
        .bind(asset_id)
        .fetch_all(&mut *db)
        .await?;
        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let rejections = sqlx::query_as("SELECT detection_id, pet_id FROM pet_rejections WHERE detection_id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *db)
            .await?;
        let covers = sqlx::query_as("SELECT id, cover_detection_id FROM pets WHERE cover_detection_id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *db)
            .await?;
        Ok(Self {
            rows,
            rejections,
            covers,
        })
    }

    /// Is there anything the owner decided?
    fn carries(&self) -> bool {
        !self.rejections.is_empty() || !self.covers.is_empty() || self.rows.iter().any(|r| r.pet_id.is_some())
    }

    /// Hand the decisions on to the new animals `ids` (stored from `new`, in order). Animals are matched one to one within a kind, by
    /// the fingerprint of their crops (the animal moved: a rotate or a crop), then by overlapping boxes.
    async fn carry(&self, db: &mut sqlx::PgConnection, new: &[NewDetection], ids: &[Uuid]) -> Result<()> {
        if !self.carries() || new.is_empty() {
            // Nothing to hand on, or nothing to hand it to: the pet stays, the animal is gone.
            return Ok(());
        }
        let vectors: Vec<Option<Vec<f32>>> = self
            .rows
            .iter()
            .map(|r| r.embedding.as_deref().and_then(people_math::parse_literal))
            .collect();
        for kind in KINDS {
            let old_ix: Vec<usize> = (0..self.rows.len()).filter(|&j| self.rows[j].kind == kind).collect();
            let new_ix: Vec<usize> = (0..new.len()).filter(|&i| new[i].kind == kind).collect();
            let old: Vec<people_math::Seen> = old_ix
                .iter()
                .map(|&j| {
                    let r = &self.rows[j];
                    people_math::Seen {
                        bbox: [r.box_x, r.box_y, r.box_w, r.box_h],
                        embedding: match (&r.embed_model, &vectors[j]) {
                            (Some(m), Some(v)) => Some((m.as_str(), v.as_slice())),
                            _ => None,
                        },
                    }
                })
                .collect();
            let fresh: Vec<people_math::Seen> = new_ix
                .iter()
                .map(|&i| people_math::Seen {
                    bbox: new[i].bbox,
                    embedding: new[i].embedding.as_deref().map(|v| (EMBED_MODEL, v)),
                })
                .collect();
            let mut matched = people_math::match_by(&old, &fresh, SAME_CROP, SAME_BOX_IOU);
            // A photo's only animal of a kind, and the only one the new scan found: the same animal, wherever the picture put it (a Rotate).
            if old.len() == 1 && fresh.len() == 1 && matched[0].is_none() {
                matched[0] = Some(0);
            }
            for (n, from) in matched.into_iter().enumerate() {
                let Some(j) = from else { continue };
                let (was, to) = (&self.rows[old_ix[j]], ids[new_ix[n]]);
                if let Some(pet) = was.pet_id {
                    sqlx::query("UPDATE pet_detections SET pet_id = $2 WHERE id = $1")
                        .bind(to)
                        .bind(pet)
                        .execute(&mut *db)
                        .await?;
                }
                for (_, pet) in self.rejections.iter().filter(|(d, _)| *d == was.id) {
                    sqlx::query(
                        "INSERT INTO pet_rejections (detection_id, pet_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                    )
                    .bind(to)
                    .bind(pet)
                    .execute(&mut *db)
                    .await?;
                }
                for (pet, _) in self.covers.iter().filter(|(_, d)| *d == was.id) {
                    sqlx::query("UPDATE pets SET cover_detection_id = $2 WHERE id = $1")
                        .bind(pet)
                        .bind(to)
                        .execute(&mut *db)
                        .await?;
                }
            }
        }
        Ok(())
    }
}

/// The search model's vectors the check reads: the photos that can be scanned are the ones that have one (`model` and `version` of the
/// `embed` job), and a photo whose vector arrives later is queued when it does (`enqueue_for_asset`).
#[derive(Debug, Clone, Copy)]
pub struct Basis<'a> {
    pub model: &'a str,
    pub version: i32,
}

/// Queue a `pets` job at backfill priority for every visible photo that has a preview and a search vector and has not been scanned by
/// `(model, version)`, or whose animals have no fingerprint by [`EMBED_MODEL`] yet (animals found before named pets, or before a new
/// fingerprint model: the job fingerprints them without finding them again): the backfill of an existing library, the catch-up after a model switch, and jobs that were lost. A job already
/// queued, running or failed is left alone; one that finished without a scan is requeued. Returns how many were queued.
pub async fn enqueue_missing(db: impl PgExecutor<'_>, model: &str, version: i32, basis: Basis<'_>) -> Result<u64> {
    let done = sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version, priority)
         SELECT 'pets', a.id, $2, $3 FROM assets a
         WHERE a.thumbhash IS NOT NULL AND a.primary_id IS NULL AND a.purging_at IS NULL
           AND EXISTS (SELECT 1 FROM asset_embeddings e WHERE e.asset_id = a.id AND e.model = $4 AND e.version = $5)
           AND (NOT EXISTS (SELECT 1 FROM pet_scans s WHERE s.asset_id = a.id AND s.model = $1 AND s.version = $2)
                OR EXISTS (SELECT 1 FROM pet_detections d WHERE d.asset_id = a.id AND d.embed_model IS DISTINCT FROM $6))
         ON CONFLICT (kind, asset_id, version) DO UPDATE
             SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL,
                 last_error = NULL, priority = EXCLUDED.priority
             WHERE jobs.state = 'done'",
    )
    .bind(model)
    .bind(version)
    .bind(jobs::PRIORITY_BACKFILL)
    .bind(basis.model)
    .bind(basis.version)
    .bind(EMBED_MODEL)
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

/// Queue the `pets` job of one photo, at backfill priority (pets are the least urgent thing to know): called when the photo's search
/// embedding has just been stored, because the job reads it. A job that finished or failed before is queued again (the embedding it
/// read may be of the photo as it was before an edit); one that is queued or running is left alone.
pub async fn enqueue_for_asset(db: impl PgExecutor<'_>, asset_id: Uuid, version: i32) -> Result<()> {
    sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version, priority) VALUES ($1, $2, $3, $4)
         ON CONFLICT (kind, asset_id, version) DO UPDATE
             SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL,
                 last_error = NULL, priority = EXCLUDED.priority
             WHERE jobs.state IN ('done', 'failed')",
    )
    .bind(jobs::KIND_PETS)
    .bind(asset_id)
    .bind(version)
    .bind(jobs::PRIORITY_BACKFILL)
    .execute(db)
    .await?;
    Ok(())
}

/// A photo's picture changed (a rotate or crop): the animals found in the old one, and where they were, are wrong now. Forget the
/// scan and the detections nobody decided anything about; the animals the owner put in a pet, refused for one or chose as a cover stay,
/// unread (every reader asks for a photo with a scan) until the new scan hands what was decided over to its animals ([`store`]). With `requeue` (the feature is on), scan the new preview again
/// (queued again even if the photo was scanned before). Nothing is queued for a photo that was never scanned (the ordinary sweep will,
/// if the feature is on). Returns whether there was anything to forget.
pub async fn forget_and_requeue(db: &PgPool, asset_id: Uuid, version: i32, requeue: bool) -> Result<bool> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "DELETE FROM pet_detections d WHERE d.asset_id = $1 AND d.pet_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM pet_rejections r WHERE r.detection_id = d.id)
           AND NOT EXISTS (SELECT 1 FROM pets p WHERE p.cover_detection_id = d.id)",
    )
    .bind(asset_id)
    .execute(&mut *tx)
    .await?;
    let forgot = sqlx::query("DELETE FROM pet_scans WHERE asset_id = $1")
        .bind(asset_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    tx.commit().await?;
    if forgot && requeue {
        sqlx::query(
            "INSERT INTO jobs (kind, asset_id, version, priority) VALUES ($1, $2, $3, $4)
             ON CONFLICT (kind, asset_id, version) DO UPDATE
                 SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL,
                     last_error = NULL, priority = EXCLUDED.priority
                 WHERE jobs.state IN ('done', 'failed')",
        )
        .bind(jobs::KIND_PETS)
        .bind(asset_id)
        .bind(version)
        .bind(jobs::PRIORITY_BACKFILL)
        .execute(db)
        .await?;
    }
    Ok(forgot)
}

/// An animal of a photo's current scan that has no fingerprint by [`EMBED_MODEL`] yet.
#[derive(Debug, Clone, PartialEq)]
pub struct Unfingerprinted {
    pub id: Uuid,
    pub kind: String,
    /// Left, top, width, height, fractions of the preview.
    pub bbox: [f32; 4],
}

/// The animals of the photo's scan by `(model, version)` that [`EMBED_MODEL`] has not looked at (see [`set_fingerprints`]).
pub async fn unfingerprinted(db: impl PgExecutor<'_>, asset_id: Uuid, model: &str) -> Result<Vec<Unfingerprinted>> {
    let rows: Vec<(Uuid, String, f32, f32, f32, f32)> = sqlx::query_as(
        "SELECT id, kind, box_x, box_y, box_w, box_h FROM pet_detections
         WHERE asset_id = $1 AND model = $2 AND embed_model IS DISTINCT FROM $3 ORDER BY idx",
    )
    .bind(asset_id)
    .bind(model)
    .bind(EMBED_MODEL)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, kind, x, y, w, h)| Unfingerprinted {
            id,
            kind,
            bbox: [x, y, w, h],
        })
        .collect())
}

/// Record what [`EMBED_MODEL`] made of animals that were already stored: `None` for a crop too small to be worth a fingerprint. An animal
/// the photo's newer scan has replaced meanwhile is simply not there to update. Returns how many were.
pub async fn set_fingerprints(db: &PgPool, prints: &[(Uuid, Option<Vec<f32>>)]) -> Result<u64> {
    let mut n = 0;
    let mut tx = db.begin().await?;
    for (id, v) in prints {
        n += sqlx::query("UPDATE pet_detections SET embedding = $2::text::halfvec, embed_model = $3 WHERE id = $1")
            .bind(id)
            .bind(v.as_deref().map(pg_literal))
            .bind(EMBED_MODEL)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }
    tx.commit().await?;
    Ok(n)
}

/// How many visible photos with a preview and a search vector have not been scanned by `(model, version)`.
pub async fn missing_count(db: impl PgExecutor<'_>, model: &str, version: i32, basis: Basis<'_>) -> Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM assets a
         WHERE a.thumbhash IS NOT NULL AND a.primary_id IS NULL AND a.purging_at IS NULL
           AND EXISTS (SELECT 1 FROM asset_embeddings e WHERE e.asset_id = a.id AND e.model = $3 AND e.version = $4)
           AND NOT EXISTS (SELECT 1 FROM pet_scans s WHERE s.asset_id = a.id AND s.model = $1 AND s.version = $2)",
    )
    .bind(model)
    .bind(version)
    .bind(basis.model)
    .bind(basis.version)
    .fetch_one(db)
    .await?;
    Ok(n)
}

/// Delete the detections and scans of every model other than `keep`, and older versions of `keep` itself. Called once every photo has
/// been scanned by `(keep, version)`, so nothing is left to serve from the old ones (a scan replaces a photo's older rows when it is
/// stored, so this only finds what a photo that lost its preview left behind). Returns the detection rows removed.
pub async fn prune_superseded(db: &PgPool, keep: &str, version: i32) -> Result<u64> {
    let mut tx = db.begin().await?;
    let n = sqlx::query("DELETE FROM pet_detections WHERE model <> $1 OR version <> $2")
        .bind(keep)
        .bind(version)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM pet_scans WHERE model <> $1 OR version <> $2")
        .bind(keep)
        .bind(version)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(n)
}

/// Scanning progress of one owner's library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Photos scanned by the current model, and photos that can be (they have a preview and a search vector).
    pub scanned: i64,
    pub total: i64,
    /// Animals found in the scanned photos.
    pub found: i64,
    /// Of them, the animals [`EMBED_MODEL`] has not looked at yet (the sweep is filling them in: named pets compare them).
    pub unfingerprinted: i64,
}

pub async fn progress(db: &PgPool, owner: Uuid, model: &str, version: i32, basis: Basis<'_>) -> Result<Progress> {
    let (scanned, total, found): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(s.asset_id), count(*), coalesce(sum(s.detections), 0)::bigint FROM assets a
         LEFT JOIN pet_scans s ON s.asset_id = a.id AND s.model = $2 AND s.version = $3
         WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.primary_id IS NULL AND a.thumbhash IS NOT NULL
           AND EXISTS (SELECT 1 FROM asset_embeddings e WHERE e.asset_id = a.id AND e.model = $4 AND e.version = $5)",
    )
    .bind(owner)
    .bind(model)
    .bind(version)
    .bind(basis.model)
    .bind(basis.version)
    .fetch_one(db)
    .await?;
    let (unfingerprinted,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM pet_detections d JOIN assets a ON a.id = d.asset_id
         WHERE d.owner_id = $1 AND d.embed_model IS DISTINCT FROM $2 AND a.deleted_at IS NULL AND a.primary_id IS NULL
           AND EXISTS (SELECT 1 FROM pet_scans s WHERE s.asset_id = d.asset_id AND s.model = d.model)",
    )
    .bind(owner)
    .bind(EMBED_MODEL)
    .fetch_one(db)
    .await?;
    Ok(Progress {
        scanned,
        total,
        found,
        unfingerprinted,
    })
}

/// What [`delete_all`] removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Deleted {
    pub detections: u64,
    pub scans: u64,
    pub jobs: u64,
    /// The named pets (with their names) and the "not this pet" decisions.
    pub pets: u64,
    pub rejections: u64,
}

/// Delete all pet data: every detection, every scan record, every named pet (names included) and rejection, and every queued, running or
/// finished `pets` job, of one owner or of everyone. With `dry_run` it only counts. Nothing else is touched (the photos, the search index
/// and the rest of the queue are not). With the feature still on the sweep would start scanning again, so switch `PETS_ENABLED` off first.
pub async fn delete_all(db: &PgPool, owner: Option<Uuid>, dry_run: bool) -> Result<Deleted> {
    let mut tx = db.begin().await?;
    let scope = " WHERE ($1::uuid IS NULL OR owner_id = $1)";
    let asset_scope = " WHERE ($1::uuid IS NULL OR asset_id IN (SELECT id FROM assets WHERE owner_id = $1))";
    let (detections,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM pet_detections{scope}"))
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    let (scans,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM pet_scans{asset_scope}"))
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    let (jobs_n,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM jobs{asset_scope} AND kind = 'pets'"))
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    let (named,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM pets{scope}"))
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    let (rejections,): (i64,) = sqlx::query_as(&format!(
        "SELECT count(*) FROM pet_rejections WHERE detection_id IN (SELECT id FROM pet_detections{scope})"
    ))
    .bind(owner)
    .fetch_one(&mut *tx)
    .await?;
    if !dry_run {
        // The pets first: their detections are set free, and every rejection goes with a pet or a detection.
        sqlx::query(&format!("DELETE FROM pets{scope}"))
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("DELETE FROM pet_detections{scope}"))
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("DELETE FROM pet_scans{asset_scope}"))
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("DELETE FROM jobs{asset_scope} AND kind = 'pets'"))
            .bind(owner)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Deleted {
        detections: detections as u64,
        scans: scans as u64,
        jobs: jobs_n as u64,
        pets: named as u64,
        rejections: rejections as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_is_on_unless_it_says_otherwise() {
        for off in ["0", "false", "FALSE", " no ", "Off"] {
            assert!(!parse_enabled(Some(off)), "{off:?}");
        }
        for on in ["", "1", "true", "yes", "on", "2", "enabled", "y"] {
            assert!(parse_enabled(Some(on)), "{on:?}");
        }
        assert!(parse_enabled(None), "unset is on");
    }

    #[test]
    fn a_filter_names_any_pet_or_one_kind() {
        assert_eq!(Which::parse("any"), Some(Which::Any));
        assert_eq!(Which::parse(" Dog "), Some(Which::Kind(DOG)));
        assert_eq!(Which::parse("CAT"), Some(Which::Kind(CAT)));
        for bad in ["", "dogs", "bird", "all", "pet"] {
            assert_eq!(Which::parse(bad), None, "{bad:?}");
        }
        assert_eq!((Which::Any.kind(), Which::Kind(CAT).kind()), (None, Some("cat")));
        assert_eq!((Which::Any.name(), Which::Kind(DOG).name()), ("any", "dog"));
        assert!(is_kind("dog") && is_kind("cat") && !is_kind("any"));
    }
}
