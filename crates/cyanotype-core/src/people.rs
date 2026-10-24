//! People (docs/faces.md, "People"): the worker's grouping of the faces of `faces` into the `people` the owner names and corrects,
//! and the queries the API shares with it. The arithmetic and the thresholds are in [`crate::people_math`].
//!
//! A run is per owner and follows the shape of the near-duplicate scan (`duplicates`): `people_runs` holds the request, the worker
//! claims an owner whose faces have stopped arriving, and a request that comes in while it runs stays wanted. The decisions of the owner
//! always win: a face the owner put in a person (`assigned_by = 'user'`) is never moved, a face is never put in a person it was
//! rejected from (`face_rejections`), and a named person is never deleted or merged. Every write of a run is guarded in SQL
//! (`person_id IS NULL AND NOT EXISTS (rejection)`), so a decision the owner makes while a run is in flight is not overwritten.
use std::collections::{HashMap, HashSet};

use anyhow::Result;
use sqlx::{PgConnection, PgExecutor, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    embeddings::pg_literal,
    faces::MODEL,
    people_math::{self as pm, Node},
};

/// A run waits until no face of the owner has been stored for this long (and none is queued or running), so a backfill of thousands of
/// photos groups once and not once per photo.
pub const QUIET_SECS: u64 = 120;
/// Faces per statement when asking the index for neighbours.
const CHUNK: usize = 400;

/// Ask for a run for `owner`: faces were stored for it, or the owner changed a person.
pub async fn request(db: impl PgExecutor<'_>, owner: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO people_runs (owner_id, requested_at) VALUES ($1, now())
         ON CONFLICT (owner_id) DO UPDATE SET requested_at = now()",
    )
    .bind(owner)
    .execute(db)
    .await?;
    Ok(())
}

/// [`request`] for the owner of `asset`, inside the transaction that stores the photo's faces.
pub async fn request_for_asset(db: &mut PgConnection, asset: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO people_runs (owner_id, requested_at) SELECT owner_id, now() FROM assets WHERE id = $1
         ON CONFLICT (owner_id) DO UPDATE SET requested_at = now()",
    )
    .bind(asset)
    .execute(db)
    .await?;
    Ok(())
}

/// The next owner whose run is due: wanted, not running, quiet for [`QUIET_SECS`], and with no `faces` job queued or running for
/// any of its photos (its library is still being scanned). Marks it started and returns it with the time of the request it answers.
pub async fn claim_due(db: &PgPool) -> Result<Option<(Uuid, OffsetDateTime)>> {
    // A run that started and never finished (the worker was killed) is retried after ten minutes.
    Ok(sqlx::query_as(
        "UPDATE people_runs SET started_at = now(), error = NULL
         WHERE owner_id = (
             SELECT r.owner_id FROM people_runs r
             WHERE r.requested_at IS NOT NULL
               AND (r.started_at IS NULL OR r.finished_at IS NULL AND r.started_at < now() - interval '10 minutes'
                    OR r.finished_at IS NOT NULL AND r.started_at <= r.finished_at)
               AND r.requested_at < now() - make_interval(secs => $1)
               AND NOT EXISTS (SELECT 1 FROM jobs j JOIN assets a ON a.id = j.asset_id
                               WHERE j.kind = 'faces' AND j.state IN ('queued', 'running') AND a.owner_id = r.owner_id)
             ORDER BY r.requested_at LIMIT 1 FOR UPDATE SKIP LOCKED)
         RETURNING owner_id, requested_at",
    )
    .bind(QUIET_SECS as f64)
    .fetch_optional(db)
    .await?
    .map(|(o, r): (Uuid, Option<OffsetDateTime>)| (o, r.unwrap_or_else(OffsetDateTime::now_utc))))
}

/// Record the end of a run. A request that arrived while it ran (a newer `requested_at`) stays wanted.
pub async fn finish(db: &PgPool, owner: Uuid, answered: OffsetDateTime, result: Result<Report, String>) -> Result<()> {
    let (assigned, created, grouped, error) = match &result {
        Ok(r) => (
            Some(r.assigned as i32),
            Some(r.created as i32),
            Some(r.grouped as i32),
            None,
        ),
        Err(e) => (None, None, None, Some(e.clone())),
    };
    sqlx::query(
        "UPDATE people_runs SET finished_at = now(), assigned = coalesce($3, assigned), created = coalesce($4, created),
                grouped = coalesce($5, grouped), error = $6,
                requested_at = CASE WHEN requested_at > $2 THEN requested_at END
         WHERE owner_id = $1",
    )
    .bind(owner)
    .bind(answered)
    .bind(assigned)
    .bind(created)
    .bind(grouped)
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

/// What a run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Report {
    /// Unassigned faces that joined an existing person one by one.
    pub assigned: usize,
    /// New unnamed people.
    pub created: usize,
    /// Faces in those people, and in groups that joined an existing person.
    pub grouped: usize,
    /// Fingerprinted faces that were looked at, and how many of them stayed unassigned.
    pub looked_at: usize,
    pub left: usize,
}

struct Ungrouped {
    id: Uuid,
    asset_id: Uuid,
    embedding: Vec<f32>,
}

/// The owner's fingerprinted faces that no person has: of live photos, found by the current model.
async fn load_ungrouped(db: &PgPool, owner: Uuid) -> Result<Vec<Ungrouped>> {
    let rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT f.id, f.asset_id, f.embedding::text FROM faces f JOIN assets a ON a.id = f.asset_id
         WHERE f.owner_id = $1 AND f.model = $2 AND f.person_id IS NULL AND f.embedding IS NOT NULL
           AND a.deleted_at IS NULL AND a.purging_at IS NULL AND a.primary_id IS NULL
           AND EXISTS (SELECT 1 FROM face_scans s WHERE s.asset_id = f.asset_id AND s.model = f.model)
         ORDER BY f.id",
    )
    .bind(owner)
    .bind(MODEL)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, asset_id, e)| {
            let embedding = pm::parse_literal(&e).filter(|v| v.len() == crate::faces::DIM)?;
            Some(Ungrouped {
                id,
                asset_id,
                embedding,
            })
        })
        .collect())
}

/// The index settings a nearest-neighbour step needs when most of what the index holds is filtered out (other owners, faces that
/// already have a person): the same as the search's, and local to the transaction.
async fn tune_index(tx: &mut PgConnection) -> Result<()> {
    sqlx::query(
        "SELECT set_config('hnsw.iterative_scan', 'relaxed_order', true), set_config('hnsw.ef_search', '200', true),
                set_config('hnsw.max_scan_tuples', '60000', true)",
    )
    .execute(tx)
    .await?;
    Ok(())
}

/// The centre (unit length) of every person's fingerprints, for `person` alone or the owner's whole list.
pub async fn centres(db: impl PgExecutor<'_>, owner: Uuid, person: Option<Uuid>) -> Result<Vec<(Uuid, Vec<f32>)>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT person_id, avg(embedding::vector(128))::text FROM faces f
         WHERE owner_id = $1 AND model = $2 AND person_id IS NOT NULL AND embedding IS NOT NULL AND ($3::uuid IS NULL OR person_id = $3)
           AND EXISTS (SELECT 1 FROM face_scans s WHERE s.asset_id = f.asset_id AND s.model = f.model)
         GROUP BY person_id",
    )
    .bind(owner)
    .bind(MODEL)
    .bind(person)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(p, text)| {
            let v = pm::parse_literal(&text)?;
            let c = pm::centre([v.as_slice()])?;
            Some((p, c))
        })
        .collect())
}

async fn rejections(db: &PgPool, faces: &[Uuid]) -> Result<HashMap<Uuid, HashSet<Uuid>>> {
    let mut out: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    for chunk in faces.chunks(5_000) {
        let rows: Vec<(Uuid, Uuid)> =
            sqlx::query_as("SELECT face_id, person_id FROM face_rejections WHERE face_id = ANY($1)")
                .bind(chunk)
                .fetch_all(db)
                .await?;
        for (f, p) in rows {
            out.entry(f).or_default().insert(p);
        }
    }
    Ok(out)
}

/// Put `faces` in `person` as the worker's decision: the ones that still have no person and were not rejected from it (the owner may
/// have decided meanwhile). Returns how many were.
async fn assign_guarded(db: &mut PgConnection, owner: Uuid, person: Uuid, faces: &[Uuid]) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE faces SET person_id = $3, assigned_by = 'auto'
         WHERE id = ANY($2) AND owner_id = $1 AND person_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM face_rejections r WHERE r.face_id = faces.id AND r.person_id = $3)",
    )
    .bind(owner)
    .bind(faces)
    .bind(person)
    .execute(db)
    .await?
    .rows_affected())
}

/// Step one: each unassigned face joins the person of its nearest known faces when [`pm::choose_person`] says so. All the
/// decisions are made against the people as they were when the run began (a face that joins now does not become the anchor of the
/// next one), then applied. Returns the faces that joined.
async fn assign_nearest(db: &PgPool, owner: Uuid, pool: &[Ungrouped]) -> Result<HashSet<Uuid>> {
    let (known,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM faces WHERE owner_id = $1 AND model = $2 AND person_id IS NOT NULL AND embedding IS NOT NULL)",
    )
    .bind(owner)
    .bind(MODEL)
    .fetch_one(db)
    .await?;
    if !known {
        return Ok(HashSet::new());
    }
    let ids: Vec<Uuid> = pool.iter().map(|u| u.id).collect();
    let rejected = rejections(db, &ids).await?;
    // The predicate and the expression repeat the index's (migration 0029).
    let sql = format!(
        "SELECT u.id, n.person_id, n.sim FROM faces u
         CROSS JOIN LATERAL (
             SELECT f.person_id, 1 - (f.embedding::halfvec(128) <=> u.embedding::halfvec(128)) AS sim FROM faces f
             WHERE f.model = '{MODEL}' AND f.owner_id = $1 AND f.person_id IS NOT NULL AND f.embedding IS NOT NULL
             ORDER BY f.embedding::halfvec(128) <=> u.embedding::halfvec(128) LIMIT {}
         ) n
         WHERE u.id = ANY($2)",
        pm::NEIGHBOURS
    );
    let mut decided: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let mut tx = db.begin().await?;
        tune_index(&mut tx).await?;
        let rows: Vec<(Uuid, Uuid, f64)> = sqlx::query_as(&sql)
            .bind(owner)
            .bind(chunk)
            .persistent(false)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        let mut by_face: HashMap<Uuid, Vec<(Uuid, f32)>> = HashMap::new();
        for (f, p, s) in rows {
            by_face.entry(f).or_default().push((p, s as f32));
        }
        for (face, neigh) in by_face {
            let none = HashSet::new();
            let no = rejected.get(&face).unwrap_or(&none);
            if let Some(p) = pm::choose_person(&neigh, no, pm::ASSIGN, pm::ASSIGN_MARGIN) {
                decided.entry(p).or_default().push(face);
            }
        }
    }
    let mut done = HashSet::new();
    for (person, faces) in decided {
        let mut tx = db.begin().await?;
        // Which ones took is told by a second look: the guard may have refused some.
        assign_guarded(&mut tx, owner, person, &faces).await?;
        let took: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM faces WHERE id = ANY($1) AND person_id = $2")
            .bind(&faces)
            .bind(person)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        done.extend(took);
    }
    Ok(done)
}

/// Step two: the faces still without a person are grouped ([`pm::group`]) over the nearest-neighbour graph among themselves. A group
/// of several faces next to exactly one known person joins them; a group that is no one's and has [`pm::MIN_FACES`] faces from
/// [`pm::MIN_PHOTOS`] photos becomes a new unnamed person; the rest wait. Returns `(people made, faces placed)`.
async fn group_rest(db: &PgPool, owner: Uuid, rest: &[&Ungrouped]) -> Result<(usize, usize)> {
    if rest.len() < 2 {
        return Ok((0, 0));
    }
    let index: HashMap<Uuid, usize> = rest.iter().enumerate().map(|(i, u)| (u.id, i)).collect();
    let mut photo_of: HashMap<Uuid, u32> = HashMap::new();
    let photos: Vec<u32> = rest
        .iter()
        .map(|u| {
            let n = photo_of.len() as u32;
            *photo_of.entry(u.asset_id).or_insert(n)
        })
        .collect();
    let sql = format!(
        "SELECT u.id, n.id, n.sim FROM faces u
         CROSS JOIN LATERAL (
             SELECT f.id, 1 - (f.embedding::halfvec(128) <=> u.embedding::halfvec(128)) AS sim FROM faces f
             WHERE f.model = '{MODEL}' AND f.owner_id = $1 AND f.person_id IS NULL AND f.embedding IS NOT NULL
             ORDER BY f.embedding::halfvec(128) <=> u.embedding::halfvec(128) LIMIT {}
         ) n
         WHERE u.id = ANY($2)",
        pm::NEIGHBOURS + 1
    );
    let mut edges: Vec<(f32, u32, u32)> = Vec::new();
    let ids: Vec<Uuid> = rest.iter().map(|u| u.id).collect();
    for chunk in ids.chunks(CHUNK) {
        let mut tx = db.begin().await?;
        tune_index(&mut tx).await?;
        let rows: Vec<(Uuid, Uuid, f64)> = sqlx::query_as(&sql)
            .bind(owner)
            .bind(chunk)
            .persistent(false)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        for (a, b, s) in rows {
            let (Some(&i), Some(&j)) = (index.get(&a), index.get(&b)) else {
                continue;
            };
            if i < j && s as f32 >= pm::LINK {
                edges.push((s as f32, i as u32, j as u32));
            }
        }
    }
    let nodes: Vec<Node> = rest
        .iter()
        .zip(&photos)
        .map(|(u, p)| Node {
            embedding: &u.embedding,
            photo: *p,
        })
        .collect();
    let groups = pm::group(&nodes, &mut edges, pm::MERGE);
    let people = centres(db, owner, None).await?;
    let rejected = rejections(db, &ids).await?;
    let (mut made, mut placed) = (0, 0);
    for g in groups.iter().filter(|g| g.len() >= 2) {
        let faces: Vec<Uuid> = g.iter().map(|i| rest[*i].id).collect();
        let n_photos = g.iter().map(|i| photos[*i]).collect::<HashSet<_>>().len();
        let Some(c) = pm::centre(g.iter().map(|i| rest[*i].embedding.as_slice())) else {
            continue;
        };
        // People the group may join: none of its faces was rejected from them.
        let banned: HashSet<Uuid> = faces
            .iter()
            .flat_map(|f| rejected.get(f).into_iter().flatten().copied())
            .collect();
        let near: Vec<(Uuid, f32)> = people
            .iter()
            .filter(|(p, _)| !banned.contains(p))
            .map(|(p, v)| (*p, pm::dot(&c, v)))
            .collect();
        let mut tx = db.begin().await?;
        if let Some(person) = pm::choose_person(&near, &HashSet::new(), pm::JOIN, pm::ASSIGN_MARGIN) {
            placed += assign_guarded(&mut tx, owner, person, &faces).await? as usize;
        } else if near.iter().any(|(_, s)| *s >= pm::JOIN) {
            // Two people it could be: left to the owner (suggestions).
        } else if pm::big_enough(faces.len(), n_photos) {
            let person: Uuid = sqlx::query_scalar("INSERT INTO people (owner_id) VALUES ($1) RETURNING id")
                .bind(owner)
                .fetch_one(&mut *tx)
                .await?;
            let took = assign_guarded(&mut tx, owner, person, &faces).await? as usize;
            if took >= pm::MIN_FACES {
                made += 1;
                placed += took;
            } else {
                // The owner decided about some of them meanwhile: not a group any more.
                tx.rollback().await?;
                continue;
            }
        }
        tx.commit().await?;
    }
    Ok((made, placed))
}

/// Group one owner's faces (see the module comment). Safe to run again at any time, and to run twice at once.
pub async fn run(db: &PgPool, owner: Uuid) -> Result<Report> {
    let mut report = Report::default();
    let pool = load_ungrouped(db, owner).await?;
    report.looked_at = pool.len();
    let joined = assign_nearest(db, owner, &pool).await?;
    report.assigned = joined.len();
    let rest: Vec<&Ungrouped> = pool.iter().filter(|u| !joined.contains(&u.id)).collect();
    let (made, placed) = group_rest(db, owner, &rest).await?;
    report.created = made;
    report.grouped = placed;
    report.left = rest.len().saturating_sub(placed);
    // An unnamed person whose faces are all gone (photos purged, faces taken out) is not kept. A named one never is.
    sqlx::query(
        "DELETE FROM people p WHERE p.owner_id = $1 AND p.name IS NULL
           AND NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = p.id)",
    )
    .bind(owner)
    .execute(db)
    .await?;
    Ok(report)
}

/// Ask for a first run for every owner who has fingerprinted faces and has never been grouped (faces stored before People existed).
/// Owners that already have a request or a finished run are left alone. Returns how many were asked.
pub async fn request_missing(db: impl PgExecutor<'_>) -> Result<u64> {
    Ok(sqlx::query(
        "INSERT INTO people_runs (owner_id, requested_at)
         SELECT DISTINCT owner_id, now() FROM faces WHERE person_id IS NULL AND embedding IS NOT NULL
         ON CONFLICT (owner_id) DO NOTHING",
    )
    .execute(db)
    .await?
    .rows_affected())
}

/// An unassigned face that looks like a person ("Is this X?").
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub face_id: Uuid,
    pub asset_id: Uuid,
    pub bbox: [f32; 4],
    /// Cosine similarity to the centre of the person's faces.
    pub similarity: f32,
}

/// The unassigned faces closest to the centre of `person`'s faces, at least [`pm::SUGGEST`] similar, not rejected from it, in live
/// photos (hidden ones only with `unlocked`), best first. Empty for a person without fingerprinted faces.
pub async fn suggestions(
    db: &PgPool,
    owner: Uuid,
    person: Uuid,
    unlocked: bool,
    limit: i64,
) -> Result<Vec<Suggestion>> {
    let Some((_, centre)) = centres(db, owner, Some(person)).await?.pop() else {
        return Ok(Vec::new());
    };
    let sql = format!(
        "WITH nn AS MATERIALIZED (
             SELECT f.id, 1 - (f.embedding::halfvec(128) <=> $1::text::halfvec(128)) AS sim FROM faces f
             WHERE f.model = '{MODEL}' AND f.owner_id = $2 AND f.person_id IS NULL AND f.embedding IS NOT NULL
             ORDER BY f.embedding::halfvec(128) <=> $1::text::halfvec(128) LIMIT 300)
         SELECT f.id, f.asset_id, f.box_x, f.box_y, f.box_w, f.box_h, nn.sim FROM nn
         JOIN faces f ON f.id = nn.id JOIN assets a ON a.id = f.asset_id
         WHERE nn.sim >= $3 AND a.deleted_at IS NULL AND a.purging_at IS NULL AND a.primary_id IS NULL AND (NOT a.hidden OR $4)
           AND EXISTS (SELECT 1 FROM face_scans s WHERE s.asset_id = f.asset_id AND s.model = f.model)
           AND NOT EXISTS (SELECT 1 FROM face_rejections r WHERE r.face_id = f.id AND r.person_id = $5)
         ORDER BY nn.sim DESC, f.id LIMIT $6"
    );
    let mut tx = db.begin().await?;
    tune_index(&mut tx).await?;
    let rows: Vec<(Uuid, Uuid, f32, f32, f32, f32, f64)> = sqlx::query_as(&sql)
        .bind(pg_literal(&centre))
        .bind(owner)
        .bind(f64::from(pm::SUGGEST))
        .bind(unlocked)
        .bind(person)
        .bind(limit)
        .persistent(false)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(|(face_id, asset_id, x, y, w, h, s)| Suggestion {
            face_id,
            asset_id,
            bbox: [x, y, w, h],
            similarity: s as f32,
        })
        .collect())
}
