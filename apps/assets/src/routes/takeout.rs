//! `POST /assets/takeout/plan`: which sidecar belongs to which file of a Google Takeout export, which folders are albums, what
//! is skipped (docs/import.md, "Moving from Google Photos").
//!
//! The browser can read a folder but has no business carrying a second copy of the matching rules, so it sends the *names* (not
//! the files) and gets the pairing back, computed by the same crate the command line uses (`cyanotype-takeout`). Names only:
//! nothing is stored, nothing touches the bucket, and the answer for fifty thousand names is a few milliseconds. All files of a
//! folder must be in one request (a sidecar sits next to its photo); the browser splits big exports by folder.
use axum::{Json, extract::DefaultBodyLimit};
use cyanotype_takeout::{Plan, plan};
use serde::Deserialize;

use cyanotype_service::{ApiError, ApiResult, AuthUser};

/// Names per request, and the longest one. 100,000 names of 200 bytes is 20 MB, within the route's body limit.
pub(super) const MAX_PATHS: usize = 100_000;
const MAX_PATH_CHARS: usize = 1024;
pub(super) const BODY_LIMIT: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
pub(super) struct PlanRequest {
    paths: Vec<String>,
}

pub(super) fn body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(BODY_LIMIT)
}

pub(super) async fn make_plan(_user: AuthUser, Json(req): Json<PlanRequest>) -> ApiResult<Json<Plan>> {
    if req.paths.len() > MAX_PATHS {
        return Err(ApiError::BadRequest(format!(
            "at most {MAX_PATHS} file names per request; send big exports a folder at a time"
        )));
    }
    if req
        .paths
        .iter()
        .any(|p| p.chars().count() > MAX_PATH_CHARS || p.contains('\0'))
    {
        return Err(ApiError::BadRequest("a file name is too long or not text".into()));
    }
    // Pure computation over strings, but 20,000 of them: keep it off the request threads' fast path.
    let plan = tokio::task::spawn_blocking(move || plan(&req.paths))
        .await
        .map_err(|e| anyhow::anyhow!("plan task: {e}"))?;
    Ok(Json(plan))
}
