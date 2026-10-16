//! `GET /assets/{id}/download[?file=<file id>]`: save an original.
//!
//! Answers `307` with a presigned S3 URL that is valid for one minute and signs
//! `response-content-disposition: attachment; filename="<original name>"`. The browser follows it as a plain
//! navigation (an `<a href>` click), so the bucket needs no CORS policy, the bytes never touch this service, and
//! the page stays where it is because the response is an attachment. Presigning happens when the link is clicked,
//! so a page that has been open for hours never holds an expired URL.
//!
//! `{id}` is the visible photo. `file` picks the hidden RAW of a RAW + JPEG pair (its id is in the photo's
//! `paired` list); without it the photo's own file is sent. Only live photos of the caller: another user's id, an
//! unknown id, a trashed photo and a file that does not belong to the photo are all `404`.
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser};

/// How long a download link works. The browser starts the request immediately, so a minute is plenty.
pub(super) const DOWNLOAD_TTL: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
pub(super) struct DownloadQuery {
    file: Option<Uuid>,
}

pub(super) async fn download(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<DownloadQuery>,
) -> ApiResult<Response> {
    let file = q.file.unwrap_or(id);
    let found: Option<(String, String)> = sqlx::query_as(
        "SELECT f.original_key, f.original_name
         FROM assets p JOIN assets f ON f.id = $3 AND (f.id = p.id OR f.primary_id = p.id)
         WHERE p.id = $1 AND p.owner_id = $2 AND p.deleted_at IS NULL AND p.primary_id IS NULL AND f.owner_id = $2",
    )
    .bind(id)
    .bind(user.id)
    .bind(file)
    .fetch_optional(&s.db)
    .await?;
    let (key, name) = found.ok_or(ApiError::NotFound)?;
    let url = s.storage.presign_get_download(&key, &name, DOWNLOAD_TTL).await?;
    Ok((
        StatusCode::TEMPORARY_REDIRECT,
        [(header::LOCATION, url), (header::CACHE_CONTROL, "no-store".to_string())],
    )
        .into_response())
}
