//! Near-duplicate photos (`docs/library.md`, "Near-duplicates"): the same picture twice, as a resized or re-encoded copy, an export or
//! a shot repeated moments later. (Exact copies cannot exist: the content hash is unique per owner.)
//!
//! **Finding them.** For every one of an owner's photos that has an image vector (live, not the hidden RAW half of a pair,
//! trashed ones never), the worker asks the HNSW index for its nearest neighbours ([`NEIGHBOURS`]) and judges each close pair
//! with [`is_duplicate`]: the cosine of the two embeddings must reach a threshold that depends on what else is known about the
//! pair (all calibrated in `cyanotype-clip`, `duplicates.rs`, `docs/build.md` "Near-duplicate threshold"):
//!
//! | what the pair shares | threshold |
//! |---|---|
//! | the same camera and capture times within [`MAX_CAPTURE_GAP_SECS`] | `with_metadata` (the lowest: two independent facts agree) |
//! | the same pixel dimensions | `same_size` |
//! | neither has a capture time (a copy that lost its EXIF, e.g. sent through a messenger) | `stripped` |
//!
//! Anything else is not a duplicate however close the cosine: a resized copy that lost its metadata is only found by the strict rule.
//! Never a pair from one **burst** (rapid-fire shots are bursts, not duplicates: `docs/library.md`, "Bursts") and never a pair the
//! owner dismissed ("Not duplicates"). Pairs join into groups by connected components; a group is at most [`MAX_GROUP`] photos
//! (a bigger chain is almost certainly the threshold bridging different pictures and is left out and counted).
//!
//! **Keeping them.** The groups are written to `duplicate_groups` / `duplicate_members` (a scan replaces the owner's), and a scan runs
//! in the worker, on demand (`request(manual)`) or a quiet [`QUIET_SECS`] after the last embedding of an import (`request(auto)`).
use std::collections::{HashMap, HashSet};

use anyhow::Result;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// The most that may pass between the capture times of two photos for them to count as "taken at the same time".
pub const MAX_CAPTURE_GAP_SECS: i64 = 60;
/// A group of more photos than this is dropped.
pub const MAX_GROUP: usize = 50;
/// Nearest neighbours asked for per photo.
pub const NEIGHBOURS: i64 = 12;
/// An automatic scan waits this long after the last embedding of an import, so one scan follows a whole import.
pub const QUIET_SECS: i64 = 300;
/// Photos looked up in one statement.
const CHUNK: usize = 500;

/// The cosine thresholds of one embedding model (see the module's table).
#[derive(Clone, Debug, PartialEq)]
pub struct Thresholds {
    /// The model id and version of the vectors compared (`asset_embeddings.model`, `.version`), and their dimension.
    pub model: String,
    pub version: i32,
    pub dim: usize,
    pub with_metadata: f32,
    pub same_size: f32,
    pub stripped: f32,
}

impl Thresholds {
    /// The lowest of the three: nothing below it can be a duplicate.
    pub fn lowest(&self) -> f32 {
        self.with_metadata.min(self.same_size).min(self.stripped)
    }
}

/// What a scan knows about one photo.
#[derive(Clone, Debug, PartialEq)]
pub struct Photo {
    pub id: Uuid,
    pub taken_at: Option<OffsetDateTime>,
    /// `assets.camera_label` ("Canon EOS R5").
    pub camera: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub burst_id: Option<Uuid>,
}

/// Is this pair of photos, whose embeddings have cosine `cos`, a near-duplicate?
pub fn is_duplicate(a: &Photo, b: &Photo, cos: f32, t: &Thresholds) -> bool {
    if a.id == b.id || cos < t.lowest() {
        return false;
    }
    // Rapid-fire shots are bursts, which have their own grouping and are not offered for deletion.
    if a.burst_id.is_some() && a.burst_id == b.burst_id {
        return false;
    }
    let same_moment = match (a.taken_at, b.taken_at, &a.camera, &b.camera) {
        (Some(x), Some(y), Some(ca), Some(cb)) => ca == cb && (x - y).whole_seconds().abs() <= MAX_CAPTURE_GAP_SECS,
        _ => false,
    };
    if same_moment && cos >= t.with_metadata {
        return true;
    }
    let same_size =
        matches!((a.width, a.height, b.width, b.height), (Some(w), Some(h), Some(w2), Some(h2)) if w == w2 && h == h2);
    if same_size && cos >= t.same_size {
        return true;
    }
    a.taken_at.is_none() && b.taken_at.is_none() && cos >= t.stripped
}

/// Connected components of `n` items joined by `pairs` (indexes), each of two or more, in index order.
pub fn components(n: usize, pairs: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for &(a, b) in pairs {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[rb.max(ra)] = rb.min(ra);
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() >= 2).collect();
    out.sort_by_key(|g| g[0]);
    out
}

/// What a scan found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanResult {
    /// Photos with a vector that were looked at.
    pub photos: usize,
    pub groups: usize,
    /// Groups left out for being bigger than [`MAX_GROUP`].
    pub too_big: usize,
}

#[derive(sqlx::FromRow)]
struct PhotoRow {
    id: Uuid,
    taken_at: Option<OffsetDateTime>,
    camera_label: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    burst_id: Option<Uuid>,
    sort_time: OffsetDateTime,
}

/// Find the owner's near-duplicates and replace their stored groups with what was found.
pub async fn scan(db: &PgPool, owner: Uuid, t: &Thresholds) -> Result<ScanResult> {
    let rows: Vec<PhotoRow> = sqlx::query_as(
        "SELECT a.id, a.taken_at, a.camera_label, a.width, a.height, a.burst_id,
                coalesce(a.taken_at, a.file_modified_at, a.created_at) AS sort_time
         FROM assets a
         WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.primary_id IS NULL
           AND EXISTS (SELECT 1 FROM asset_embeddings e WHERE e.asset_id = a.id AND e.model = $2 AND e.version = $3)
         ORDER BY a.id",
    )
    .bind(owner)
    .bind(&t.model)
    .bind(t.version)
    .fetch_all(db)
    .await?;
    let photos: Vec<Photo> = rows
        .iter()
        .map(|r| Photo {
            id: r.id,
            taken_at: r.taken_at,
            camera: r.camera_label.clone(),
            width: r.width,
            height: r.height,
            burst_id: r.burst_id,
        })
        .collect();
    let index: HashMap<Uuid, usize> = photos.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    let dismissed: HashSet<(Uuid, Uuid)> =
        sqlx::query_as::<_, (Uuid, Uuid)>("SELECT a, b FROM duplicate_dismissals WHERE owner_id = $1")
            .bind(owner)
            .fetch_all(db)
            .await?
            .into_iter()
            .collect();

    // The model id, version and dimension are constants from code (never request input): written into the SQL so that the
    // planner can match the model's partial HNSW index (migration 0013).
    let sql = format!(
        "SELECT a.id AS a, n.asset_id AS b, (1 - n.dist)::float4 AS cos
         FROM unnest($1::uuid[]) AS a(id)
         JOIN asset_embeddings e ON e.asset_id = a.id AND e.model = '{m}' AND e.version = {v}
         CROSS JOIN LATERAL (
             SELECT e2.asset_id, (e2.embedding::halfvec({d}) <=> e.embedding::halfvec({d})) AS dist
             FROM asset_embeddings e2
             WHERE e2.model = '{m}' AND e2.version = {v} AND e2.asset_id <> a.id
             ORDER BY e2.embedding::halfvec({d}) <=> e.embedding::halfvec({d}) LIMIT {k}
         ) n",
        m = t.model.replace('\'', ""),
        v = t.version,
        d = t.dim,
        k = NEIGHBOURS
    );
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    for chunk in photos.chunks(CHUNK) {
        let ids: Vec<Uuid> = chunk.iter().map(|p| p.id).collect();
        let near: Vec<(Uuid, Uuid, f32)> = sqlx::query_as(&sql).bind(&ids).fetch_all(db).await?;
        for (a, b, cos) in near {
            // Neighbours of other users (the index holds everyone's vectors) are not in this owner's list.
            let (Some(&i), Some(&j)) = (index.get(&a), index.get(&b)) else {
                continue;
            };
            let key = (i.min(j), i.max(j));
            if !seen.insert(key) {
                continue;
            }
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            if dismissed.contains(&(lo, hi)) {
                continue;
            }
            if is_duplicate(&photos[i], &photos[j], cos, t) {
                pairs.push(key);
            }
        }
    }
    let mut groups = components(photos.len(), &pairs);
    let before = groups.len();
    groups.retain(|g| g.len() <= MAX_GROUP);
    let too_big = before - groups.len();

    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM duplicate_groups WHERE owner_id = $1")
        .bind(owner)
        .execute(&mut *tx)
        .await?;
    for g in &groups {
        let newest = g
            .iter()
            .map(|&i| rows[i].sort_time)
            .max()
            .unwrap_or_else(OffsetDateTime::now_utc);
        let gid: Uuid =
            sqlx::query_scalar("INSERT INTO duplicate_groups (owner_id, sort_time) VALUES ($1, $2) RETURNING id")
                .bind(owner)
                .bind(newest)
                .fetch_one(&mut *tx)
                .await?;
        let members: Vec<Uuid> = g.iter().map(|&i| photos[i].id).collect();
        sqlx::query("INSERT INTO duplicate_members (group_id, asset_id) SELECT $1, unnest($2::uuid[])")
            .bind(gid)
            .bind(&members)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(ScanResult {
        photos: photos.len(),
        groups: groups.len(),
        too_big,
    })
}

/// Ask for a scan of `owner`. A manual request (the owner pressed the button) runs at once and is never downgraded to an automatic
/// one; an automatic request (a photo was just embedded) only moves the quiet period.
pub async fn request(db: &PgPool, owner: Uuid, manual: bool) -> Result<()> {
    sqlx::query(
        "INSERT INTO duplicate_scans (owner_id, requested_at, manual) VALUES ($1, now(), $2)
         ON CONFLICT (owner_id) DO UPDATE
         SET requested_at = now(), manual = $2 OR (duplicate_scans.requested_at IS NOT NULL AND duplicate_scans.manual)",
    )
    .bind(owner)
    .bind(manual)
    .execute(db)
    .await?;
    Ok(())
}

/// [`request`] (automatic) for the owner of `asset`: a photo was just embedded, so a scan is due once the import goes quiet.
pub async fn request_for_asset(db: &PgPool, asset: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO duplicate_scans (owner_id, requested_at, manual)
         SELECT owner_id, now(), false FROM assets WHERE id = $1
         ON CONFLICT (owner_id) DO UPDATE
         SET requested_at = now(), manual = duplicate_scans.requested_at IS NOT NULL AND duplicate_scans.manual",
    )
    .bind(asset)
    .execute(db)
    .await?;
    Ok(())
}

/// The next owner whose scan is due: wanted, not running, and either manual or quiet for [`QUIET_SECS`]. Marks it started.
/// Returns the owner and the time of the request it answers.
pub async fn claim_due(db: &PgPool) -> Result<Option<(Uuid, OffsetDateTime)>> {
    // A scan that started and never finished (the worker was killed) is retried after ten minutes.
    Ok(sqlx::query_as(
        "UPDATE duplicate_scans SET started_at = now(), error = NULL
         WHERE owner_id = (
             SELECT owner_id FROM duplicate_scans
             WHERE requested_at IS NOT NULL
               AND (started_at IS NULL OR finished_at IS NULL AND started_at < now() - interval '10 minutes'
                    OR finished_at IS NOT NULL AND started_at <= finished_at)
               AND (manual OR requested_at < now() - make_interval(secs => $1))
             ORDER BY manual DESC, requested_at LIMIT 1 FOR UPDATE SKIP LOCKED)
         RETURNING owner_id, requested_at",
    )
    .bind(QUIET_SECS as f64)
    .fetch_optional(db)
    .await?
    .map(|(o, r): (Uuid, Option<OffsetDateTime>)| (o, r.unwrap_or_else(OffsetDateTime::now_utc))))
}

/// Record the end of a scan. A request that arrived while it ran (a newer `requested_at`) stays wanted.
pub async fn finish(
    db: &PgPool,
    owner: Uuid,
    answered: OffsetDateTime,
    result: Result<ScanResult, String>,
) -> Result<()> {
    let (groups, photos, error) = match &result {
        Ok(r) => (Some(r.groups as i32), Some(r.photos as i32), None),
        Err(e) => (None, None, Some(e.clone())),
    };
    sqlx::query(
        "UPDATE duplicate_scans SET finished_at = now(), groups = coalesce($3, groups), photos = coalesce($4, photos), error = $5,
                requested_at = CASE WHEN requested_at > $2 THEN requested_at END,
                manual = CASE WHEN requested_at > $2 THEN manual ELSE false END
         WHERE owner_id = $1",
    )
    .bind(owner)
    .bind(answered)
    .bind(groups)
    .bind(photos)
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn t() -> Thresholds {
        Thresholds {
            model: "m".into(),
            version: 1,
            dim: 4,
            with_metadata: 0.85,
            same_size: 0.92,
            stripped: 0.94,
        }
    }

    fn photo(n: u128) -> Photo {
        Photo {
            id: Uuid::from_u128(n),
            taken_at: Some(datetime!(2024-05-01 12:00:00 UTC)),
            camera: Some("Canon EOS R5".into()),
            width: Some(4000),
            height: Some(3000),
            burst_id: None,
        }
    }

    #[test]
    fn the_threshold_depends_on_what_else_the_pair_shares() {
        let (a, mut b) = (photo(1), photo(2));
        // Same camera, 30 s apart, different size: the metadata rule.
        b.taken_at = Some(datetime!(2024-05-01 12:00:30 UTC));
        b.width = Some(2000);
        assert!(is_duplicate(&a, &b, 0.86, &t()));
        assert!(!is_duplicate(&a, &b, 0.84, &t()));
        // Too far apart in time and another size: nothing confirms it.
        b.taken_at = Some(datetime!(2024-05-01 12:05:00 UTC));
        assert!(!is_duplicate(&a, &b, 0.99, &t()));
        // Same size, any time: the stricter size rule.
        b.width = Some(4000);
        assert!(!is_duplicate(&a, &b, 0.91, &t()));
        assert!(is_duplicate(&a, &b, 0.93, &t()));
        // Another camera at the same time and size: still only the size rule.
        b.camera = Some("Sony A7".into());
        b.taken_at = a.taken_at;
        assert!(!is_duplicate(&a, &b, 0.90, &t()));
        assert!(is_duplicate(&a, &b, 0.93, &t()));
    }

    #[test]
    fn a_copy_that_lost_its_metadata_needs_the_strictest_rule() {
        let (mut a, mut b) = (photo(1), photo(2));
        (a.taken_at, b.taken_at) = (None, None);
        (a.camera, b.camera) = (None, None);
        b.width = Some(1000);
        assert!(!is_duplicate(&a, &b, 0.93, &t()));
        assert!(is_duplicate(&a, &b, 0.95, &t()));
        // Only one lost its time: not "stripped".
        b.taken_at = Some(datetime!(2024-05-01 12:00:00 UTC));
        assert!(!is_duplicate(&a, &b, 0.99, &t()));
        // Unknown dimensions never match by size.
        (a.width, b.width) = (None, None);
        assert!(!is_duplicate(&a, &b, 0.99, &t()));
    }

    #[test]
    fn bursts_are_not_duplicates_and_nothing_is_its_own_duplicate() {
        let (mut a, mut b) = (photo(1), photo(2));
        let burst = Uuid::from_u128(9);
        (a.burst_id, b.burst_id) = (Some(burst), Some(burst));
        assert!(!is_duplicate(&a, &b, 0.999, &t()), "one burst");
        b.burst_id = Some(Uuid::from_u128(10));
        assert!(
            is_duplicate(&a, &b, 0.999, &t()),
            "two different bursts: a copy of a burst shot"
        );
        assert!(!is_duplicate(&a, &a, 1.0, &t()));
        assert!(!is_duplicate(&a, &photo(3), 0.5, &t()), "below every threshold");
    }

    #[test]
    fn components_join_chains_and_leave_singles() {
        let g = components(7, &[(0, 1), (1, 2), (4, 5)]);
        assert_eq!(g, vec![vec![0, 1, 2], vec![4, 5]]);
        assert!(components(3, &[]).is_empty());
        // Order of the pairs does not matter.
        assert_eq!(components(4, &[(3, 2), (1, 0), (2, 1)]), vec![vec![0, 1, 2, 3]]);
    }
}
