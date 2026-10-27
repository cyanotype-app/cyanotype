//! `POST /assets/rating {ids | scope, rating}`: give photos 0 to 5 stars (0 clears the rating).
//!
//! Owner-scoped and idempotent like the favorite flag: foreign ids, unknown ids and trashed photos are ignored, a RAW + JPEG
//! pair is one photo whose visible primary carries the rating (the id of the hidden RAW means its primary), and `count` is the
//! number of photos that changed (repeating a request returns 0). A hidden photo is rated only by a session that has the
//! Hidden view unlocked.
//!
//! A rating set here is the owner's: its source becomes `user` and the worker never overwrites it with the photo's own
//! metadata (`assets.rating_source`, migration 0024). The rating is in the recovery manifest, so it comes back with
//! `cyanotype reindex`; the manifest is rewritten shortly after a change (`visibility::manifest_soon`).
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::scope::Selector;
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

#[derive(Deserialize)]
pub(super) struct RatingRequest {
    #[serde(flatten)]
    pick: Selector,
    rating: i16,
}

#[derive(Serialize)]
pub(super) struct Count {
    count: u64,
}

pub(super) async fn set(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Json(req): Json<RatingRequest>,
) -> ApiResult<Json<Count>> {
    if !(0..=5).contains(&req.rating) {
        return Err(ApiError::BadRequest("rating must be 0 to 5".into()));
    }
    let pick = req.pick.validated(unlock, s.features())?;
    let mut tx = s.db.begin().await?;
    let ids: Vec<Uuid> = pick.resolve(&mut tx, user.id).await?.unwrap_or_default();
    let done = sqlx::query(
        "UPDATE assets SET rating = $3, rating_source = 'user'
         WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND (NOT hidden OR $4)
           AND (rating IS DISTINCT FROM $3 OR rating_source = 'file')
           AND (id = ANY($2) OR id IN (SELECT primary_id FROM assets
                                       WHERE owner_id = $1 AND id = ANY($2) AND primary_id IS NOT NULL))",
    )
    .bind(user.id)
    .bind(&ids)
    .bind(req.rating)
    .bind(unlock.0)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if done.rows_affected() > 0 {
        super::visibility::manifest_soon(&s, user.id);
    }
    Ok(Json(Count {
        count: done.rows_affected(),
    }))
}
