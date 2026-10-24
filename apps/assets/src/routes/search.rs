//! `GET /assets/search` and `GET /assets/search/facets`: semantic (natural-language) search over the owner's photos,
//! combinable with structured filters, or filters alone.
//!
//! ```text
//! GET /assets/search?q=dog on the beach&from=2024-01-01&to=2024-12-31&favorite=true&camera=Canon EOS R5
//!                   &place=Lisbon&type=raw&album=<id>&has_location=true&limit=60&cursor=...&weaker=true&skip=date
//! ```
//!
//! With `q`, the dates and places in it are first read out as filters (`query_parse.rs`; `skip` turns that off, an explicit
//! date or place filter wins), and what is left is embedded and compared with the photos' image embeddings (the worker's `embed`
//! jobs; `docs/library.md`, "Search") by the model that answers for this user (the configured one once it has indexed enough of
//! the library, until then the model it replaces: `search.rs`). At most [`DEPTH`] are ranked, best first, paged by a keyset
//! cursor over `(distance, id)`. The list is cut into a strong tier (what is returned) and a weaker one (`weaker=true`) by the
//! model's rules (`cyanotype-clip`, `cutoff.rs`); the cursor carries where the tiers end. Without `q` the same filters list
//! photos in timeline order (newest first), with no depth limit. Items have the shape of `GET /assets` (paired RAW, favorite,
//! thumbhash and thumbnail URL) plus `score`, `probability` and `tier`.
//!
//! Always the caller's own photos, never trashed ones, and never the hidden RAW half of a pair.
//!
//! The queries are built per request with a different shape for every filter combination and are not cached as
//! prepared statements (`persistent(false)`): a cached statement is planned once with placeholders, and that generic plan
//! (a date range or an album of unknown size) was measured 20x slower than the plan chosen for the actual values.
//!
//! How a ranked query runs (measured numbers in docs/library.md; the SQL repeats the model's partial index expression and
//! predicate, migration 0013, so each model's search uses its own HNSW index):
//! - **At most [`EXACT_MAX`] photos pass** the filters (a small library, an album, a month, one camera): compute the
//!   exact distance to all of them and sort. Fast at that size, complete, and an approximate index is the wrong tool
//!   for a few rows (it would walk most of the graph to find them).
//! - **More than that** (a big library, however it is filtered): the nearest `k` vectors from the HNSW index, `k` sized
//!   from the share of photos that pass so that about [`DEPTH`] survive, then the filters, then the [`DEPTH`] best.
use axum::{
    Json,
    extract::{Query, State},
};
use cyanotype_clip::Model;
use cyanotype_core::{embeddings::pg_literal, keys, metrics, pairing::RAW_EXTS};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, QueryBuilder};
use time::{Date, OffsetDateTime, Time};
use uuid::Uuid;

use super::{ListItem, ListRow, decode_cursor, encode_cursor, group, list_items};
use crate::{
    AppState,
    query_parse::{self, Skip},
    search::{ModelMissing, Reindexing},
};
use cyanotype_service::{ApiError, ApiResult, AuthUser};

/// Most results a text query returns (best first). A semantic query has no natural cut-off; beyond a few hundred the
/// matches are noise.
pub(super) const DEPTH: i64 = 300;
/// Searches over at most this many photos (a small library, or a narrow filter) are ranked by exact distance instead of
/// the HNSW index: about 50 ms at this size, and complete.
pub(super) const EXACT_MAX: i64 = 8_000;

#[cfg(test)]
thread_local! {
    /// Tests lower this to reach the HNSW plan without inserting thousands of vectors.
    pub(super) static EXACT_MAX_OVERRIDE: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

fn exact_max() -> i64 {
    #[cfg(test)]
    if let Some(v) = EXACT_MAX_OVERRIDE.with(std::cell::Cell::get) {
        return v;
    }
    EXACT_MAX
}
pub(super) const DEFAULT_LIMIT: i64 = 60;
pub(super) const MAX_LIMIT: i64 = 200;
const MAX_QUERY_CHARS: usize = 200;
/// HNSW candidates examined per query before the iterative scan widens (pgvector's default is 40). The ranked list is
/// [`DEPTH`] deep, and recall at that depth needs `ef_search` near it (measured in docs/library.md).
const EF_SEARCH: i32 = 400;
/// The iterative scan stops after this many index tuples (pgvector's default is 20,000).
const MAX_SCAN_TUPLES: i32 = 20_000;

const SORT_TIME: &str = "coalesce(a.taken_at, a.file_modified_at, a.created_at)";
const LIST_COLS: &str = "a.id, a.original_name, a.content_type, a.size_bytes, a.width, a.height,
        coalesce(a.taken_at, a.file_modified_at, a.created_at) AS sort_time, a.favorite, a.archived, a.hidden, a.thumbhash, a.burst_id";
/// The camera as the facets list it ("Canon EOS R5", "Apple iPhone 15"): a stored generated column (migration 0011).
const CAMERA_LABEL: &str = "a.camera_label";
/// Lowercase extension of the original's file name (a stored generated column, migration 0011).
const EXT: &str = "a.file_ext";
const JPEG_EXTS: &[&str] = &["jpg", "jpeg", "jpe"];
const HEIF_EXTS: &[&str] = &["heic", "heif", "hif", "avif"];

#[derive(Deserialize, Default, Clone, Debug, PartialEq, Eq)]
pub(super) struct SearchQuery {
    /// Natural-language query. Optional: filters alone list photos.
    pub q: Option<String>,
    /// First day (inclusive), `YYYY-MM-DD`, UTC.
    pub from: Option<String>,
    /// Last day (inclusive), `YYYY-MM-DD`, UTC.
    pub to: Option<String>,
    pub favorite: Option<bool>,
    /// `false` leaves archived photos out. They are in by default (they are only off the timeline, the map and the memories) and
    /// each result carries `archived`, which the app shows as a badge. Hidden photos are never in a search.
    pub include_archived: Option<bool>,
    /// A camera as the facets list it ("Canon EOS R5"); case-insensitive.
    pub camera: Option<String>,
    /// A lens as the facets list it (the EXIF lens model); case-insensitive.
    pub lens: Option<String>,
    /// A city, region or country name (or country code) as the facets list it; case-insensitive.
    pub place: Option<String>,
    /// `jpeg`, `heic` (HEIC, HEIF and AVIF), `raw` (a RAW file, or a photo with a RAW paired behind it) or `other`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub album: Option<Uuid>,
    pub has_location: Option<bool>,
    /// Photos with (or without) text the worker read off them (`ocr` job, docs/library.md, "Text in photos").
    pub has_text: Option<bool>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    /// Also list the weaker matches after the strong ones (`weaker.cursor` of a response continues where the strong ones
    /// ended).
    pub weaker: Option<bool>,
    /// `false`: no cut-off, the whole ranked list (up to 300) with every result's tier shown. For tooling and tests; the
    /// app uses the default.
    pub cutoff: Option<bool>,
    /// Do not read this kind of thing out of `q`: `date`, `place`, or both (`date,place`). What the chips' "x" sends.
    pub skip: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FileType {
    Jpeg,
    Heic,
    Raw,
    Other,
}

/// The structured part of a search, validated.
#[derive(Default, Clone)]
pub(super) struct Filters {
    from: Option<OffsetDateTime>,
    to_exclusive: Option<OffsetDateTime>,
    favorite: bool,
    /// Leave archived photos out (`include_archived=false`).
    exclude_archived: bool,
    camera: Option<String>,
    lens: Option<String>,
    place: Option<String>,
    kind: Option<FileType>,
    album: Option<Uuid>,
    has_location: Option<bool>,
    has_text: Option<bool>,
    /// "More like this": never the source photo itself, nor the other photos of its burst.
    exclude: Option<Uuid>,
    exclude_burst: Option<Uuid>,
}

pub(super) fn clean(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn parse_day(name: &str, s: &str) -> ApiResult<Date> {
    Date::parse(s, &time::macros::format_description!("[year]-[month]-[day]"))
        .map_err(|_| ApiError::BadRequest(format!("{name} must be a date like 2024-07-31")))
}

impl Filters {
    pub(super) fn parse(q: &SearchQuery) -> ApiResult<Self> {
        let day = |name: &str, v: &Option<String>| -> ApiResult<Option<Date>> {
            clean(v).map(|s| parse_day(name, &s)).transpose()
        };
        let (from, to) = (day("from", &q.from)?, day("to", &q.to)?);
        if let (Some(f), Some(t)) = (from, to)
            && f > t
        {
            return Err(ApiError::BadRequest("from is after to".into()));
        }
        let kind = match clean(&q.kind).map(|k| k.to_ascii_lowercase()).as_deref() {
            None => None,
            Some("jpeg" | "jpg") => Some(FileType::Jpeg),
            Some("heic" | "heif" | "avif") => Some(FileType::Heic),
            Some("raw") => Some(FileType::Raw),
            Some("other") => Some(FileType::Other),
            Some(_) => return Err(ApiError::BadRequest("type must be jpeg, heic, raw or other".into())),
        };
        let midnight = |d: Date| d.with_time(Time::MIDNIGHT).assume_utc();
        Ok(Self {
            from: from.map(midnight),
            to_exclusive: to.map(|d| midnight(d) + time::Duration::days(1)),
            favorite: q.favorite.unwrap_or(false),
            exclude_archived: q.include_archived == Some(false),
            camera: clean(&q.camera),
            lens: clean(&q.lens),
            place: clean(&q.place),
            kind,
            album: q.album,
            has_location: q.has_location,
            has_text: q.has_text,
            exclude: None,
            exclude_burst: None,
        })
    }

    /// The same filters, leaving one photo out (the source of a "More like this") and the rest of its burst, if it is in one.
    pub(super) fn excluding(mut self, id: Uuid, burst: Option<Uuid>) -> Self {
        self.exclude = Some(id);
        self.exclude_burst = burst;
        self
    }

    /// Anything beyond "the owner's visible live photos".
    pub(super) fn any(&self) -> bool {
        self.from.is_some()
            || self.to_exclusive.is_some()
            || self.favorite
            || self.camera.is_some()
            || self.lens.is_some()
            || self.place.is_some()
            || self.kind.is_some()
            || self.album.is_some()
            || self.has_location.is_some()
            || self.has_text.is_some()
    }

    /// ` a.owner_id = $ AND ...` for `assets a`: ownership, not trashed, not a hidden RAW, then the filters.
    pub(super) fn push_where(&self, qb: &mut QueryBuilder<'_, Postgres>, owner: Uuid) {
        qb.push(" a.owner_id = ").push_bind(owner);
        // (`purging_at` is never set on a photo that is not trashed: a constraint says so. Leaving it out lets ownership,
        // the trash and the hidden-RAW rule be answered from `assets_timeline_idx` alone, without reading rows.)
        // A hidden photo is in no search, whatever the filters say: the one place every search path goes through (the ranked
        // plans, the timeline-order list, the caption matches, the counts, "More like this", the scopes that select by search).
        // It is written as the partial indexes' own predicate, so it costs them nothing.
        qb.push(" AND a.deleted_at IS NULL AND a.primary_id IS NULL AND NOT a.hidden");
        if self.exclude_archived {
            qb.push(" AND NOT a.archived");
        }
        if let Some(id) = self.exclude {
            qb.push(" AND a.id <> ").push_bind(id);
        }
        if let Some(b) = self.exclude_burst {
            qb.push(" AND (a.burst_id IS NULL OR a.burst_id <> ")
                .push_bind(b)
                .push(")");
        }
        if let Some(t) = self.from {
            qb.push(format!(" AND {SORT_TIME} >= ")).push_bind(t);
        }
        if let Some(t) = self.to_exclusive {
            qb.push(format!(" AND {SORT_TIME} < ")).push_bind(t);
        }
        if self.favorite {
            qb.push(" AND a.favorite");
        }
        if let Some(c) = &self.camera {
            qb.push(format!(" AND lower({CAMERA_LABEL}) = lower("))
                .push_bind(c.clone())
                .push(")");
        }
        if let Some(l) = &self.lens {
            qb.push(" AND lower(a.lens_model) = lower(")
                .push_bind(l.clone())
                .push(")");
        }
        if let Some(p) = &self.place {
            qb.push(" AND (lower(a.place_city) = lower(").push_bind(p.clone());
            qb.push(") OR lower(a.place_region) = lower(").push_bind(p.clone());
            qb.push(") OR lower(a.place_country) = lower(").push_bind(p.clone());
            qb.push(") OR lower(a.place_country_code) = lower(")
                .push_bind(p.clone())
                .push("))");
        }
        if let Some(k) = self.kind {
            qb.push(" AND ");
            push_type_test(qb, k);
        }
        if let Some(album) = self.album {
            qb.push(" AND EXISTS (SELECT 1 FROM album_assets aa WHERE aa.asset_id = a.id AND aa.album_id = ")
                .push_bind(album)
                .push(")");
        }
        match self.has_location {
            Some(true) => {
                qb.push(" AND a.latitude IS NOT NULL AND a.longitude IS NOT NULL");
            }
            Some(false) => {
                qb.push(" AND (a.latitude IS NULL OR a.longitude IS NULL)");
            }
            None => {}
        }
        match self.has_text {
            Some(true) => {
                qb.push(" AND EXISTS (SELECT 1 FROM asset_text xt WHERE xt.asset_id = a.id AND xt.has_text)");
            }
            Some(false) => {
                qb.push(" AND NOT EXISTS (SELECT 1 FROM asset_text xt WHERE xt.asset_id = a.id AND xt.has_text)");
            }
            None => {}
        }
    }
}

fn ext_list(exts: &[&str]) -> String {
    exts.iter().map(|e| format!("'{e}'")).collect::<Vec<_>>().join(",")
}

/// SQL boolean for "this photo is of file type `k`". Extensions come from constants, never from the request.
fn type_sql(k: FileType) -> String {
    let jpeg = format!("{EXT} IN ({})", ext_list(JPEG_EXTS));
    let heic = format!("{EXT} IN ({})", ext_list(HEIF_EXTS));
    let raw = format!(
        "({EXT} IN ({}) OR EXISTS (SELECT 1 FROM assets r WHERE r.primary_id = a.id))",
        ext_list(RAW_EXTS)
    );
    match k {
        FileType::Jpeg => format!("({jpeg})"),
        FileType::Heic => format!("({heic})"),
        FileType::Raw => raw,
        FileType::Other => format!("NOT ({jpeg} OR {heic} OR {raw})"),
    }
}

fn push_type_test(qb: &mut QueryBuilder<'_, Postgres>, k: FileType) {
    qb.push(type_sql(k));
}

#[derive(FromRow)]
pub(super) struct SearchRow {
    #[sqlx(flatten)]
    pub(super) row: ListRow,
    pub(super) dist: Option<f64>,
    /// HNSW plan only: how many vectors the index step produced and how many photos of them passed the filters (the
    /// size of the ranked list before paging), to tell "the filters thinned it out, look further" from "that is all".
    #[sqlx(default)]
    nn_n: Option<i64>,
    #[sqlx(default)]
    ranked_n: Option<i64>,
    /// The burst the photo is in (`bursts.rs`), if any.
    #[sqlx(default)]
    pub(super) burst_id: Option<Uuid>,
}

#[derive(Serialize)]
pub(super) struct SearchItem {
    #[serde(flatten)]
    item: ListItem,
    /// Cosine similarity of the photo to the query (higher is closer; about 0.1 to 0.2 for a good SigLIP 2 match, 0.25 to
    /// 0.35 for CLIP). Null without `q`.
    score: Option<f32>,
    /// The model's own estimate that the photo matches the query, `sigmoid(scale * score + bias)`: SigLIP 2 only (CLIP's
    /// scores have no such reading). It is small for a keyword query even for a right answer (0.5 % to 15 %): compare it
    /// with other results, not with 50 %.
    probability: Option<f32>,
    /// `strong` or `weak` for a ranked search (see `cyanotype-clip`, `cutoff.rs`); null without `q`.
    tier: Option<&'static str>,
    /// A burst of near-identical shots collapsed into this result (this photo is its cover). Absent for a photo on its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    burst: Option<Burst>,
}

/// The photos of a burst that are in the results, collapsed behind the cover: `size` counts the cover too. A burst ranks by
/// its best photo (the cover is that one, or a favorite of the burst), and its tier is the best photo's; the members are
/// listed whatever their own scores (`docs/library.md`, "Bursts").
#[derive(Serialize)]
pub(super) struct Burst {
    size: usize,
    /// The other photos, best first.
    members: Vec<BurstMember>,
}

#[derive(Serialize)]
pub(super) struct BurstMember {
    #[serde(flatten)]
    item: ListItem,
    score: Option<f32>,
}

/// What the query was understood to say besides the words to search for.
#[derive(Serialize)]
pub(super) struct ParsedQuery {
    /// What was searched for: `q` without the words that became filters (empty: a filter-only search).
    text: String,
    chips: Vec<Chip>,
}

#[derive(Serialize)]
pub(super) struct Chip {
    /// `date` or `place`; the `skip` value that turns it off.
    kind: &'static str,
    /// What the chip says: "June 2023", "Maui".
    label: String,
    /// The words of the query it replaced ("in June 2023").
    matched: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to: Option<String>,
    /// For a place: the library's own spelling, and whether it is a city, region or country.
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    place_kind: Option<query_parse::PlaceKind>,
}

/// The weaker matches that exist beyond the strong ones, when they were not asked for.
#[derive(Serialize)]
pub(super) struct Weaker {
    count: i64,
    /// Pass this as `cursor` with `weaker=true` to continue right after the last strong match. Null while there are more
    /// strong pages, and when there are no strong matches at all (then `weaker=true` alone lists them).
    cursor: Option<String>,
}

#[derive(Serialize)]
pub(super) struct SearchResponse {
    pub(super) items: Vec<SearchItem>,
    next_cursor: Option<String>,
    /// Ranked by similarity to `q` (true) or listed newest first (false).
    pub(super) ranked: bool,
    /// How many photos match the filters, for the first page of a filter-only search (what "Select all" selects).
    /// Null for a ranked search (its depth is capped, so the page count is what there is) and for later pages.
    total: Option<i64>,
    /// Ranked search: how many strong matches the whole result has.
    strong_count: Option<i64>,
    /// Ranked search without `weaker=true`: the weaker matches behind the strong ones, if there are any.
    weaker: Option<Weaker>,
    /// With `q`: what was read out of it.
    parsed: Option<ParsedQuery>,
    /// Which model answered, for a ranked search: `siglip2-b16`, `clip-b32`.
    pub(super) model: Option<&'static str>,
    /// While the configured model is still indexing the library and an older one answers: how far along it is.
    reindexing: Option<Reindexing>,
    /// "More like this": the photo the list is similar to (`GET /assets/{id}/similar`). Absent for a search.
    #[serde(skip_serializing_if = "Option::is_none")]
    similar_to: Option<Uuid>,
}

/// Cursor of a ranked page: the `k` the list was built from (so every page ranks the same list), how many groups came before
/// the next page, and where the strong and weaker tiers end in the grouped list.
fn encode_rank_cursor(k: i64, tiers: &Position) -> String {
    keys::hex(format!("g|{k}|{}|{}|{}", tiers.offset, tiers.strong, tiers.weak).as_bytes())
}

/// How far into the grouped list a page starts and where its tiers end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Position {
    pub(super) offset: usize,
    pub(super) strong: usize,
    pub(super) weak: usize,
}

/// A decoded rank cursor.
pub(super) struct RankCursor {
    pub(super) k: i64,
    pub(super) position: Position,
}

pub(super) fn decode_rank_cursor(c: &str) -> Option<RankCursor> {
    if !c.len().is_multiple_of(2) || !c.is_ascii() || c.len() > 300 {
        return None;
    }
    let bytes: Option<Vec<u8>> = c
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect();
    let text = String::from_utf8(bytes?).ok()?;
    let mut parts = text.splitn(5, '|');
    (parts.next()? == "g").then_some(())?;
    let k: i64 = parts.next()?.parse().ok()?;
    (0..=i64::from(MAX_SCAN_TUPLES)).contains(&k).then_some(())?;
    let offset: usize = parts.next()?.parse().ok()?;
    let strong: usize = parts.next()?.parse().ok()?;
    let weak: usize = parts.next()?.parse().ok()?;
    (strong <= weak && weak <= DEPTH as usize && offset <= DEPTH as usize).then_some(())?;
    Some(RankCursor {
        k,
        position: Position { offset, strong, weak },
    })
}

/// Which way a query ran, for the metrics (and the tests).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Plan {
    Timeline,
    Exact,
    Hnsw,
}

impl Plan {
    pub(super) fn name(self) -> &'static str {
        match self {
            Plan::Timeline => "timeline",
            Plan::Exact => "exact",
            Plan::Hnsw => "hnsw",
        }
    }
}

pub(super) async fn search(
    State(s): State<AppState>,
    user: AuthUser,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<SearchResponse>> {
    let started = std::time::Instant::now();
    let result = run(&s, user.id, &q).await;
    let m = metrics::global();
    let text = clean(&q.q).is_some();
    let filters = Filters::parse(&q).map(|f| f.any()).unwrap_or(false);
    let mode = match (text, filters) {
        (true, true) => "text_filters",
        (true, false) => "text",
        (false, _) => "filters",
    };
    let (outcome, plan) = match &result {
        Ok((_, plan)) => ("ok", plan.name()),
        Err(ApiError::BadRequest(_)) => ("bad_request", "none"),
        Err(ApiError::Unavailable(_)) => ("unavailable", "none"),
        Err(_) => ("error", "none"),
    };
    let tags = [("mode", mode), ("plan", plan), ("outcome", outcome)];
    m.count("search.queries", 1, &tags);
    m.histogram("search.latency_ms", started.elapsed().as_secs_f64() * 1000.0, &tags);
    if let Ok((r, _)) = &result {
        if let Some(model) = r.model {
            m.count("search.model", 1, &[("model", model)]);
        }
        if r.ranked && r.items.is_empty() {
            m.count(
                "search.no_strong_matches",
                1,
                &[("weaker", if r.weaker.is_some() { "yes" } else { "no" })],
            );
        }
    }
    result.map(|(r, _)| Json(r))
}

/// Read the dates and places out of `q` (unless the request already filters on them or turned them off), and return the
/// request as it is to be run, with what was understood.
async fn understand(s: &AppState, owner: Uuid, q: &SearchQuery) -> ApiResult<(SearchQuery, Option<ParsedQuery>)> {
    let Some(text) = clean(&q.q) else {
        return Ok((q.clone(), None));
    };
    let mut skip = Skip::parse(q.skip.as_deref());
    // A date or place chosen in the filters wins: the words stay in the query.
    skip.date |= clean(&q.from).is_some() || clean(&q.to).is_some();
    skip.place |= clean(&q.place).is_some();
    let places = if skip.place {
        Default::default()
    } else {
        s.search.places(&s.db, owner).await.map_err(ApiError::Internal)?
    };
    let parsed = query_parse::parse(&text, query_parse::today(), &places, skip);
    let mut eff = q.clone();
    let mut chips = Vec::new();
    if let Some(d) = &parsed.date {
        let day = |d: time::Date| d.to_string();
        eff.from = Some(day(d.from));
        eff.to = Some(day(d.to));
        chips.push(Chip {
            kind: "date",
            label: d.label.clone(),
            matched: d.matched.clone(),
            from: Some(day(d.from)),
            to: Some(day(d.to)),
            value: None,
            place_kind: None,
        });
    }
    if let Some(p) = &parsed.place {
        eff.place = Some(p.value.clone());
        chips.push(Chip {
            kind: "place",
            label: p.value.clone(),
            matched: p.matched.clone(),
            from: None,
            to: None,
            value: Some(p.value.clone()),
            place_kind: Some(p.kind),
        });
    }
    eff.q = (!parsed.text.is_empty()).then(|| parsed.text.clone());
    Ok((
        eff,
        Some(ParsedQuery {
            text: parsed.text,
            chips,
        }),
    ))
}

pub(super) async fn run(s: &AppState, owner: Uuid, request: &SearchQuery) -> ApiResult<(SearchResponse, Plan)> {
    let (q, parsed) = understand(s, owner, request).await?;
    let filters = Filters::parse(&q)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let text = clean(&q.q);
    if let Some(t) = &text
        && t.chars().count() > MAX_QUERY_CHARS
    {
        return Err(ApiError::BadRequest(format!(
            "q is at most {MAX_QUERY_CHARS} characters"
        )));
    }
    let cursor = q.cursor.as_deref().filter(|c| !c.is_empty());
    let page = Page {
        filters: &filters,
        limit,
        cursor,
        weaker_requested: q.weaker.unwrap_or(false),
        cutoff: q.cutoff.unwrap_or(true),
    };

    let Some(t) = text else {
        // Filters alone: photos newest first, no ranking and no bursts (the timeline's order is the library's own).
        let after = match cursor {
            Some(c) => Some(decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
            None => None,
        };
        let rows = timeline_rows(&s.db, owner, &filters, limit, after).await?;
        return respond_timeline(s, owner, &page, rows, parsed).await;
    };
    let after = match cursor {
        Some(c) => Some(decode_rank_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
        None => None,
    };
    let serving = s
        .search
        .serving(&s.db, s.text.as_ref(), owner)
        .await
        .map_err(ApiError::Internal)?;
    let (model, embedder) = (serving.model, s.text.clone());
    let query_text = t.clone();
    let vector = tokio::task::spawn_blocking(move || embedder.embed(model, &t))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
        .map_err(|e| match e.downcast::<ModelMissing>() {
            Ok(missing) => {
                tracing::warn!(error = %missing, "search model is not installed");
                ApiError::Unavailable("search is not available: the model files are not installed".into())
            }
            Err(e) => ApiError::Internal(e),
        })?;
    // The whole ranked list (at most DEPTH photos) every page: bursts are collapsed in it before the tiers are cut and the
    // page is taken (`group.rs`), and the cursor says where in the grouped list the next page starts.
    let r = ranked_rows(
        &s.db,
        owner,
        &filters,
        model,
        &vector,
        DEPTH,
        Some(After::whole(after.as_ref().map(|c| c.k))),
    )
    .await?;
    let ranking = Ranking {
        model,
        reindexing: serving.reindexing,
        kind: RankKind::Text,
    };
    // Photos whose caption, or whose text read off the photo itself, has the words come first (see `caption_matches`).
    let lead = caption_matches(&s.db, owner, &filters, &query_text).await?;
    respond_ranked(
        s,
        owner,
        &page,
        r,
        after.map(|c| c.position),
        ranking,
        parsed,
        None,
        lead,
    )
    .await
}

/// Words too common to ask a caption for.
const CAPTION_STOP_WORDS: &[&str] = &[
    "the", "and", "with", "for", "from", "that", "this", "are", "was", "our", "his", "her", "its", "you", "your",
    "into", "onto", "over", "under", "near",
];
/// Most photos a caption match puts in front of the ranked ones.
const CAPTION_MAX: i64 = 50;

/// The words of a query that a caption is asked for: lower case, at least three letters, not a stop word, at most eight.
pub(super) fn caption_words(q: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for w in q.split(|c: char| !c.is_alphanumeric()) {
        let w = w.to_lowercase();
        if w.chars().count() >= 3 && !CAPTION_STOP_WORDS.contains(&w.as_str()) && !words.contains(&w) {
            words.push(w);
        }
    }
    words.truncate(8);
    words
}

/// Photos whose caption contains **every** word of the query (case-insensitive, as part of a word), newest first, at most
/// [`CAPTION_MAX`], with the request's filters applied. A caption is what the owner wrote about the photo, so such a photo is
/// as good a match as there can be: these go **in front of** the model's ranking and are not part of it (no score, and they
/// leave the model's cut-off, which is relative to its own best match, alone). A photo that is both a caption match and
/// ranked is shown once, in front.
///
/// The text the worker read off the photo (`asset_text`: signs, receipts, screenshots) is matched the same way and comes after
/// the captions (a caption is the owner's own words; text in the picture is the picture's), newest first within each; the
/// two share the [`CAPTION_MAX`] budget, captions first. The trigram index on `asset_text.text` serves the `ILIKE`.
async fn caption_matches(db: &sqlx::PgPool, owner: Uuid, f: &Filters, q: &str) -> ApiResult<Vec<SearchRow>> {
    let words = caption_words(q);
    if words.is_empty() {
        return Ok(Vec::new());
    }
    let mut lead = lead_rows(db, owner, f, &words, "a.caption", None, CAPTION_MAX).await?;
    let room = CAPTION_MAX - lead.len() as i64;
    if room > 0 {
        let seen: Vec<Uuid> = lead.iter().map(|r| r.row.summary.id).collect();
        lead.extend(lead_rows(db, owner, f, &words, "xt.text", Some(&seen), room).await?);
    }
    Ok(lead)
}

/// Photos whose `column` (a caption, or the text read off the photo) contains every word, newest first. `column` is
/// `a.caption` or `xt.text`; for the latter the photo's `asset_text` row is joined. `skip`: ids already in the list.
async fn lead_rows(
    db: &sqlx::PgPool,
    owner: Uuid,
    f: &Filters,
    words: &[String],
    column: &str,
    skip: Option<&[Uuid]>,
    limit: i64,
) -> ApiResult<Vec<SearchRow>> {
    let from = if column == "xt.text" {
        "assets a JOIN asset_text xt ON xt.asset_id = a.id"
    } else {
        "assets a"
    };
    let mut qb = QueryBuilder::<Postgres>::new(format!("SELECT {LIST_COLS}, NULL::float8 AS dist FROM {from} WHERE"));
    f.push_where(&mut qb, owner);
    qb.push(format!(" AND {column} IS NOT NULL"));
    if let Some(skip) = skip {
        qb.push(" AND a.id <> ALL(").push_bind(skip.to_vec()).push(")");
    }
    for w in words {
        // LIKE's own characters are escaped (backslash is its default escape).
        let escaped: String = w
            .chars()
            .flat_map(|c| match c {
                '%' | '_' | '\\' => vec!['\\', c],
                c => vec![c],
            })
            .collect();
        qb.push(format!(" AND {column} ILIKE "))
            .push_bind(format!("%{escaped}%"));
    }
    qb.push(format!(" ORDER BY {SORT_TIME} DESC, a.id DESC LIMIT "))
        .push_bind(limit);
    Ok(qb.build_query_as::<SearchRow>().persistent(false).fetch_all(db).await?)
}

/// Which cut-off rules a ranked list gets: a text query's, or "More like this"'s (`cyanotype-clip`, `cutoff.rs`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RankKind {
    Text,
    Similar,
}

/// The model that ranked a list.
pub(super) struct Ranking {
    pub(super) model: Model,
    pub(super) reindexing: Option<Reindexing>,
    pub(super) kind: RankKind,
}

/// What a request asked of its page.
pub(super) struct Page<'a> {
    pub(super) filters: &'a Filters,
    pub(super) limit: i64,
    pub(super) cursor: Option<&'a str>,
    pub(super) weaker_requested: bool,
    pub(super) cutoff: bool,
}

/// A ranked list to the response: bursts collapsed, the tiers cut from the groups, the page the request asked for, with the
/// cursors, the scores and the thumbnails of covers and members. Shared by the text search and "More like this".
#[allow(clippy::too_many_arguments)]
pub(super) async fn respond_ranked(
    s: &AppState,
    owner: Uuid,
    page: &Page<'_>,
    ranked: Ranked,
    cursor_position: Option<Position>,
    ranking: Ranking,
    parsed: Option<ParsedQuery>,
    similar_to: Option<Uuid>,
    lead: Vec<SearchRow>,
) -> ApiResult<(SearchResponse, Plan)> {
    let Page {
        limit,
        weaker_requested,
        cutoff,
        ..
    } = *page;
    let Ranked { mut rows, plan, k } = ranked;
    // Caption matches (text search only) come first, each a result of its own in the strong tier; the model's ranking and its
    // tiers are computed without them.
    let lead_n = lead.len();
    if lead_n > 0 {
        let ids: std::collections::HashSet<Uuid> = lead.iter().map(|r| r.row.summary.id).collect();
        rows.retain(|r| !ids.contains(&r.row.summary.id));
    }
    let mut groups = group::collapse(&rows);
    // The tiers are cut from the groups' best photos on the first page and carried in the cursor after that.
    let pos = cursor_position.unwrap_or_else(|| {
        let cos: Vec<f32> = groups
            .iter()
            .map(|g| (1.0 - rows[g.cover_best].dist.unwrap_or(1.0)) as f32)
            .collect();
        let t = match ranking.kind {
            RankKind::Text => cyanotype_clip::tiers(ranking.model, &cos),
            RankKind::Similar => cyanotype_clip::image_tiers(ranking.model, &cos),
        };
        Position {
            offset: 0,
            strong: t.strong + lead_n,
            weak: t.weak + lead_n,
        }
    });
    if lead_n > 0 {
        let mut all = lead;
        all.append(&mut rows);
        rows = all;
        let mut shifted: Vec<group::Group> = (0..lead_n)
            .map(|i| group::Group {
                cover: i,
                cover_best: i,
                others: Vec::new(),
            })
            .collect();
        shifted.extend(groups.drain(..).map(|g| group::Group {
            cover: g.cover + lead_n,
            cover_best: g.cover_best + lead_n,
            others: g.others.into_iter().map(|o| o + lead_n).collect(),
        }));
        groups = shifted;
    }
    // Show the strong tier, or strong and weaker when asked, and stop there.
    let boundary = if !cutoff {
        usize::MAX
    } else if weaker_requested {
        pos.weak
    } else {
        pos.strong
    }
    .min(groups.len());
    let start = pos.offset.min(boundary);
    let end = (start + limit as usize).min(boundary);
    let cursor_at = |at: usize| encode_rank_cursor(k, &Position { offset: at, ..pos });
    let next_cursor = (end < boundary).then(|| cursor_at(end));
    let weaker = (cutoff && !weaker_requested && pos.weak > pos.strong).then(|| Weaker {
        count: (pos.weak - pos.strong) as i64,
        // The cursor that continues after the last strong match exists once a page has reached it.
        cursor: (end == pos.strong && end > start).then(|| cursor_at(end)),
    });

    // Items: each shown group's cover, and behind it the other members of its burst.
    let model_calibration = (ranking.kind == RankKind::Text)
        .then(|| ranking.model.calibration())
        .flatten();
    let mut rows: Vec<Option<SearchRow>> = rows.into_iter().map(Some).collect();
    let mut order: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut list: Vec<ListRow> = Vec::new();
    let mut meta: Vec<(Option<f32>, Option<&'static str>)> = Vec::new();
    for (gi, g) in groups.iter().enumerate().take(end).skip(start) {
        let tier = if gi < pos.strong { "strong" } else { "weak" };
        let mut members = Vec::new();
        for &ri in std::iter::once(&g.cover).chain(g.others.iter()) {
            let row = rows[ri].take().expect("a row is in one group");
            meta.push((row.dist.map(|d| (1.0 - d) as f32), Some(tier)));
            list.push(row.row);
            members.push(list.len() - 1);
        }
        order.push((members[0], members[1..].to_vec()));
    }
    let mut items: Vec<Option<ListItem>> = list_items(s, owner, list).await?.into_iter().map(Some).collect();
    let items: Vec<SearchItem> = order
        .into_iter()
        .map(|(cover, others)| {
            let (score, tier) = meta[cover];
            SearchItem {
                item: items[cover].take().expect("an item is on one tile"),
                score,
                probability: score.zip(model_calibration).map(|(s, c)| c.probability(s)),
                tier,
                burst: (!others.is_empty()).then(|| Burst {
                    size: others.len() + 1,
                    members: others
                        .into_iter()
                        .map(|i| BurstMember {
                            item: items[i].take().expect("an item is on one tile"),
                            score: meta[i].0,
                        })
                        .collect(),
                }),
            }
        })
        .collect();
    Ok((
        SearchResponse {
            items,
            next_cursor,
            ranked: true,
            total: None,
            strong_count: Some(pos.strong as i64),
            weaker,
            parsed,
            model: Some(ranking.model.name()),
            reindexing: ranking.reindexing,
            similar_to,
        },
        plan,
    ))
}

/// A page of a filter-only search (newest first): the items, a cursor, and on the first page how many photos match.
async fn respond_timeline(
    s: &AppState,
    owner: Uuid,
    page: &Page<'_>,
    rows: Vec<SearchRow>,
    parsed: Option<ParsedQuery>,
) -> ApiResult<(SearchResponse, Plan)> {
    let Page {
        filters, limit, cursor, ..
    } = *page;
    let next_cursor = (rows.len() as i64 == limit)
        .then(|| rows.last())
        .flatten()
        .map(|r| encode_cursor(r.row.summary.sort_time, r.row.summary.id));
    let total = if cursor.is_some() {
        None
    } else {
        let mut qb = QueryBuilder::<Postgres>::new("SELECT count(*) FROM assets a WHERE");
        filters.push_where(&mut qb, owner);
        Some(
            qb.build_query_scalar::<i64>()
                .persistent(false)
                .fetch_one(&s.db)
                .await?,
        )
    };
    let items = list_items(s, owner, rows.into_iter().map(|r| r.row).collect()).await?;
    Ok((
        SearchResponse {
            items: items
                .into_iter()
                .map(|item| SearchItem {
                    item,
                    score: None,
                    probability: None,
                    tier: None,
                    burst: None,
                })
                .collect(),
            next_cursor,
            ranked: false,
            total,
            strong_count: None,
            weaker: None,
            parsed,
            model: None,
            reindexing: None,
            similar_to: None,
        },
        Plan::Timeline,
    ))
}

async fn timeline_rows(
    db: &sqlx::PgPool,
    owner: Uuid,
    f: &Filters,
    limit: i64,
    after: Option<(OffsetDateTime, Uuid)>,
) -> ApiResult<Vec<SearchRow>> {
    let mut qb = QueryBuilder::<Postgres>::new(format!("SELECT {LIST_COLS}, NULL::float8 AS dist FROM assets a WHERE"));
    f.push_where(&mut qb, owner);
    if let Some((ts, id)) = after {
        qb.push(format!(" AND ({SORT_TIME}, a.id) < ("))
            .push_bind(ts)
            .push(", ")
            .push_bind(id)
            .push(")");
    }
    qb.push(format!(" ORDER BY {SORT_TIME} DESC, a.id DESC LIMIT "))
        .push_bind(limit);
    Ok(qb.build_query_as::<SearchRow>().persistent(false).fetch_all(db).await?)
}

/// How many photos pass the filters, counting no further than `cap` (so a scan stops early once it is clear the answer
/// is "a lot").
async fn candidates(tx: &mut sqlx::PgConnection, owner: Uuid, f: &Filters, cap: i64) -> ApiResult<i64> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT count(*) FROM (SELECT 1 FROM assets a WHERE");
    f.push_where(&mut qb, owner);
    qb.push(" LIMIT ").push_bind(cap).push(") c");
    Ok(qb.build_query_scalar::<i64>().persistent(false).fetch_one(tx).await?)
}

/// First guess of how many nearest neighbours the HNSW plan fetches before the filters are applied.
///
/// Without a filter half as much again as the depth (the library is one user's: nearly every vector passes). With one,
/// the plan only runs when more than `exact_max` photos pass, so at least `exact_max / embedded` of the vectors do, and
/// `1.5 x DEPTH / that share` is enough in the worst case (2,800 at 50,000 photos). A second run costs more than
/// the extra index work of a generous first guess (measured: 450 neighbours 18 ms, 900 23 ms, 2,800 43 ms, a second pass
/// about the same again).
fn first_k(filtered: bool, embedded: i64) -> i64 {
    if !filtered {
        return DEPTH + DEPTH / 2;
    }
    if embedded <= 0 {
        return DEPTH * 3;
    }
    let share = ((exact_max() + 1) as f64 / embedded as f64).clamp(0.01, 1.0);
    ((DEPTH as f64 * 1.5 / share).ceil() as i64).clamp(DEPTH * 3, i64::from(MAX_SCAN_TUPLES))
}

/// Vectors of `model` in the table, as the planner estimates it (free, and exact enough to size a scan): the number of
/// entries of that model's partial index.
async fn embedded_estimate(tx: &mut sqlx::PgConnection, model: Model) -> ApiResult<i64> {
    let n: Option<f32> = sqlx::query_scalar("SELECT reltuples FROM pg_class WHERE relname = $1")
        .bind(model.index_name())
        .fetch_optional(tx)
        .await?;
    Ok(n.map_or(0, |n| n.max(0.0) as i64))
}

/// Each further try fetches this many times more, up to [`MAX_SCAN_TUPLES`].
const K_GROWTH: i64 = 4;

/// A page of ranked rows, the plan that produced them, and (HNSW plan) the `k` it settled on, which the next page's cursor
/// carries so that every page ranks the same list.
pub(super) struct Ranked {
    pub(super) rows: Vec<SearchRow>,
    pub(super) plan: Plan,
    pub(super) k: i64,
}

/// Where a ranked page starts: after this `(distance, id)`, over a list built from `k` nearest vectors.
#[derive(Clone, Copy)]
pub(super) struct After {
    pub(super) dist: f64,
    pub(super) id: Uuid,
    pub(super) k: i64,
    /// Not a position: the list from its start, only carrying the `k` of an earlier page (what the grouped pages use).
    pub(super) start: bool,
}

impl After {
    /// The whole list from its first photo, built from at least the `k` of the cursor that continues it.
    pub(super) fn whole(k: Option<i64>) -> Self {
        After {
            dist: 0.0,
            id: Uuid::nil(),
            k: k.unwrap_or(0),
            start: true,
        }
    }
}

pub(super) async fn ranked_rows(
    db: &sqlx::PgPool,
    owner: Uuid,
    f: &Filters,
    model: Model,
    vector: &[f32],
    limit: i64,
    after: Option<After>,
) -> ApiResult<Ranked> {
    ranked_rows_planned(db, owner, f, model, vector, limit, after, None).await
}

/// [`ranked_rows`] with the plan optionally forced (the tests compare the two plans' answers).
#[allow(clippy::too_many_arguments)]
pub(super) async fn ranked_rows_planned(
    db: &sqlx::PgPool,
    owner: Uuid,
    f: &Filters,
    model: Model,
    vector: &[f32],
    limit: i64,
    after: Option<After>,
    force: Option<Plan>,
) -> ApiResult<Ranked> {
    if vector.len() != model.dim() {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "query vector has {} dimensions, {} has {}",
            vector.len(),
            model.name(),
            model.dim()
        )));
    }
    let mut tx = db.begin().await?;
    let lit = pg_literal(vector);
    // Few enough photos to rank exactly? Counted only up to one past the threshold, so a big library is told so after
    // reading a few thousand index entries instead of all of them.
    let exact = match force {
        Some(p) => p == Plan::Exact,
        None => candidates(&mut tx, owner, f, exact_max() + 1).await? <= exact_max(),
    };
    let ranked = if exact {
        exact_rows(&mut tx, owner, f, model, &lit, limit, after).await?
    } else {
        hnsw_rows(&mut tx, owner, f, model, &lit, limit, after).await?
    };
    tx.commit().await?;
    Ok(ranked)
}

/// The vector column as the model's index sees it, and the query vector at the same size. The model id and version are
/// constants from code (never request input), written into the SQL so that the planner can match the partial index
/// `WHERE model = '<id>'` of migration 0013.
struct ModelSql {
    column: String,
    query_cast: String,
    predicate: String,
}

impl ModelSql {
    fn of(model: Model) -> Self {
        ModelSql {
            column: format!("(e.embedding::halfvec({}))", model.dim()),
            query_cast: format!("::text::halfvec({})", model.dim()),
            predicate: format!("e.model = '{}' AND e.version = {}", model.id(), model.version()),
        }
    }
}

/// Appends the page of a `ranked` CTE: the rows after the cursor, in `(dist, id)` order.
fn push_page(qb: &mut QueryBuilder<'_, Postgres>, after: Option<After>, limit: i64) {
    if let Some(a) = after.filter(|a| !a.start) {
        qb.push(" WHERE (dist, id) > (")
            .push_bind(a.dist)
            .push(", ")
            .push_bind(a.id)
            .push(")");
    }
    qb.push(" ORDER BY dist, id LIMIT ").push_bind(limit);
}

/// Exact distance to every photo that passes the filters.
async fn exact_rows(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    f: &Filters,
    model: Model,
    lit: &str,
    limit: i64,
    after: Option<After>,
) -> ApiResult<Ranked> {
    // Candidates first, then one primary-key lookup of each one's vector. Left to itself the planner prefers a hash join
    // that reads the whole embeddings table (56 MB at 50,000 photos) even for an album of 40 photos.
    sqlx::query("SELECT set_config('enable_hashjoin', 'off', true), set_config('enable_mergejoin', 'off', true)")
        .execute(&mut *tx)
        .await?;
    let m = ModelSql::of(model);
    let mut qb = QueryBuilder::<Postgres>::new(format!(
        "WITH ranked AS MATERIALIZED (SELECT {LIST_COLS}, ({} <=> ",
        m.column
    ));
    qb.push_bind(lit.to_string()).push(format!(
        "{}) AS dist FROM asset_embeddings e JOIN assets a ON a.id = e.asset_id WHERE {} AND",
        m.query_cast, m.predicate
    ));
    f.push_where(&mut qb, owner);
    // `+ 0` makes the sort key something other than the index's operator expression: without it the planner may walk the
    // HNSW index for the first key and finish with an incremental sort on `id`, which returns only the index's
    // `ef_search` nearest rows, mostly filtered out.
    qb.push(format!(" ORDER BY ({} <=> ", m.column))
        .push_bind(lit.to_string())
        .push(format!("{}) + 0, a.id", m.query_cast));
    qb.push(" LIMIT ").push_bind(DEPTH).push(") SELECT ranked.*");
    qb.push(" FROM ranked");
    push_page(&mut qb, after, limit);
    let rows = qb
        .build_query_as::<SearchRow>()
        .persistent(false)
        .fetch_all(&mut *tx)
        .await?;
    Ok(Ranked {
        rows,
        plan: Plan::Exact,
        k: 0,
    })
}

/// The nearest `k` vectors from the HNSW index alone, then the filters on their photos.
///
/// No join with the filters in the index step: with one the planner has to guess how many rows pass, and a wrong guess
/// silently turns the index scan into a scan of the whole table. The iterative scan keeps the index producing past
/// `ef_search` when `k` is larger. When the filters thin the `k` out below [`DEPTH`] (a selective filter, other users'
/// photos in the same index) and the index has more, `k` grows and the step runs again.
async fn hnsw_rows(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    f: &Filters,
    model: Model,
    lit: &str,
    limit: i64,
    after: Option<After>,
) -> ApiResult<Ranked> {
    sqlx::query(
        "SELECT set_config('hnsw.iterative_scan', 'relaxed_order', true), set_config('hnsw.ef_search', $1, true),
                set_config('hnsw.max_scan_tuples', $2, true), set_config('enable_seqscan', 'off', true)",
    )
    .bind(EF_SEARCH.to_string())
    .bind(MAX_SCAN_TUPLES.to_string())
    .execute(&mut *tx)
    .await?;
    let first = if f.any() {
        first_k(true, embedded_estimate(&mut *tx, model).await?)
    } else {
        first_k(false, 0)
    };
    let mut k = after.map_or(first, |a| a.k.max(first));
    let m = ModelSql::of(model);
    loop {
        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "WITH nn AS MATERIALIZED (SELECT e.asset_id, ({} <=> ",
            m.column
        ));
        qb.push_bind(lit.to_string()).push(format!(
            "{}) AS dist FROM asset_embeddings e WHERE {}",
            m.query_cast, m.predicate
        ));
        qb.push(format!(" ORDER BY {} <=> ", m.column))
            .push_bind(lit.to_string())
            .push(format!("{} LIMIT ", m.query_cast))
            .push_bind(k);
        qb.push(format!(
            "), ranked AS MATERIALIZED (SELECT {LIST_COLS}, nn.dist FROM nn JOIN assets a ON a.id = nn.asset_id WHERE"
        ));
        f.push_where(&mut qb, owner);
        qb.push(" ORDER BY nn.dist, a.id LIMIT ").push_bind(DEPTH).push(")");
        qb.push(" SELECT ranked.*, (SELECT count(*) FROM nn) AS nn_n, (SELECT count(*) FROM ranked) AS ranked_n");
        qb.push(" FROM ranked");
        push_page(&mut qb, after, limit);
        let rows = qb
            .build_query_as::<SearchRow>()
            .persistent(false)
            .fetch_all(&mut *tx)
            .await?;
        // A page past the end of the list carries no counts, and its cursor already has the `k` that built the list.
        let thinned = match rows.first().map(|r| (r.nn_n, r.ranked_n)) {
            Some((Some(nn), Some(ranked))) => ranked < DEPTH && nn >= k,
            Some(_) => false,
            None => after.is_none_or(|a| a.start),
        };
        if thinned && k < i64::from(MAX_SCAN_TUPLES) {
            k = (k * K_GROWTH).min(i64::from(MAX_SCAN_TUPLES));
            continue;
        }
        return Ok(Ranked {
            rows,
            plan: Plan::Hnsw,
            k,
        });
    }
}

// ---- facets -----------------------------------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
pub(super) struct Facet {
    value: String,
    count: i64,
}

#[derive(Serialize, FromRow)]
pub(super) struct PlaceFacet {
    value: String,
    /// `country`, `region` or `city`.
    kind: String,
    count: i64,
}

#[derive(Serialize)]
pub(super) struct Facets {
    total: i64,
    favorites: i64,
    with_location: i64,
    /// Photos with text read off them (the "has text" filter).
    with_text: i64,
    /// Photos that are, or have behind them, a file of each type (a JPEG + RAW pair counts under both).
    types: Vec<Facet>,
    cameras: Vec<Facet>,
    /// Lens models, most used first.
    lenses: Vec<Facet>,
    places: Vec<PlaceFacet>,
    /// Calendar years (UTC) of the photo's timeline time, newest first.
    years: Vec<Facet>,
}

const MAX_CAMERAS: i64 = 50;
const MAX_LENSES: i64 = 50;
const MAX_PLACES: i64 = 120;

/// What the filters can be set to, with how many photos each matches: the whole library (not narrowed by the filters
/// already chosen). Visible live photos only, like search itself.
pub(super) async fn facets(State(s): State<AppState>, user: AuthUser) -> ApiResult<Json<Facets>> {
    let base = "FROM assets a WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.primary_id IS NULL AND NOT a.hidden";
    let (total, favorites, with_location, with_text, jpeg, heic, raw, other): (i64, i64, i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(&format!(
            "SELECT count(*), count(*) FILTER (WHERE a.favorite),
                    count(*) FILTER (WHERE a.latitude IS NOT NULL AND a.longitude IS NOT NULL),
                    count(*) FILTER (WHERE EXISTS (SELECT 1 FROM asset_text xt WHERE xt.asset_id = a.id AND xt.has_text)),
                    count(*) FILTER (WHERE {j}), count(*) FILTER (WHERE {h}), count(*) FILTER (WHERE {r}),
                    count(*) FILTER (WHERE {o})
             {base}",
            j = type_sql(FileType::Jpeg),
            h = type_sql(FileType::Heic),
            r = type_sql(FileType::Raw),
            o = type_sql(FileType::Other),
        ))
        .bind(user.id)
        .fetch_one(&s.db)
        .await?;
    let types = [("jpeg", jpeg), ("heic", heic), ("raw", raw), ("other", other)]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(v, count)| Facet { value: v.into(), count })
        .collect();

    let cameras: Vec<Facet> = sqlx::query_as(&format!(
        "SELECT {CAMERA_LABEL} AS value, count(*) AS count {base} AND {CAMERA_LABEL} IS NOT NULL
         GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT {MAX_CAMERAS}"
    ))
    .bind(user.id)
    .fetch_all(&s.db)
    .await?;

    let lenses: Vec<Facet> = sqlx::query_as(&format!(
        "SELECT a.lens_model AS value, count(*) AS count {base} AND a.lens_model IS NOT NULL
         GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT {MAX_LENSES}"
    ))
    .bind(user.id)
    .fetch_all(&s.db)
    .await?;

    let place_part = |col: &str, kind: &str| {
        format!(
            "SELECT a.{col} AS value, '{kind}' AS kind, count(*) AS count {base} AND a.{col} IS NOT NULL GROUP BY 1"
        )
    };
    let places: Vec<PlaceFacet> = sqlx::query_as(&format!(
        "{} UNION ALL {} UNION ALL {} ORDER BY count DESC, value LIMIT {MAX_PLACES}",
        place_part("place_country", "country"),
        place_part("place_region", "region"),
        place_part("place_city", "city"),
    ))
    .bind(user.id)
    .fetch_all(&s.db)
    .await?;

    let years: Vec<Facet> = sqlx::query_as(&format!(
        "SELECT extract(year FROM ({SORT_TIME}) AT TIME ZONE 'UTC')::int::text AS value, count(*) AS count {base}
         GROUP BY 1 ORDER BY 1 DESC"
    ))
    .bind(user.id)
    .fetch_all(&s.db)
    .await?;

    Ok(Json(Facets {
        total,
        favorites,
        with_location,
        with_text,
        types,
        cameras,
        lenses,
        places,
        years,
    }))
}
