//! `GET /assets/timeline`: how many photos the owner has per calendar day, newest day first. The web app lays the
//! whole library out from this (month and day headers, scrollbar length, the year/month scrubber) and then loads
//! photos only for the part that is on screen, starting at any date with `GET /assets?before=...`.
//!
//! Days are UTC dates of the same timeline time as the list (`coalesce(taken_at, file_modified_at, created_at)`),
//! counting exactly what the list returns: not trashed, not a hidden RAW file. The query reads only that expression
//! from `assets_timeline_idx` (or `assets_favorites_idx` with `?favorite=true`), so it is an index-only scan.
use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::AppState;
use cyanotype_service::{ApiResult, AuthUser};

#[derive(Deserialize)]
pub(super) struct TimelineQuery {
    /// Count favorites only.
    #[serde(default)]
    favorite: bool,
}

#[derive(Serialize, FromRow)]
pub(super) struct Day {
    /// `YYYY-MM-DD`, UTC.
    day: String,
    count: i64,
}

#[derive(Serialize)]
pub(super) struct TimelineResponse {
    /// Sum of the counts.
    total: i64,
    days: Vec<Day>,
}

pub(super) const DAYS_SQL: &str = "SELECT to_char(d, 'YYYY-MM-DD') AS day, count(*) AS count FROM (
         SELECT (coalesce(taken_at, file_modified_at, created_at) AT TIME ZONE 'UTC')::date AS d
         FROM assets WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL";

pub(super) fn days_sql(favorite: bool) -> String {
    let fav = if favorite { " AND favorite" } else { "" };
    format!("{DAYS_SQL}{fav}) t GROUP BY d ORDER BY d DESC")
}

pub(super) async fn days(
    State(s): State<AppState>,
    user: AuthUser,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<TimelineResponse>> {
    let days: Vec<Day> = sqlx::query_as(&days_sql(q.favorite))
        .bind(user.id)
        .fetch_all(&s.db)
        .await?;
    let total = days.iter().map(|d| d.count).sum();
    Ok(Json(TimelineResponse { total, days }))
}
