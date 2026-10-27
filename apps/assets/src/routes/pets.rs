//! Pets (`docs/pets.md`): the dogs and cats the worker found in photos. On unless `PETS_ENABLED` turns it off; off, every route here but the
//! status is a `404`, as if Pets did not exist. Owner-only like the detections themselves: nothing here is reachable from a share link, a
//! public page, a download or a recovery manifest (detections are derived data, rebuilt by a rescan).
//!
//! - `GET /assets/pets/status`: `{enabled}`. The only answer while the feature is off; the web app reads it to show or hide Pets.
//! - `GET /assets/pets`: `{any, kinds: [{kind, photos, cover}], scanned, total}`: how many photos this session may see have any pet found
//!   in them, and the same per kind (`dog`, `cat`: every kind is listed, with `photos: 0` and a null `cover` when there is none). `cover`
//!   is `{asset_id, box: [x, y, w, h], preview_url}` of the photo with the clearest animal of that kind (the box is fractions of the
//!   preview image the URL serves; the web app crops it with CSS), or null. `scanned` and `total` are how far finding them has got
//!   (photos with a preview, of the owner's live ones): the page can say "still looking" until they meet.
//! - `GET /assets/pets/photos?kind=any|dog|cat&limit&cursor|before`: the photos with a pet of that kind (default `any`), same items,
//!   ordering and keyset pagination as `GET /assets`. `GET /assets/pets/timeline?kind=`: `{total, days: [{day, count}]}`, the same photos
//!   per day as `GET /assets/timeline` counts them, so the view is laid out and scrubbed like the library. Photos are in the view when
//!   they are live (archived ones too); hidden ones only with the Hidden view unlocked. A `{"pets": "any"|"dog"|"cat"}` scope
//!   (`scope.rs`) names the same photos for the bulk actions, and the `pets` filter and rule of search and smart albums read the same
//!   detections (hidden photos are never in a search).
//! - `GET /assets/{id}/pets`: the animals found in one photo, `{items: [{id, kind, box, score}], scanned}`, strongest first. `scanned` says
//!   whether the photo has been looked at (an empty list of a scanned photo is "no pets", of an unscanned one "not yet").
//!
//! A `kind` that is not `any`, `dog` or `cat` is a `400`; ids of someone else's photos are `404`.
use axum::{
    Json,
    extract::{Path, Query, State},
};
use cyanotype_core::{keys, pets};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    ListResponse, ListRow, decode_cursor, encode_cursor, list_items,
    people::seen,
    timeline::{Day, TimelineResponse},
};
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

/// Same expression as the library timeline (`x` is the assets alias).
const SORT: &str = "coalesce(x.taken_at, x.file_modified_at, x.created_at)";

/// SQL: photo `alias` has a dog, a cat or either found in it. `kind` is the parameter (`$n`) that is NULL for any pet and the kind
/// otherwise (`Which::kind`). A detection counts when its photo has a scan by the detection's model.
pub(super) fn with_pets(alias: &str, kind: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM pet_detections d WHERE d.asset_id = {alias}.id AND d.owner_id = {alias}.owner_id
                 AND ({kind}::text IS NULL OR d.kind = {kind}::text) AND {})",
        pets::SCANNED
    )
}

/// While the feature is off nothing about Pets exists.
fn on(s: &AppState) -> ApiResult<()> {
    if s.pets_enabled {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

fn which(kind: Option<&str>) -> ApiResult<pets::Which> {
    match kind.map(str::trim).filter(|k| !k.is_empty()) {
        None => Ok(pets::Which::Any),
        Some(k) => pets::Which::parse(k).ok_or_else(|| ApiError::BadRequest("kind must be any, dog or cat".into())),
    }
}

pub(super) async fn status(State(s): State<AppState>, _user: AuthUser) -> Json<Value> {
    Json(json!({"enabled": s.pets_enabled}))
}

#[derive(FromRow)]
struct CoverRow {
    kind: String,
    asset_id: Uuid,
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
}

#[derive(Serialize)]
struct Cover {
    asset_id: Uuid,
    #[serde(rename = "box")]
    bbox: [f32; 4],
    preview_url: String,
}

pub(super) async fn summary(State(s): State<AppState>, user: AuthUser, unlock: Unlock) -> ApiResult<Json<Value>> {
    on(&s)?;
    let seen_x = seen("x", "$2");
    let by_kind: Vec<(String, i64)> = sqlx::query_as(&format!(
        "SELECT d.kind, count(DISTINCT d.asset_id) FROM pet_detections d JOIN assets x ON x.id = d.asset_id
         WHERE d.owner_id = $1 AND {seen_x} AND {} GROUP BY d.kind",
        pets::SCANNED
    ))
    .bind(user.id)
    .bind(unlock.0)
    .fetch_all(&s.db)
    .await?;
    let (any,): (i64,) = sqlx::query_as(&format!(
        "SELECT count(DISTINCT d.asset_id) FROM pet_detections d JOIN assets x ON x.id = d.asset_id
         WHERE d.owner_id = $1 AND {seen_x} AND {}",
        pets::SCANNED
    ))
    .bind(user.id)
    .bind(unlock.0)
    .fetch_one(&s.db)
    .await?;
    // The clearest animal of each kind: big and sure of itself, in a photo that has a preview.
    let covers: Vec<CoverRow> = sqlx::query_as(&format!(
        "SELECT DISTINCT ON (d.kind) d.kind, d.asset_id, d.box_x, d.box_y, d.box_w, d.box_h
         FROM pet_detections d JOIN assets x ON x.id = d.asset_id
         WHERE d.owner_id = $1 AND {seen_x} AND x.thumbhash IS NOT NULL AND {}
         ORDER BY d.kind, d.score * d.box_w * d.box_h DESC, d.id",
        pets::SCANNED
    ))
    .bind(user.id)
    .bind(unlock.0)
    .fetch_all(&s.db)
    .await?;
    let progress = pets::progress(
        &s.db,
        user.id,
        pets::MODEL,
        pets::VERSION,
        super::processing::pets_basis(s.search.model),
    )
    .await
    .map_err(ApiError::Internal)?;
    let mut kinds = Vec::new();
    for kind in pets::KINDS {
        let photos = by_kind.iter().find(|(k, _)| k == kind).map_or(0, |(_, n)| *n);
        let cover = match covers.iter().find(|c| c.kind == kind) {
            Some(c) => Some(Cover {
                asset_id: c.asset_id,
                bbox: [c.box_x, c.box_y, c.box_w, c.box_h],
                preview_url: s.storage.presign_get(&keys::preview(user.id, c.asset_id)).await?,
            }),
            None => None,
        };
        kinds.push(json!({"kind": kind, "photos": photos, "cover": cover}));
    }
    Ok(Json(json!({
        "any": any,
        "kinds": kinds,
        "scanned": progress.scanned,
        "total": progress.total,
    })))
}

#[derive(Deserialize)]
pub(super) struct PetsListQuery {
    /// `any` (default), `dog` or `cat`.
    kind: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<time::OffsetDateTime>,
}

pub(super) async fn photos(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Query(q): Query<PetsListQuery>,
) -> ApiResult<Json<ListResponse>> {
    on(&s)?;
    let which = which(q.kind.as_deref())?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let after = match (&q.cursor, q.before) {
        (Some(c), _) => Some(decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
        (None, Some(t)) => Some((t, Uuid::nil())),
        (None, None) => None,
    };
    let select = format!(
        "SELECT x.id, x.original_name, x.content_type, x.size_bytes, x.width, x.height, {SORT} AS sort_time, x.favorite, x.rating, x.archived, x.hidden, (x.recipe IS NOT NULL) AS edited, x.thumbhash
         FROM assets x
         WHERE x.owner_id = $2 AND {} AND {}",
        seen("x", "$3"),
        with_pets("x", "$1")
    );
    let rows = match after {
        Some((ts, aid)) => {
            sqlx::query_as::<_, ListRow>(&format!(
                "{select} AND ({SORT}, x.id) < ($4, $5) ORDER BY {SORT} DESC, x.id DESC LIMIT $6"
            ))
            .bind(which.kind())
            .bind(user.id)
            .bind(unlock.0)
            .bind(ts)
            .bind(aid)
            .bind(limit)
            .fetch_all(&s.db)
            .await?
        }
        None => {
            sqlx::query_as::<_, ListRow>(&format!("{select} ORDER BY {SORT} DESC, x.id DESC LIMIT $4"))
                .bind(which.kind())
                .bind(user.id)
                .bind(unlock.0)
                .bind(limit)
                .fetch_all(&s.db)
                .await?
        }
    };
    let last = if rows.len() as i64 == limit { rows.last() } else { None };
    let next_cursor = last.map(|r| encode_cursor(r.summary.sort_time, r.summary.id));
    let next_before = last.map(|r| r.summary.sort_time);
    let items = list_items(&s, user.id, rows).await?;
    Ok(Json(ListResponse {
        items,
        next_cursor,
        next_before,
    }))
}

#[derive(Deserialize)]
pub(super) struct KindQuery {
    kind: Option<String>,
}

pub(super) async fn timeline(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Query(q): Query<KindQuery>,
) -> ApiResult<Json<TimelineResponse>> {
    on(&s)?;
    let which = which(q.kind.as_deref())?;
    let days: Vec<Day> = sqlx::query_as(&format!(
        "SELECT to_char(d, 'YYYY-MM-DD') AS day, count(*) AS count FROM (
         SELECT ({SORT} AT TIME ZONE 'UTC')::date AS d FROM assets x
         WHERE x.owner_id = $2 AND {} AND {}) t
         GROUP BY d ORDER BY d DESC",
        seen("x", "$3"),
        with_pets("x", "$1")
    ))
    .bind(which.kind())
    .bind(user.id)
    .bind(unlock.0)
    .fetch_all(&s.db)
    .await?;
    let total = days.iter().map(|d| d.count).sum();
    Ok(Json(TimelineResponse { total, days }))
}

#[derive(FromRow)]
struct PhotoPet {
    id: Uuid,
    kind: String,
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
    score: f32,
}

pub(super) async fn of_photo(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(asset): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    let live: Option<Uuid> = sqlx::query_scalar(&format!(
        "SELECT a.id FROM assets a WHERE a.id = $1 AND a.owner_id = $2 AND {}",
        seen("a", "$3")
    ))
    .bind(asset)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_optional(&s.db)
    .await?;
    live.ok_or(ApiError::NotFound)?;
    // The animals of the model that read the photo last (the current one once it has; an older one's meanwhile).
    let scanned_by: Option<String> = sqlx::query_scalar(
        "SELECT s.model FROM pet_scans s WHERE s.asset_id = $1 ORDER BY (s.model = $2) DESC, s.scanned_at DESC LIMIT 1",
    )
    .bind(asset)
    .bind(pets::MODEL)
    .fetch_optional(&s.db)
    .await?;
    let rows: Vec<PhotoPet> = match &scanned_by {
        Some(model) => {
            sqlx::query_as(
                "SELECT id, kind, box_x, box_y, box_w, box_h, score FROM pet_detections
                 WHERE asset_id = $1 AND owner_id = $2 AND model = $3 ORDER BY idx",
            )
            .bind(asset)
            .bind(user.id)
            .bind(model)
            .fetch_all(&s.db)
            .await?
        }
        None => Vec::new(),
    };
    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| json!({"id": r.id, "kind": r.kind, "box": [r.box_x, r.box_y, r.box_w, r.box_h], "score": r.score}))
        .collect();
    Ok(Json(json!({"items": items, "scanned": scanned_by.is_some()})))
}
