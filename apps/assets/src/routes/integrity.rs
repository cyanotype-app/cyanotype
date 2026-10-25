//! The storage integrity check on Admin > Health (`cyanotype_core::integrity`, docs/admin.md, "Storage integrity"): the last run,
//! the findings, "Run check now", and the actions on a finding. Administrators only, by login session; every change is audited.
//! The scan itself runs in the worker; these routes only queue it, read what it stored and carry out an admin's click.
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use cyanotype_core::{
    admin_tasks,
    integrity::{self, ActError, Action},
};
use cyanotype_service::{AdminUser, ApiError, ApiResult, audit};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;

/// `GET /assets/admin/integrity`: the last run, open findings per kind, how much of the library the checksum sample has covered,
/// and the check that is queued or running now, if any.
pub(super) async fn summary(State(s): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Value>> {
    let acknowledged: i64 = sqlx::query_scalar("SELECT count(*) FROM integrity_findings WHERE state = 'acknowledged'")
        .fetch_one(&s.db)
        .await?;
    let task = admin_tasks::latest(&s.db, admin_tasks::KIND_INTEGRITY_CHECK)
        .await?
        .filter(|t| t.state == "queued" || t.state == "running")
        .map(|t| json!({"state": t.state, "created_at": t.created_at, "started_at": t.started_at}));
    Ok(Json(json!({
        "last_run": integrity::last_run(&s.db).await?,
        "open": integrity::open_counts(&s.db).await?,
        "acknowledged": acknowledged,
        "coverage": integrity::coverage(&s.db).await?,
        "task": task,
    })))
}

/// `POST /assets/admin/integrity/run`: queue a check now. 202 either way; if one is already queued or running, that one.
pub(super) async fn start(State(s): State<AppState>, admin: AdminUser) -> ApiResult<(StatusCode, Json<Value>)> {
    let (task, created) = admin_tasks::enqueue(
        &s.db,
        admin_tasks::KIND_INTEGRITY_CHECK,
        Some(admin.id),
        json!({"trigger": "manual"}),
    )
    .await?;
    if created {
        audit::record(
            &s.db,
            Some(admin.id),
            "integrity.run",
            Some("task"),
            Some(&task.id.to_string()),
            json!({}),
        )
        .await?;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task": {"state": task.state, "created_at": task.created_at}, "already_running": !created})),
    ))
}

#[derive(Deserialize)]
pub(super) struct ListQuery {
    kind: Option<String>,
    /// `open` (default), `acknowledged` or `resolved`.
    state: Option<String>,
    /// The last id of the previous page.
    before: Option<i64>,
    limit: Option<i64>,
}

const PAGE: i64 = 25;

/// `GET /assets/admin/integrity/findings`: a page, newest first. `next_before` is null on the last page.
pub(super) async fn list(
    State(s): State<AppState>,
    _admin: AdminUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let state = q.state.as_deref().unwrap_or("open");
    if !matches!(state, "open" | "acknowledged" | "resolved") {
        return Err(ApiError::BadRequest(
            "state must be open, acknowledged or resolved".into(),
        ));
    }
    if let Some(k) = q.kind.as_deref()
        && !integrity::KINDS.contains(&k)
    {
        return Err(ApiError::BadRequest("unknown kind of finding".into()));
    }
    let limit = q.limit.unwrap_or(PAGE).clamp(1, 100);
    let mut items = integrity::list_findings(&s.db, q.kind.as_deref(), state, q.before, limit + 1).await?;
    let more = items.len() as i64 > limit;
    items.truncate(limit as usize);
    let next_before = if more { items.last().map(|f| f.id) } else { None };
    Ok(Json(json!({"items": items, "next_before": next_before})))
}

#[derive(Deserialize)]
pub(super) struct ActReq {
    /// `restore_version`, `trash`, `acknowledge`, `restore_photo` or `delete_object`.
    action: String,
    /// `"delete"` for `delete_object`: a destructive call must say so in words.
    #[serde(default)]
    confirm: Option<String>,
}

/// `POST /assets/admin/integrity/findings/{id}`: do an action on a finding. Idempotent (`changed: false` the second time).
pub(super) async fn act(
    State(s): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    Json(req): Json<ActReq>,
) -> ApiResult<Json<Value>> {
    let action = Action::parse(&req.action).ok_or_else(|| ApiError::BadRequest("unknown action".into()))?;
    let confirmed = req.confirm.as_deref() == Some("delete");
    if action == Action::DeleteObject && !confirmed {
        return Err(ApiError::BadRequest(
            "deleting an object needs \"confirm\": \"delete\"".into(),
        ));
    }
    let done = integrity::act(&s.db, &s.storage, id, action, admin.id, confirmed)
        .await
        .map_err(|e| match e {
            ActError::NotFound => ApiError::NotFound,
            ActError::Conflict(m) => ApiError::Conflict(m),
            ActError::Failed(e) => {
                tracing::warn!(finding = id, action = action.name(), error = %format!("{e:#}"), "integrity action failed");
                ApiError::BadGateway("the action could not be completed; try again".into())
            }
        })?;
    if done.changed {
        // The key holds the owner and the content hash, never a file name.
        audit::record(
            &s.db,
            Some(admin.id),
            &format!("integrity.{}", action.name()),
            Some("integrity_finding"),
            Some(&id.to_string()),
            json!({"kind": done.finding.kind, "key": done.finding.key, "result": done.detail}),
        )
        .await?;
    }
    Ok(Json(json!({"changed": done.changed, "finding": done.finding})))
}
