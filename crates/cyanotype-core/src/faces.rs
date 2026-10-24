//! Faces found in photos (`faces`, `face_scans`) and the jobs that find them (docs/faces.md). The models live in the `cyanotype-faces`
//! crate (this crate must not link ONNX Runtime), so the worker passes what it found in.
//!
//! Everything here is owner-scoped data, read only by the owner's own People pages (`crate::people` groups the faces into people).
//! The feature is off unless `FACES_ENABLED` says otherwise ([`enabled`]): the worker neither queues nor runs the job, and the
//! assets service answers as if People did not exist.
use anyhow::Result;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::{embeddings::pg_literal, jobs, people, people_math};

/// The models that find faces: YuNet (2026-05 export) and SFace (2021-12), `models/faces.lock`. A new model gets a new id (and its own
/// index, in its own migration), a change of how faces are made (a different threshold, a fixed bug) a new version: either scans
/// the library again, at low priority, and the old rows answer until their photo's new ones replace them. The `faces` job's
/// version is [`VERSION`], so a model's next version must be higher than every version before it.
pub const MODEL: &str = "yunet2026may-sface2021dec";
pub const VERSION: i32 = 1;
/// Length of a face's fingerprint (fixed by the model, and by its `halfvec(128)` index).
pub const DIM: usize = 128;
/// Name of this model's HNSW index (migration 0029).
pub const INDEX_NAME: &str = "faces_yunet2026may_sface2021dec_hnsw";

/// Is the feature on? `FACES_ENABLED` is `1`, `true`, `yes` or `on` (any case). Unset, empty or anything else: off.
pub fn enabled() -> bool {
    parse_enabled(std::env::var("FACES_ENABLED").ok().as_deref())
}

pub fn parse_enabled(v: Option<&str>) -> bool {
    matches!(
        v.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// One face to store. Coordinates are fractions (0..1) of the photo's preview image.
#[derive(Debug, Clone, PartialEq)]
pub struct NewFace {
    /// Left, top, width, height.
    pub bbox: [f32; 4],
    /// Right eye, left eye, nose, right mouth corner, left mouth corner: x0, y0, ..., x4, y4.
    pub landmarks: [f32; 10],
    pub score: f32,
    /// [`DIM`] values, unit length; `None` for a face too small to fingerprint.
    pub embedding: Option<Vec<f32>>,
}

/// Store the faces found in a photo for `(model, version)`, replacing that model's previous rows for it (so running the job
/// again never duplicates a face), and note that the photo has been looked at, also when it has no faces. Nothing is stored
/// for a photo that is gone or being deleted. Returns whether anything was stored.
///
/// What the owner decided about the photo's earlier faces is carried over to the new ones ([`people_math::match_faces`]): the person
/// a face was put in and who put it there, the people it was rejected from, and being a person's chosen cover. The earlier faces
/// are those of every model, so a new scan after a rotate or crop, a new version of the model and a new model all keep their people.
/// Faces of another model stay (the sweep removes them once the library is covered) but give up what moved to the new ones.
pub async fn store(db: &PgPool, asset_id: Uuid, model: &str, version: i32, faces: &[NewFace]) -> Result<bool> {
    let mut tx = db.begin().await?;
    // The scan row first: its lock serialises two workers that scan the same photo.
    let scanned = sqlx::query(
        "INSERT INTO face_scans (asset_id, model, version, faces)
         SELECT id, $2, $3, $4 FROM assets WHERE id = $1 AND purging_at IS NULL
         ON CONFLICT (asset_id, model) DO UPDATE SET version = EXCLUDED.version, faces = EXCLUDED.faces, scanned_at = now()",
    )
    .bind(asset_id)
    .bind(model)
    .bind(version)
    .bind(faces.len() as i32)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if scanned == 0 {
        return Ok(false);
    }
    let earlier = Earlier::load(&mut tx, asset_id).await?;
    sqlx::query("DELETE FROM faces WHERE asset_id = $1 AND model = $2")
        .bind(asset_id)
        .bind(model)
        .execute(&mut *tx)
        .await?;
    let mut ids: Vec<Uuid> = Vec::with_capacity(faces.len());
    for (i, f) in faces.iter().enumerate() {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO faces (asset_id, owner_id, model, version, idx, box_x, box_y, box_w, box_h, landmarks, score, embedding)
             SELECT id, owner_id, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::text::halfvec FROM assets WHERE id = $1
             RETURNING id",
        )
        .bind(asset_id)
        .bind(model)
        .bind(version)
        .bind(i as i16)
        .bind(f.bbox[0])
        .bind(f.bbox[1])
        .bind(f.bbox[2])
        .bind(f.bbox[3])
        .bind(f.landmarks.to_vec())
        .bind(f.score)
        .bind(f.embedding.as_deref().map(pg_literal))
        .fetch_one(&mut *tx)
        .await?;
        ids.push(id);
    }
    earlier.carry(&mut tx, model, faces, &ids).await?;
    // Grouping into people follows once the owner's faces have stopped arriving.
    if faces.iter().any(|f| f.embedding.is_some()) || earlier.carries() {
        people::request_for_asset(&mut tx, asset_id).await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// A photo's faces before a new scan replaces them, and what was decided about them.
struct Earlier {
    rows: Vec<EarlierFace>,
    /// `(face, person)`: the people each face was rejected from.
    rejections: Vec<(Uuid, Uuid)>,
    /// `(person, face)`: people whose chosen cover is one of these faces.
    covers: Vec<(Uuid, Uuid)>,
}

#[derive(sqlx::FromRow)]
struct EarlierFace {
    id: Uuid,
    model: String,
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
    embedding: Option<String>,
    person_id: Option<Uuid>,
    assigned_by: String,
}

impl Earlier {
    async fn load(db: &mut sqlx::PgConnection, asset_id: Uuid) -> Result<Self> {
        let rows: Vec<EarlierFace> = sqlx::query_as(
            "SELECT id, model, box_x, box_y, box_w, box_h, embedding::text AS embedding, person_id, assigned_by
             FROM faces WHERE asset_id = $1 ORDER BY model, idx",
        )
        .bind(asset_id)
        .fetch_all(&mut *db)
        .await?;
        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        let rejections = sqlx::query_as("SELECT face_id, person_id FROM face_rejections WHERE face_id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *db)
            .await?;
        let covers = sqlx::query_as("SELECT id, cover_face_id FROM people WHERE cover_face_id = ANY($1)")
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
        !self.rejections.is_empty() || !self.covers.is_empty() || self.rows.iter().any(|r| r.person_id.is_some())
    }

    /// Hand the decisions on to the new faces `ids` (stored from `new`, in order).
    async fn carry(&self, db: &mut sqlx::PgConnection, model: &str, new: &[NewFace], ids: &[Uuid]) -> Result<()> {
        if !self.carries() || new.is_empty() {
            // Nothing to hand on, or nothing to hand it to: the person stays, the face is gone.
            return Ok(());
        }
        let vectors: Vec<Option<Vec<f32>>> = self
            .rows
            .iter()
            .map(|r| r.embedding.as_deref().and_then(people_math::parse_literal))
            .collect();
        let old: Vec<people_math::Seen> = self
            .rows
            .iter()
            .zip(&vectors)
            .map(|(r, v)| people_math::Seen {
                bbox: [r.box_x, r.box_y, r.box_w, r.box_h],
                embedding: v.as_deref().map(|v| (r.model.as_str(), v)),
            })
            .collect();
        let fresh: Vec<people_math::Seen> = new
            .iter()
            .map(|f| people_math::Seen {
                bbox: f.bbox,
                embedding: f.embedding.as_deref().map(|v| (model, v)),
            })
            .collect();
        for (i, from) in people_math::match_faces(&old, &fresh).into_iter().enumerate() {
            let Some(j) = from else { continue };
            let (was, to) = (&self.rows[j], ids[i]);
            if let Some(person) = was.person_id {
                sqlx::query("UPDATE faces SET person_id = $2, assigned_by = $3 WHERE id = $1")
                    .bind(to)
                    .bind(person)
                    .bind(&was.assigned_by)
                    .execute(&mut *db)
                    .await?;
            }
            for (_, person) in self.rejections.iter().filter(|(f, _)| *f == was.id) {
                sqlx::query("INSERT INTO face_rejections (face_id, person_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                    .bind(to)
                    .bind(person)
                    .execute(&mut *db)
                    .await?;
            }
            for (person, _) in self.covers.iter().filter(|(_, f)| *f == was.id) {
                sqlx::query("UPDATE people SET cover_face_id = $2 WHERE id = $1")
                    .bind(person)
                    .bind(to)
                    .execute(&mut *db)
                    .await?;
            }
            if was.model != model {
                // The earlier face of another model stays until the sweep removes it, without the decisions that moved on.
                sqlx::query("DELETE FROM face_rejections WHERE face_id = $1")
                    .bind(was.id)
                    .execute(&mut *db)
                    .await?;
                sqlx::query("UPDATE faces SET person_id = NULL, assigned_by = 'auto' WHERE id = $1")
                    .bind(was.id)
                    .execute(&mut *db)
                    .await?;
            }
        }
        Ok(())
    }
}

/// Queue a `faces` job at backfill priority for every visible photo that has a preview and has not been scanned by
/// `(model, version)`: the backfill of an existing library, the catch-up after a model switch, and jobs that were lost. A job
/// already queued, running or failed is left alone; one that finished without a scan is requeued. Returns how many were queued.
pub async fn enqueue_missing(db: impl PgExecutor<'_>, model: &str, version: i32) -> Result<u64> {
    let done = sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version, priority)
         SELECT 'faces', a.id, $2, $3 FROM assets a
         WHERE a.thumbhash IS NOT NULL AND a.primary_id IS NULL AND a.purging_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM face_scans s WHERE s.asset_id = a.id AND s.model = $1 AND s.version = $2)
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

/// Queue the `faces` job of one photo (after its embedding, at backfill priority: faces are the least urgent thing to know).
pub async fn enqueue_for_asset(db: impl PgExecutor<'_>, asset_id: Uuid, version: i32) -> Result<()> {
    jobs::enqueue_with_priority(db, jobs::KIND_FACES, asset_id, version, jobs::PRIORITY_BACKFILL).await
}

/// A photo's picture changed (a rotate or crop): the faces found in the old one, and where they were, are wrong now. Forget
/// the scan and the faces nobody decided anything about; the faces the owner put in a person, took out of one or chose as a
/// cover stay, unread (every reader asks for a photo with a scan) until the new scan hands what was decided over to its faces
/// ([`store`]). With `requeue` (the feature is on), scan the new preview again (queued again even if the photo was scanned
/// before). Nothing is queued for a photo that was never scanned (the ordinary sweep will, if the feature is on). Returns whether
/// there was anything to forget.
pub async fn forget_and_requeue(db: &PgPool, asset_id: Uuid, version: i32, requeue: bool) -> Result<bool> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "DELETE FROM faces f WHERE f.asset_id = $1 AND f.person_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM face_rejections r WHERE r.face_id = f.id)
           AND NOT EXISTS (SELECT 1 FROM people p WHERE p.cover_face_id = f.id)",
    )
    .bind(asset_id)
    .execute(&mut *tx)
    .await?;
    let forgot = sqlx::query("DELETE FROM face_scans WHERE asset_id = $1")
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
        .bind(jobs::KIND_FACES)
        .bind(asset_id)
        .bind(version)
        .bind(jobs::PRIORITY_BACKFILL)
        .execute(db)
        .await?;
    }
    Ok(forgot)
}

/// How many visible photos with a preview have not been scanned by `(model, version)`.
pub async fn missing_count(db: impl PgExecutor<'_>, model: &str, version: i32) -> Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM assets a
         WHERE a.thumbhash IS NOT NULL AND a.primary_id IS NULL AND a.purging_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM face_scans s WHERE s.asset_id = a.id AND s.model = $1 AND s.version = $2)",
    )
    .bind(model)
    .bind(version)
    .fetch_one(db)
    .await?;
    Ok(n)
}

/// Delete the faces and scans of every model other than `keep`, and older versions of `keep` itself. Called once every photo has
/// been scanned by `(keep, version)`, so nothing is left to serve from the old ones. Returns the face rows removed.
pub async fn prune_superseded(db: &PgPool, keep: &str, version: i32) -> Result<u64> {
    let mut tx = db.begin().await?;
    let n = sqlx::query("DELETE FROM faces WHERE model <> $1 OR version <> $2")
        .bind(keep)
        .bind(version)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM face_scans WHERE model <> $1 OR version <> $2")
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
    /// Photos scanned by the current model, and photos that can be (they have a preview).
    pub scanned: i64,
    pub total: i64,
    /// Faces found in the scanned photos.
    pub faces: i64,
}

pub async fn progress(db: impl PgExecutor<'_>, owner: Uuid, model: &str, version: i32) -> Result<Progress> {
    let (scanned, total, faces): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(s.asset_id), count(*), coalesce(sum(s.faces), 0)::bigint FROM assets a
         LEFT JOIN face_scans s ON s.asset_id = a.id AND s.model = $2 AND s.version = $3
         WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.primary_id IS NULL AND a.thumbhash IS NOT NULL",
    )
    .bind(owner)
    .bind(model)
    .bind(version)
    .fetch_one(db)
    .await?;
    Ok(Progress { scanned, total, faces })
}

/// What [`delete_all`] removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Deleted {
    pub faces: u64,
    pub scans: u64,
    pub jobs: u64,
    /// The people (with their names) and the "not this person" decisions.
    pub people: u64,
    pub rejections: u64,
}

/// Delete all face data: every face, every scan record, every person (names included) and rejection, and every queued, running or
/// finished `faces` job, of one owner or of everyone. With `dry_run` it only counts. Nothing else is touched (the photos, the search
/// index and the rest of the queue are not). With the feature still on the sweep would start scanning again, so switch
/// `FACES_ENABLED` off first.
pub async fn delete_all(db: &PgPool, owner: Option<Uuid>, dry_run: bool) -> Result<Deleted> {
    let mut tx = db.begin().await?;
    let scope = " WHERE ($1::uuid IS NULL OR owner_id = $1)";
    let asset_scope = " WHERE ($1::uuid IS NULL OR asset_id IN (SELECT id FROM assets WHERE owner_id = $1))";
    let count = |table: &'static str, scope: &'static str| format!("SELECT count(*) FROM {table}{scope}");
    let (faces,): (i64,) = sqlx::query_as(&count("faces", scope))
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    let (scans,): (i64,) = sqlx::query_as(&count("face_scans", asset_scope))
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    let (jobs_n,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM jobs{asset_scope} AND kind = 'faces'"))
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    let (people,): (i64,) = sqlx::query_as(&count("people", scope))
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    let (rejections,): (i64,) = sqlx::query_as(&format!(
        "SELECT count(*) FROM face_rejections WHERE face_id IN (SELECT id FROM faces{scope})"
    ))
    .bind(owner)
    .fetch_one(&mut *tx)
    .await?;
    if !dry_run {
        // People first: their faces are set free, and every rejection goes with a person or a face.
        sqlx::query(&format!("DELETE FROM people{scope}"))
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("DELETE FROM people_runs{scope}"))
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("DELETE FROM faces{scope}"))
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("DELETE FROM face_scans{asset_scope}"))
            .bind(owner)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&format!("DELETE FROM jobs{asset_scope} AND kind = 'faces'"))
            .bind(owner)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Deleted {
        faces: faces as u64,
        scans: scans as u64,
        jobs: jobs_n as u64,
        people: people as u64,
        rejections: rejections as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_yes_values_switch_it_on() {
        for on in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(parse_enabled(Some(on)), "{on:?}");
        }
        for off in ["", "0", "false", "no", "off", "2", "enabled", "y"] {
            assert!(!parse_enabled(Some(off)), "{off:?}");
        }
        assert!(!parse_enabled(None));
    }

    #[test]
    fn the_model_id_matches_its_index_in_the_migration() {
        let sql = include_str!("../migrations/0029_faces.sql");
        assert!(sql.contains(&format!("WHERE model = '{MODEL}'")));
        assert!(sql.contains(INDEX_NAME));
        assert!(sql.contains(&format!("halfvec({DIM})")));
    }
}
