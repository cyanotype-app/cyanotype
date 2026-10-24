//! Google Takeout metadata, held until the photo it belongs to is processed and then applied (`docs/import.md`, "Moving from
//! Google Photos").
//!
//! The client (browser or `cyanotype import --takeout`) sends the sidecar text and the album folder with the upload's
//! check call. [`record`] keeps it in `takeout_imports`, keyed by the photo's content hash. The worker's `takeout` job
//! ([`apply`]) runs once the photo's metadata job has read the file's own EXIF, and applies:
//!
//! * **capture time**, as an *edit* (`edits`): so it is revertible, the file's own value stays beside it, and it is written to
//!   the bucket where `reindex` finds it. Not when the file's own time and Takeout's differ only by a time zone (the file has
//!   a local time without a zone, Takeout the true instant: replacing it would make the displayed time wrong by hours), and
//!   never over an edit the owner made themselves.
//! * **location**, likewise: when the file has none, or Takeout's differs by more than about 100 m (the owner placed the photo
//!   in Google Photos).
//! * **caption** from `description`, when the photo has none.
//! * **favourite**, only ever turned on.
//! * **archive**: a photo Google Photos had archived is archived here (off the timeline, in albums and search), with its paired RAW,
//!   as the Archive action does. Only ever turned on.
//! * **album**: the named folder's album is found by title (or made) and the photo added.
//!
//! Everything is idempotent: running it again, or re-importing the same export, changes nothing.
use anyhow::{Context, Result};
use cyanotype_takeout::Meta;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{edits, jobs, storage::Storage};

/// Unapplied rows older than this are dropped (the upload never happened).
pub const STALE_DAYS: i32 = 30;
/// The longest album title (the library's own limit).
pub const MAX_ALBUM_CHARS: usize = 200;
/// A location differing by less than this many degrees (about 100 m) is the same place.
const SAME_PLACE_DEGREES: f64 = 0.001;
/// Capture times closer than this are the same time.
const SAME_TIME_SECS: i64 = 60;

/// What a client sends with a check call (`takeout` field).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Request {
    /// The raw text of the photo's sidecar.
    #[serde(default)]
    pub sidecar: Option<String>,
    /// The album folder the photo was in; absent when the client was told not to create albums.
    #[serde(default)]
    pub album: Option<String>,
    /// Import the favourite flag (default yes).
    #[serde(default = "yes")]
    pub favorites: bool,
}

fn yes() -> bool {
    true
}

/// What is held for a photo.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Payload {
    #[serde(default)]
    pub meta: Meta,
    /// Every album folder this photo was in (Takeout repeats a photo in each album and in its year folder).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub albums: Vec<String>,
}

impl Payload {
    pub fn is_empty(&self) -> bool {
        self.meta == Meta::default() && self.albums.is_empty()
    }

    /// Fold in what another copy of the same photo said: its albums are added, the first readable sidecar stays (copies carry
    /// the same one), and a favourite in any copy counts.
    pub fn merge(&mut self, other: &Payload) {
        for a in &other.albums {
            if !self.albums.contains(a) {
                self.albums.push(a.clone());
            }
        }
        self.albums.sort();
        let fav = self.meta.favorited || other.meta.favorited;
        if self.meta == Meta::default() {
            self.meta = other.meta.clone();
        }
        self.meta.favorited = fav;
    }
}

/// Turn what a client sent into what is held; None when there is nothing to apply (no readable sidecar, no album).
pub fn payload_of(req: &Request) -> Option<Payload> {
    let mut meta = req
        .sidecar
        .as_deref()
        .and_then(cyanotype_takeout::parse_sidecar)
        .unwrap_or_default();
    if !req.favorites {
        meta.favorited = false;
    }
    // A photo the owner put in Google Photos' trash is not made visible by importing it: leave its flags out.
    let album = req
        .album
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty() && !a.contains('\0'))
        .map(|a| a.chars().take(MAX_ALBUM_CHARS).collect::<String>());
    let p = Payload {
        meta,
        albums: album.into_iter().collect(),
    };
    (!p.is_empty()).then_some(p)
}

/// Hold `payload` for the photo with this hash. Returns whether it is new or changed (a new revision to apply); an identical
/// payload that was already applied changes nothing.
pub async fn record(db: &PgPool, owner: Uuid, sha256: &[u8], payload: &Payload) -> Result<bool> {
    let mut tx = db.begin().await?;
    // Two copies of one photo (a year folder and an album) arrive as two calls, perhaps at once: the row lock makes the second
    // merge into the first instead of replacing it.
    sqlx::query(
        "INSERT INTO takeout_imports (owner_id, sha256, payload, rev) VALUES ($1, $2, '{}', 0) ON CONFLICT DO NOTHING",
    )
    .bind(owner)
    .bind(sha256)
    .execute(&mut *tx)
    .await?;
    let (held, rev): (serde_json::Value, i32) =
        sqlx::query_as("SELECT payload, rev FROM takeout_imports WHERE owner_id = $1 AND sha256 = $2 FOR UPDATE")
            .bind(owner)
            .bind(sha256)
            .fetch_one(&mut *tx)
            .await?;
    let mut merged: Payload = if rev == 0 {
        Payload::default()
    } else {
        serde_json::from_value(held).context("held takeout payload")?
    };
    let before = merged.clone();
    merged.merge(payload);
    let changed = rev == 0 || merged != before;
    if changed {
        sqlx::query(
            "UPDATE takeout_imports SET payload = $3, rev = rev + 1, updated_at = now() WHERE owner_id = $1 AND sha256 = $2",
        )
        .bind(owner)
        .bind(sha256)
        .bind(serde_json::to_value(&merged)?)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await.context("hold takeout metadata")?;
    Ok(changed)
}

/// Queue the `takeout` job for an asset whose held metadata is not applied yet (nothing if there is none, or it is done).
/// Safe to call any number of times: a job is queued per revision.
pub async fn enqueue_for_asset(db: &PgPool, asset_id: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version, priority)
         SELECT $2, a.id, p.rev, 0
         FROM assets a JOIN takeout_imports p ON p.owner_id = a.owner_id AND p.sha256 = a.sha256
         WHERE a.id = $1 AND a.purging_at IS NULL AND p.applied_rev < p.rev
         ON CONFLICT DO NOTHING",
    )
    .bind(asset_id)
    .bind(jobs::KIND_TAKEOUT)
    .execute(db)
    .await?;
    Ok(())
}

/// The worker's sweep: queue the job for every held payload whose photo exists and is not waiting on its metadata job (which
/// queues the takeout job itself when it finishes), including photos whose metadata job failed for good. Returns how many.
pub async fn enqueue_ready(db: &PgPool) -> Result<u64> {
    let n = sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version, priority)
         SELECT $1, a.id, p.rev, 0
         FROM takeout_imports p JOIN assets a ON a.owner_id = p.owner_id AND a.sha256 = p.sha256
         WHERE p.applied_rev < p.rev AND a.purging_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.asset_id = a.id AND j.kind = 'metadata'
                           AND j.state IN ('queued', 'running'))
         ORDER BY p.updated_at
         LIMIT 500
         ON CONFLICT DO NOTHING",
    )
    .bind(jobs::KIND_TAKEOUT)
    .execute(db)
    .await?
    .rows_affected();
    Ok(n)
}

/// Delete what nothing applied for [`STALE_DAYS`] days (the photo never arrived).
pub async fn purge_stale(db: &PgPool) -> Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM takeout_imports WHERE applied_rev < rev AND updated_at < now() - make_interval(days => $1)",
    )
    .bind(STALE_DAYS)
    .execute(db)
    .await?
    .rows_affected())
}

/// Whether Takeout's capture time should replace the file's: yes when the file has none or the two really differ; no when
/// they are the same moment, or differ by what a time zone explains (a multiple of a quarter hour, up to 14 h).
pub fn should_set_time(file: Option<OffsetDateTime>, takeout: OffsetDateTime) -> bool {
    let Some(file) = file else { return true };
    let diff = (takeout - file).whole_seconds().abs();
    if diff <= SAME_TIME_SECS {
        return false;
    }
    let off_quarter = diff % 900;
    let zone_sized =
        diff <= 14 * 3600 + SAME_TIME_SECS && (off_quarter <= SAME_TIME_SECS || 900 - off_quarter <= SAME_TIME_SECS);
    !zone_sized
}

/// Whether Takeout's location should replace the file's: yes when the file has none or it is more than about 100 m away.
pub fn should_set_location(file: Option<(f64, f64)>, takeout: (f64, f64)) -> bool {
    match file {
        None => true,
        Some((la, lo)) => (la - takeout.0).abs() > SAME_PLACE_DEGREES || (lo - takeout.1).abs() > SAME_PLACE_DEGREES,
    }
}

/// What [`apply`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    /// There was held metadata for this photo that had not been applied.
    pub found: bool,
    pub date: bool,
    pub location: bool,
    pub caption: bool,
    pub favorite: bool,
    pub archive: bool,
    pub album: bool,
}

#[derive(sqlx::FromRow)]
struct Row {
    taken_at: Option<OffsetDateTime>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    target: Uuid,
    payload: serde_json::Value,
    rev: i32,
}

/// Apply the held metadata of `asset_id` (see the module docs), then mark it applied. Edits go to the bucket first, then the
/// row (`edits::put`, `edits::apply`), the order every edit uses.
pub async fn apply(db: &PgPool, storage: &Storage, asset_id: Uuid) -> Result<Applied> {
    let Some(r) = sqlx::query_as::<_, Row>(
        "SELECT a.taken_at, a.latitude, a.longitude, COALESCE(a.primary_id, a.id) AS target, p.payload, p.rev
         FROM assets a JOIN takeout_imports p ON p.owner_id = a.owner_id AND p.sha256 = a.sha256
         WHERE a.id = $1 AND a.purging_at IS NULL AND p.applied_rev < p.rev",
    )
    .bind(asset_id)
    .fetch_optional(db)
    .await?
    else {
        return Ok(Applied::default());
    };
    let payload: Payload = serde_json::from_value(r.payload).context("held takeout payload")?;
    let mut done = Applied {
        found: true,
        ..Applied::default()
    };
    let Some((owner, sha, current)) = edits::current(db, asset_id).await? else {
        return Ok(Applied::default());
    };
    let meta = &payload.meta;

    // Time, location and caption: edits, and never over one the owner made.
    let mut next = current.clone();
    if current.taken_at.is_none()
        && let Some(t) = meta.taken_at.and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok())
        && should_set_time(r.taken_at, t)
    {
        next.taken_at = Some(t);
        next.taken_offset = None;
        done.date = true;
    }
    if current.latitude.is_none()
        && let (Some(la), Some(lo)) = (meta.latitude, meta.longitude)
        && should_set_location(r.latitude.zip(r.longitude), (la, lo))
    {
        next.latitude = Some(la);
        next.longitude = Some(lo);
        done.location = true;
    }
    if current.caption.is_none()
        && let Some(c) = &meta.description
    {
        next.caption = Some(c.clone());
        done.caption = true;
    }
    if done.date || done.location || done.caption {
        next.updated_at = edits::now();
        // A value Takeout sent that the library refuses (a date before 1900) is dropped, not an error: the rest still applies.
        let next = match next.clone().validated() {
            Ok(n) => n,
            Err(_) => {
                let mut n = current.clone();
                n.updated_at = next.updated_at;
                (done.date, done.location, done.caption) = (false, false, false);
                n
            }
        };
        if done.date || done.location || done.caption {
            edits::put(storage, owner, &sha, &next).await.context("write edits")?;
            edits::apply(db, owner, asset_id, &next).await.context("apply edits")?;
        }
    }

    // A favourite is only ever turned on, on the visible half of a RAW + JPEG pair.
    if meta.favorited {
        done.favorite = sqlx::query("UPDATE assets SET favorite = true WHERE id = $1 AND NOT favorite")
            .bind(r.target)
            .execute(db)
            .await?
            .rows_affected()
            > 0;
    }

    if meta.archived {
        // As the Archive action does: the visible photo and the RAW paired behind it, stamped so it can be undone as a batch.
        done.archive = sqlx::query(
            "UPDATE assets SET archived = true, archived_at = now()
             WHERE owner_id = $1 AND deleted_at IS NULL AND NOT archived AND (id = $2 OR primary_id = $2)",
        )
        .bind(owner)
        .bind(r.target)
        .execute(db)
        .await?
        .rows_affected()
            > 0;
    }

    for title in &payload.albums {
        done.album |= add_to_album(db, owner, r.target, title).await?;
    }

    sqlx::query("UPDATE takeout_imports SET applied_rev = $3, applied_at = now() WHERE owner_id = $1 AND sha256 = $2")
        .bind(owner)
        .bind(&sha[..])
        .bind(r.rev)
        .execute(db)
        .await?;
    Ok(done)
}

/// The album of this owner called `title` (made when there is none; two jobs at once make one), and the photo in it. True when
/// the photo was added now.
async fn add_to_album(db: &PgPool, owner: Uuid, asset: Uuid, title: &str) -> Result<bool> {
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("takeout-album:{owner}:{title}"))
        .execute(&mut *tx)
        .await?;
    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM albums WHERE owner_id = $1 AND title = $2 ORDER BY created_at, id LIMIT 1")
            .bind(owner)
            .bind(title)
            .fetch_optional(&mut *tx)
            .await?;
    let album = match existing {
        Some(a) => a,
        None => {
            sqlx::query_scalar("INSERT INTO albums (owner_id, title) VALUES ($1, $2) RETURNING id")
                .bind(owner)
                .bind(title)
                .fetch_one(&mut *tx)
                .await?
        }
    };
    let added = sqlx::query("INSERT INTO album_assets (album_id, asset_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(album)
        .bind(asset)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    if added {
        sqlx::query("UPDATE albums SET updated_at = now() WHERE id = $1")
            .bind(album)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn payload_needs_something_to_apply() {
        assert!(payload_of(&Request::default()).is_none());
        let only_album = payload_of(&Request {
            album: Some("  Trip ".into()),
            ..Request::default()
        })
        .unwrap();
        assert_eq!(only_album.albums, vec!["Trip".to_string()]);
        assert!(
            payload_of(&Request {
                sidecar: Some("not json".into()),
                ..Request::default()
            })
            .is_none()
        );
    }

    #[test]
    fn copies_of_one_photo_add_their_albums_and_keep_the_first_sidecar() {
        let mut a = payload_of(&Request {
            sidecar: Some(r#"{"description":"first"}"#.into()),
            album: None,
            favorites: true,
        })
        .unwrap();
        let b = payload_of(&Request {
            sidecar: Some(r#"{"description":"second","favorited":true}"#.into()),
            album: Some("Trip".into()),
            favorites: true,
        })
        .unwrap();
        let c = payload_of(&Request {
            album: Some("Beach".into()),
            ..Request::default()
        })
        .unwrap();
        a.merge(&b);
        a.merge(&c);
        a.merge(&b);
        assert_eq!(a.albums, vec!["Beach".to_string(), "Trip".to_string()]);
        assert_eq!(a.meta.description.as_deref(), Some("first"));
        assert!(a.meta.favorited, "a favourite in any copy counts");
    }

    #[test]
    fn the_favorites_option_leaves_the_flag_out() {
        let side = r#"{"favorited":true,"description":"hi"}"#.to_string();
        let on = payload_of(&Request {
            sidecar: Some(side.clone()),
            album: None,
            favorites: true,
        })
        .unwrap();
        let off = payload_of(&Request {
            sidecar: Some(side),
            album: None,
            favorites: false,
        })
        .unwrap();
        assert!(on.meta.favorited && !off.meta.favorited);
        assert_eq!(off.meta.description.as_deref(), Some("hi"));
    }

    #[test]
    fn time_is_replaced_only_when_it_really_differs() {
        let file = datetime!(2019-06-30 12:00 UTC);
        let at = |secs: i64| file + time::Duration::seconds(secs);
        assert!(should_set_time(None, file), "a file without a time takes Takeout's");
        assert!(!should_set_time(Some(file), at(0)));
        assert!(!should_set_time(Some(file), at(30)), "seconds apart is the same moment");
        assert!(
            !should_set_time(Some(file), at(2 * 3600)),
            "a whole zone apart is a naive local time, keep the file's"
        );
        assert!(
            !should_set_time(Some(file), at(-5 * 3600 - 1800)),
            "half-hour zones too"
        );
        assert!(
            !should_set_time(Some(file), at(5 * 3600 + 45 * 60 + 20)),
            "and Nepal's +5:45"
        );
        assert!(
            should_set_time(Some(file), at(3600 + 7 * 60)),
            "an hour and seven minutes is a correction"
        );
        assert!(
            should_set_time(Some(file), at(3 * 86_400)),
            "days apart is a correction"
        );
        assert!(should_set_time(Some(file), at(15 * 3600)), "beyond any zone");
    }

    #[test]
    fn location_is_replaced_when_missing_or_far() {
        assert!(should_set_location(None, (38.7, -9.1)));
        assert!(!should_set_location(Some((38.7001, -9.1001)), (38.7, -9.1)));
        assert!(should_set_location(Some((38.7, -9.1)), (38.8, -9.1)));
        assert!(should_set_location(Some((38.7, -9.1)), (38.7, -9.0)));
    }
}
