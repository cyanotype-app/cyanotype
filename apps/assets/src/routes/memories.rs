//! "On this day": the owner's photos taken on today's month and day in earlier years, grouped by year.
//!
//! - `GET /assets/memories?date=YYYY-MM-DD&all=true`: `date` is the day to look up (the browser sends its own local date,
//!   so the strip follows the user's day, not the server's; UTC today when absent). Without `all` it is the strip: a few
//!   photos per year, and nothing at all (`years: []`) when the user turned the strip off. With `all` it is the page behind the
//!   strip: every photo of every year, whatever the strip setting.
//! - `POST /assets/memories/dismiss {ids, dismissed?}`: "not this one" (`dismissed: false` takes it back). A photo that is part
//!   of a burst takes the rest of its burst with it, so the next shot of the same moment does not come up in its place.
//!
//! **Which date**: the photo's own local date, `taken_at` read in the zone the camera recorded (the EXIF `OffsetTime*` tag) or,
//! for a photo whose date was edited, in the zone the edit chose; with no zone known the clock reads as UTC. Edits are honoured
//! because `taken_at` is the effective value (migration 0015). A photo without a capture time has no date of its own and is
//! never a memory (its file date is not when it was taken).
//!
//! **Which photos**: never hidden, archived or trashed ones, nor ones the user dismissed; a RAW + JPEG pair is one photo; the
//! photos of a burst count once (the best: a favorite, else the first).
//!
//! **How it stays fast**: one index range per earlier year (`assets_timeline_idx`, a window of 14 h before to 36 h after
//! midnight UTC of the day, which holds every local date that can fall on it), then the zone arithmetic on the handful of rows
//! in those windows. 50,000 photos over 20 years read some hundreds of index entries.
use std::collections::{BTreeMap, HashSet};

use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::{Date, OffsetDateTime, Time};
use uuid::Uuid;

use super::{ListItem, ListRow, list_items, visibility};
use crate::{AppState, details};
use cyanotype_service::{ApiError, ApiResult, AuthUser};

/// Earliest year looked at, counted back from this year.
const YEARS_BACK: i32 = 80;
/// What the strip carries per year and in all.
const STRIP_PER_YEAR: usize = 10;
const STRIP_YEARS: usize = 8;
/// A year of the page is cut here (a day with 500 photos in one year is already a lot to look at).
const PAGE_PER_YEAR: usize = 500;

#[derive(Deserialize)]
pub(super) struct MemoriesQuery {
    /// `YYYY-MM-DD`: the day to look up. UTC today when absent.
    date: Option<String>,
    #[serde(default)]
    all: bool,
}

#[derive(Serialize)]
pub(super) struct Year {
    year: i32,
    years_ago: i32,
    /// Photos of that year on this day, bursts collapsed (more than `items` carries on the strip).
    count: usize,
    items: Vec<ListItem>,
}

#[derive(Serialize)]
pub(super) struct Memories {
    /// The strip is switched on in the user's settings.
    enabled: bool,
    /// The day looked up, `MM-DD`.
    day: String,
    years: Vec<Year>,
}

#[derive(FromRow)]
struct Candidate {
    #[sqlx(flatten)]
    row: ListRow,
    burst_id: Option<Uuid>,
    taken_at: Option<OffsetDateTime>,
    file_modified_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    exif: Option<serde_json::Value>,
    date_edited: bool,
    taken_offset_edit: Option<i32>,
}

fn parse_date(s: &str) -> ApiResult<Date> {
    Date::parse(s, &time::macros::format_description!("[year]-[month]-[day]"))
        .ok()
        .filter(|d| (1900..=9998).contains(&d.year()))
        .ok_or_else(|| ApiError::BadRequest("date must be a day like 2026-10-19".into()))
}

/// `[lo, hi)` in UTC that holds every instant whose local date (any zone, UTC-12 to UTC+14) is `day`.
fn window(day: Date) -> (OffsetDateTime, OffsetDateTime) {
    let midnight = day.with_time(Time::MIDNIGHT).assume_utc();
    (
        midnight - time::Duration::hours(14),
        midnight + time::Duration::hours(36),
    )
}

pub(super) async fn list(
    State(s): State<AppState>,
    user: AuthUser,
    Query(q): Query<MemoriesQuery>,
) -> ApiResult<Json<Memories>> {
    let today = match &q.date {
        Some(d) => parse_date(d)?,
        None => OffsetDateTime::now_utc().date(),
    };
    let (month, day) = (today.month(), today.day());
    let day_text = format!("{:02}-{:02}", month as u8, day);
    let enabled = visibility::load_prefs(&s.db, user.id).await?.memories_enabled;
    if !enabled && !q.all {
        return Ok(Json(Memories {
            enabled,
            day: day_text,
            years: Vec::new(),
        }));
    }

    // The same month and day in each earlier year (a 29 February only in leap years).
    let (mut los, mut his) = (Vec::new(), Vec::new());
    for y in (today.year() - YEARS_BACK).max(1900)..today.year() {
        if let Ok(d) = Date::from_calendar_date(y, month, day) {
            let (lo, hi) = window(d);
            los.push(lo);
            his.push(hi);
        }
    }
    let candidates: Vec<Candidate> = sqlx::query_as(
        "SELECT a.id, a.original_name, a.content_type, a.size_bytes, a.width, a.height, a.sort_time, a.favorite, a.rating, a.archived,
                a.hidden, a.edited, a.thumbhash, a.burst_id, a.taken_at, a.file_modified_at, a.created_at, a.exif, a.date_edited,
                a.taken_offset_edit
         FROM unnest($2::timestamptz[], $3::timestamptz[]) AS w(lo, hi)
         CROSS JOIN LATERAL (
             SELECT id, original_name, content_type, size_bytes, width, height,
                    coalesce(taken_at, file_modified_at, created_at) AS sort_time, favorite, rating, archived, hidden, (recipe IS NOT NULL) AS edited, thumbhash,
                    burst_id, taken_at, file_modified_at, created_at, exif, date_edited, taken_offset_edit
             FROM assets
             WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND NOT hidden AND NOT archived
               AND NOT memory_dismissed AND taken_at IS NOT NULL
               AND coalesce(taken_at, file_modified_at, created_at) >= w.lo
               AND coalesce(taken_at, file_modified_at, created_at) < w.hi
         ) a
         ORDER BY a.favorite DESC, a.sort_time, a.id",
    )
    .bind(user.id)
    .bind(&los)
    .bind(&his)
    .fetch_all(&s.db)
    .await?;

    // The photo's own local date decides, not the UTC window it was found in.
    let md = format!("-{day_text}T");
    let mut by_year: BTreeMap<i32, Vec<Candidate>> = BTreeMap::new();
    for c in candidates {
        let info = details::date_info_in(
            c.taken_at,
            c.file_modified_at,
            c.created_at,
            c.exif.as_ref(),
            c.date_edited.then_some(c.taken_offset_edit),
        );
        let Some(year) = info.local.get(0..4).and_then(|y| y.parse::<i32>().ok()) else {
            continue;
        };
        if year < today.year() && info.local.get(4..).is_some_and(|rest| rest.starts_with(&md)) {
            by_year.entry(year).or_default().push(c);
        }
    }

    let cap = if q.all { PAGE_PER_YEAR } else { STRIP_PER_YEAR };
    let mut years = Vec::new();
    for (year, list) in by_year
        .into_iter()
        .rev()
        .take(if q.all { usize::MAX } else { STRIP_YEARS })
    {
        // A burst counts once: the first of its photos in this order (favorites first).
        let mut seen: HashSet<Uuid> = HashSet::new();
        let kept: Vec<ListRow> = list
            .into_iter()
            .filter(|c| c.burst_id.is_none_or(|b| seen.insert(b)))
            .map(|c| c.row)
            .collect();
        let count = kept.len();
        let rows: Vec<ListRow> = kept.into_iter().take(cap).collect();
        years.push(Year {
            year,
            years_ago: today.year() - year,
            count,
            items: list_items(&s, user.id, rows).await?,
        });
    }
    Ok(Json(Memories {
        enabled,
        day: day_text,
        years,
    }))
}

#[derive(Deserialize)]
pub(super) struct DismissRequest {
    ids: Vec<Uuid>,
    #[serde(default = "yes")]
    dismissed: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize)]
pub(super) struct Count {
    count: u64,
}

pub(super) async fn dismiss(
    State(s): State<AppState>,
    user: AuthUser,
    Json(req): Json<DismissRequest>,
) -> ApiResult<Json<Count>> {
    if req.ids.len() > super::trash::MAX_IDS {
        return Err(ApiError::BadRequest(format!(
            "at most {} ids per request",
            super::trash::MAX_IDS
        )));
    }
    let done = sqlx::query(
        "UPDATE assets SET memory_dismissed = $3
         WHERE owner_id = $1 AND primary_id IS NULL AND purging_at IS NULL AND memory_dismissed IS DISTINCT FROM $3
           AND (id = ANY($2) OR burst_id IN (SELECT burst_id FROM assets
                                             WHERE owner_id = $1 AND id = ANY($2) AND burst_id IS NOT NULL))",
    )
    .bind(user.id)
    .bind(&req.ids)
    .bind(req.dismissed)
    .execute(&s.db)
    .await?;
    Ok(Json(Count {
        count: done.rows_affected(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_holds_every_local_date_of_the_day() {
        let (lo, hi) = window(time::macros::date!(2023 - 07 - 04));
        // 04 July 00:00 at UTC+14 is 03 July 10:00 UTC; 04 July 23:59 at UTC-12 is 05 July 11:59 UTC.
        assert!(lo <= time::macros::datetime!(2023-07-03 10:00 UTC));
        assert!(hi > time::macros::datetime!(2023-07-05 11:59 UTC));
    }

    #[test]
    fn dates_are_validated() {
        assert!(parse_date("2026-10-19").is_ok());
        assert!(parse_date("2026-02-30").is_err());
        assert!(parse_date("yesterday").is_err());
        assert!(parse_date("1066-10-14").is_err());
    }
}
