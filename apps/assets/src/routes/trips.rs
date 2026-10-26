//! Trips: "New York · October 2026", the memories behind the home page's "Recent places" row (docs/library.md, "Trips").
//!
//! - `GET /assets/trips?date=YYYY-MM-DD&limit=12&before=YYYY-MM-DD`: the owner's recent trips, newest first. `date` is the
//!   user's own today (like `/assets/memories`; UTC today when absent), `before` pages (trips that ended before that day;
//!   `next_before` is the value for the next page, null on the last). Empty with `enabled: false` when the user turned the
//!   memories off.
//! - The photos of a trip are `GET /assets?from=<from>&to=<to>&memories=true` (the library's own list: same items, cursor and
//!   stack handling), see [`range_sql`].
//!
//! **Computed on read**, like On this day. One aggregate query reads the last [`WINDOW_DAYS`] days of the owner's visible
//! photos as (local day, place) counts (a few thousand rows however many photos there are), then the rest is arithmetic; only
//! the trips returned cost one count and one cover query each, both index ranges over the trip's days.
//!
//! **Home** is the place with the most photo-days in that window; every place within [`HOME_RADIUS_KM`] of its centre is home
//! too (suburbs, the next town). A day is *away* when more of its placed photos are away than at home, *home* otherwise, and
//! neutral when none has a place (photos without GPS say nothing about where the day was spent).
//!
//! **A run** is away days with at most [`GAP_DAYS`] day without any away photos between two of them and no home day in the
//! gap. **A trip** is a *leg* of a run: a run is cut where the dominant country of a day differs from the leg's, or its
//! photos are more than [`LEG_KM`] from the leg's centre so far (a week in Houston, then Mexico City, are two trips; days
//! without placed away photos stay in the leg they are in). A leg under [`MIN_PHOTOS`] photos joins the leg before it (the
//! one after it when first), then neighbours of the same country and within [`LEG_KM`] merge again (a border hop in a
//! stay is not a trip). A leg holding at least [`MIN_PHOTOS`] photos is a trip (every photo of its days: GPS-less ones
//! count, they are the same trip). It is *ongoing* when it reaches yesterday or today: it keeps growing as photos come in.
//!
//! **Title**: the most specific place that holds at least [`DOMINANT`] of the away photos: the city, else the region, else the
//! country with the most (so "Italy" for a tour of three cities). Then the month label.
//!
//! **Which photos**: the timeline's: never hidden, archived, trashed or dismissed ones, a RAW + JPEG pair once, and with the
//! user's stacks setting on, a stack once (its cover). **Which day**: the photo's own local date (the EXIF `OffsetTime*` zone, or
//! the zone of an edit; UTC when unknown), as On this day does. A photo without a capture time is never in a trip.
//!
//! **Cover**: a processed photo of the trip's days, away from home, preferring a favorite, then the best star rating, then the
//! one closest to the middle of the trip.
use std::collections::HashMap;

use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::{Date, Duration, Month, OffsetDateTime, Time};
use uuid::Uuid;

use super::{ListItem, ListRow, list_items, list_select, stacks, visibility};
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser};

/// How far back trips (and the home area) are looked for.
const WINDOW_DAYS: i64 = 730;
/// A place this close to the home centre is home.
const HOME_RADIUS_KM: f64 = 50.0;
/// The home area needs at least this many photo-days; fewer and nothing is known about where home is.
const MIN_HOME_DAYS: usize = 3;
/// Days without away photos that may sit between two away days of the same trip.
const GAP_DAYS: i64 = 1;
/// A trip holds at least this many photos.
const MIN_PHOTOS: i64 = 8;
/// A day this far from the centre of the leg so far (or in another country) starts a new leg of the run.
const LEG_KM: f64 = 400.0;
/// The share of the away photos a place needs to name the trip.
const DOMINANT: f64 = 0.6;
const DEFAULT_LIMIT: usize = 12;
const MAX_LIMIT: usize = 50;

/// The timeline's visibility, as `visibility::view_sql(false, false, false, ..)` writes it, without the dismissed photos.
const VIEW: &str = " AND NOT hidden AND NOT archived AND NOT memory_dismissed";

/// SQL (unaliased `assets`): seconds east of UTC of the zone the photo's clock was in, 0 when unknown. The zone of an edit,
/// else the first `OffsetTime*` tag of the EXIF dump (the same tags `details::recorded_offset` reads, without its check
/// against the stored instant).
fn offset_secs_sql() -> &'static str {
    "(CASE WHEN date_edited THEN coalesce(taken_offset_edit, 0)
           WHEN taken_at IS NULL THEN 0
           ELSE coalesce((SELECT CASE WHEN t.o ~ '^[+-][0-9]{2}:[0-9]{2}$' AND substr(t.o, 2, 2)::int <= 14
                                      THEN (CASE WHEN left(t.o, 1) = '-' THEN -1 ELSE 1 END)
                                           * (substr(t.o, 2, 2)::int * 3600 + substr(t.o, 5, 2)::int * 60) END
                          FROM (SELECT btrim(coalesce(exif->>'OffsetTimeOriginal', exif->>'offset_time_original',
                                                      exif->>'OffsetTimeDigitized', exif->>'offset_time_digitized',
                                                      exif->>'OffsetTime', exif->>'offset_time')) AS o) t), 0) END)"
}

/// SQL (unaliased `assets`): the photo's own local date.
fn local_day_sql() -> String {
    format!(
        "(((coalesce(taken_at, file_modified_at, created_at) AT TIME ZONE 'UTC') + {} * interval '1 second')::date)",
        offset_secs_sql()
    )
}

const DAY: &[time::format_description::BorrowedFormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]");

fn parse_day(name: &str, s: &str) -> ApiResult<Date> {
    Date::parse(s, DAY)
        .ok()
        .filter(|d| (1900..=9998).contains(&d.year()))
        .ok_or_else(|| ApiError::BadRequest(format!("{name} must be a day like 2026-10-19")))
}

fn day_text(d: Date) -> String {
    d.format(DAY).unwrap_or_default()
}

/// The extra WHERE terms (starting with ` AND`) that keep the photos whose local date is within `from..=to` (the list's
/// `from` / `to`). The bounds on the sort expression hold every instant whose local date, in any zone from UTC-12 to UTC+14,
/// falls in the range, so `assets_timeline_idx` serves it; the zone arithmetic then runs on those rows only.
pub(super) fn range_sql(from: Option<&str>, to: Option<&str>) -> ApiResult<String> {
    if from.is_none() && to.is_none() {
        return Ok(String::new());
    }
    let from = parse_day("from", from.unwrap_or("1900-01-01"))?;
    let to = parse_day("to", to.unwrap_or("9998-12-31"))?;
    if to < from {
        return Err(ApiError::BadRequest("to must not be before from".into()));
    }
    let midnight = |d: Date| d.with_time(Time::MIDNIGHT).assume_utc();
    let lo = midnight(from) - Duration::hours(14);
    let hi = midnight(to) + Duration::hours(24 + 12);
    let ts = |t: OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    };
    let expr = visibility::Sort::Taken.expr();
    Ok(format!(
        " AND taken_at IS NOT NULL AND {expr} >= '{}'::timestamptz AND {expr} < '{}'::timestamptz
          AND {} BETWEEN '{}'::date AND '{}'::date",
        ts(lo),
        ts(hi),
        local_day_sql(),
        day_text(from),
        day_text(to)
    ))
}

// ---- detection ---------------------------------------------------------------------------------------------------

/// Photos of one local day at one place (`city` None: no place), as the aggregate query reads them.
#[derive(FromRow, Debug, Clone)]
struct DayPlace {
    day: Date,
    city: Option<String>,
    region: Option<String>,
    country: Option<String>,
    cc: Option<String>,
    n: i64,
    lat: Option<f64>,
    lon: Option<f64>,
}

impl DayPlace {
    fn key(&self) -> Option<String> {
        self.city
            .as_ref()
            .map(|c| place_key(c, self.region.as_deref(), self.cc.as_deref()))
    }
}

/// The identity of a place; the cover query compares the same text in SQL.
fn place_key(city: &str, region: Option<&str>, cc: Option<&str>) -> String {
    format!("{city}|{}|{}", region.unwrap_or(""), cc.unwrap_or(""))
}

fn km_between(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (p1, p2) = (a.0.to_radians(), b.0.to_radians());
    let (dp, dl) = ((b.0 - a.0).to_radians(), (b.1 - a.1).to_radians());
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 6371.0088 * h.sqrt().min(1.0).asin()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Level {
    City,
    Region,
    Country,
}

#[derive(Debug, Clone, Serialize)]
struct TripPlace {
    /// The city, the region or the country, as `level` says.
    name: String,
    level: Level,
    /// Absent below the level (a region-level trip has no city).
    #[serde(skip_serializing_if = "Option::is_none")]
    city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<String>,
    country: String,
    country_code: Option<String>,
}

#[derive(Debug, Clone)]
struct Found {
    from: Date,
    to: Date,
    /// Photos of its days, bursts and stacks counted once per day and place: an estimate; the answer counts exactly.
    #[cfg_attr(not(test), allow(dead_code))]
    photos: i64,
    place: TripPlace,
    ongoing: bool,
    /// Place keys of the home area, to keep the cover away from home.
    home: Vec<String>,
}

#[derive(Default)]
struct PlaceSum {
    city: String,
    region: Option<String>,
    country: String,
    cc: Option<String>,
    days: usize,
    photos: i64,
    lat: f64,
    lon: f64,
    weight: f64,
}

impl PlaceSum {
    fn centre(&self) -> Option<(f64, f64)> {
        (self.weight > 0.0).then(|| (self.lat / self.weight, self.lon / self.weight))
    }
}

/// The keys of the places that make up home: the place with the most photo-days and everything within [`HOME_RADIUS_KM`]
/// of its centre. Empty when too little is known.
fn home_area(sums: &HashMap<String, PlaceSum>) -> Vec<String> {
    let Some((hk, home)) = sums
        .iter()
        .max_by(|a, b| (a.1.days, a.1.photos, a.0).cmp(&(b.1.days, b.1.photos, b.0)))
    else {
        return Vec::new();
    };
    let total_days: usize = sums.values().map(|p| p.days).sum();
    if total_days < MIN_HOME_DAYS {
        return Vec::new();
    }
    let centre = home.centre();
    let mut keys: Vec<String> = sums
        .iter()
        .filter(|(k, p)| {
            *k == hk || matches!((centre, p.centre()), (Some(a), Some(b)) if km_between(a, b) <= HOME_RADIUS_KM)
        })
        .map(|(k, _)| k.clone())
        .collect();
    keys.sort();
    keys
}

/// The title place of a trip from the away photos `(key, count)`.
fn title_place(away: &HashMap<String, i64>, sums: &HashMap<String, PlaceSum>) -> Option<TripPlace> {
    let total: i64 = away.values().sum();
    if total == 0 {
        return None;
    }
    let share = |n: i64| n as f64 / total as f64;
    let mut cities: Vec<(&String, i64)> = away.iter().map(|(k, n)| (k, *n)).collect();
    cities.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let (top_key, top_n) = cities[0];
    let top = &sums[top_key];
    if share(top_n) >= DOMINANT {
        return Some(TripPlace {
            name: top.city.clone(),
            level: Level::City,
            city: Some(top.city.clone()),
            region: top.region.clone(),
            country: top.country.clone(),
            country_code: top.cc.clone(),
        });
    }
    // Region, then country: group the same counts.
    let mut regions: HashMap<(String, Option<String>), (i64, &PlaceSum)> = HashMap::new();
    let mut countries: HashMap<Option<String>, (i64, &PlaceSum)> = HashMap::new();
    for (k, n) in &cities {
        let p = &sums[*k];
        if let Some(r) = &p.region {
            regions.entry((r.clone(), p.cc.clone())).or_insert((0, p)).0 += n;
        }
        countries.entry(p.cc.clone()).or_insert((0, p)).0 += n;
    }
    fn best(m: Vec<(i64, &PlaceSum)>) -> Option<(i64, &PlaceSum)> {
        m.into_iter().max_by(|a, b| {
            a.0.cmp(&b.0)
                .then(b.1.country.cmp(&a.1.country))
                .then(b.1.city.cmp(&a.1.city))
        })
    }
    if let Some((n, p)) = best(regions.into_values().collect())
        && share(n) >= DOMINANT
    {
        return Some(TripPlace {
            name: p.region.clone().unwrap_or_default(),
            level: Level::Region,
            city: None,
            region: p.region.clone(),
            country: p.country.clone(),
            country_code: p.cc.clone(),
        });
    }
    let (_, p) = best(countries.into_values().collect())?;
    Some(TripPlace {
        name: p.country.clone(),
        level: Level::Country,
        city: None,
        region: None,
        country: p.country.clone(),
        country_code: p.cc.clone(),
    })
}

#[derive(Default, Clone)]
struct Day {
    home: i64,
    away: i64,
    total: i64,
    /// Away photos by place key.
    places: HashMap<String, i64>,
    /// Away photos by country (code, else name).
    countries: HashMap<String, i64>,
    /// Sum of the away photos' coordinates, weighted by count.
    lat: f64,
    lon: f64,
    weight: f64,
}

/// A stretch of a run of away days that is one place: it becomes a trip.
struct Leg {
    from: Date,
    to: Date,
    photos: i64,
    countries: HashMap<String, i64>,
    lat: f64,
    lon: f64,
    weight: f64,
}

impl Leg {
    fn new(date: Date, d: &Day) -> Leg {
        let mut leg = Leg {
            from: date,
            to: date,
            photos: 0,
            countries: HashMap::new(),
            lat: 0.0,
            lon: 0.0,
            weight: 0.0,
        };
        leg.add(date, d);
        leg
    }

    fn add(&mut self, date: Date, d: &Day) {
        self.to = date;
        self.photos += d.total;
        for (c, n) in &d.countries {
            *self.countries.entry(c.clone()).or_default() += n;
        }
        self.lat += d.lat;
        self.lon += d.lon;
        self.weight += d.weight;
    }

    fn absorb(&mut self, o: Leg) {
        self.from = self.from.min(o.from);
        self.to = self.to.max(o.to);
        self.photos += o.photos;
        for (c, n) in o.countries {
            *self.countries.entry(c).or_default() += n;
        }
        self.lat += o.lat;
        self.lon += o.lon;
        self.weight += o.weight;
    }

    fn centre(&self) -> Option<(f64, f64)> {
        (self.weight > 0.0).then(|| (self.lat / self.weight, self.lon / self.weight))
    }

    fn country(&self) -> Option<&String> {
        dominant_country(&self.countries)
    }

    /// Whether a day or leg of that country and centre is the same place as this leg: the same dominant country, and centres
    /// within [`LEG_KM`] when both are known.
    fn same_place(&self, country: Option<&String>, centre: Option<(f64, f64)>) -> bool {
        self.country() == country && !matches!((self.centre(), centre), (Some(a), Some(b)) if km_between(a, b) > LEG_KM)
    }
}

/// The country with the most photos (the first by name on a tie).
fn dominant_country(m: &HashMap<String, i64>) -> Option<&String> {
    m.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(c, _)| c)
}

/// The days of a run (`from..=to`) cut into legs, see the module doc.
fn legs(days: &std::collections::BTreeMap<Date, Day>, from: Date, to: Date) -> Vec<Leg> {
    let mut legs: Vec<Leg> = Vec::new();
    for (&date, d) in days.range(from..=to) {
        let centre = (d.weight > 0.0).then(|| (d.lat / d.weight, d.lon / d.weight));
        match legs.last_mut() {
            // No away photos: the day stays where it is.
            Some(leg) if d.countries.is_empty() => leg.add(date, d),
            Some(leg) if leg.same_place(dominant_country(&d.countries), centre) => leg.add(date, d),
            _ => legs.push(Leg::new(date, d)),
        }
    }
    // Small legs join a neighbour, then neighbours that are the same place join each other.
    while legs.len() > 1
        && let Some(i) = legs.iter().position(|l| l.photos < MIN_PHOTOS)
    {
        let small = legs.remove(i);
        legs[i.saturating_sub(1)].absorb(small);
    }
    let mut i = 1;
    while i < legs.len() {
        if legs[i - 1].same_place(legs[i].country(), legs[i].centre()) {
            let next = legs.remove(i);
            legs[i - 1].absorb(next);
            i = 1;
        } else {
            i += 1;
        }
    }
    legs
}

/// The trips in the rows (a window of (day, place) counts), newest first. Pure: the arithmetic of the module doc.
fn detect(rows: &[DayPlace], today: Date) -> Vec<Found> {
    let mut sums: HashMap<String, PlaceSum> = HashMap::new();
    for r in rows {
        let (Some(key), Some(city), Some(country)) = (r.key(), &r.city, &r.country) else {
            continue;
        };
        let s = sums.entry(key).or_default();
        s.city = city.clone();
        s.region = r.region.clone();
        s.country = country.clone();
        s.cc = r.cc.clone();
        s.days += 1;
        s.photos += r.n;
        if let (Some(lat), Some(lon)) = (r.lat, r.lon) {
            let w = r.n as f64;
            s.lat += lat * w;
            s.lon += lon * w;
            s.weight += w;
        }
    }
    let home = home_area(&sums);
    if home.is_empty() {
        return Vec::new();
    }
    let is_home = |k: &str| home.binary_search_by(|h| h.as_str().cmp(k)).is_ok();

    let mut days: std::collections::BTreeMap<Date, Day> = std::collections::BTreeMap::new();
    for r in rows {
        let d = days.entry(r.day).or_default();
        d.total += r.n;
        match r.key() {
            Some(k) if is_home(&k) => d.home += r.n,
            Some(k) => {
                d.away += r.n;
                *d.places.entry(k).or_default() += r.n;
                if let Some(c) = r.cc.as_ref().or(r.country.as_ref()) {
                    *d.countries.entry(c.clone()).or_default() += r.n;
                }
                if let (Some(lat), Some(lon)) = (r.lat, r.lon) {
                    let w = r.n as f64;
                    d.lat += lat * w;
                    d.lon += lon * w;
                    d.weight += w;
                }
            }
            None => {}
        }
    }
    let away_day = |d: &Day| d.away > d.home;
    let home_day = |d: &Day| !away_day(d) && d.home > 0;

    // Runs of away days.
    let mut runs: Vec<(Date, Date)> = Vec::new();
    for (&date, _) in days.iter().filter(|(_, d)| away_day(d)) {
        if let Some((_, last)) = runs.last_mut() {
            let gap = (date - *last).whole_days();
            let broken = (1..gap).any(|i| days.get(&(*last + Duration::days(i))).is_some_and(home_day));
            if gap <= GAP_DAYS + 1 && !broken {
                *last = date;
                continue;
            }
        }
        runs.push((date, date));
    }

    let mut out = Vec::new();
    for (from, to) in runs
        .into_iter()
        .flat_map(|(from, to)| legs(&days, from, to))
        .map(|l| (l.from, l.to))
    {
        let (mut photos, mut away) = (0, HashMap::<String, i64>::new());
        for (_, d) in days.range(from..=to) {
            photos += d.total;
            for (k, n) in &d.places {
                *away.entry(k.clone()).or_default() += n;
            }
        }
        if photos < MIN_PHOTOS {
            continue;
        }
        let Some(place) = title_place(&away, &sums) else {
            continue;
        };
        out.push(Found {
            from,
            to,
            photos,
            place,
            ongoing: to >= today - Duration::days(1),
            home: home.clone(),
        });
    }
    out.sort_by(|a, b| b.to.cmp(&a.to).then(b.from.cmp(&a.from)));
    out
}

/// "October 2026", "Sep – Oct 2026", "Dec 2025 – Jan 2026".
fn date_label(from: Date, to: Date) -> String {
    let full = |m: Month| m.to_string();
    let short = |m: Month| m.to_string().chars().take(3).collect::<String>();
    if (from.year(), from.month()) == (to.year(), to.month()) {
        format!("{} {}", full(from.month()), from.year())
    } else if from.year() == to.year() {
        format!("{} – {} {}", short(from.month()), short(to.month()), to.year())
    } else {
        format!(
            "{} {} – {} {}",
            short(from.month()),
            from.year(),
            short(to.month()),
            to.year()
        )
    }
}

// ---- the endpoint ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct TripsQuery {
    date: Option<String>,
    limit: Option<usize>,
    before: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Trip {
    /// `<from>_<to>`: stable while the trip is not extended.
    id: String,
    /// `YYYY-MM-DD`, the local dates of the first and the last day.
    from: String,
    to: String,
    /// The trip reaches today or yesterday: it may still grow.
    ongoing: bool,
    /// "New York · October 2026".
    title: String,
    /// "October 2026", "Sep – Oct 2026", "Dec 2025 – Jan 2026".
    date_label: String,
    place: TripPlace,
    /// Photos the trip's grid shows (stacks collapsed as the user's setting says).
    photos: i64,
    cover_id: Option<Uuid>,
    cover: Option<ListItem>,
}

#[derive(Serialize)]
pub(super) struct Trips {
    /// The memories are switched on in the user's settings; off, `trips` is empty.
    enabled: bool,
    trips: Vec<Trip>,
    /// Pass as `before` for the next page; null on the last.
    next_before: Option<String>,
}

pub(super) async fn list(
    State(s): State<AppState>,
    user: AuthUser,
    Query(q): Query<TripsQuery>,
) -> ApiResult<Json<Trips>> {
    let today = match &q.date {
        Some(d) => parse_day("date", d)?,
        None => OffsetDateTime::now_utc().date(),
    };
    let before = q.before.as_deref().map(|b| parse_day("before", b)).transpose()?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let enabled = visibility::load_prefs(&s.db, user.id).await?.memories_enabled;
    if !enabled {
        return Ok(Json(Trips {
            enabled,
            trips: Vec::new(),
            next_before: None,
        }));
    }

    // One pass over the window: photos per local day and place. A stack counts once per group.
    let expr = visibility::Sort::Taken.expr();
    let lo = (today - Duration::days(WINDOW_DAYS))
        .with_time(Time::MIDNIGHT)
        .assume_utc()
        - Duration::hours(14);
    let hi = (today + Duration::days(2)).with_time(Time::MIDNIGHT).assume_utc();
    let rows: Vec<DayPlace> = sqlx::query_as(&format!(
        "SELECT d AS day, place_city AS city, place_region AS region, place_country AS country, place_country_code AS cc,
                count(DISTINCT coalesce(sk, id)) AS n, avg(latitude) AS lat, avg(longitude) AS lon
         FROM (SELECT id, {} AS d, place_city, place_region, place_country, place_country_code, latitude, longitude,
                      {} AS sk
               FROM assets
               WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL{VIEW} AND taken_at IS NOT NULL
                 AND {expr} >= $2 AND {expr} < $3) x
         GROUP BY 1, 2, 3, 4, 5",
        local_day_sql(),
        stacks::key_sql("assets")
    ))
    .bind(user.id)
    .bind(lo)
    .bind(hi)
    .fetch_all(&s.db)
    .await?;

    let mut found = detect(&rows, today);
    if let Some(b) = before {
        found.retain(|t| t.to < b);
    }
    let more = found.len() > limit;
    found.truncate(limit);
    let next_before = more.then(|| found.last().map(|t| day_text(t.from))).flatten();

    let collapsed = visibility::stacks_collapsed(&s.db, user.id, None).await?;
    let view = VIEW;
    let mut trips = Vec::new(); // `found` was cut to `limit` (at most MAX_LIMIT) above
    for t in found {
        let range = range_sql(Some(&day_text(t.from)), Some(&day_text(t.to)))?;
        let collapse = if collapsed {
            stacks::collapse_sql(view)
        } else {
            String::new()
        };
        let photos: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM assets WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL{view}{range}{collapse}"
        ))
        .bind(user.id)
        .fetch_one(&s.db)
        .await?;
        if photos == 0 {
            continue;
        }
        let middle = |d: Date| d.with_time(Time::MIDNIGHT).assume_utc().unix_timestamp() as f64;
        let mid = (middle(t.from) + middle(t.to) + 86_400.0) / 2.0;
        let cover_row: Option<ListRow> = sqlx::query_as(&format!(
            "{}{view}{range}{collapse}
             ORDER BY (thumbhash IS NULL), favorite DESC, rating DESC,
                      ((coalesce(place_city, '') || '|' || coalesce(place_region, '') || '|' || coalesce(place_country_code, ''))
                       = ANY($2)),
                      abs(extract(epoch FROM {expr}) - $3), id
             LIMIT 1",
            list_select(visibility::Sort::Taken)
        ))
        .bind(user.id)
        .bind(&t.home)
        .bind(mid)
        .fetch_optional(&s.db)
        .await?;
        let cover = match cover_row {
            Some(r) => list_items(&s, user.id, vec![r]).await?.pop(),
            None => None,
        };
        let label = date_label(t.from, t.to);
        trips.push(Trip {
            id: format!("{}_{}", day_text(t.from), day_text(t.to)),
            from: day_text(t.from),
            to: day_text(t.to),
            ongoing: t.ongoing,
            title: format!("{} · {label}", t.place.name),
            date_label: label,
            place: t.place,
            photos,
            cover_id: cover.as_ref().map(|c| c.summary.id),
            cover,
        });
    }
    Ok(Json(Trips {
        enabled,
        trips,
        next_before,
    }))
}

#[cfg(test)]
mod tests {
    use time::macros::date;

    use super::*;

    fn at(day: Date, city: Option<(&str, &str, &str, f64, f64)>, n: i64) -> DayPlace {
        DayPlace {
            day,
            city: city.map(|c| c.0.to_string()),
            region: city.map(|c| c.1.to_string()),
            country: city.map(|c| {
                if c.2 == "US" {
                    "United States".into()
                } else {
                    c.2.to_string()
                }
            }),
            cc: city.map(|c| c.2.to_string()),
            n,
            lat: city.map(|c| c.3),
            lon: city.map(|c| c.4),
        }
    }

    type Spot = (&'static str, &'static str, &'static str, f64, f64);

    const HOME: (&str, &str, &str, f64, f64) = ("Home", "Reg", "US", 40.0, -74.0);
    const NEAR: (&str, &str, &str, f64, f64) = ("Nearby", "Reg", "US", 40.1, -74.1);
    const FAR: (&str, &str, &str, f64, f64) = ("Far", "Other", "US", 42.0, -71.0);

    fn home_days() -> Vec<DayPlace> {
        (1..=5)
            .map(|d| at(date!(2026 - 09 - 01) + Duration::days(d), Some(HOME), 4))
            .collect()
    }

    #[test]
    fn labels_follow_the_months() {
        assert_eq!(date_label(date!(2026 - 10 - 03), date!(2026 - 10 - 20)), "October 2026");
        assert_eq!(
            date_label(date!(2026 - 09 - 28), date!(2026 - 10 - 02)),
            "Sep – Oct 2026"
        );
        assert_eq!(
            date_label(date!(2025 - 12 - 28), date!(2026 - 01 - 02)),
            "Dec 2025 – Jan 2026"
        );
    }

    #[test]
    fn a_gap_of_a_day_joins_and_a_home_day_splits() {
        let mut rows = home_days();
        // Away on the 10th and 12th (the 11th empty): one trip. The 15th and 17th with a home day between: two runs.
        for d in [10, 12] {
            rows.push(at(date!(2026 - 10 - 01) + Duration::days(d - 1), Some(FAR), 6));
        }
        rows.push(at(date!(2026 - 10 - 15), Some(FAR), 9));
        rows.push(at(date!(2026 - 10 - 16), Some(HOME), 3));
        rows.push(at(date!(2026 - 10 - 17), Some(FAR), 9));
        let trips = detect(&rows, date!(2026 - 10 - 26));
        let spans: Vec<_> = trips.iter().map(|t| (t.from, t.to)).collect();
        assert_eq!(
            spans,
            vec![
                (date!(2026 - 10 - 17), date!(2026 - 10 - 17)),
                (date!(2026 - 10 - 15), date!(2026 - 10 - 15)),
                (date!(2026 - 10 - 10), date!(2026 - 10 - 12)),
            ]
        );
        assert_eq!(trips[2].photos, 12);
    }

    #[test]
    fn a_run_is_cut_into_legs_by_country_and_distance() {
        const HOUSTON: (&str, &str, &str, f64, f64) = ("Houston", "TX", "US", 29.76, -95.37);
        const AUSTIN: (&str, &str, &str, f64, f64) = ("Austin", "TX", "US", 30.27, -97.74);
        const MEXICO: (&str, &str, &str, f64, f64) = ("Mexico City", "CDMX", "MX", 19.43, -99.13);
        const TIJUANA: (&str, &str, &str, f64, f64) = ("Tijuana", "BC", "MX", 32.51, -117.04);
        const SAN_DIEGO: (&str, &str, &str, f64, f64) = ("San Diego", "CA", "US", 32.72, -117.16);
        let spans = |rows: &[DayPlace]| -> Vec<(Date, Date)> {
            let mut t = detect(rows, date!(2026 - 12 - 31));
            t.reverse();
            t.iter().map(|t| (t.from, t.to)).collect()
        };
        let stay = |spots: &[(i64, Spot, i64)]| {
            let mut rows = home_days();
            rows.extend(
                spots
                    .iter()
                    .map(|&(d, spot, n)| at(date!(2026 - 10 - 01) + Duration::days(d - 1), Some(spot), n)),
            );
            rows
        };
        // Houston, then Mexico City, back to back: two trips.
        let rows = stay(&[(10, HOUSTON, 5), (11, HOUSTON, 5), (12, MEXICO, 5), (13, MEXICO, 5)]);
        assert_eq!(
            spans(&rows),
            [
                (date!(2026 - 10 - 10), date!(2026 - 10 - 11)),
                (date!(2026 - 10 - 12), date!(2026 - 10 - 13))
            ]
        );
        // Houston and Austin are in one country and 235 km apart: one trip.
        assert_eq!(spans(&stay(&[(10, HOUSTON, 5), (11, AUSTIN, 5)])).len(), 1);
        // A short hop to Tijuana inside a San Diego stay: one trip. A first leg that is too small joins the next one.
        let rows = stay(&[
            (10, SAN_DIEGO, 5),
            (11, SAN_DIEGO, 5),
            (12, TIJUANA, 3),
            (13, SAN_DIEGO, 5),
        ]);
        assert_eq!(spans(&rows), [(date!(2026 - 10 - 10), date!(2026 - 10 - 13))]);
        let rows = stay(&[(10, TIJUANA, 3), (11, SAN_DIEGO, 5), (12, SAN_DIEGO, 5)]);
        assert_eq!(spans(&rows), [(date!(2026 - 10 - 10), date!(2026 - 10 - 12))]);
        // A day without place stays in the leg it follows.
        let mut rows = stay(&[(10, HOUSTON, 8), (12, MEXICO, 8)]);
        rows.push(at(date!(2026 - 10 - 11), None, 2));
        assert_eq!(
            spans(&rows),
            [
                (date!(2026 - 10 - 10), date!(2026 - 10 - 11)),
                (date!(2026 - 10 - 12), date!(2026 - 10 - 12))
            ]
        );
    }

    #[test]
    fn small_trips_and_unknown_homes_are_not_trips() {
        let mut rows = home_days();
        rows.push(at(date!(2026 - 10 - 10), Some(FAR), 7));
        assert!(detect(&rows, date!(2026 - 10 - 26)).is_empty());
        // Two photo-days in all: home is unknown.
        let few = vec![
            at(date!(2026 - 10 - 10), Some(FAR), 30),
            at(date!(2026 - 10 - 11), Some(FAR), 30),
        ];
        assert!(detect(&few, date!(2026 - 10 - 26)).is_empty());
    }

    #[test]
    fn nearby_towns_are_home_and_gps_less_photos_join_a_trip() {
        let mut rows = home_days();
        rows.push(at(date!(2026 - 10 - 09), Some(NEAR), 20));
        rows.push(at(date!(2026 - 10 - 10), Some(FAR), 5));
        rows.push(at(date!(2026 - 10 - 10), None, 2));
        rows.push(at(date!(2026 - 10 - 11), None, 3));
        // 7 photos on the away day alone is under the minimum; the neutral day after it is not part of the run.
        assert!(detect(&rows, date!(2026 - 10 - 11)).is_empty());
        let mut more = rows.clone();
        more.push(at(date!(2026 - 10 - 10), None, 5));
        let trips = detect(&more, date!(2026 - 10 - 11));
        assert_eq!(trips[0].photos, 12);
        assert!(trips[0].ongoing);
    }
}
