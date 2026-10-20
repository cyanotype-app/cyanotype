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
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

/// How long a download link works. The browser starts the request immediately, so a minute is plenty.
pub(super) const DOWNLOAD_TTL: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
pub(super) struct DownloadQuery {
    file: Option<Uuid>,
}

/// `GET /assets/{id}/download/edited`: the photo as edited (rotated, straightened, cropped), a JPEG of at most 4096 px on
/// the long edge with no metadata. `307` to a presigned URL once the worker has written it; until then `202 {"pending":
/// true}` (the job is queued by this request) and the page asks again. `404` for a photo that is not the caller's, not
/// visible, or in the trash; `400` for a photo with no rotate or crop to apply.
pub(super) async fn download_edited(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    let found: Option<(String, bool, bool)> = sqlx::query_as(
        "SELECT original_name, recipe IS NOT NULL, edited_copy_at IS NOT NULL FROM assets
         WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL AND primary_id IS NULL AND purging_at IS NULL
           AND (NOT hidden OR $3)",
    )
    .bind(id)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_optional(&s.db)
    .await?;
    let (name, edited, ready) = found.ok_or(ApiError::NotFound)?;
    if !edited {
        return Err(ApiError::BadRequest("this photo has no rotate or crop to apply".into()));
    }
    if !ready {
        cyanotype_core::jobs::requeue(&s.db, cyanotype_core::jobs::KIND_EDITED_COPY, id, 1).await?;
        return Ok((
            StatusCode::ACCEPTED,
            [(header::CACHE_CONTROL, "no-store")],
            axum::Json(serde_json::json!({"pending": true})),
        )
            .into_response());
    }
    let url = s
        .storage
        .presign_get_download(
            &cyanotype_core::keys::edited_copy(user.id, id),
            &cyanotype_core::keys::edited_file_name(&name),
            DOWNLOAD_TTL,
        )
        .await?;
    Ok((
        StatusCode::TEMPORARY_REDIRECT,
        [(header::LOCATION, url), (header::CACHE_CONTROL, "no-store".to_string())],
    )
        .into_response())
}

pub(super) async fn download(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Query(q): Query<DownloadQuery>,
) -> ApiResult<Response> {
    let file = q.file.unwrap_or(id);
    let found: Option<(String, String)> = sqlx::query_as(
        "SELECT f.original_key, f.original_name
         FROM assets p JOIN assets f ON f.id = $3 AND (f.id = p.id OR f.primary_id = p.id)
         WHERE p.id = $1 AND p.owner_id = $2 AND p.deleted_at IS NULL AND p.primary_id IS NULL AND f.owner_id = $2
           AND (NOT p.hidden OR $4)",
    )
    .bind(id)
    .bind(user.id)
    .bind(file)
    .bind(unlock.0)
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
