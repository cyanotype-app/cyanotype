//! Editing photos: `POST /assets/edits` (date, location, caption for one or many photos) and `GET /assets/places`.
//! See `cyanotype_core::edits` for what an edit is and where it is kept (docs/library.md, "Editing a photo").
//!
//! ```json
//! {"ids": ["<id>"],                    // or {"scope": ..., "except": [...]} like the other bulk endpoints
//!  "date":     {"local": "2023-07-04T13:05:09", "offset": "+02:00"}   // set (offset null/absent = zone unknown)
//!            | {"shift_seconds": -3600}                              // move every photo by this much
//!            | {"revert": true},                                     // back to what the file says
//!  "location": {"latitude": 38.72, "longitude": -9.14} | {"revert": true},
//!  "caption":  {"text": "Dinner by the river"} | {"revert": true}}
//! ```
//!
//! Only the caller's own photos that are not in the trash (and not the hidden half of a RAW + JPEG pair) are edited; any
//! other id is counted as `missing`. Each photo's edits are written to the bucket first and then to its row, so a failure
//! leaves the bucket (the source of truth) at least as new as the database; `failed` lists the ids that could not be
//! written, so a retry can be limited to them (a shift would otherwise move the others twice). The photo's original is
//! never touched.
use axum::{
    Json,
    extract::{Query, State},
};
use cyanotype_core::edits::{self, Edits};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};
use tokio::task::JoinSet;
use uuid::Uuid;

use super::{scope::Selector, trash::MAX_IDS};
use crate::{AppState, details};
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

/// Edits written at once (each is one object write and one transaction).
const CONCURRENCY: usize = 8;

#[derive(Deserialize, Debug, PartialEq)]
#[serde(untagged)]
pub(super) enum DateEdit {
    Revert { revert: bool },
    Shift { shift_seconds: i64 },
    Set { local: String, offset: Option<String> },
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(untagged)]
pub(super) enum LocationEdit {
    Revert { revert: bool },
    Set { latitude: f64, longitude: f64 },
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(untagged)]
pub(super) enum CaptionEdit {
    Revert { revert: bool },
    Set { text: String },
}

#[derive(Deserialize)]
pub(super) struct EditRequest {
    #[serde(flatten)]
    pick: Selector,
    date: Option<DateEdit>,
    location: Option<LocationEdit>,
    caption: Option<CaptionEdit>,
}

#[derive(Serialize, Debug, Default, PartialEq, Eq)]
pub(super) struct EditResult {
    /// Photos whose edits changed.
    updated: u64,
    /// Photos that already were as asked.
    unchanged: u64,
    /// Ids that are not an editable photo of the caller's.
    missing: u64,
    /// Ids that could not be written (object storage or database error): retry these.
    failed: Vec<Uuid>,
}

/// `YYYY-MM-DDTHH:MM[:SS]` and an optional `+HH:MM` to the instant and the zone in seconds east of UTC.
fn parse_local(local: &str, offset: Option<&str>) -> Result<(OffsetDateTime, Option<i32>), String> {
    let local = local.trim();
    let fmt = time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
    let short = time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]");
    let wall = PrimitiveDateTime::parse(local, &fmt)
        .or_else(|_| PrimitiveDateTime::parse(local, &short))
        .map_err(|_| "the date must look like 2023-07-04T13:05".to_string())?;
    let off = match offset.map(str::trim).filter(|o| !o.is_empty()) {
        None => None,
        Some(o) => {
            let sign = match o.as_bytes().first() {
                Some(b'+') => 1,
                Some(b'-') => -1,
                _ => return Err("the time zone must look like +02:00".into()),
            };
            let (h, m) = o[1..].split_once(':').ok_or("the time zone must look like +02:00")?;
            let (h, m): (i32, i32) = (
                h.parse().map_err(|_| "the time zone must look like +02:00")?,
                m.parse().map_err(|_| "the time zone must look like +02:00")?,
            );
            if !(0..=18).contains(&h) || !(0..60).contains(&m) {
                return Err("a time zone is within 18 hours of UTC".into());
            }
            Some(sign * (h * 3600 + m * 60))
        }
    };
    let zone = UtcOffset::from_whole_seconds(off.unwrap_or(0)).map_err(|_| "a time zone is within 18 hours of UTC")?;
    Ok((wall.assume_offset(zone), off))
}

/// What one photo is now: the editable row, with the effective time and zone a shift starts from.
#[derive(FromRow)]
struct Target {
    sha256: Vec<u8>,
    sort_time: OffsetDateTime,
    taken_at: Option<OffsetDateTime>,
    date_edited: bool,
    taken_offset_edit: Option<i32>,
    exif: Option<serde_json::Value>,
}

enum Outcome {
    Updated,
    Unchanged,
    Missing,
    Failed(Uuid),
}

async fn edit_one(s: AppState, owner: Uuid, id: Uuid, req: std::sync::Arc<Plan>) -> Outcome {
    match try_edit_one(&s, owner, id, &req).await {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(asset_id = %id, error = ?e, "could not edit a photo");
            Outcome::Failed(id)
        }
    }
}

async fn try_edit_one(s: &AppState, owner: Uuid, id: Uuid, plan: &Plan) -> anyhow::Result<Outcome> {
    let t: Option<Target> = sqlx::query_as(
        "SELECT sha256, coalesce(taken_at, file_modified_at, created_at) AS sort_time, taken_at, date_edited,
                taken_offset_edit, exif
         FROM assets WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL AND primary_id IS NULL AND purging_at IS NULL",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&s.db)
    .await?;
    let Some(t) = t else { return Ok(Outcome::Missing) };
    let sha: [u8; 32] = t.sha256.as_slice().try_into()?;
    let Some((_, _, mut e)) = edits::current(&s.db, id).await? else {
        return Ok(Outcome::Missing);
    };
    let before = e.clone();

    match &plan.date {
        None => {}
        Some(PlanDate::Revert) => {
            e.taken_at = None;
            e.taken_offset = None;
        }
        Some(PlanDate::Set(at, off)) => {
            e.taken_at = Some(*at);
            e.taken_offset = *off;
        }
        Some(PlanDate::Shift(secs)) => {
            let zone = if t.date_edited {
                t.taken_offset_edit
            } else {
                details::recorded_offset(t.exif.as_ref(), t.taken_at).map(UtcOffset::whole_seconds)
            };
            e.taken_at = Some(
                t.sort_time
                    .checked_add(time::Duration::seconds(*secs))
                    .ok_or_else(|| anyhow::anyhow!("the shifted date is out of range"))?,
            );
            e.taken_offset = zone;
        }
    }
    match &plan.location {
        None => {}
        Some(None) => (e.latitude, e.longitude) = (None, None),
        Some(Some((la, lo))) => (e.latitude, e.longitude) = (Some(*la), Some(*lo)),
    }
    match &plan.caption {
        None => {}
        Some(c) => e.caption.clone_from(c),
    }
    let e = e.validated().map_err(anyhow::Error::msg)?;
    // Same content as now: nothing to write (and no new `updated_at` to disagree about).
    if (e.taken_at, e.taken_offset, e.latitude, e.longitude, &e.caption)
        == (
            before.taken_at,
            before.taken_offset,
            before.latitude,
            before.longitude,
            &before.caption,
        )
    {
        return Ok(Outcome::Unchanged);
    }
    let e = Edits {
        updated_at: edits::now(),
        ..e
    };
    // The bucket first: it is the source of truth, and a database that fails after this is healed by `reindex`.
    edits::put(&s.storage, owner, &sha, &e).await?;
    let applied = edits::apply(&s.db, owner, id, &e).await?;
    Ok(if applied.found {
        Outcome::Updated
    } else {
        Outcome::Missing
    })
}

/// The request, checked and turned into what to do with each photo.
pub(super) struct Plan {
    date: Option<PlanDate>,
    /// Some(None) reverts, Some(Some(..)) sets.
    location: Option<Option<(f64, f64)>>,
    caption: Option<Option<String>>,
}

enum PlanDate {
    Revert,
    Set(OffsetDateTime, Option<i32>),
    Shift(i64),
}

/// A shift of more than this many seconds (about 300 years) is not a correction.
const MAX_SHIFT_SECS: i64 = 300 * 366 * 86_400;

impl Plan {
    pub(super) fn from(req: &EditRequest) -> ApiResult<Plan> {
        let bad = |m: &str| ApiError::BadRequest(m.to_string());
        let revert_only = |r: &bool| if *r { Ok(()) } else { Err(bad("revert must be true")) };
        let date = match &req.date {
            None => None,
            Some(DateEdit::Revert { revert }) => {
                revert_only(revert)?;
                Some(PlanDate::Revert)
            }
            Some(DateEdit::Shift { shift_seconds }) => {
                if shift_seconds.abs() > MAX_SHIFT_SECS {
                    return Err(bad("that is too large a shift"));
                }
                Some(PlanDate::Shift(*shift_seconds))
            }
            Some(DateEdit::Set { local, offset }) => {
                let (at, off) = parse_local(local, offset.as_deref()).map_err(ApiError::BadRequest)?;
                Some(PlanDate::Set(at, off))
            }
        };
        let location = match &req.location {
            None => None,
            Some(LocationEdit::Revert { revert }) => {
                revert_only(revert)?;
                Some(None)
            }
            Some(LocationEdit::Set { latitude, longitude }) => Some(Some((*latitude, *longitude))),
        };
        let caption = match &req.caption {
            None => None,
            Some(CaptionEdit::Revert { revert }) => {
                revert_only(revert)?;
                Some(None)
            }
            // An empty caption is no caption.
            Some(CaptionEdit::Set { text }) => Some(Some(text.clone())),
        };
        if date.is_none() && location.is_none() && caption.is_none() {
            return Err(bad("nothing to change: send date, location or caption"));
        }
        // What is the same for every photo is checked once, so a bad coordinate or caption is one 400, not a failure per photo.
        let probe = Edits {
            taken_at: match &date {
                Some(PlanDate::Set(at, _)) => Some(*at),
                _ => None,
            },
            latitude: location.flatten().map(|l| l.0),
            longitude: location.flatten().map(|l| l.1),
            caption: caption.clone().flatten(),
            ..Edits::none(OffsetDateTime::UNIX_EPOCH)
        };
        probe.validated().map_err(ApiError::BadRequest)?;
        Ok(Plan {
            date,
            location,
            caption: caption.map(|c| c.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())),
        })
    }
}

pub(super) async fn edit(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Json(req): Json<EditRequest>,
) -> ApiResult<Json<EditResult>> {
    let plan = std::sync::Arc::new(Plan::from(&req)?);
    let pick = req.pick.validated(unlock)?;
    let mut tx = s.db.begin().await?;
    let ids: Vec<Uuid> = pick.resolve(&mut tx, user.id).await?.unwrap_or_default();
    tx.commit().await?;
    if ids.len() > MAX_IDS {
        return Err(ApiError::BadRequest(format!(
            "edit at most {MAX_IDS} photos at a time: narrow the selection"
        )));
    }
    let mut result = EditResult::default();
    for chunk in ids.chunks(CONCURRENCY) {
        let mut set = JoinSet::new();
        for id in chunk {
            set.spawn(edit_one(s.clone(), user.id, *id, plan.clone()));
        }
        while let Some(done) = set.join_next().await {
            match done.map_err(|e| ApiError::Internal(e.into()))? {
                Outcome::Updated => result.updated += 1,
                Outcome::Unchanged => result.unchanged += 1,
                Outcome::Missing => result.missing += 1,
                Outcome::Failed(id) => result.failed.push(id),
            }
        }
    }
    result.failed.sort_unstable();
    Ok(Json(result))
}

#[derive(Deserialize)]
pub(super) struct PlacesQuery {
    q: Option<String>,
}

#[derive(Serialize, FromRow)]
pub(super) struct PlaceHit {
    city: String,
    region: Option<String>,
    country: String,
    country_code: Option<String>,
    /// The mean of its photos' coordinates (a place is a point to put a photo on, not an outline).
    latitude: f64,
    longitude: f64,
    count: i64,
}

/// `GET /assets/places?q=lis`: the places the caller's own photos are in (the offline geocoder's city, region and country),
/// most photographed first, to pick a location for another photo from. At most 20; `q` matches the start of a word of the
/// city, region or country, case-insensitively.
pub(super) async fn places(
    State(s): State<AppState>,
    user: AuthUser,
    Query(q): Query<PlacesQuery>,
) -> ApiResult<Json<Vec<PlaceHit>>> {
    let q = q.q.unwrap_or_default();
    let q = q.trim();
    if q.chars().count() > 100 {
        return Err(ApiError::BadRequest("q is at most 100 characters".into()));
    }
    let escaped: String = q
        .chars()
        .flat_map(|c| match c {
            '%' | '_' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    let (starts, word) = (format!("{escaped}%"), format!("% {escaped}%"));
    let rows: Vec<PlaceHit> = sqlx::query_as(
        "SELECT place_city AS city, place_region AS region, place_country AS country, place_country_code AS country_code,
                avg(latitude) AS latitude, avg(longitude) AS longitude, count(*) AS count
         FROM assets
         WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND NOT hidden AND place_city IS NOT NULL
           AND latitude IS NOT NULL AND longitude IS NOT NULL
           AND ($2 = '' OR place_city ILIKE $3 OR place_city ILIKE $4 OR place_region ILIKE $3 OR place_region ILIKE $4
                OR place_country ILIKE $3 OR place_country ILIKE $4)
         GROUP BY place_city, place_region, place_country, place_country_code
         ORDER BY count(*) DESC, place_city, place_country
         LIMIT 20",
    )
    .bind(user.id)
    .bind(q)
    .bind(starts)
    .bind(word)
    .fetch_all(&s.db)
    .await?;
    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn local_time_with_and_without_a_zone() {
        assert_eq!(
            parse_local("2023-07-04T13:05:09", Some("+02:00")).unwrap(),
            (datetime!(2023-07-04 11:05:09 UTC), Some(7200))
        );
        assert_eq!(
            parse_local("2023-07-04T13:05", Some("-05:30")).unwrap(),
            (datetime!(2023-07-04 18:35 UTC), Some(-19800))
        );
        // No zone: the clock is read as UTC and the zone stays unknown.
        assert_eq!(
            parse_local(" 2023-07-04T13:05:09 ", None).unwrap(),
            (datetime!(2023-07-04 13:05:09 UTC), None)
        );
        assert_eq!(parse_local("2023-07-04T13:05:09", Some("")).unwrap().1, None);
        for bad in ["2023-07-04", "yesterday", "2023-13-04T10:00", "2023-07-04 13:05"] {
            assert!(parse_local(bad, None).is_err(), "{bad}");
        }
        for bad in ["02:00", "+2", "+19:00", "+02:60", "+aa:bb"] {
            assert!(parse_local("2023-07-04T13:05", Some(bad)).is_err(), "{bad}");
        }
    }

    fn request(json: serde_json::Value) -> Result<Plan, ApiError> {
        Plan::from(&serde_json::from_value::<EditRequest>(json).unwrap())
    }

    #[test]
    fn the_request_forms() {
        let id = "00000000-0000-0000-0000-000000000001";
        assert!(matches!(
            request(serde_json::json!({"ids": [id], "date": {"shift_seconds": -3600}}))
                .unwrap()
                .date,
            Some(PlanDate::Shift(-3600))
        ));
        assert!(matches!(
            request(serde_json::json!({"ids": [id], "date": {"local": "2020-01-01T00:00", "offset": null}}))
                .unwrap()
                .date,
            Some(PlanDate::Set(_, None))
        ));
        assert!(matches!(
            request(serde_json::json!({"ids": [id], "date": {"revert": true}}))
                .unwrap()
                .date,
            Some(PlanDate::Revert)
        ));
        let p = request(serde_json::json!({"ids": [id], "location": {"latitude": 1.5, "longitude": 2.5}, "caption": {"text": "  hi "}}))
            .unwrap();
        assert_eq!(p.location, Some(Some((1.5, 2.5))));
        assert_eq!(p.caption, Some(Some("hi".to_string())));
        // An empty caption clears it.
        let p = request(serde_json::json!({"ids": [id], "caption": {"text": "  "}})).unwrap();
        assert_eq!(p.caption, Some(None));
    }

    #[test]
    fn bad_requests() {
        let id = "00000000-0000-0000-0000-000000000001";
        for body in [
            serde_json::json!({"ids": [id]}),
            serde_json::json!({"ids": [id], "date": {"revert": false}}),
            serde_json::json!({"ids": [id], "date": {"shift_seconds": i64::MAX}}),
            serde_json::json!({"ids": [id], "date": {"local": "soon"}}),
            serde_json::json!({"ids": [id], "location": {"latitude": 95.0, "longitude": 0.0}}),
            serde_json::json!({"ids": [id], "caption": {"text": "x".repeat(2001)}}),
        ] {
            assert!(matches!(request(body.clone()), Err(ApiError::BadRequest(_))), "{body}");
        }
    }
}
