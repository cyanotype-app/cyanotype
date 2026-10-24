//! Photo edits: a corrected date and time, a corrected or added location, a caption (`docs/library.md`, "Editing a photo").
//!
//! **The original file is never touched.** An edit is two things:
//!
//! * the **bucket's copy**, `edits/{user}/{hash[0:2]}/{hash}.json` ([`keys::edits`]): a small JSON object written before the
//!   database changes, named by the photo's content hash (which survives a rebuilt database, unlike the asset id). It is
//!   what `cyanotype reindex` restores from ([`crate::reindex`]), so edits made after the last database dump survive a
//!   recovery. A photo whose edits were all reverted keeps an empty object (a tombstone), so a later reindex can tell
//!   "reverted" from "never edited" and a restored dump that still has the old edit is corrected.
//! * the **row**: `assets.taken_at` / `latitude` / `longitude` hold the effective values, so the timeline, the map, the date
//!   filters, bursts and pairing need no change; the values the file itself said are kept in `taken_at_exif` /
//!   `latitude_exif` / `longitude_exif` for "Revert to original", and the worker's metadata job writes the file's values
//!   there (not over the edit) while a photo is edited.
//!
//! Side effects of [`apply`]: a new location clears the place and queues the `geocode` job again; a new time regroups the
//! burst the photo was in and the one it is in now.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{bursts, jobs, keys, places, recipe::Recipe, storage::Storage};

/// The layout version of the object in the bucket.
pub const VERSION: u32 = 1;
/// The longest caption, in characters.
pub const MAX_CAPTION_CHARS: usize = 2000;
/// The farthest a time zone offset reaches (UTC+14 is the largest in use; a margin for hand-written values).
const MAX_OFFSET_SECS: i32 = 18 * 3600;

/// The edits of one photo, as stored in the bucket. Absent fields are not edited (they are what the file says).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edits {
    pub v: u32,
    /// When the edits last changed.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// The corrected capture time (an instant).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub taken_at: Option<OffsetDateTime>,
    /// The zone `taken_at` is shown in, seconds east of UTC; absent means "unknown" (the time reads as UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taken_offset: Option<i32>,
    /// The corrected location; both or neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// Rotation, straighten angle and crop (`crate::recipe`). Additive to layout version 1: a server that predates it
    /// ignores the field and shows the photo as the file is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<Recipe>,
}

/// "Now" at the precision Postgres stores (microseconds), so a time written to the row and to the object compares equal.
pub fn now() -> OffsetDateTime {
    let t = OffsetDateTime::now_utc();
    t.replace_nanosecond(t.nanosecond() / 1000 * 1000).unwrap_or(t)
}

impl Edits {
    /// No edits at all, as of `now`.
    pub fn none(now: OffsetDateTime) -> Self {
        Edits {
            v: VERSION,
            updated_at: now,
            taken_at: None,
            taken_offset: None,
            latitude: None,
            longitude: None,
            caption: None,
            recipe: None,
        }
    }

    /// Nothing is edited (every part is what the file says).
    pub fn is_empty(&self) -> bool {
        self.taken_at.is_none() && self.latitude.is_none() && self.caption.is_none() && self.recipe.is_none()
    }

    /// Normalise what a client sent (a caption is trimmed, and an empty one is none) and refuse what makes no sense: a
    /// coordinate out of range or half a location, a time zone beyond +-18 h, a caption over [`MAX_CAPTION_CHARS`], a
    /// capture time before 1900 or after 2200.
    pub fn validated(mut self) -> Result<Self, String> {
        self.caption = self
            .caption
            .take()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty());
        if self
            .caption
            .as_ref()
            .is_some_and(|c| c.chars().count() > MAX_CAPTION_CHARS)
        {
            return Err(format!("a caption is at most {MAX_CAPTION_CHARS} characters"));
        }
        if self.caption.as_ref().is_some_and(|c| c.contains('\0')) {
            return Err("a caption cannot contain NUL characters".into());
        }
        match (self.latitude, self.longitude) {
            (None, None) => {}
            (Some(lat), Some(lon)) if lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0 => {
            }
            _ => {
                return Err("a location needs a latitude within -90 and 90 and a longitude within -180 and 180".into());
            }
        }
        if let Some(t) = self.taken_at
            && !(1900..=2200).contains(&t.year())
        {
            return Err("the date must be between the years 1900 and 2200".into());
        }
        if self.taken_offset.is_some_and(|o| o.abs() > MAX_OFFSET_SECS) {
            return Err("a time zone is within 18 hours of UTC".into());
        }
        if self.taken_at.is_none() {
            self.taken_offset = None;
        }
        self.recipe = match self.recipe {
            None => None,
            Some(r) => r.normalized()?,
        };
        Ok(self)
    }
}

/// The edits a photo has now, with its owner and content hash (the bucket key's parts). None for an unknown photo.
pub async fn current(db: &PgPool, asset_id: Uuid) -> Result<Option<(Uuid, [u8; 32], Edits)>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        owner_id: Uuid,
        sha256: Vec<u8>,
        date_edited: bool,
        taken_at: Option<OffsetDateTime>,
        taken_offset_edit: Option<i32>,
        location_edited: bool,
        latitude: Option<f64>,
        longitude: Option<f64>,
        caption: Option<String>,
        edited_at: Option<OffsetDateTime>,
        recipe: Option<sqlx::types::Json<Recipe>>,
    }
    let Some(r) = sqlx::query_as::<_, Row>(
        "SELECT owner_id, sha256, date_edited, taken_at, taken_offset_edit, location_edited, latitude, longitude, caption,
                edited_at, recipe
         FROM assets WHERE id = $1",
    )
    .bind(asset_id)
    .fetch_optional(db)
    .await?
    else {
        return Ok(None);
    };
    let sha: [u8; 32] = r.sha256.as_slice().try_into().context("sha256 is not 32 bytes")?;
    let mut e = Edits::none(r.edited_at.unwrap_or(OffsetDateTime::UNIX_EPOCH));
    if r.date_edited {
        e.taken_at = r.taken_at;
        e.taken_offset = r.taken_offset_edit;
    }
    if r.location_edited {
        e.latitude = r.latitude;
        e.longitude = r.longitude;
    }
    e.caption = r.caption;
    e.recipe = r.recipe.map(|j| j.0);
    Ok(Some((r.owner_id, sha, e)))
}

/// Write a photo's edits to the bucket (overwriting the previous ones; the bucket is versioned, so earlier ones stay as
/// versions until the photo is purged).
pub async fn put(storage: &Storage, owner: Uuid, sha256: &[u8; 32], edits: &Edits) -> Result<()> {
    let body = serde_json::to_vec(edits).context("serialise edits")?;
    storage
        .put_bytes(&keys::edits(owner, sha256), body, "application/json")
        .await
}

/// Read an edits object. Refuses what is not valid JSON of this layout or newer than we understand.
pub fn parse(bytes: &[u8]) -> Result<Edits> {
    let e: Edits = serde_json::from_slice(bytes).context("edits object is not valid")?;
    anyhow::ensure!(e.v >= 1 && e.v <= VERSION, "edits object has layout version {}", e.v);
    e.validated().map_err(anyhow::Error::msg)
}

/// After the metadata job has stored the file's own size: put the edited picture's size back in `width` / `height` for a photo
/// that has a recipe (the file's stays in `width_orig` / `height_orig`). Nothing for a photo without one.
pub async fn refresh_dims(db: &PgPool, asset_id: Uuid) -> Result<()> {
    let row: Option<(sqlx::types::Json<Recipe>, Option<i32>, Option<i32>)> =
        sqlx::query_as("SELECT recipe, width_orig, height_orig FROM assets WHERE id = $1 AND recipe IS NOT NULL")
            .bind(asset_id)
            .fetch_optional(db)
            .await?;
    let Some((recipe, Some(w), Some(h))) = row else {
        return Ok(());
    };
    if w <= 0 || h <= 0 {
        return Ok(());
    }
    let (ow, oh) = recipe.0.output_dims(w as u32, h as u32);
    sqlx::query("UPDATE assets SET width = $2, height = $3 WHERE id = $1 AND recipe IS NOT NULL")
        .bind(asset_id)
        .bind(ow as i32)
        .bind(oh as i32)
        .execute(db)
        .await?;
    Ok(())
}

/// What [`apply`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    /// There is such a photo of this owner, not queued for deletion.
    pub found: bool,
    pub date_changed: bool,
    pub location_changed: bool,
    pub caption_changed: bool,
    /// The recipe changed: the thumbnail and preview are rendered again and the embedding with them.
    pub recipe_changed: bool,
}

/// Make the row say what `edits` says: edited parts are written over the effective columns (the file's own values are kept
/// the first time), parts that are not edited are put back to the file's values. Idempotent. Returns what changed.
pub async fn apply(db: &PgPool, owner: Uuid, asset_id: Uuid, edits: &Edits) -> Result<Applied> {
    #[derive(sqlx::FromRow)]
    struct Row {
        date_edited: bool,
        taken_at: Option<OffsetDateTime>,
        taken_at_exif: Option<OffsetDateTime>,
        location_edited: bool,
        latitude: Option<f64>,
        longitude: Option<f64>,
        latitude_exif: Option<f64>,
        longitude_exif: Option<f64>,
        caption: Option<String>,
        burst_id: Option<Uuid>,
        recipe: Option<sqlx::types::Json<Recipe>>,
        width: Option<i32>,
        height: Option<i32>,
        width_orig: Option<i32>,
        height_orig: Option<i32>,
    }
    let mut tx = db.begin().await?;
    let Some(r) = sqlx::query_as::<_, Row>(
        "SELECT date_edited, taken_at, taken_at_exif, location_edited, latitude, longitude, latitude_exif, longitude_exif,
                caption, burst_id, recipe, width, height, width_orig, height_orig
         FROM assets WHERE id = $1 AND owner_id = $2 AND purging_at IS NULL FOR UPDATE",
    )
    .bind(asset_id)
    .bind(owner)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(Applied::default());
    };

    // Date: an edit keeps the file's value the first time; reverting brings it back and forgets the edit.
    let (date_edited, taken_at, taken_at_exif, offset) = match edits.taken_at {
        Some(t) => (
            true,
            Some(t),
            if r.date_edited { r.taken_at_exif } else { r.taken_at },
            edits.taken_offset,
        ),
        None if r.date_edited => (false, r.taken_at_exif, None, None),
        None => (false, r.taken_at, None, None),
    };
    let (location_edited, lat, lon, lat_exif, lon_exif) = match (edits.latitude, edits.longitude) {
        (Some(la), Some(lo)) => (
            true,
            Some(la),
            Some(lo),
            if r.location_edited { r.latitude_exif } else { r.latitude },
            if r.location_edited {
                r.longitude_exif
            } else {
                r.longitude
            },
        ),
        _ if r.location_edited => (false, r.latitude_exif, r.longitude_exif, None, None),
        _ => (false, r.latitude, r.longitude, None, None),
    };
    let date_changed = taken_at != r.taken_at;
    let location_changed = (lat, lon) != (r.latitude, r.longitude);
    let caption_changed = edits.caption != r.caption;

    // Recipe: the size the grid draws is the edited picture's, the file's own is kept for "Revert to original".
    let old_recipe = r.recipe.map(|j| j.0);
    let recipe_changed = edits.recipe != old_recipe;
    let (file_w, file_h) = if old_recipe.is_some() {
        (r.width_orig, r.height_orig)
    } else {
        (r.width, r.height)
    };
    let (width, height, width_orig, height_orig) = match (edits.recipe, file_w, file_h) {
        (Some(rc), Some(w), Some(h)) if w > 0 && h > 0 => {
            let (ow, oh) = rc.output_dims(w as u32, h as u32);
            (Some(ow as i32), Some(oh as i32), Some(w), Some(h))
        }
        (Some(_), w, h) => (w, h, w, h),
        (None, w, h) => (w, h, None, None),
    };

    sqlx::query(
        "UPDATE assets SET date_edited = $2, taken_at = $3, taken_at_exif = $4, taken_offset_edit = $5,
                location_edited = $6, latitude = $7, longitude = $8, latitude_exif = $9, longitude_exif = $10,
                caption = $11, edited_at = $12,
                -- A new place is looked up again: the old one is wrong until then.
                place_city = CASE WHEN $13 THEN NULL ELSE place_city END,
                place_region = CASE WHEN $13 THEN NULL ELSE place_region END,
                place_country = CASE WHEN $13 THEN NULL ELSE place_country END,
                place_country_code = CASE WHEN $13 THEN NULL ELSE place_country_code END,
                recipe = $14, width = $15, height = $16, width_orig = $17, height_orig = $18,
                edited_copy_at = CASE WHEN $19 THEN NULL ELSE edited_copy_at END,
                edited_copy_bytes = CASE WHEN $19 THEN NULL ELSE edited_copy_bytes END
         WHERE id = $1",
    )
    .bind(asset_id)
    .bind(date_edited)
    .bind(taken_at)
    .bind(taken_at_exif)
    .bind(offset)
    .bind(location_edited)
    .bind(lat)
    .bind(lon)
    .bind(lat_exif)
    .bind(lon_exif)
    .bind(&edits.caption)
    .bind(edits.updated_at)
    .bind(location_changed)
    .bind(edits.recipe.map(sqlx::types::Json))
    .bind(width)
    .bind(height)
    .bind(width_orig)
    .bind(height_orig)
    .bind(recipe_changed)
    .execute(&mut *tx)
    .await?;
    if location_changed {
        places::requeue(&mut *tx, asset_id).await?;
    }
    if recipe_changed {
        // The pictures the grid, the viewer and the search model see are rendered again from the original.
        jobs::requeue(&mut *tx, jobs::KIND_THUMBNAIL, asset_id, jobs::RERENDER_VERSION).await?;
    }
    tx.commit().await?;
    if date_changed {
        bursts::requeue_after_time_change(db, asset_id, r.burst_id).await?;
    }
    Ok(Applied {
        found: true,
        date_changed,
        location_changed,
        caption_changed,
        recipe_changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn base() -> Edits {
        Edits::none(datetime!(2024-05-05 10:00 UTC))
    }

    #[test]
    fn the_object_roundtrips_and_leaves_out_what_is_not_edited() {
        let mut e = base();
        e.taken_at = Some(datetime!(2023-07-04 11:05:09 UTC));
        e.taken_offset = Some(7200);
        e.latitude = Some(38.7223);
        e.longitude = Some(-9.1393);
        e.caption = Some("Dinner by the river".into());
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(parse(json.as_bytes()).unwrap(), e);
        let empty = serde_json::to_string(&base()).unwrap();
        assert_eq!(empty, r#"{"v":1,"updated_at":"2024-05-05T10:00:00Z"}"#);
        assert!(parse(empty.as_bytes()).unwrap().is_empty());
    }

    #[test]
    fn validation() {
        let mut e = base();
        e.caption = Some("  hello \n".into());
        assert_eq!(e.validated().unwrap().caption.as_deref(), Some("hello"));
        let mut e = base();
        e.caption = Some("   ".into());
        assert_eq!(e.validated().unwrap().caption, None, "an empty caption is no caption");
        let mut e = base();
        e.caption = Some("x".repeat(MAX_CAPTION_CHARS + 1));
        assert!(e.validated().is_err());
        let mut e = base();
        e.caption = Some("ü".repeat(MAX_CAPTION_CHARS));
        assert!(e.validated().is_ok(), "counted in characters, not bytes");
        for (lat, lon) in [
            (Some(91.0), Some(0.0)),
            (Some(0.0), Some(181.0)),
            (Some(1.0), None),
            (None, Some(1.0)),
            (Some(f64::NAN), Some(0.0)),
        ] {
            let mut e = base();
            (e.latitude, e.longitude) = (lat, lon);
            assert!(e.validated().is_err(), "{lat:?} {lon:?}");
        }
        let mut e = base();
        e.taken_at = Some(datetime!(1850-01-01 0:00 UTC));
        assert!(e.validated().is_err());
        let mut e = base();
        e.taken_at = Some(datetime!(2024-01-01 0:00 UTC));
        e.taken_offset = Some(19 * 3600);
        assert!(e.validated().is_err());
        // A zone without a date means nothing.
        let mut e = base();
        e.taken_offset = Some(3600);
        assert_eq!(e.validated().unwrap().taken_offset, None);
    }

    #[test]
    fn a_recipe_travels_with_the_other_edits() {
        use crate::recipe::Crop;
        let mut e = base();
        e.recipe = Some(Recipe {
            rotate: 90,
            straighten: 1.5,
            crop: Some(Crop {
                x: 0.1,
                y: 0.2,
                w: 0.5,
                h: 0.4,
            }),
        });
        assert!(!e.is_empty(), "a recipe is an edit: the object is not a tombstone");
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.contains(r#""recipe":{"rotate":90,"straighten":1.5,"crop":{"x":0.1,"y":0.2,"w":0.5,"h":0.4}}"#),
            "{json}"
        );
        assert_eq!(parse(json.as_bytes()).unwrap(), e);
        // An object written before recipes existed has none; one with a recipe that changes nothing is a tombstone.
        assert!(
            parse(br#"{"v":1,"updated_at":"2024-05-05T10:00:00Z","caption":"x"}"#)
                .unwrap()
                .recipe
                .is_none()
        );
        let none = parse(br#"{"v":1,"updated_at":"2024-05-05T10:00:00Z","recipe":{"rotate":360}}"#).unwrap();
        assert!(none.recipe.is_none() && none.is_empty());
        // A recipe the layout allows but nobody can render is refused like any other bad object.
        assert!(parse(br#"{"v":1,"updated_at":"2024-05-05T10:00:00Z","recipe":{"rotate":45}}"#).is_err());
        assert!(
            parse(br#"{"v":1,"updated_at":"2024-05-05T10:00:00Z","recipe":{"crop":{"x":0,"y":0,"w":2,"h":1}}}"#)
                .is_err()
        );
    }

    #[test]
    fn a_newer_layout_or_garbage_is_refused() {
        assert!(parse(b"{}").is_err());
        assert!(parse(br#"{"v":2,"updated_at":"2024-05-05T10:00:00Z"}"#).is_err());
        assert!(parse(br#"{"v":1,"updated_at":"yesterday"}"#).is_err());
        assert!(parse(br#"{"v":1,"updated_at":"2024-05-05T10:00:00Z","latitude":95,"longitude":0}"#).is_err());
    }
}
