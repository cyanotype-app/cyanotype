//! RAW + JPEG pairs. A camera's RAW and its JPEG/HEIC rendering of the same shot are two assets (both originals stay
//! in S3, each with its own row); this module decides which two belong together and links them:
//! `assets.primary_id` on the RAW points at the visible primary. See `docs/library.md`.
//!
//! The matching rule is pure ([`choose`]) so it is unit tested; [`pair_asset`] applies it inside one transaction,
//! serialised per (owner, filename stem) with an advisory lock so two workers finishing the two halves of a pair at
//! the same moment cannot both pair them differently, and so unpairing cannot interleave with pairing.
use anyhow::Result;
use sqlx::{PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Camera RAW extensions (lowercase). The worker routes the same set to its RAW pipeline.
pub const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "rwl", "pef", "ptx", "dng", "srw",
    "3fr", "fff", "iiq", "erf", "kdc", "dcr", "mef", "mos", "mrw", "raw", "rwz", "x3f", "gpr", "cap", "eip",
];
/// Camera-rendered files that become the primary of a pair.
pub const PRIMARY_EXTS: &[&str] = &["jpg", "jpeg", "jpe", "heic", "heif", "hif", "avif"];
const PRIMARY_TYPES: &[&str] = &["image/jpeg", "image/jpg", "image/heic", "image/heif", "image/avif"];

/// Capture times this close (either direction, inclusive) count as the same moment.
pub const TIME_TOLERANCE: Duration = Duration::seconds(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
    Raw,
    Primary,
    /// PNG, WebP, TIFF, video, ...: never paired.
    Other,
}

fn extension(name: &str) -> Option<String> {
    let base = basename(name);
    let (stem, ext) = base.rsplit_once('.')?;
    (!stem.is_empty()).then(|| ext.to_ascii_lowercase())
}

fn basename(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// RAW by extension; primary by extension or, failing that, by content type.
pub fn classify(name: &str, content_type: &str) -> FileClass {
    let ext = extension(name);
    let ext = ext.as_deref().unwrap_or("");
    if RAW_EXTS.contains(&ext) {
        return FileClass::Raw;
    }
    let ct = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if PRIMARY_EXTS.contains(&ext) || (ext.is_empty() && PRIMARY_TYPES.contains(&ct.as_str())) {
        FileClass::Primary
    } else {
        FileClass::Other
    }
}

/// The filename without directory, last extension and case: `IMG_1234.CR3` and `img_1234.jpg` share `img_1234`.
/// Mirrors the SQL function `asset_stem`. Empty for names like `.jpg`.
pub fn stem(name: &str) -> String {
    let base = basename(name);
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    stem.to_lowercase()
}

/// What the matcher knows about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: Uuid,
    pub taken_at: Option<OffsetDateTime>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
}

fn camera(c: &Candidate) -> Option<(String, String)> {
    let norm = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
    };
    Some((norm(&c.camera_make)?, norm(&c.camera_model)?))
}

/// Pick the partner of `subject` among `others` (already filtered to same owner, same stem, opposite class, live and
/// unpaired). Two files pair when:
/// - both have a capture time and they are within [`TIME_TOLERANCE`]: the closest wins (ties go to the lowest id);
/// - otherwise, when at least one capture time is missing: both have camera make and model, they are equal
///   (case-insensitive) and exactly one candidate fits. More than one is ambiguous and pairs nothing.
///
/// A differing capture time (more than a second apart) never falls back to the camera.
pub fn choose(subject: &Candidate, others: &[Candidate]) -> Option<Uuid> {
    let by_time = others
        .iter()
        .filter_map(|o| {
            let (a, b) = (subject.taken_at?, o.taken_at?);
            let gap = (a - b).abs();
            (gap <= TIME_TOLERANCE).then_some((gap, o.id))
        })
        .min();
    if let Some((_, id)) = by_time {
        return Some(id);
    }
    let cam = camera(subject)?;
    let mut fits = others
        .iter()
        .filter(|o| (subject.taken_at.is_none() || o.taken_at.is_none()) && camera(o).as_ref() == Some(&cam));
    match (fits.next(), fits.next()) {
        (Some(o), None) => Some(o.id),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paired {
    /// Linked now: `raw` is hidden behind `primary`.
    Linked { raw: Uuid, primary: Uuid },
    /// Already part of a pair (this call changed nothing).
    Already,
    /// Not a RAW or JPEG/HEIC, gone, trashed or being deleted.
    NotApplicable,
    /// Eligible, but no partner (yet).
    NoMatch,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    original_name: String,
    content_type: String,
    taken_at: Option<OffsetDateTime>,
    camera_make: Option<String>,
    camera_model: Option<String>,
    live: bool,
    has_partner: bool,
}

impl Row {
    fn candidate(&self) -> Candidate {
        Candidate {
            id: self.id,
            taken_at: self.taken_at,
            camera_make: self.camera_make.clone(),
            camera_model: self.camera_model.clone(),
        }
    }
}

const ROW: &str = "SELECT a.id, a.original_name, a.content_type, a.taken_at, a.camera_make, a.camera_model,
        (a.deleted_at IS NULL AND a.purging_at IS NULL) AS live,
        (a.primary_id IS NOT NULL OR EXISTS (SELECT 1 FROM assets r WHERE r.primary_id = a.id)) AS has_partner
 FROM assets a";

/// Serialise everything that links or unlinks files of one (owner, stem) until the transaction ends.
async fn lock_stem(tx: &mut Transaction<'_, Postgres>, owner: Uuid, stem: &str) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1 || '/' || $2, 0))")
        .bind(owner.to_string())
        .bind(stem)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Try to pair `id` with a partner. Call it once the asset's metadata (capture time, camera) is known; it is
/// idempotent and safe to call from any number of workers at once.
pub async fn pair_asset(pool: &PgPool, id: Uuid) -> Result<Paired> {
    let head: Option<(Uuid, String, String)> =
        sqlx::query_as("SELECT owner_id, original_name, content_type FROM assets WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some((owner, name, content_type)) = head else {
        return Ok(Paired::NotApplicable);
    };
    let class = classify(&name, &content_type);
    let stem = stem(&name);
    if class == FileClass::Other || stem.is_empty() {
        return Ok(Paired::NotApplicable);
    }

    let mut tx = pool.begin().await?;
    lock_stem(&mut tx, owner, &stem).await?;
    // Re-read under the lock: the first read may be stale by now.
    let Some(me) = sqlx::query_as::<_, Row>(&format!("{ROW} WHERE a.id = $1 FOR UPDATE OF a"))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Ok(Paired::NotApplicable);
    };
    if !me.live {
        return Ok(Paired::NotApplicable);
    }
    if me.has_partner {
        return Ok(Paired::Already);
    }

    // Candidates: same owner and stem, live, not paired (a primary with a RAW attached is taken), not separated
    // by the user. The Rust-side checks re-apply the stem and class rules, so the SQL stem index only narrows.
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "{ROW} WHERE a.owner_id = $1 AND asset_stem(a.original_name) = $2 AND a.id <> $3
           AND a.deleted_at IS NULL AND a.purging_at IS NULL AND a.primary_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM assets r WHERE r.primary_id = a.id)
           AND NOT EXISTS (SELECT 1 FROM asset_pair_rejections j
                           WHERE (j.raw_id = $3 AND j.primary_id = a.id) OR (j.raw_id = a.id AND j.primary_id = $3))
         ORDER BY a.id FOR UPDATE OF a"
    ))
    .bind(owner)
    .bind(&stem)
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let others: Vec<Candidate> = rows
        .iter()
        .filter(|r| {
            let c = classify(&r.original_name, &r.content_type);
            c != FileClass::Other && c != class && self::stem(&r.original_name) == stem
        })
        .map(Row::candidate)
        .collect();
    let Some(partner) = choose(&me.candidate(), &others) else {
        return Ok(Paired::NoMatch);
    };

    let (raw, primary) = if class == FileClass::Raw {
        (id, partner)
    } else {
        (partner, id)
    };
    sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
        .bind(raw)
        .bind(primary)
        .execute(&mut *tx)
        .await?;
    // The visible photo carries the favorite flag: a RAW that was favorited on its own hands it over.
    sqlx::query("UPDATE assets SET favorite = favorite OR (SELECT favorite FROM assets WHERE id = $1) WHERE id = $2")
        .bind(raw)
        .bind(primary)
        .execute(&mut *tx)
        .await?;
    // The stars are the visible photo's too: a RAW that was rated (by the owner, or by its own metadata) hands them over when
    // they are higher than the photo's, and never over a rating the owner chose on the visible photo itself.
    sqlx::query(
        "UPDATE assets p SET rating = r.rating, rating_source = r.rating_source
         FROM assets r
         WHERE r.id = $1 AND p.id = $2 AND r.rating > p.rating AND p.rating_source IS DISTINCT FROM 'user'",
    )
    .bind(raw)
    .bind(primary)
    .execute(&mut *tx)
    .await?;
    // Hidden and archived are the pair's, carried by the visible photo: a RAW that was hidden or archived on its own hides or
    // archives the pair (never the other way round: a hidden photo does not become visible because a file joined it), and the
    // RAW takes the primary's state so that separating them later keeps it.
    sqlx::query(
        "UPDATE assets SET hidden = hidden OR (SELECT hidden FROM assets WHERE id = $1),
                           archived = archived OR (SELECT archived FROM assets WHERE id = $1) WHERE id = $2",
    )
    .bind(raw)
    .bind(primary)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE assets SET hidden = (SELECT hidden FROM assets WHERE id = $2),
                           archived = (SELECT archived FROM assets WHERE id = $2) WHERE id = $1",
    )
    .bind(raw)
    .bind(primary)
    .execute(&mut *tx)
    .await?;
    // Tags are the visible photo's too: the RAW's move onto it (a tag the owner set stays the owner's).
    sqlx::query(
        "INSERT INTO asset_tags (asset_id, tag_id, imported, added_at)
         SELECT $2, tag_id, imported, added_at FROM asset_tags WHERE asset_id = $1
         ON CONFLICT (asset_id, tag_id) DO UPDATE SET imported = asset_tags.imported AND EXCLUDED.imported",
    )
    .bind(raw)
    .bind(primary)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM asset_tags WHERE asset_id = $1")
        .bind(raw)
        .execute(&mut *tx)
        .await?;
    // Albums reference the visible photo: move the RAW's memberships and cover choice onto the primary.
    sqlx::query(
        "INSERT INTO album_assets (album_id, asset_id, added_at)
         SELECT album_id, $2, added_at FROM album_assets WHERE asset_id = $1 ON CONFLICT DO NOTHING",
    )
    .bind(raw)
    .bind(primary)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE albums SET cover_asset_id = $2 WHERE cover_asset_id = $1")
        .bind(raw)
        .bind(primary)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM album_assets WHERE asset_id = $1")
        .bind(raw)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Paired::Linked { raw, primary })
}

/// Separate a pair (`id` is either file of it) and remember the choice so the matcher never joins these two again.
/// False when `id` is not an unpurged member of a pair of `owner`.
pub async fn unpair(pool: &PgPool, owner: Uuid, id: Uuid) -> Result<bool> {
    let name: Option<(String,)> =
        sqlx::query_as("SELECT original_name FROM assets WHERE id = $1 AND owner_id = $2 AND purging_at IS NULL")
            .bind(id)
            .bind(owner)
            .fetch_optional(pool)
            .await?;
    let Some((name,)) = name else { return Ok(false) };
    let mut tx = pool.begin().await?;
    lock_stem(&mut tx, owner, &stem(&name)).await?;
    let link: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT r.id, r.primary_id FROM assets r
         WHERE r.owner_id = $2 AND r.primary_id IS NOT NULL AND (r.id = $1 OR r.primary_id = $1)
           AND r.purging_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((raw, primary)) = link else { return Ok(false) };
    sqlx::query("UPDATE assets SET primary_id = NULL WHERE id = $1")
        .bind(raw)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO asset_pair_rejections (raw_id, primary_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(raw)
        .bind(primary)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    // 2024-05-01 12:00:00 UTC
    const T: OffsetDateTime = match OffsetDateTime::from_unix_timestamp(1_714_564_800) {
        Ok(t) => t,
        Err(_) => panic!(),
    };

    fn cand(n: u128, at: Option<OffsetDateTime>, cam: Option<(&str, &str)>) -> Candidate {
        Candidate {
            id: Uuid::from_u128(n),
            taken_at: at,
            camera_make: cam.map(|c| c.0.into()),
            camera_model: cam.map(|c| c.1.into()),
        }
    }

    #[test]
    fn classifies_by_extension_then_content_type() {
        use FileClass::*;
        for (name, ct, want) in [
            ("IMG_1.CR3", "image/x-canon-cr3", Raw),
            ("a.dng", "application/octet-stream", Raw),
            ("IMG_1.JPG", "image/jpeg", Primary),
            ("IMG_1.heic", "application/octet-stream", Primary),
            ("x.AVIF", "", Primary),
            ("noext", "image/jpeg", Primary),
            ("a.png", "image/png", Other),
            ("a.tif", "image/tiff", Other),
            ("clip.mov", "video/quicktime", Other),
        ] {
            assert_eq!(classify(name, ct), want, "{name}");
        }
    }

    #[test]
    fn stems_ignore_case_directories_and_the_last_extension() {
        assert_eq!(stem("IMG_1234.CR3"), "img_1234");
        assert_eq!(stem("img_1234.jpg"), "img_1234");
        assert_eq!(stem("DCIM/100CANON/IMG_1234.JPG"), "img_1234");
        assert_eq!(stem("C:\\pics\\IMG_1234.JPG"), "img_1234");
        assert_eq!(stem("a.b.jpg"), "a.b");
        assert_eq!(stem("noext"), "noext");
        assert_eq!(stem(".jpg"), "");
        // Different shots never share a stem.
        assert_ne!(stem("IMG_1234.JPG"), stem("IMG_1235.JPG"));
        assert_ne!(stem("IMG_1234 (1).JPG"), stem("IMG_1234.JPG"));
    }

    #[test]
    fn pairs_within_one_second_both_ways() {
        let s = cand(1, Some(T), None);
        for secs in [-1, 0, 1] {
            let o = cand(2, Some(T + Duration::seconds(secs)), None);
            assert_eq!(choose(&s, &[o]), Some(Uuid::from_u128(2)), "{secs}");
        }
        for ms in [1001, -1001, 5000] {
            let o = cand(2, Some(T + Duration::milliseconds(ms)), None);
            assert_eq!(choose(&s, &[o]), None, "{ms}");
        }
    }

    #[test]
    fn a_time_mismatch_does_not_fall_back_to_the_camera() {
        let cam = Some(("Canon", "EOS R5"));
        let s = cand(1, Some(T), cam);
        let o = cand(2, Some(T + Duration::seconds(60)), cam);
        assert_eq!(choose(&s, &[o]), None);
    }

    #[test]
    fn missing_time_falls_back_to_equal_camera() {
        let cam = Some(("Canon", "EOS R5"));
        let same = cand(2, Some(T), Some(("CANON", " eos r5 ")));
        assert_eq!(choose(&cand(1, None, cam), std::slice::from_ref(&same)), Some(same.id));
        // Either side may be the one without a time.
        assert_eq!(
            choose(&cand(1, Some(T), cam), &[cand(2, None, cam)]),
            Some(Uuid::from_u128(2))
        );
        assert_eq!(
            choose(&cand(1, None, cam), &[cand(2, None, cam)]),
            Some(Uuid::from_u128(2))
        );
        // Different or incomplete camera info: no pair.
        assert_eq!(
            choose(&cand(1, None, cam), &[cand(2, None, Some(("Canon", "EOS R6")))]),
            None
        );
        assert_eq!(choose(&cand(1, None, cam), &[cand(2, None, Some(("Canon", "")))]), None);
        assert_eq!(choose(&cand(1, None, None), &[cand(2, None, None)]), None);
        assert_eq!(choose(&cand(1, None, cam), &[cand(2, None, None)]), None);
        assert_eq!(choose(&cand(1, Some(T), None), &[cand(2, None, cam)]), None);
    }

    #[test]
    fn the_closest_capture_time_wins_and_ties_are_deterministic() {
        let s = cand(9, Some(T), None);
        let far = cand(3, Some(T + Duration::milliseconds(900)), None);
        let near = cand(5, Some(T + Duration::milliseconds(100)), None);
        assert_eq!(choose(&s, &[far.clone(), near.clone()]), Some(near.id));
        assert_eq!(choose(&s, &[near, far.clone()]).unwrap(), Uuid::from_u128(5));
        let a = cand(7, Some(T + Duration::milliseconds(500)), None);
        let b = cand(4, Some(T - Duration::milliseconds(500)), None);
        assert_eq!(choose(&s, &[a, b]), Some(Uuid::from_u128(4)), "equal gaps: lowest id");
    }

    #[test]
    fn several_camera_only_candidates_are_ambiguous() {
        let cam = Some(("Sony", "A7"));
        let s = cand(1, None, cam);
        assert_eq!(choose(&s, &[cand(2, None, cam), cand(3, None, cam)]), None);
        // A time match still beats the camera-only candidates.
        let timed = cand(4, Some(T), cam);
        assert_eq!(
            choose(&cand(1, Some(T), cam), &[cand(2, None, cam), timed.clone()]),
            Some(timed.id)
        );
    }

    #[test]
    fn nothing_to_pair_with() {
        assert_eq!(choose(&cand(1, Some(T), None), &[]), None);
    }
}
