//! `GET /assets/timeline`: how many photos the owner has per calendar day, newest day first. The web app lays the
//! whole library out from this (month and day headers, scrollbar length, the year/month scrubber) and then loads
//! photos only for the part that is on screen, starting at any date with `GET /assets?before=...`.
//!
//! Days are UTC dates of the same timeline time as the list (`coalesce(taken_at, file_modified_at, created_at)`, or the
//! upload time with `?sort=added`), counting exactly what the list returns (one photo per stack while stacks are collapsed, `stacks.rs`): not trashed, not a hidden RAW file, not a hidden
//! photo and (outside favorites and the archive) not archived. The query reads only that expression (and the `archived` flag
//! the index carries) from `assets_timeline_idx` (`assets_added_idx` for `sort=added`, `assets_favorites_idx` with
//! `?favorite=true`, `assets_archived_idx` with `?archived=true`), so it is an index-only scan.
use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::AppState;
use cyanotype_service::{ApiResult, AuthUser, Unlock};

#[derive(Deserialize)]
pub(super) struct TimelineQuery {
    /// Count favorites only (archived ones included, hidden never).
    #[serde(default)]
    favorite: bool,
    /// Count the archive instead of the timeline.
    #[serde(default)]
    archived: bool,
    /// Count the Hidden view: refused unless this session has unlocked it.
    #[serde(default)]
    hidden: bool,
    /// `taken` (default): days of capture time; `added`: days of upload time (the Recently added view).
    #[serde(default)]
    sort: super::visibility::Sort,
    /// `collapsed` or `expanded`: count one photo per stack or every photo. Absent: the user's setting.
    stacks: Option<super::visibility::StacksMode>,
}

#[derive(Serialize, FromRow)]
pub(super) struct Day {
    /// `YYYY-MM-DD`, UTC.
    pub(super) day: String,
    pub(super) count: i64,
}

#[derive(Serialize)]
pub(super) struct TimelineResponse {
    /// Sum of the counts.
    pub(super) total: i64,
    pub(super) days: Vec<Day>,
}

/// The day counts of one view: exactly the photos the same view's list returns (`visibility::view_sql`).
pub(super) fn days_sql(sort: super::visibility::Sort, view: &str) -> String {
    format!(
        "SELECT to_char(d, 'YYYY-MM-DD') AS day, count(*) AS count FROM (
         SELECT ({} AT TIME ZONE 'UTC')::date AS d
         FROM assets WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL{view}) t GROUP BY d ORDER BY d DESC",
        sort.expr()
    )
}

pub(super) async fn days(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<TimelineResponse>> {
    let mut view = super::visibility::view_sql(q.favorite, q.archived, q.hidden, unlock)?;
    // The same rule as the list (`routes::list`): only the timeline collapses stacks.
    if !q.favorite && !q.archived && !q.hidden && super::visibility::stacks_collapsed(&s.db, user.id, q.stacks).await? {
        let collapse = super::stacks::collapse_sql(&view);
        view.push_str(&collapse);
    }
    let days: Vec<Day> = sqlx::query_as(&days_sql(q.sort, &view))
        .bind(user.id)
        .fetch_all(&s.db)
        .await?;
    let total = days.iter().map(|d| d.count).sum();
    Ok(Json(TimelineResponse { total, days }))
}
