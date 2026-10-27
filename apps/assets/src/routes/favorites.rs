//! `POST /assets/favorite {ids, favorite}`: mark or unmark photos as favorites.
//!
//! Owner-scoped and idempotent like the other bulk endpoints: foreign ids, unknown ids and trashed photos are
//! ignored, and `count` is the number of photos whose flag actually changed (repeating a request returns 0). A
//! RAW + JPEG pair is one photo and its visible primary carries the flag; the id of the hidden RAW is accepted and
//! means its primary.
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::scope::Selector;
use crate::AppState;
use cyanotype_service::{ApiResult, AuthUser, Unlock};

#[derive(Deserialize)]
pub(super) struct FavoriteRequest {
    #[serde(flatten)]
    pick: Selector,
    favorite: bool,
}

#[derive(Serialize)]
pub(super) struct Count {
    count: u64,
}

pub(super) async fn set(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Json(req): Json<FavoriteRequest>,
) -> ApiResult<Json<Count>> {
    let pick = req.pick.validated(unlock, s.features())?;
    let mut tx = s.db.begin().await?;
    let ids: Vec<Uuid> = pick.resolve(&mut tx, user.id).await?.unwrap_or_default();
    let done = sqlx::query(
        "UPDATE assets SET favorite = $3
         WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND favorite IS DISTINCT FROM $3
           AND (id = ANY($2) OR id IN (SELECT primary_id FROM assets
                                       WHERE owner_id = $1 AND id = ANY($2) AND primary_id IS NOT NULL))",
    )
    .bind(user.id)
    .bind(&ids)
    .bind(req.favorite)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(Count {
        count: done.rows_affected(),
    }))
}
