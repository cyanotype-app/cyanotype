//! `POST /assets/favorite {ids, favorite}`: mark or unmark photos as favorites.
//!
//! Owner-scoped and idempotent like the other bulk endpoints: foreign ids, unknown ids and trashed photos are
//! ignored, and `count` is the number of photos whose flag actually changed (repeating a request returns 0). A
//! RAW + JPEG pair is one photo and its visible primary carries the flag; the id of the hidden RAW is accepted and
//! means its primary.
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::trash::MAX_IDS;
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser};

#[derive(Deserialize)]
pub(super) struct FavoriteRequest {
    ids: Vec<Uuid>,
    favorite: bool,
}

#[derive(Serialize)]
pub(super) struct Count {
    count: u64,
}

pub(super) async fn set(
    State(s): State<AppState>,
    user: AuthUser,
    Json(mut req): Json<FavoriteRequest>,
) -> ApiResult<Json<Count>> {
    if req.ids.len() > MAX_IDS {
        return Err(ApiError::BadRequest(format!("at most {MAX_IDS} ids per request")));
    }
    req.ids.sort_unstable();
    req.ids.dedup();
    let done = sqlx::query(
        "UPDATE assets SET favorite = $3
         WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND favorite IS DISTINCT FROM $3
           AND (id = ANY($2) OR id IN (SELECT primary_id FROM assets
                                       WHERE owner_id = $1 AND id = ANY($2) AND primary_id IS NOT NULL))",
    )
    .bind(user.id)
    .bind(&req.ids)
    .bind(req.favorite)
    .execute(&s.db)
    .await?;
    Ok(Json(Count {
        count: done.rows_affected(),
    }))
}
