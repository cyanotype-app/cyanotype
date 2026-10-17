//! `GET /assets/search` and `GET /assets/search/facets`: semantic (natural-language) search over the owner's photos,
//! combinable with structured filters, or filters alone.
//!
//! ```text
//! GET /assets/search?q=dog on the beach&from=2024-01-01&to=2024-12-31&favorite=true&camera=Canon EOS R5
//!                   &place=Lisbon&type=raw&album=<id>&has_location=true&limit=60&cursor=...
//! ```
//!
//! With `q`, results are ranked by CLIP similarity between the query's text embedding and each photo's image embedding
//! (the worker's `embed` jobs; `docs/library.md`, "Search") and at most [`DEPTH`] are returned, best first, paged by a
//! keyset cursor over `(distance, id)`. Without `q` the same filters list photos in timeline order (newest first), with no depth
//! limit. Items have the shape of `GET /assets` (paired RAW, favorite, thumbhash and thumbnail URL) plus `score`.
//!
//! Always the caller's own photos, never trashed ones, and never the hidden RAW half of a pair.
//!
//! The queries are built per request with a different shape for every filter combination and are not cached as
//! prepared statements (`persistent(false)`): a cached statement is planned once with placeholders, and that generic plan
//! (a date range or an album of unknown size) was measured 20x slower than the plan chosen for the actual values.
//!
//! How a ranked query runs (measured numbers in docs/library.md):
//! - **At most [`EXACT_MAX`] photos pass** the filters (a small library, an album, a month, one camera): compute the
//!   exact distance to all of them and sort. Fast at that size, complete, and an approximate index is the wrong tool
//!   for a few rows (it would walk most of the graph to find them).
//! - **More than that** (a big library, however it is filtered): the nearest `k` vectors from the HNSW index, `k` sized
//!   from the share of photos that pass so that about [`DEPTH`] survive, then the filters, then the [`DEPTH`] best.
use axum::{
    Json,
    extract::{Query, State},
};
use cyanotype_clip::{MODEL_ID, MODEL_VERSION};
use cyanotype_core::{embeddings::pg_literal, keys, metrics, pairing::RAW_EXTS};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, QueryBuilder};
use time::{Date, OffsetDateTime, Time};
use uuid::Uuid;

use super::{ListItem, ListRow, decode_cursor, encode_cursor, list_items};
use crate::{AppState, search::ModelMissing};
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
const DEFAULT_LIMIT: i64 = 60;
const MAX_LIMIT: i64 = 200;
const MAX_QUERY_CHARS: usize = 200;
/// HNSW candidates examined per query before the iterative scan widens (pgvector's default is 40). The ranked list is
/// [`DEPTH`] deep, and recall at that depth needs `ef_search` near it (measured in docs/library.md).
const EF_SEARCH: i32 = 400;
/// The iterative scan stops after this many index tuples (pgvector's default is 20,000).
const MAX_SCAN_TUPLES: i32 = 20_000;

const SORT_TIME: &str = "coalesce(a.taken_at, a.file_modified_at, a.created_at)";
const LIST_COLS: &str = "a.id, a.original_name, a.content_type, a.size_bytes, a.width, a.height,
        coalesce(a.taken_at, a.file_modified_at, a.created_at) AS sort_time, a.favorite, a.thumbhash";
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
    /// A camera as the facets list it ("Canon EOS R5"); case-insensitive.
    pub camera: Option<String>,
    /// A city, region or country name (or country code) as the facets list it; case-insensitive.
    pub place: Option<String>,
    /// `jpeg`, `heic` (HEIC, HEIF and AVIF), `raw` (a RAW file, or a photo with a RAW paired behind it) or `other`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub album: Option<Uuid>,
    pub has_location: Option<bool>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
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
    camera: Option<String>,
    place: Option<String>,
    kind: Option<FileType>,
    album: Option<Uuid>,
    has_location: Option<bool>,
}

fn clean(s: &Option<String>) -> Option<String> {
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
            camera: clean(&q.camera),
            place: clean(&q.place),
            kind,
            album: q.album,
            has_location: q.has_location,
        })
    }

    /// Anything beyond "the owner's visible live photos".
    pub(super) fn any(&self) -> bool {
        self.from.is_some()
            || self.to_exclusive.is_some()
            || self.favorite
            || self.camera.is_some()
            || self.place.is_some()
            || self.kind.is_some()
            || self.album.is_some()
            || self.has_location.is_some()
    }

    /// ` a.owner_id = $ AND ...` for `assets a`: ownership, not trashed, not a hidden RAW, then the filters.
    pub(super) fn push_where(&self, qb: &mut QueryBuilder<'_, Postgres>, owner: Uuid) {
        qb.push(" a.owner_id = ").push_bind(owner);
        // (`purging_at` is never set on a photo that is not trashed: a constraint says so. Leaving it out lets ownership,
        // the trash and the hidden-RAW rule be answered from `assets_timeline_idx` alone, without reading rows.)
        qb.push(" AND a.deleted_at IS NULL AND a.primary_id IS NULL");
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
}

#[derive(Serialize)]
pub(super) struct SearchItem {
    #[serde(flatten)]
    item: ListItem,
    /// Cosine similarity of the photo to the query (about 0.15 to 0.35 for CLIP; higher is closer). Null without `q`.
    score: Option<f32>,
}

#[derive(Serialize)]
pub(super) struct SearchResponse {
    pub(super) items: Vec<SearchItem>,
    next_cursor: Option<String>,
    /// Ranked by similarity to `q` (true) or listed newest first (false).
    ranked: bool,
    /// How many photos match the filters, for the first page of a filter-only search (what "Select all" selects).
    /// Null for a ranked search (its depth is capped, so the page count is what there is) and for later pages.
    total: Option<i64>,
}

/// Cursor of a ranked page: the last row's `(distance, id)` and the `k` the list was built from.
fn encode_rank_cursor(dist: f64, id: Uuid, k: i64) -> String {
    keys::hex(format!("r|{:016x}|{id}|{k}", dist.to_bits()).as_bytes())
}

fn decode_rank_cursor(c: &str) -> Option<After> {
    if !c.len().is_multiple_of(2) || !c.is_ascii() || c.len() > 200 {
        return None;
    }
    let bytes: Option<Vec<u8>> = c
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect();
    let text = String::from_utf8(bytes?).ok()?;
    let mut parts = text.splitn(4, '|');
    (parts.next()? == "r").then_some(())?;
    let bits = u64::from_str_radix(parts.next()?, 16).ok()?;
    let dist = f64::from_bits(bits);
    dist.is_finite().then_some(())?;
    let id = Uuid::parse_str(parts.next()?).ok()?;
    let k: i64 = parts.next()?.parse().ok()?;
    (0..=i64::from(MAX_SCAN_TUPLES)).contains(&k).then_some(())?;
    Some(After { dist, id, k })
}

/// Which way a query ran, for the metrics (and the tests).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Plan {
    Timeline,
    Exact,
    Hnsw,
}

impl Plan {
    fn name(self) -> &'static str {
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
    result.map(|(r, _)| Json(r))
}

pub(super) async fn run(s: &AppState, owner: Uuid, q: &SearchQuery) -> ApiResult<(SearchResponse, Plan)> {
    let filters = Filters::parse(q)?;
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

    let (rows, plan, ranked, k): (Vec<SearchRow>, Plan, bool, i64) = match text {
        Some(t) => {
            let after = match cursor {
                Some(c) => Some(decode_rank_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
                None => None,
            };
            let embedder = s.text.clone();
            let vector = tokio::task::spawn_blocking(move || embedder.embed(&t))
                .await
                .map_err(|e| ApiError::Internal(e.into()))?
                .map_err(|e| match e.downcast::<ModelMissing>() {
                    Ok(missing) => {
                        tracing::warn!(error = %missing, "search model is not installed");
                        ApiError::Unavailable("search is not available: the model files are not installed".into())
                    }
                    Err(e) => ApiError::Internal(e),
                })?;
            let r = ranked_rows(&s.db, owner, &filters, &vector, limit, after).await?;
            (r.rows, r.plan, true, r.k)
        }
        None => {
            let after = match cursor {
                Some(c) => Some(decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
                None => None,
            };
            (
                timeline_rows(&s.db, owner, &filters, limit, after).await?,
                Plan::Timeline,
                false,
                0,
            )
        }
    };

    let next_cursor = (rows.len() as i64 == limit)
        .then(|| rows.last())
        .flatten()
        .map(|r| match r.dist {
            Some(d) if ranked => encode_rank_cursor(d, r.row.summary.id, k),
            _ => encode_cursor(r.row.summary.sort_time, r.row.summary.id),
        });
    let scores: Vec<Option<f32>> = rows.iter().map(|r| r.dist.map(|d| (1.0 - d) as f32)).collect();
    let total = if ranked || cursor.is_some() {
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
                .zip(scores)
                .map(|(item, score)| SearchItem { item, score })
                .collect(),
            next_cursor,
            ranked,
            total,
        },
        plan,
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

/// Vectors in the table, as the planner estimates it (free, and exact enough to size a scan).
async fn embedded_estimate(tx: &mut sqlx::PgConnection) -> ApiResult<i64> {
    let n: Option<f32> = sqlx::query_scalar("SELECT reltuples FROM pg_class WHERE oid = 'asset_embeddings'::regclass")
        .fetch_one(tx)
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
}

pub(super) async fn ranked_rows(
    db: &sqlx::PgPool,
    owner: Uuid,
    f: &Filters,
    vector: &[f32],
    limit: i64,
    after: Option<After>,
) -> ApiResult<Ranked> {
    ranked_rows_planned(db, owner, f, vector, limit, after, None).await
}

/// [`ranked_rows`] with the plan optionally forced (the tests compare the two plans' answers).
pub(super) async fn ranked_rows_planned(
    db: &sqlx::PgPool,
    owner: Uuid,
    f: &Filters,
    vector: &[f32],
    limit: i64,
    after: Option<After>,
    force: Option<Plan>,
) -> ApiResult<Ranked> {
    let mut tx = db.begin().await?;
    let lit = pg_literal(vector);
    // Few enough photos to rank exactly? Counted only up to one past the threshold, so a big library is told so after
    // reading a few thousand index entries instead of all of them.
    let exact = match force {
        Some(p) => p == Plan::Exact,
        None => candidates(&mut tx, owner, f, exact_max() + 1).await? <= exact_max(),
    };
    let ranked = if exact {
        exact_rows(&mut tx, owner, f, &lit, limit, after).await?
    } else {
        hnsw_rows(&mut tx, owner, f, &lit, limit, after).await?
    };
    tx.commit().await?;
    Ok(ranked)
}

/// Appends the page of a `ranked` CTE: the rows after the cursor, in `(dist, id)` order.
fn push_page(qb: &mut QueryBuilder<'_, Postgres>, after: Option<After>, limit: i64) {
    if let Some(a) = after {
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
    lit: &str,
    limit: i64,
    after: Option<After>,
) -> ApiResult<Ranked> {
    // Candidates first, then one primary-key lookup of each one's vector. Left to itself the planner prefers a hash join
    // that reads the whole embeddings table (56 MB at 50,000 photos) even for an album of 40 photos.
    sqlx::query("SELECT set_config('enable_hashjoin', 'off', true), set_config('enable_mergejoin', 'off', true)")
        .execute(&mut *tx)
        .await?;
    let mut qb = QueryBuilder::<Postgres>::new(format!(
        "WITH ranked AS MATERIALIZED (SELECT {LIST_COLS}, (e.embedding <=> "
    ));
    qb.push_bind(lit.to_string())
        .push("::text::halfvec) AS dist FROM asset_embeddings e JOIN assets a ON a.id = e.asset_id WHERE e.model = ");
    qb.push_bind(MODEL_ID)
        .push(" AND e.version = ")
        .push_bind(MODEL_VERSION)
        .push(" AND");
    f.push_where(&mut qb, owner);
    // `+ 0` makes the sort key something other than the index's operator expression: without it the planner may walk the
    // HNSW index for the first key and finish with an incremental sort on `id`, which returns only the index's
    // `ef_search` nearest rows, mostly filtered out.
    qb.push(" ORDER BY (e.embedding <=> ")
        .push_bind(lit.to_string())
        .push("::text::halfvec) + 0, a.id");
    qb.push(" LIMIT ").push_bind(DEPTH).push(") SELECT * FROM ranked");
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
        first_k(true, embedded_estimate(&mut *tx).await?)
    } else {
        first_k(false, 0)
    };
    let mut k = after.map_or(first, |a| a.k.max(first));
    loop {
        let mut qb = QueryBuilder::<Postgres>::new("WITH nn AS MATERIALIZED (SELECT e.asset_id, (e.embedding <=> ");
        qb.push_bind(lit.to_string())
            .push("::text::halfvec) AS dist FROM asset_embeddings e WHERE e.model = ");
        qb.push_bind(MODEL_ID)
            .push(" AND e.version = ")
            .push_bind(MODEL_VERSION);
        qb.push(" ORDER BY e.embedding <=> ")
            .push_bind(lit.to_string())
            .push("::text::halfvec LIMIT ")
            .push_bind(k);
        qb.push(format!(
            "), ranked AS MATERIALIZED (SELECT {LIST_COLS}, nn.dist FROM nn JOIN assets a ON a.id = nn.asset_id WHERE"
        ));
        f.push_where(&mut qb, owner);
        qb.push(" ORDER BY nn.dist, a.id LIMIT ").push_bind(DEPTH).push(")");
        qb.push(
            " SELECT ranked.*, (SELECT count(*) FROM nn) AS nn_n, (SELECT count(*) FROM ranked) AS ranked_n FROM ranked",
        );
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
            None => after.is_none(),
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
    /// Photos that are, or have behind them, a file of each type (a JPEG + RAW pair counts under both).
    types: Vec<Facet>,
    cameras: Vec<Facet>,
    places: Vec<PlaceFacet>,
    /// Calendar years (UTC) of the photo's timeline time, newest first.
    years: Vec<Facet>,
}

const MAX_CAMERAS: i64 = 50;
const MAX_PLACES: i64 = 120;

/// What the filters can be set to, with how many photos each matches: the whole library (not narrowed by the filters
/// already chosen). Visible live photos only, like search itself.
pub(super) async fn facets(State(s): State<AppState>, user: AuthUser) -> ApiResult<Json<Facets>> {
    let base = "FROM assets a WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.primary_id IS NULL";
    let (total, favorites, with_location, jpeg, heic, raw, other): (i64, i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(&format!(
            "SELECT count(*), count(*) FILTER (WHERE a.favorite),
                    count(*) FILTER (WHERE a.latitude IS NOT NULL AND a.longitude IS NOT NULL),
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
        types,
        cameras,
        places,
        years,
    }))
}
