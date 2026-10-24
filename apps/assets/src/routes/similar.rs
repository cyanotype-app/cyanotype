//! `GET /assets/{id}/similar`: "More like this", the owner's photos that look like one photo, best first.
//!
//! ```text
//! GET /assets/<id>/similar?from=2024-01-01&to=2024-12-31&favorite=true&camera=Canon EOS R5&place=Lisbon&type=raw
//!                         &album=<id>&has_location=true&limit=60&cursor=...&weaker=true
//! ```
//!
//! The query is the photo's own stored image vector, from the model that answers text searches for this user (the same
//! rule, `search.rs`: the configured model once it covers enough of the library, until then the one it replaces), so the
//! two sides of the cosine always come from one model. The filters are those of `GET /assets/search`, without the text.
//! Ranking, paging, the strong and weaker tiers and the response are search's (`search.rs`, `respond`); what differs is
//! the cut-off, which has its own rules for photo-to-photo similarity (`cyanotype-clip`, `cutoff.rs`, `ImageRules`), and
//! that `probability` is null (the sigmoid head of SigLIP reads image-text cosines, not image-image ones).
//!
//! Left out: the source photo itself, trashed photos, and the hidden RAW half of a pair (which is the source's partner when
//! the source has one: a RAW + JPEG pair is one photo). Always the caller's own photos; a photo that is not theirs (or does
//! not exist, or is in the trash) is a 404.
//!
//! A photo whose vector for the serving model does not exist yet (just uploaded, still in the embed queue, or embedded only
//! by the model that is about to take over) answers 409: the web app shows "This photo is still being indexed".
use axum::{
    Json,
    extract::{Path, Query, State},
};
use cyanotype_core::metrics;
use serde::Deserialize;
use uuid::Uuid;

use super::search::{
    After, DEFAULT_LIMIT, DEPTH, Filters, MAX_LIMIT, Page, Plan, RankKind, Ranking, SearchQuery, SearchResponse,
    decode_rank_cursor, ranked_rows, respond_ranked,
};
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

/// The filters of `GET /assets/search` and its paging, without `q` (and without `skip`, which only turns off the
/// reading of a query).
#[derive(Deserialize, Default, Debug)]
pub(super) struct SimilarQuery {
    from: Option<String>,
    to: Option<String>,
    favorite: Option<bool>,
    tags: Option<String>,
    tag_mode: Option<String>,
    rating: Option<i16>,
    include_archived: Option<bool>,
    camera: Option<String>,
    lens: Option<String>,
    place: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    album: Option<Uuid>,
    has_location: Option<bool>,
    limit: Option<i64>,
    cursor: Option<String>,
    weaker: Option<bool>,
    /// `false`: the whole ranked list with every result's tier. For tooling and tests, as in search.
    cutoff: Option<bool>,
}

impl SimilarQuery {
    fn as_search(&self) -> SearchQuery {
        SearchQuery {
            from: self.from.clone(),
            to: self.to.clone(),
            favorite: self.favorite,
            tags: self.tags.clone(),
            tag_mode: self.tag_mode.clone(),
            rating: self.rating,
            include_archived: self.include_archived,
            camera: self.camera.clone(),
            lens: self.lens.clone(),
            place: self.place.clone(),
            kind: self.kind.clone(),
            album: self.album,
            has_location: self.has_location,
            ..Default::default()
        }
    }
}

pub(super) async fn similar(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Query(q): Query<SimilarQuery>,
) -> ApiResult<Json<SearchResponse>> {
    let started = std::time::Instant::now();
    // A hidden photo is not a starting point for anyone who has not unlocked the Hidden view (the results never hold hidden
    // photos either way: that is in `Filters::push_where`).
    let result = if unlock.0 || !is_hidden(&s.db, user.id, id).await? {
        run(&s, user.id, id, &q).await
    } else {
        Err(ApiError::NotFound)
    };
    let m = metrics::global();
    let (outcome, plan) = match &result {
        Ok((_, plan)) => ("ok", plan.name()),
        Err(ApiError::BadRequest(_)) => ("bad_request", "none"),
        Err(ApiError::NotFound) => ("not_found", "none"),
        Err(ApiError::Conflict(_)) => ("not_indexed", "none"),
        Err(_) => ("error", "none"),
    };
    let tags = [("mode", "similar"), ("plan", plan), ("outcome", outcome)];
    m.count("search.queries", 1, &tags);
    m.histogram("search.latency_ms", started.elapsed().as_secs_f64() * 1000.0, &tags);
    if let Ok((r, _)) = &result
        && let Some(model) = r.model
    {
        m.count("search.model", 1, &[("model", model)]);
    }
    result.map(|(r, _)| Json(r))
}

async fn is_hidden(db: &sqlx::PgPool, owner: Uuid, id: Uuid) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT hidden FROM assets WHERE id = $1 AND owner_id = $2")
            .bind(id)
            .bind(owner)
            .fetch_optional(db)
            .await?
            .unwrap_or(false),
    )
}

/// pgvector's text form (`[0.1,0.2,...]`) back to numbers.
fn parse_vector(text: &str) -> Option<Vec<f32>> {
    text.trim()
        .strip_prefix('[')?
        .strip_suffix(']')?
        .split(',')
        .map(|x| x.trim().parse().ok())
        .collect()
}

pub(super) async fn run(s: &AppState, owner: Uuid, id: Uuid, q: &SimilarQuery) -> ApiResult<(SearchResponse, Plan)> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let cursor = q.cursor.as_deref().filter(|c| !c.is_empty());
    let after = match cursor {
        Some(c) => Some(decode_rank_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
        None => None,
    };

    // The source must be a photo of the caller's that is listed (not trashed, not the hidden half of a pair). Its burst, if
    // it is in one, is left out of the list with it: the shots of one moment are not "more like this".
    let source: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT burst_id FROM assets WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL AND primary_id IS NULL",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(&s.db)
    .await?;
    let (burst,) = source.ok_or(ApiError::NotFound)?;
    let filters = Filters::parse(&q.as_search())?.excluding(id, burst);

    let serving = s
        .search
        .serving(&s.db, s.text.as_ref(), owner)
        .await
        .map_err(ApiError::Internal)?;
    let model = serving.model;
    let stored: Option<(String,)> = sqlx::query_as(
        "SELECT embedding::text FROM asset_embeddings WHERE asset_id = $1 AND model = $2 AND version = $3",
    )
    .bind(id)
    .bind(model.id())
    .bind(model.version())
    .fetch_optional(&s.db)
    .await?;
    let vector = stored
        .ok_or_else(|| {
            ApiError::Conflict("this photo is still being indexed: similar photos are available once it is".into())
        })
        .and_then(|(text,)| {
            parse_vector(&text).filter(|v| v.len() == model.dim()).ok_or_else(|| {
                ApiError::Internal(anyhow::anyhow!(
                    "stored vector of {id} is not {}-dimensional",
                    model.dim()
                ))
            })
        })?;

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
    let page = Page {
        filters: &filters,
        limit,
        cursor,
        weaker_requested: q.weaker.unwrap_or(false),
        cutoff: q.cutoff.unwrap_or(true),
    };
    let ranking = Ranking {
        model,
        reindexing: serving.reindexing,
        kind: RankKind::Similar,
    };
    respond_ranked(
        s,
        owner,
        &page,
        r,
        after.map(|c| c.position),
        ranking,
        None,
        Some(id),
        Vec::new(),
    )
    .await
}
