//! Upload flow: `check` (dedupe + part URLs) -> client PUTs parts -> `complete`.
//!
//! Two transports, chosen by the client in `check`:
//! - `direct` (default, the CLI): presigned S3 URLs; bytes never pass through the API.
//! - `proxy` (browsers, temporary): relative URLs of `PUT /assets/uploads/{upload_id}/parts/{n}`, which relays the
//!   part to S3 through this service. Needed while the bucket cannot be given a CORS policy.
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{StatusCode, header},
    routing::{delete, get, patch, post, put},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use bytes::{Bytes, BytesMut};
use cyanotype_core::{
    jobs, keys, limits, metrics, objmeta, pairing,
    storage::{Storage, UploadedPart},
};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

mod albums;
#[cfg(test)]
mod burst_tests;
mod details;
#[cfg(test)]
mod details_tests;
mod download;
mod duplicates;
#[cfg(test)]
mod duplicates_tests;
mod edits;
#[cfg(test)]
mod edits_tests;
mod favorites;
mod group;
mod health;
#[cfg(test)]
mod health_tests;
mod integrity;
#[cfg(test)]
mod integrity_tests;
#[cfg(test)]
mod key_scope_tests;
#[cfg(test)]
mod limits_tests;
mod map;
#[cfg(test)]
mod map_tests;
mod memories;
#[cfg(test)]
mod pair_tests;
mod people;
#[cfg(test)]
mod people_tests;
mod processing;
mod ratings;
#[cfg(test)]
mod ratings_tests;
#[cfg(test)]
mod rotate_tests;
mod scope;
#[cfg(test)]
mod scope_tests;
mod search;
#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod search_v2_tests;
mod share_public;
#[cfg(test)]
mod share_tests;
mod shares;
mod similar;
#[cfg(test)]
mod similar_tests;
mod smart;
#[cfg(test)]
mod smart_tests;
mod tags;
#[cfg(test)]
mod tags_tests;
mod takeout;
#[cfg(test)]
mod takeout_tests;
mod text;
#[cfg(test)]
mod text_tests;
mod tiles;
#[cfg(test)]
mod tiles_tests;
mod timeline;
#[cfg(test)]
mod timeline_tests;
mod trash;
#[cfg(test)]
mod upload_meta_tests;
mod usage;
#[cfg(test)]
mod usage_tests;
mod visibility;
#[cfg(test)]
mod visibility_tests;

use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, ImportUser, Unlock};

const PART_SIZE: i64 = 16 * 1024 * 1024;
const MAX_PARTS: i64 = 10_000; // S3 limit

/// Parts relayed at once per process. Each in-flight proxied part is buffered in memory (one `PART_SIZE`
/// allocation, handed to the SDK without further copies), so worst-case memory for the proxy path is
/// `PROXY_CONCURRENCY * PART_SIZE` = 48 MiB, well inside the assets container's `mem_limit: 256m`.
const PROXY_CONCURRENCY: usize = 3;
static PROXY_PARTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(PROXY_CONCURRENCY);
/// Overall budget for receiving one part body from the client (after a permit is held), so a stalled client
/// cannot pin a permit. 16 MiB in 120 s is ~140 KiB/s.
const PART_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Slack on top of `PART_SIZE` for the route's body limit (the exact size is checked separately).
const BODY_LIMIT_MARGIN: usize = 64 * 1024;

pub fn routes() -> Router<AppState> {
    routes_with(share_public::ShareState::from_env().expect("invalid COOKIE_SECURE or TRUSTED_PROXIES"))
}

/// The routes, with explicit limiter and cookie settings for the public share links (tests use small budgets).
fn routes_with(share_state: share_public::ShareState) -> Router<AppState> {
    Router::new()
        .merge(shares::routes())
        .merge(share_public::routes(share_state))
        .route("/assets/check", post(check))
        .route("/assets/complete", post(complete))
        .route(
            "/assets/takeout/plan",
            post(takeout::make_plan).layer(takeout::body_limit()),
        )
        .route(
            "/assets/uploads/{upload_id}/parts/{number}",
            put(put_part).layer(DefaultBodyLimit::max(PART_SIZE as usize + BODY_LIMIT_MARGIN)),
        )
        .route("/assets", get(list))
        .route("/assets/count", get(count))
        .route("/assets/timeline", get(timeline::days))
        .route("/assets/search", get(search::search))
        .route("/assets/search/facets", get(search::facets))
        .route("/assets/map", get(map::clusters))
        .route("/assets/tiles", get(tiles::installed))
        .route("/assets/tiles/{name}", get(tiles::tile_range))
        .route("/assets/favorite", post(favorites::set))
        .route("/assets/rating", post(ratings::set))
        .route("/assets/hide", post(visibility::hide))
        .route("/assets/archive", post(visibility::archive))
        .route("/assets/prefs", get(visibility::prefs).put(visibility::set_prefs))
        .route("/assets/memories", get(memories::list))
        .route("/assets/memories/dismiss", post(memories::dismiss))
        .route("/assets/tags", get(tags::list).post(tags::create))
        .route("/assets/tags/apply", post(tags::apply))
        .route("/assets/tags/{id}", patch(tags::rename).delete(tags::remove))
        .route("/assets/tags/{id}/merge", post(tags::merge))
        .route("/assets/people/status", get(people::status))
        .route("/assets/people", get(people::list))
        .route(
            "/assets/people/{id}",
            get(people::get).patch(people::patch).delete(people::remove),
        )
        .route("/assets/people/bulk", post(people::bulk))
        .route("/assets/people/{id}/merge", post(people::merge))
        .route("/assets/people/{id}/faces", get(people::cover_faces))
        .route("/assets/people/{id}/photos", get(people::photos))
        .route("/assets/people/{id}/timeline", get(people::timeline))
        .route("/assets/people/{id}/faces/remove", post(people::remove_faces))
        .route("/assets/people/{id}/suggestions", get(people::suggestions))
        .route("/assets/faces/{id}/assign", post(people::assign))
        .route("/assets/faces/{id}/reject", post(people::reject))
        .route(
            "/assets/faces/{id}/not-a-face",
            post(people::not_a_face).delete(people::is_a_face),
        )
        .route("/assets/duplicates", get(duplicates::list))
        .route("/assets/duplicates/scan", post(duplicates::scan))
        .route("/assets/duplicates/{id}/keep", post(duplicates::keep))
        .route("/assets/duplicates/{id}/dismiss", post(duplicates::dismiss))
        .route("/assets/admin/duplicates/scan", post(duplicates::scan_all))
        .route("/assets/edits", post(edits::edit))
        .route("/assets/places", get(edits::places))
        .route("/assets/processing", get(processing::status))
        .route("/assets/processing/retry", post(processing::retry))
        .route("/assets/usage", get(usage::mine))
        .route("/assets/admin/health", get(health::health))
        .route("/assets/admin/jobs/retry", post(health::retry_failed))
        .route("/assets/admin/integrity", get(integrity::summary))
        .route("/assets/admin/integrity/run", post(integrity::start))
        .route("/assets/admin/integrity/findings", get(integrity::list))
        .route("/assets/admin/integrity/findings/{id}", post(integrity::act))
        .route("/assets/admin/accounts", get(usage::accounts))
        .route("/assets/admin/accounts/{id}/limits", put(usage::set_limits))
        .route(
            "/assets/admin/default-limits",
            get(usage::default_limits).put(usage::set_default_limits),
        )
        .route(
            "/assets/admin/bucket-check",
            get(usage::bucket_check).post(usage::start_bucket_check),
        )
        .route("/assets/trash", get(trash::list).post(trash::trash))
        .route("/assets/trash/empty", post(trash::empty))
        .route("/assets/restore", post(trash::restore))
        .route("/assets/purge", post(trash::purge))
        // Static segments win over `/assets/{id}`. Albums live under /assets because the gateway sends only
        // /auth/* and /assets/* to the APIs; `/albums` is the web app's page.
        .route("/assets/albums", get(albums::list).post(albums::create))
        .route("/assets/albums/preview", post(albums::preview))
        .route(
            "/assets/albums/{id}",
            get(albums::get).patch(albums::update).delete(albums::remove),
        )
        .route("/assets/albums/{id}/assets", get(albums::assets).post(albums::add))
        .route("/assets/albums/{id}/assets/remove", post(albums::remove_assets))
        .route("/assets/{id}/unpair", post(unpair))
        .route("/assets/{id}/similar", get(similar::similar))
        .route("/assets/{id}/details", get(details::get))
        .route("/assets/{id}/text", get(text::get))
        .route("/assets/{id}/faces", get(people::of_photo))
        .route("/assets/{id}/download", get(download::download))
        .route("/assets/{id}/download/edited", get(download::download_edited))
        .route("/assets/{id}", get(get_one))
        .route("/assets/{id}", delete(remove))
}

#[derive(Deserialize)]
struct CheckRequest {
    sha256: String,
    size_bytes: i64,
    filename: String,
    content_type: Option<String>,
    /// The source file's mtime (RFC 3339). Stored on the asset; the timeline falls back to it when
    /// EXIF `taken_at` is missing.
    #[serde(default, with = "time::serde::rfc3339::option")]
    file_modified_at: Option<OffsetDateTime>,
    /// How the client will send parts. Absent means `direct` (presigned S3 URLs, the CLI).
    #[serde(default)]
    transport: Transport,
    /// Google Takeout metadata for this file (the sidecar's text and the album folder), held by hash until the photo has been
    /// processed and then applied as an edit (docs/import.md). Absent for ordinary imports.
    #[serde(default)]
    takeout: Option<cyanotype_core::takeout::Request>,
}

#[derive(Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
enum Transport {
    #[default]
    Direct,
    /// Part URLs point at this service (`PUT /assets/uploads/{upload_id}/parts/{n}`), same origin as the API.
    Proxy,
}

/// Percent-encode everything but RFC 3986 unreserved characters (S3 upload ids may hold odd characters).
fn percent_encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn proxy_part_url(upload_id: &str, number: i32) -> String {
    format!("/assets/uploads/{}/parts/{number}", percent_encode_segment(upload_id))
}

/// URLs for the parts still to send, per the requested transport.
async fn part_urls(
    s: &AppState,
    transport: Transport,
    key: &str,
    upload_id: &str,
    layout: (i64, i64),
    numbers: impl IntoIterator<Item = i32>,
) -> ApiResult<Vec<PartUrl>> {
    let (size_bytes, part_size) = layout;
    Ok(match transport {
        // Each URL is signed for the exact length the layout gives its part; `complete` checks what arrived either way.
        Transport::Direct => s
            .storage
            .presign_parts(
                key,
                upload_id,
                numbers
                    .into_iter()
                    .map(|n| (n, expected_part_size(n, size_bytes, part_size))),
            )
            .await?
            .into_iter()
            .map(|p| PartUrl {
                number: p.number,
                url: p.url,
            })
            .collect(),
        Transport::Proxy => numbers
            .into_iter()
            .map(|number| PartUrl {
                number,
                url: proxy_part_url(upload_id, number),
            })
            .collect(),
    })
}

#[derive(Serialize)]
struct PartUrl {
    number: i32,
    url: String,
}

#[derive(Serialize)]
struct DonePart {
    number: i32,
    etag: String,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum CheckResponse {
    Exists {
        asset_id: Uuid,
    },
    /// `parts` are the parts still to PUT (with fresh URLs); `uploaded` are parts a previous, interrupted
    /// attempt already stored (resume). Send both lists in `/assets/complete`.
    Upload {
        upload_id: String,
        part_size: i64,
        parts: Vec<PartUrl>,
        uploaded: Vec<DonePart>,
    },
}

/// Size S3 must have stored for part `number` of an upload of `size` bytes cut into `part_size` pieces.
fn expected_part_size(number: i32, size: i64, part_size: i64) -> i64 {
    (size - (i64::from(number) - 1) * part_size).clamp(0, part_size)
}

/// Parts that match the expected size exactly (a differently sized part is re-uploaded, overwriting it).
fn usable_parts(found: Vec<UploadedPart>, size: i64, part_size: i64) -> Vec<UploadedPart> {
    let n_parts = (size + part_size - 1) / part_size;
    let mut by_number = std::collections::BTreeMap::new();
    for p in found {
        if i64::from(p.number) >= 1
            && i64::from(p.number) <= n_parts
            && p.size == expected_part_size(p.number, size, part_size)
        {
            by_number.insert(p.number, p);
        }
    }
    by_number.into_values().collect()
}

#[derive(FromRow)]
struct PendingUpload {
    upload_id: String,
    key: String,
    part_size: i64,
}

/// An earlier `check` for the same content left a multipart upload behind (client died, network drop).
/// Resume it instead of creating (and leaking) another one. Returns None when there is nothing usable.
async fn resume_upload(
    s: &AppState,
    conn: &mut sqlx::PgConnection,
    owner: Uuid,
    sha: &[u8],
    req: &CheckRequest,
) -> ApiResult<Option<CheckResponse>> {
    let Some(p) = sqlx::query_as::<_, PendingUpload>(
        "SELECT upload_id, key, part_size FROM uploads
         WHERE owner_id = $1 AND sha256 = $2 AND size_bytes = $3 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(owner)
    .bind(sha)
    .bind(req.size_bytes)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let Some(found) = s.storage.list_parts(&p.key, &p.upload_id).await? else {
        // S3 forgot the upload (aborted or expired): drop the stale row and start over.
        sqlx::query("DELETE FROM uploads WHERE upload_id = $1")
            .bind(&p.upload_id)
            .execute(&mut *conn)
            .await?;
        return Ok(None);
    };
    let done = usable_parts(found, req.size_bytes, p.part_size);
    let n_parts = ((req.size_bytes + p.part_size - 1) / p.part_size) as i32;
    let missing: Vec<i32> = (1..=n_parts).filter(|n| !done.iter().any(|d| d.number == *n)).collect();
    let parts = part_urls(
        s,
        req.transport,
        &p.key,
        &p.upload_id,
        (req.size_bytes, p.part_size),
        missing,
    )
    .await?;
    // Keep an upload that is being actively resumed away from the stale-upload janitor.
    sqlx::query(
        "UPDATE uploads SET created_at = now(), file_modified_at = coalesce($2, file_modified_at) WHERE upload_id = $1",
    )
    .bind(&p.upload_id)
    .bind(req.file_modified_at)
    .execute(&mut *conn)
    .await?;
    Ok(Some(CheckResponse::Upload {
        upload_id: p.upload_id,
        part_size: p.part_size,
        parts,
        uploaded: done
            .into_iter()
            .map(|p| DonePart {
                number: p.number,
                etag: p.etag,
            })
            .collect(),
    }))
}

/// The same content is being permanently deleted right now. Reviving that row would point a library entry at objects
/// that are about to disappear, so refuse until the purge has finished (the client just tries again).
async fn refuse_if_purging(s: &AppState, owner: Uuid, sha: &[u8]) -> ApiResult<()> {
    let purging: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM assets WHERE owner_id = $1 AND sha256 = $2 AND purging_at IS NOT NULL")
            .bind(owner)
            .bind(sha)
            .fetch_optional(&s.db)
            .await?;
    match purging {
        Some(_) => Err(purging_conflict()),
        None => Ok(()),
    }
}

async fn check(
    State(s): State<AppState>,
    user: ImportUser,
    Json(req): Json<CheckRequest>,
) -> ApiResult<Json<CheckResponse>> {
    let sha =
        keys::parse_sha256(&req.sha256).ok_or_else(|| ApiError::BadRequest("sha256 must be 64 hex chars".into()))?;
    if req.size_bytes <= 0 || req.size_bytes > PART_SIZE * MAX_PARTS {
        return Err(ApiError::BadRequest("size_bytes out of range".into()));
    }

    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM assets WHERE owner_id = $1 AND sha256 = $2 AND deleted_at IS NULL")
            .bind(user.id)
            .bind(&sha[..])
            .fetch_optional(&s.db)
            .await?;
    // Takeout metadata is held first, whatever follows (a limit refusal, a resumed upload): it is keyed by the hash, so it
    // applies whenever the photo is, or already was, in the library.
    let held = req.takeout.as_ref().and_then(cyanotype_core::takeout::payload_of);
    if let Some(p) = &held {
        cyanotype_core::takeout::record(&s.db, user.id, &sha[..], p).await?;
    }
    if let Some((asset_id,)) = existing {
        if held.is_some() {
            cyanotype_core::takeout::enqueue_for_asset(&s.db, asset_id).await?;
        }
        return Ok(Json(CheckResponse::Exists { asset_id }));
    }

    refuse_if_purging(&s, user.id, &sha[..]).await?;

    // The account's limits (docs/admin.md, "Limits"), checked under a per-account lock that is held until the `uploads` row
    // exists, so concurrent uploads cannot each fit while together passing a limit.
    let mut tx = s.db.begin().await?;
    limits::lock_account_uploads(&mut tx, user.id).await?;
    let in_trash: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM assets WHERE owner_id = $1 AND sha256 = $2 AND deleted_at IS NOT NULL)",
    )
    .bind(user.id)
    .bind(&sha[..])
    .fetch_one(&mut *tx)
    .await?;
    if let Some(refusal) = limits::check_new_upload(&mut tx, user.id, &sha[..], req.size_bytes, in_trash).await? {
        metrics::global().count("uploads.refused", 1, &[("reason", refusal_reason(&refusal))]);
        return Err(limit_error(&refusal));
    }

    if let Some(resp) = resume_upload(&s, &mut tx, user.id, &sha[..], &req).await? {
        return Ok(Json(resp));
    }

    let content_type = req
        .content_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".into());
    let key = keys::original(user.id, &sha, &keys::sanitize_ext(&req.filename));
    // The object describes itself (name, owner, upload time, client mtime) so `cyanotype reindex` can rebuild its
    // row from the bucket alone. It is set here, on CreateMultipartUpload, so no client ever has to send it and
    // presigned part URLs are unaffected. A multipart upload started before this shipped simply lacks it.
    let metadata = objmeta::encode(&objmeta::OriginalMeta {
        name: Some(req.filename.clone()),
        owner: Some(user.id),
        created_at: Some(OffsetDateTime::now_utc()),
        file_modified_at: req.file_modified_at,
    });
    let upload_id = s.storage.create_multipart(&key, &content_type, metadata).await?;
    let n_parts = ((req.size_bytes + PART_SIZE - 1) / PART_SIZE) as i32;
    let parts = part_urls(
        &s,
        req.transport,
        &key,
        &upload_id,
        (req.size_bytes, PART_SIZE),
        1..=n_parts,
    )
    .await?;

    sqlx::query(
        "INSERT INTO uploads (upload_id, owner_id, sha256, key, size_bytes, part_size, original_name, content_type, file_modified_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(&upload_id)
    .bind(user.id)
    .bind(&sha[..])
    .bind(&key)
    .bind(req.size_bytes)
    .bind(PART_SIZE)
    .bind(&req.filename)
    .bind(&content_type)
    .bind(req.file_modified_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(CheckResponse::Upload {
        upload_id,
        part_size: PART_SIZE,
        parts,
        uploaded: Vec::new(),
    }))
}

fn refusal_reason(r: &limits::Refusal) -> &'static str {
    match r {
        limits::Refusal::FileTooLarge { .. } => "file_too_large",
        limits::Refusal::FileExceedsDailyCap { .. } => "file_exceeds_daily_cap",
        limits::Refusal::QuotaExceeded { .. } => "quota_exceeded",
        limits::Refusal::DailyCap { .. } => "daily_cap_exceeded",
    }
}

/// A refused upload as the API answers it: 413 (quota, file size) or 429 (daily cap, with when it can resume), a stable
/// `code` for clients and a message a person can read as it is.
fn limit_error(r: &limits::Refusal) -> ApiError {
    use limits::human;
    let rfc = |t: OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    };
    let (status, message, details, retry_after_secs) = match r {
        limits::Refusal::FileTooLarge { size, max_bytes } => (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "This file is {} and the largest file your account may upload is {}.",
                human(*size),
                human(*max_bytes)
            ),
            serde_json::json!({"size_bytes": size, "max_file_bytes": max_bytes}),
            None,
        ),
        limits::Refusal::FileExceedsDailyCap { size, cap } => (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "This file is {}, more than your whole daily upload limit of {}, so it cannot be uploaded with this account.",
                human(*size),
                human(*cap)
            ),
            serde_json::json!({"size_bytes": size, "daily_upload_bytes": cap}),
            None,
        ),
        limits::Refusal::QuotaExceeded {
            size,
            quota_bytes,
            stored_bytes,
            reserved_bytes,
        } => {
            let remaining = (quota_bytes - stored_bytes - reserved_bytes).max(0);
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "Not enough storage: this file needs {} and your account has {} left of {}. Photos in the trash count until it is emptied.",
                    human(*size),
                    human(remaining),
                    human(*quota_bytes)
                ),
                serde_json::json!({
                    "size_bytes": size, "quota_bytes": quota_bytes, "stored_bytes": stored_bytes,
                    "reserved_bytes": reserved_bytes, "remaining_bytes": remaining,
                }),
                None,
            )
        }
        limits::Refusal::DailyCap {
            size,
            cap,
            used_bytes,
            retry_at,
        } => {
            let secs = (*retry_at - OffsetDateTime::now_utc()).whole_seconds().max(1) as u64;
            (
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "Daily upload limit reached: {} of {} used in the last 24 hours. Uploads can resume at {}.",
                    human(*used_bytes),
                    human(*cap),
                    rfc(*retry_at)
                ),
                serde_json::json!({
                    "size_bytes": size, "daily_upload_bytes": cap, "used_bytes": used_bytes,
                    "retry_at": rfc(*retry_at), "retry_after_secs": secs,
                }),
                Some(secs),
            )
        }
    };
    ApiError::Limited {
        status,
        code: refusal_reason(r),
        message,
        details,
        retry_after_secs,
    }
}

#[derive(FromRow)]
struct PartTarget {
    key: String,
    size_bytes: i64,
    part_size: i64,
}

/// What `put_part` checks before it touches the body: the part number and declared `Content-Length` must match
/// what the upload layout says. Returns the expected part size.
fn validate_part(number: i32, size_bytes: i64, part_size: i64, content_length: Option<u64>) -> ApiResult<i64> {
    let n_parts = (size_bytes + part_size - 1) / part_size;
    if number < 1 || i64::from(number) > n_parts {
        return Err(ApiError::BadRequest(format!("part number must be in 1..={n_parts}")));
    }
    let expected = expected_part_size(number, size_bytes, part_size);
    let Some(len) = content_length else {
        return Err(ApiError::BadRequest("Content-Length is required".into()));
    };
    if len > (part_size as u64).max(expected as u64) {
        return Err(ApiError::PayloadTooLarge(format!("part is at most {part_size} bytes")));
    }
    if len != expected as u64 {
        return Err(ApiError::BadRequest(format!(
            "part {number} must be exactly {expected} bytes"
        )));
    }
    Ok(expected)
}

/// Where a proxied part got to, so a failure can say what went wrong without the error type carrying it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PartProgress {
    stage: PartStage,
    /// Bytes of the body received so far.
    received: u64,
    /// The size this part must have, once the request has been validated.
    expected: Option<u64>,
    /// The body stream itself failed (the client went away or the connection broke), as opposed to ending early.
    body_errored: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum PartStage {
    #[default]
    Validating,
    Receiving,
    Storing,
    Recording,
}

/// Why a proxied part failed. A small fixed set: it is the `reason` tag of `uploads.parts` and a field of the warn
/// log, so it must never grow with user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartFailure {
    /// The body stream broke before the part was complete: the browser or the connection went away mid-part.
    ClientAborted,
    /// The body ended cleanly, with fewer bytes than the declared `Content-Length`.
    BodyTooShort,
    /// Nothing complete arrived within `PART_READ_TIMEOUT`.
    Timeout,
    /// The bucket refused or failed the relayed part.
    StorageError,
    /// Anything else: a bad part number or length, an unknown upload, a longer body, the database.
    Other,
}

impl PartFailure {
    fn as_str(self) -> &'static str {
        match self {
            Self::ClientAborted => "client_aborted",
            Self::BodyTooShort => "body_too_short",
            Self::Timeout => "timeout",
            Self::StorageError => "storage_error",
            Self::Other => "other",
        }
    }

    fn classify(err: &ApiError, progress: &PartProgress) -> Self {
        match (err, progress.stage) {
            (ApiError::RequestTimeout(_), _) => Self::Timeout,
            (_, PartStage::Receiving) if progress.body_errored => Self::ClientAborted,
            (_, PartStage::Receiving) if progress.expected.is_some_and(|e| progress.received < e) => Self::BodyTooShort,
            (ApiError::BadGateway(_), PartStage::Storing) => Self::StorageError,
            _ => Self::Other,
        }
    }
}

/// Read exactly `expected` bytes of `req`'s body into one buffer (never more than `expected` is allocated).
async fn read_part_body(req: Request, expected: usize, progress: &mut PartProgress) -> ApiResult<Bytes> {
    let mut body = req.into_body();
    let mut buf = BytesMut::with_capacity(expected);
    progress.stage = PartStage::Receiving;
    progress.expected = Some(expected as u64);
    let read = async {
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| {
                progress.body_errored = true;
                ApiError::BadRequest("upload interrupted".into())
            })?;
            if let Some(data) = frame.data_ref() {
                if buf.len() + data.len() > expected {
                    // More than declared: count it, so it is not mistaken for a short body.
                    progress.received = (buf.len() + data.len()) as u64;
                    return Err(ApiError::BadRequest("body is longer than Content-Length".into()));
                }
                buf.extend_from_slice(data);
                progress.received = buf.len() as u64;
            }
        }
        Ok(())
    };
    tokio::time::timeout(PART_READ_TIMEOUT, read)
        .await
        .map_err(|_| ApiError::RequestTimeout("timed out receiving the part".into()))??;
    if buf.len() != expected {
        return Err(ApiError::BadRequest("body is shorter than Content-Length".into()));
    }
    Ok(buf.freeze())
}

/// Browser upload transport: relay one part to S3 and return its ETag (what `complete` needs).
/// Emits `uploads.parts{outcome}` (`ok`, `timeout` = the 408 a stalled client gets, `error`; failures also carry a
/// `reason` tag, see [`PartFailure`]) and, for relayed parts, `uploads.part_latency_ms` (time from the request starting
/// to the ETag, including the client's upload). A failed part is also logged at warn, with ids and sizes only: never
/// the file name, the owner or a URL.
async fn put_part(
    state: State<AppState>,
    user: ImportUser,
    path: Path<(String, i32)>,
    req: Request,
) -> ApiResult<Json<serde_json::Value>> {
    let started = std::time::Instant::now();
    let upload_id = path.0.0.clone();
    let number = path.0.1;
    let mut progress = PartProgress::default();
    let result = relay_part(state, user, path, req, &mut progress).await;
    let m = metrics::global();
    match &result {
        Ok(_) => {
            m.count("uploads.parts", 1, &[("outcome", "ok")]);
            m.histogram("uploads.part_latency_ms", started.elapsed().as_secs_f64() * 1000.0, &[]);
        }
        Err(e) => {
            let reason = PartFailure::classify(e, &progress);
            let outcome = if reason == PartFailure::Timeout {
                "timeout"
            } else {
                "error"
            };
            m.count("uploads.parts", 1, &[("outcome", outcome), ("reason", reason.as_str())]);
            tracing::warn!(
                upload_id = %upload_id,
                part = number,
                reason = reason.as_str(),
                bytes_received = progress.received,
                bytes_expected = progress.expected,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "part upload failed"
            );
        }
    }
    result
}

async fn relay_part(
    State(s): State<AppState>,
    user: ImportUser,
    Path((upload_id, number)): Path<(String, i32)>,
    req: Request,
    progress: &mut PartProgress,
) -> ApiResult<Json<serde_json::Value>> {
    // Unknown id and someone else's id are both 404, so ids can't be probed.
    let up: PartTarget =
        sqlx::query_as("SELECT key, size_bytes, part_size FROM uploads WHERE upload_id = $1 AND owner_id = $2")
            .bind(&upload_id)
            .bind(user.id)
            .fetch_optional(&s.db)
            .await?
            .ok_or(ApiError::NotFound)?;
    let content_length = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let expected = validate_part(number, up.size_bytes, up.part_size, content_length)? as usize;

    let _permit = PROXY_PARTS.acquire().await.map_err(|e| ApiError::Internal(e.into()))?;
    let body = read_part_body(req, expected, progress).await?;
    progress.stage = PartStage::Storing;
    let etag = s
        .storage
        .upload_part(&up.key, &upload_id, number, body)
        .await
        .map_err(|e| {
            tracing::warn!(error = ?e, part = number, "proxied part upload to storage failed");
            ApiError::BadGateway("storage rejected the part".into())
        })?;
    progress.stage = PartStage::Recording;
    // Keep an upload that is actively receiving parts away from the stale-upload janitor.
    sqlx::query("UPDATE uploads SET created_at = now() WHERE upload_id = $1")
        .bind(&upload_id)
        .execute(&s.db)
        .await?;
    metrics::global().count("uploads.part_bytes", expected as u64, &[]);
    tracing::info!(part = number, bytes_proxied = expected, "part proxied");
    Ok(Json(serde_json::json!({"etag": etag})))
}

#[derive(Deserialize)]
struct CompletedPart {
    number: i32,
    etag: String,
}

#[derive(Deserialize)]
struct CompleteRequest {
    upload_id: String,
    parts: Vec<CompletedPart>,
}

#[derive(FromRow)]
struct UploadRow {
    sha256: Vec<u8>,
    key: String,
    size_bytes: i64,
    original_name: String,
    content_type: String,
    file_modified_at: Option<OffsetDateTime>,
}

/// Record a completed upload: insert the asset (or revive the trashed one with the same content), queue its jobs and
/// forget the pending upload, atomically.
async fn register_asset(db: &sqlx::PgPool, owner: Uuid, upload_id: &str, up: &UploadRow) -> ApiResult<Uuid> {
    let mut tx = db.begin().await?;
    // A concurrent upload of the same content may have won; either way return the single asset.
    // A trashed row is revived. One whose purge was queued meanwhile is left alone (the WHERE matches nothing),
    // so the upload fails with a retryable conflict instead of reviving a half-deleted asset.
    let revived: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, file_modified_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (owner_id, sha256) DO UPDATE
            SET deleted_at = NULL, file_modified_at = coalesce(assets.file_modified_at, EXCLUDED.file_modified_at)
            WHERE assets.purging_at IS NULL
         RETURNING id",
    )
    .bind(owner)
    .bind(&up.sha256)
    .bind(&up.key)
    .bind(&up.original_name)
    .bind(&up.content_type)
    .bind(up.size_bytes)
    .bind(up.file_modified_at)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((asset_id,)) = revived else {
        return Err(purging_conflict());
    };
    // A trashed RAW + JPEG pair comes back as a whole, whichever of the two was uploaded again.
    sqlx::query(
        "UPDATE assets SET deleted_at = NULL
         WHERE deleted_at IS NOT NULL AND purging_at IS NULL
           AND (primary_id = $1 OR id = (SELECT primary_id FROM assets WHERE id = $1))",
    )
    .bind(asset_id)
    .execute(&mut *tx)
    .await?;
    for kind in jobs::DEFAULT_PIPELINE {
        jobs::enqueue(&mut *tx, kind, asset_id, 1).await?;
    }
    sqlx::query("DELETE FROM uploads WHERE upload_id = $1")
        .bind(upload_id)
        .execute(&mut *tx)
        .await?;
    // The bytes were uploaded whether the content was new or a trashed copy came back: this feeds "uploaded in the last
    // 24 hours / 30 days" (the storage counters follow the `assets` row itself, by trigger).
    sqlx::query("INSERT INTO upload_log (owner_id, bytes) VALUES ($1, $2)")
        .bind(owner)
        .bind(up.size_bytes.max(0))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(asset_id)
}

fn purging_conflict() -> ApiError {
    ApiError::Conflict("this photo is being permanently deleted; try uploading it again in a minute".into())
}

async fn complete(
    State(s): State<AppState>,
    user: ImportUser,
    Json(req): Json<CompleteRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let up: UploadRow = sqlx::query_as(
        "SELECT sha256, key, size_bytes, original_name, content_type, file_modified_at FROM uploads WHERE upload_id = $1 AND owner_id = $2",
    )
    .bind(&req.upload_id)
    .bind(user.id)
    .fetch_optional(&s.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    // Do not complete into content whose purge is running: it would delete the object we are about to store.
    refuse_if_purging(&s, user.id, &up.sha256).await?;

    let mut parts: Vec<(i32, String)> = req.parts.into_iter().map(|p| (p.number, p.etag)).collect();
    parts.sort_by_key(|p| p.0);
    // What reached storage must not add up to more than was declared (and reserved against the limits). A client that
    // lied about the size is refused before the object exists, and its parts are discarded.
    if let Some(stored) = s.storage.list_parts(&up.key, &req.upload_id).await? {
        let total: i64 = stored
            .iter()
            .filter(|p| parts.iter().any(|(n, _)| *n == p.number))
            .map(|p| p.size)
            .sum();
        if total > up.size_bytes {
            s.storage.abort_multipart(&up.key, &req.upload_id).await?;
            sqlx::query("DELETE FROM uploads WHERE upload_id = $1")
                .bind(&req.upload_id)
                .execute(&s.db)
                .await?;
            metrics::global().count("uploads.refused", 1, &[("reason", "larger_than_declared")]);
            return Err(ApiError::Limited {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                code: "upload_larger_than_declared",
                message: format!(
                    "The uploaded parts add up to {}, more than the {} that was declared. The upload was discarded.",
                    limits::human(total),
                    limits::human(up.size_bytes)
                ),
                details: serde_json::json!({"declared_bytes": up.size_bytes, "received_bytes": total}),
                retry_after_secs: None,
            });
        }
    }
    s.storage.complete_multipart(&up.key, &req.upload_id, &parts).await?;

    // Trust but verify: the stored object must be the size the client declared.
    // (Content hash is re-verified by the metadata job, which has to read the bytes anyway.)
    if s.storage.head_size(&up.key).await? != Some(up.size_bytes) {
        return Err(ApiError::BadRequest(
            "uploaded size does not match declared size".into(),
        ));
    }

    let asset_id = register_asset(&s.db, user.id, &req.upload_id, &up).await?;
    let m = metrics::global();
    let tags = [("family", metrics::family(&up.content_type, &up.original_name))];
    m.count("uploads.completed", 1, &tags);
    m.count("uploads.bytes", up.size_bytes.max(0) as u64, &tags);

    Ok((StatusCode::CREATED, Json(serde_json::json!({"asset_id": asset_id}))))
}

#[derive(Serialize, FromRow, Clone)]
struct AssetSummary {
    id: Uuid,
    original_name: String,
    content_type: String,
    size_bytes: i64,
    width: Option<i32>,
    height: Option<i32>,
    #[serde(with = "time::serde::rfc3339")]
    sort_time: OffsetDateTime,
    /// Favorited (a RAW + JPEG pair carries it on the visible photo).
    favorite: bool,
    /// Star rating 0 to 5 (0 = not rated); a RAW + JPEG pair carries it on the visible photo.
    rating: i16,
    /// In the archive: off the timeline and the map, still in albums and search (shown with a badge there).
    archived: bool,
    /// Hidden. Only the Hidden view (unlocked) ever lists such a photo, and the viewer needs the flag for "Unhide".
    hidden: bool,
    /// Rotated, straightened or cropped (`width` / `height` and the thumbnail are the edited picture's).
    edited: bool,
}

#[derive(FromRow)]
struct ListRow {
    #[sqlx(flatten)]
    summary: AssetSummary,
    thumbhash: Option<Vec<u8>>,
}

/// A list entry: the summary plus what the grid needs to draw a tile without a request per item.
#[derive(Serialize)]
struct ListItem {
    #[serde(flatten)]
    summary: AssetSummary,
    /// ThumbHash (base64) for a blurred placeholder; null until the worker has produced the derived images.
    thumbhash: Option<String>,
    /// Short-lived presigned URL of the thumbnail; null until the derived images exist.
    thumb_url: Option<String>,
    /// The RAW file stacked behind this photo (empty for a single file).
    paired: Vec<PairedFile>,
}

/// The other file of a RAW + JPEG pair, shown with its primary.
#[derive(Serialize, FromRow, Debug)]
struct PairedFile {
    id: Uuid,
    original_name: String,
    content_type: String,
    size_bytes: i64,
    /// Always `raw` today: the file hidden behind the visible primary.
    kind: &'static str,
}

#[derive(FromRow)]
struct PairedRow {
    primary_id: Uuid,
    id: Uuid,
    original_name: String,
    content_type: String,
    size_bytes: i64,
}

/// The RAW files behind the given visible photos, keyed by primary id (one query per page).
async fn paired_files(
    db: &sqlx::PgPool,
    primaries: &[Uuid],
) -> ApiResult<std::collections::HashMap<Uuid, Vec<PairedFile>>> {
    let rows: Vec<PairedRow> = sqlx::query_as(
        "SELECT primary_id, id, original_name, content_type, size_bytes FROM assets
         WHERE primary_id = ANY($1) ORDER BY id",
    )
    .bind(primaries)
    .fetch_all(db)
    .await?;
    let mut map: std::collections::HashMap<Uuid, Vec<PairedFile>> = std::collections::HashMap::new();
    for r in rows {
        map.entry(r.primary_id).or_default().push(PairedFile {
            id: r.id,
            original_name: r.original_name,
            content_type: r.content_type,
            size_bytes: r.size_bytes,
            kind: "raw",
        });
    }
    Ok(map)
}

/// `thumbhash` is only set after both derived objects are uploaded, so it also says whether the
/// thumbnail exists: returns `(thumbhash base64, thumb_url)`, both None until then.
async fn thumb_fields(
    storage: &Storage,
    owner_id: Uuid,
    id: Uuid,
    thumbhash: Option<&[u8]>,
) -> ApiResult<(Option<String>, Option<String>)> {
    match thumbhash {
        Some(h) => Ok((
            Some(B64.encode(h)),
            Some(storage.presign_get(&keys::thumb(owner_id, id)).await?),
        )),
        None => Ok((None, None)),
    }
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    /// Opaque keyset cursor from a previous `next_cursor`. Preferred over `before`.
    cursor: Option<String>,
    /// Deprecated, use `cursor`: assets strictly older than this timestamp (RFC 3339). Loses assets that share
    /// a sort time across a page boundary.
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    /// `true` lists favorites only (served by the partial index `assets_favorites_idx`). Archived favorites are listed.
    #[serde(default)]
    favorite: bool,
    /// `true` lists the archive (`assets_archived_idx`) instead of the timeline.
    #[serde(default)]
    archived: bool,
    /// `true` lists the Hidden view: refused (`403 hidden_locked`) unless this session has unlocked it.
    #[serde(default)]
    hidden: bool,
    /// `taken` (default) or `added`: order, and `sort_time`, by capture time or by upload time.
    #[serde(default)]
    sort: visibility::Sort,
    /// `west,south,east,north`: only geotagged photos inside this box (the map's viewport list; see `map`).
    bbox: Option<String>,
}

#[derive(Serialize)]
struct ListResponse {
    items: Vec<ListItem>,
    /// Pass as `cursor` to fetch the next page; null when this was the last page.
    next_cursor: Option<String>,
    /// Deprecated, use `next_cursor`. Pass as `before`; null when this was the last page.
    #[serde(with = "time::serde::rfc3339::option")]
    next_before: Option<OffsetDateTime>,
}

/// Cursor = hex of `<rfc3339 sort_time>|<uuid>`: the last row's position in `(sort_time DESC, id DESC)` order.
fn encode_cursor(t: OffsetDateTime, id: Uuid) -> String {
    let ts = t
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    keys::hex(format!("{ts}|{id}").as_bytes())
}

fn decode_cursor(c: &str) -> Option<(OffsetDateTime, Uuid)> {
    if !c.len().is_multiple_of(2) || !c.is_ascii() || c.len() > 200 {
        return None;
    }
    let bytes: Option<Vec<u8>> = c
        .as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect();
    let text = String::from_utf8(bytes?).ok()?;
    let (ts, id) = text.split_once('|')?;
    Some((
        OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339).ok()?,
        Uuid::parse_str(id).ok()?,
    ))
}

fn list_select(sort: visibility::Sort) -> String {
    format!(
        "SELECT id, original_name, content_type, size_bytes, width, height,
        {} AS sort_time, favorite, rating, archived, hidden, (recipe IS NOT NULL) AS edited, thumbhash
 FROM assets WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL",
        sort.expr()
    )
}

async fn list(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<ListResponse>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    // Keyset position: rows strictly after (ts, id) in (sort_time DESC, id DESC) order. The legacy `before` has no
    // id, so it compares against the nil uuid, which makes the predicate a strict `sort_time < before`.
    let after = match (&q.cursor, q.before) {
        (Some(c), _) => Some(decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
        (None, Some(t)) => Some((t, Uuid::nil())),
        (None, None) => None,
    };
    // Row comparison against the (owner_id, sort expression DESC, id DESC) index; two statements so the planner
    // never has to guess around an optional predicate.
    let expr = q.sort.expr();
    let mut select = format!(
        "{}{}",
        list_select(q.sort),
        visibility::view_sql(q.favorite, q.archived, q.hidden, unlock)?
    );
    if let Some(b) = &q.bbox {
        select.push_str(&map::Bbox::parse(b)?.sql());
    }
    let rows = match after {
        Some((ts, id)) => {
            sqlx::query_as::<_, ListRow>(&format!(
                "{select} AND ({expr}, id) < ($2, $3)
                 ORDER BY {expr} DESC, id DESC LIMIT $4"
            ))
            .bind(user.id)
            .bind(ts)
            .bind(id)
            .bind(limit)
            .fetch_all(&s.db)
            .await?
        }
        None => {
            sqlx::query_as::<_, ListRow>(&format!(
                "{select}
                 ORDER BY {expr} DESC, id DESC LIMIT $2"
            ))
            .bind(user.id)
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

/// Rows to list entries: thumbnail fields plus the paired RAW files.
async fn list_items(s: &AppState, owner: Uuid, rows: Vec<ListRow>) -> ApiResult<Vec<ListItem>> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.summary.id).collect();
    let mut paired = paired_files(&s.db, &ids).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        let (thumbhash, thumb_url) = thumb_fields(&s.storage, owner, r.summary.id, r.thumbhash.as_deref()).await?;
        items.push(ListItem {
            paired: paired.remove(&r.summary.id).unwrap_or_default(),
            summary: r.summary,
            thumbhash,
            thumb_url,
        });
    }
    Ok(items)
}

/// Number of live photos (a RAW + JPEG pair counts once), for `cyanotype status`.
async fn count(State(s): State<AppState>, user: ImportUser) -> ApiResult<Json<serde_json::Value>> {
    let (n,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM assets WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL")
            .bind(user.id)
            .fetch_one(&s.db)
            .await?;
    Ok(Json(serde_json::json!({"count": n})))
}

async fn get_one(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let a: AssetSummary = sqlx::query_as(
        "SELECT id, original_name, content_type, size_bytes, width, height, coalesce(taken_at, file_modified_at, created_at) AS sort_time, favorite, rating, archived, hidden, (recipe IS NOT NULL) AS edited
         FROM assets WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL AND (NOT hidden OR $3)",
    )
    .bind(id)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_optional(&s.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    #[derive(FromRow)]
    struct Own {
        original_key: String,
        owner_id: Uuid,
        thumbhash: Option<Vec<u8>>,
        recipe: Option<sqlx::types::Json<cyanotype_core::recipe::Recipe>>,
        rendering: bool,
        file_width: Option<i32>,
        file_height: Option<i32>,
    }
    let Own {
        original_key: key,
        owner_id,
        thumbhash,
        recipe,
        rendering,
        file_width,
        file_height,
    } = sqlx::query_as(
        "SELECT original_key, owner_id, thumbhash, recipe, coalesce(width_orig, width) AS file_width,
                coalesce(height_orig, height) AS file_height,
                EXISTS (SELECT 1 FROM jobs WHERE kind = 'thumbnail' AND asset_id = $1 AND state IN ('queued', 'running')) AS rendering
         FROM assets WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&s.db)
    .await?;
    let mut v = serde_json::to_value(a).map_err(anyhow::Error::from)?;
    v["albums"] = albums::containing(&s.db, user.id, id).await?;
    v["paired"] = serde_json::to_value(paired_files(&s.db, &[id]).await?.remove(&id).unwrap_or_default())
        .map_err(anyhow::Error::from)?;
    v["place"] = place_of(&s.db, id).await?;
    v["tags"] = serde_json::to_value(tags::of_photo(&s.db, user.id, id).await?).map_err(anyhow::Error::from)?;
    let caption: Option<String> = sqlx::query_scalar("SELECT caption FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(&s.db)
        .await?;
    v["caption"] = caption.into();
    v["original_url"] = s.storage.presign_get(&key).await?.into();
    // The worker sets `thumbhash` only after both derived objects are uploaded, so it doubles
    // as the "derived images exist" flag. Until then the URLs are null rather than 404s.
    let (hash, thumb_url) = thumb_fields(&s.storage, owner_id, id, thumbhash.as_deref()).await?;
    v["thumbhash"] = hash.into();
    v["thumb_url"] = thumb_url.into();
    v["preview_url"] = if thumbhash.is_some() {
        s.storage.presign_get(&keys::preview(owner_id, id)).await?.into()
    } else {
        serde_json::Value::Null
    };
    // Rotate and crop: the recipe, whether the worker is still rendering it, and (once it has) the whole picture the editor
    // works on, since the preview is already cropped.
    v["recipe"] = serde_json::to_value(recipe.as_ref().map(|j| j.0)).map_err(anyhow::Error::from)?;
    v["rendering"] = rendering.into();
    // The file's own size (`width` / `height` are the edited picture's while there is a recipe).
    v["file_width"] = file_width.into();
    v["file_height"] = file_height.into();
    v["edit_base_url"] = if recipe.is_some() && !rendering && thumbhash.is_some() {
        s.storage.presign_get(&keys::edit_base(owner_id, id)).await?.into()
    } else {
        serde_json::Value::Null
    };
    Ok(Json(v))
}

/// Where the photo was taken (`city`, `region`, `country`, `country_code`), or null: no coordinates, or nothing
/// close enough in the places dataset (docs/library.md, "Places").
async fn place_of(db: &sqlx::PgPool, id: Uuid) -> ApiResult<serde_json::Value> {
    #[derive(FromRow)]
    struct PlaceRow {
        place_city: Option<String>,
        place_region: Option<String>,
        place_country: Option<String>,
        place_country_code: Option<String>,
    }
    let row: Option<PlaceRow> =
        sqlx::query_as("SELECT place_city, place_region, place_country, place_country_code FROM assets WHERE id = $1")
            .bind(id)
            .fetch_optional(db)
            .await?;
    Ok(match row {
        Some(PlaceRow {
            place_city: Some(city),
            place_region: region,
            place_country: Some(country),
            place_country_code: code,
        }) => serde_json::json!({"city": city, "region": region, "country": country, "country_code": code}),
        _ => serde_json::Value::Null,
    })
}

/// Move one photo (and its paired RAW) to the trash (`POST /assets/trash` does the same for many). Permanent
/// deletion happens later, see `trash` and `purge`.
async fn remove(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    if trash::apply(&s.db, user.id, trash::Op::Trash, Some(&[id]))
        .await?
        .photos
        == 0
    {
        Err(ApiError::NotFound)
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}

/// Separate a RAW + JPEG pair into two independent photos; they are not paired again automatically. `id` is either
/// file. `404` when it is not part of a pair.
async fn unpair(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    if pairing::unpair(&s.db, user.id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    //! DB-backed tests need `DATABASE_URL` (role must be able to CREATE DATABASE) and are skipped without it.
    use axum::{
        body::Body,
        http::{Request, header},
    };
    use cyanotype_core::{config::S3Config, db, storage::Storage};
    use sqlx::{Connection, Executor, PgConnection, PgPool};
    use tower::ServiceExt;

    use super::*;

    fn part(number: i32, size: i64) -> UploadedPart {
        UploadedPart {
            number,
            etag: format!("\"e{number}\""),
            size,
        }
    }

    #[test]
    fn part_sizes_are_validated_for_resume() {
        let ps = 16;
        assert_eq!(expected_part_size(1, 40, ps), 16);
        assert_eq!(expected_part_size(2, 40, ps), 16);
        assert_eq!(expected_part_size(3, 40, ps), 8);
        assert_eq!(expected_part_size(4, 40, ps), 0);
        // Wrong-sized, out-of-range and duplicate parts are dropped / de-duplicated; the rest is kept.
        let got = usable_parts(
            vec![
                part(1, 16),
                part(2, 15),
                part(3, 8),
                part(3, 8),
                part(4, 8),
                part(0, 16),
            ],
            40,
            ps,
        );
        assert_eq!(got.iter().map(|p| p.number).collect::<Vec<_>>(), vec![1, 3]);
    }

    #[test]
    fn cursors_round_trip_and_reject_garbage() {
        let t = OffsetDateTime::parse(
            "2020-06-01T12:34:56.123456Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let id = Uuid::new_v4();
        assert_eq!(decode_cursor(&encode_cursor(t, id)), Some((t, id)));
        for bad in ["", "zz", "abc", "6e6f7069706573", "ff", &"00".repeat(150)] {
            assert!(decode_cursor(bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn cursor_pagination_has_no_gaps_or_dupes_when_sort_times_tie() {
        let Some(t) = TestDb::new().await else { return };
        let (uid,): (Uuid,) =
            sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ('c@example.com','x') RETURNING id")
                .fetch_one(&t.pool)
                .await
                .unwrap();
        let key = "cyk_cursor";
        sqlx::query("INSERT INTO api_keys (user_id, name, key_hash) VALUES ($1,'t',$2)")
            .bind(uid)
            .bind(cyanotype_service::auth::sha256(key.as_bytes()))
            .execute(&t.pool)
            .await
            .unwrap();
        // 23 burst shots with the identical taken_at, 9 files with the identical mtime, 3 distinct ones.
        for i in 0..35 {
            let (taken, mtime) = match i {
                0..=22 => (Some("2021-05-05T10:00:00Z"), None),
                23..=31 => (None, Some("2018-01-01T00:00:00Z")),
                _ => (
                    Some(["2022-01-01T00:00:00Z", "2019-01-01T00:00:00Z", "2023-01-01T00:00:00Z"][i - 32]),
                    None,
                ),
            };
            let name = format!("f{i}.jpg");
            sqlx::query(
                "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, taken_at, file_modified_at)
                 VALUES ($1, sha256($2::bytea), $3, $3, 'image/jpeg', 1, $4::timestamptz, $5::timestamptz)",
            )
            .bind(uid)
            .bind(name.as_bytes())
            .bind(&name)
            .bind(taken)
            .bind(mtime)
            .execute(&t.pool)
            .await
            .unwrap();
        }
        let app = routes().with_state(crate::AppState {
            db: t.pool.clone(),
            storage: dummy_storage(),
            trash_retention_days: 30,
            purge_wake: Default::default(),
            text: std::sync::Arc::new(crate::testutil::FakeText::default()),
            search: crate::testutil::default_search(),
            faces_enabled: false,
        });

        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let path = match &cursor {
                Some(c) => format!("/assets?limit=7&cursor={c}"),
                None => "/assets?limit=7".to_string(),
            };
            let page = get_json(&app, &path, key).await;
            pages += 1;
            for i in page["items"].as_array().unwrap() {
                seen.push(i["id"].as_str().unwrap().to_string());
            }
            match page["next_cursor"].as_str() {
                Some(c) => cursor = Some(c.to_string()),
                None => break,
            }
            assert!(pages < 20, "runaway pagination");
        }
        assert!(pages >= 3, "{pages} pages");
        assert_eq!(seen.len(), 35, "every asset exactly once");
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(unique.len(), 35, "no duplicates");

        // Same set as one big page, in the same order.
        let all = get_json(&app, "/assets?limit=500", key).await;
        let all: Vec<_> = all["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(all, seen);

        // Invalid cursors are a 400, not a silent first page.
        for bad in ["nonsense", "6e6f7069706573", ""] {
            let req = Request::builder()
                .uri(format!("/assets?cursor={bad}"))
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "cursor={bad:?}");
        }

        // The deprecated `before` is still accepted and still reports next_before.
        let old = get_json(&app, "/assets?limit=3", key).await;
        assert!(old["next_before"].is_string());
        t.teardown().await;
    }

    #[test]
    fn part_validation_covers_number_length_and_size() {
        let ps = 16;
        // 40 bytes in 16-byte parts: 16, 16, 8.
        assert_eq!(validate_part(1, 40, ps, Some(16)).unwrap(), 16);
        assert_eq!(validate_part(3, 40, ps, Some(8)).unwrap(), 8);
        for (n, len) in [(0, 16), (4, 8), (-1, 16)] {
            assert!(matches!(
                validate_part(n, 40, ps, Some(len)),
                Err(ApiError::BadRequest(_))
            ));
        }
        assert!(matches!(validate_part(1, 40, ps, None), Err(ApiError::BadRequest(_))));
        assert!(matches!(
            validate_part(1, 40, ps, Some(15)),
            Err(ApiError::BadRequest(_))
        ));
        assert!(matches!(
            validate_part(3, 40, ps, Some(16)),
            Err(ApiError::BadRequest(_))
        ));
        assert!(matches!(
            validate_part(1, 40, ps, Some(17)),
            Err(ApiError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn proxy_urls_percent_encode_the_upload_id() {
        assert_eq!(
            proxy_part_url("abc-DEF_1.2~", 3),
            "/assets/uploads/abc-DEF_1.2~/parts/3"
        );
        assert_eq!(
            proxy_part_url("a/b c?d#e%f+g==", 1),
            "/assets/uploads/a%2Fb%20c%3Fd%23e%25f%2Bg%3D%3D/parts/1"
        );
    }

    #[test]
    fn transport_defaults_to_direct() {
        let base = r#""sha256":"00","size_bytes":1,"filename":"a.jpg""#;
        let d: CheckRequest = serde_json::from_str(&format!("{{{base}}}")).unwrap();
        assert_eq!(d.transport, Transport::Direct);
        let p: CheckRequest = serde_json::from_str(&format!(r#"{{{base},"transport":"proxy"}}"#)).unwrap();
        assert_eq!(p.transport, Transport::Proxy);
        assert!(serde_json::from_str::<CheckRequest>(&format!(r#"{{{base},"transport":"x"}}"#)).is_err());
    }

    async fn put(app: &Router, path: &str, key: Option<&str>, body: Vec<u8>) -> StatusCode {
        let mut b = Request::builder()
            .method("PUT")
            .uri(path)
            .header(header::CONTENT_LENGTH, body.len());
        if let Some(k) = key {
            b = b.header(header::AUTHORIZATION, format!("Bearer {k}"));
        }
        app.clone()
            .oneshot(b.body(Body::from(body)).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn put_part_rejects_bad_requests_before_touching_storage() {
        let Some(t) = TestDb::new().await else { return };
        let mut keys = Vec::new();
        let mut uids = Vec::new();
        for (email, key) in [("a@example.com", "cyk_a"), ("b@example.com", "cyk_b")] {
            let (uid,): (Uuid,) =
                sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ($1,'x') RETURNING id")
                    .bind(email)
                    .fetch_one(&t.pool)
                    .await
                    .unwrap();
            sqlx::query("INSERT INTO api_keys (user_id, name, key_hash) VALUES ($1,'t',$2)")
                .bind(uid)
                .bind(cyanotype_service::auth::sha256(key.as_bytes()))
                .execute(&t.pool)
                .await
                .unwrap();
            keys.push(key);
            uids.push(uid);
        }
        // 20 MiB: parts of 16 MiB and 4 MiB. Odd upload id to exercise percent-decoding.
        let upload_id = "id/with space+odd";
        sqlx::query(
            "INSERT INTO uploads (upload_id, owner_id, sha256, key, size_bytes, part_size, original_name, content_type)
             VALUES ($1,$2,sha256('x'::bytea),'k',$3,$4,'a.jpg','image/jpeg')",
        )
        .bind(upload_id)
        .bind(uids[0])
        .bind(20 * 1024 * 1024_i64)
        .bind(PART_SIZE)
        .execute(&t.pool)
        .await
        .unwrap();
        let app = routes().with_state(crate::AppState {
            db: t.pool.clone(),
            storage: dummy_storage(),
            trash_retention_days: 30,
            purge_wake: Default::default(),
            text: std::sync::Arc::new(crate::testutil::FakeText::default()),
            search: crate::testutil::default_search(),
            faces_enabled: false,
        });
        let url = |n: i32| proxy_part_url(upload_id, n);

        assert_eq!(put(&app, &url(1), None, vec![0; 4]).await, StatusCode::UNAUTHORIZED);
        assert_eq!(
            put(&app, &proxy_part_url("nope", 1), Some(keys[0]), vec![0; 4]).await,
            StatusCode::NOT_FOUND
        );
        // Another user's upload looks the same as a missing one.
        assert_eq!(
            put(&app, &url(1), Some(keys[1]), vec![0; 4]).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            put(&app, &url(0), Some(keys[0]), vec![0; 4]).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            put(&app, &url(3), Some(keys[0]), vec![0; 4]).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            put(&app, &url(1), Some(keys[0]), vec![0; 4]).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            put(&app, &url(2), Some(keys[0]), vec![0; 5]).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            put(&app, &url(2), Some(keys[0]), vec![0; PART_SIZE as usize + 1]).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        // Without a Content-Length header (e.g. chunked) is refused.
        let req = Request::builder()
            .method("PUT")
            .uri(url(2))
            .header(header::AUTHORIZATION, "Bearer cyk_a")
            .body(Body::from(vec![0u8; 4 * 1024 * 1024]))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        // A correct part reaches storage, which is unreachable here: 502, not 500.
        assert_eq!(
            put(&app, &url(2), Some(keys[0]), vec![0; 4 * 1024 * 1024]).await,
            StatusCode::BAD_GATEWAY
        );
        t.teardown().await;
    }

    fn upload_for(owner: Uuid, name: &str) -> UploadRow {
        UploadRow {
            sha256: cyanotype_service::auth::sha256(name.as_bytes()),
            key: format!("originals/{owner}/{name}"),
            size_bytes: 1,
            original_name: name.into(),
            content_type: "image/jpeg".into(),
            file_modified_at: None,
        }
    }

    #[tokio::test]
    async fn reuploading_a_trashed_photo_revives_it() {
        use crate::testutil::{TestDb, asset, user};
        let Some(t) = TestDb::new().await else { return };
        let (uid, _) = user(&t.pool, "r@example.com").await;
        let id = asset(&t.pool, uid, "again.jpg").await;
        sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
            .bind(id)
            .execute(&t.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO uploads (upload_id, owner_id, sha256, key, size_bytes, part_size, original_name, content_type)
             VALUES ('u1', $1, sha256('again.jpg'::bytea), 'k', 1, 1, 'again.jpg', 'image/jpeg')",
        )
        .bind(uid)
        .execute(&t.pool)
        .await
        .unwrap();

        let revived = register_asset(&t.pool, uid, "u1", &upload_for(uid, "again.jpg"))
            .await
            .unwrap();
        assert_eq!(revived, id, "same row, not a duplicate");
        let (deleted, purging): (Option<OffsetDateTime>, Option<OffsetDateTime>) =
            sqlx::query_as("SELECT deleted_at, purging_at FROM assets WHERE id = $1")
                .bind(id)
                .fetch_one(&t.pool)
                .await
                .unwrap();
        assert!(deleted.is_none() && purging.is_none());
        let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM uploads")
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert_eq!(pending, 0);
        t.teardown().await;
    }

    #[tokio::test]
    async fn a_photo_mid_purge_is_never_revived_but_can_be_uploaded_once_gone() {
        use crate::testutil::{FakeObjects, TestDb, app, asset, send, user};
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "p@example.com").await;
        let id = asset(&t.pool, uid, "doomed.jpg").await;
        let app = app(&t.pool);
        let check = serde_json::json!({
            "sha256": keys::hex(&cyanotype_service::auth::sha256(b"doomed.jpg")),
            "size_bytes": 1, "filename": "doomed.jpg"
        });

        // In the trash (purge not started): check goes on to start an upload (storage is a dummy here, so it
        // fails with a 500, but never with the purge conflict).
        sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
            .bind(id)
            .execute(&t.pool)
            .await
            .unwrap();
        let (st, _) = send(&app, "POST", "/assets/check", &key, Some(check.clone())).await;
        assert_ne!(st, StatusCode::CONFLICT);

        // Purge queued: check refuses with a retryable 409 before touching storage...
        sqlx::query("UPDATE assets SET purging_at = now() WHERE id = $1")
            .bind(id)
            .execute(&t.pool)
            .await
            .unwrap();
        let (st, body) = send(&app, "POST", "/assets/check", &key, Some(check.clone())).await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert!(
            body["error"].as_str().unwrap().contains("try uploading it again"),
            "{body}"
        );
        // ...and a upload that was already in flight cannot complete into the dying row.
        let err = register_asset(&t.pool, uid, "u1", &upload_for(uid, "doomed.jpg"))
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Conflict(_)));
        let (deleted, purging): (Option<OffsetDateTime>, Option<OffsetDateTime>) =
            sqlx::query_as("SELECT deleted_at, purging_at FROM assets WHERE id = $1")
                .bind(id)
                .fetch_one(&t.pool)
                .await
                .unwrap();
        assert!(deleted.is_some() && purging.is_some(), "row left exactly as it was");

        // Once the sweep has finished, the same content is a brand new asset.
        crate::purge::sweep(&t.pool, &FakeObjects::default()).await.unwrap();
        let fresh = register_asset(&t.pool, uid, "u1", &upload_for(uid, "doomed.jpg"))
            .await
            .unwrap();
        assert_ne!(fresh, id);
        let (st, resp) = send(&app, "POST", "/assets/check", &key, Some(check)).await;
        assert_eq!((st, resp["status"].as_str()), (StatusCode::OK, Some("exists")));
        t.teardown().await;
    }

    struct TestDb {
        pool: PgPool,
        admin_url: String,
        name: String,
    }

    impl TestDb {
        async fn new() -> Option<Self> {
            let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
            let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
            let mut admin = PgConnection::connect(&admin_url).await.unwrap();
            admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
            let (base, _) = admin_url.rsplit_once('/').unwrap();
            let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
            db::migrate(&pool).await.unwrap();
            Some(Self { pool, admin_url, name })
        }

        async fn teardown(self) {
            self.pool.close().await;
            let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
            admin
                .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str())
                .await
                .unwrap();
        }
    }

    fn dummy_storage() -> Storage {
        Storage::new(&S3Config {
            endpoint: Some("http://127.0.0.1:1".into()),
            public_endpoint: Some("http://127.0.0.1:1".into()),
            region: "us-east-1".into(),
            bucket: "b".into(),
            access_key: "k".into(),
            secret_key: "s".into(),
            force_path_style: true,
            presign_ttl_secs: 60,
        })
    }

    async fn get_json(app: &Router, path: &str, key: &str) -> serde_json::Value {
        let req = Request::builder()
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path}");
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn timeline_falls_back_to_file_modified_at_when_taken_at_is_missing() {
        let Some(t) = TestDb::new().await else { return };
        let (uid,): (Uuid,) =
            sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ('t@example.com','x') RETURNING id")
                .fetch_one(&t.pool)
                .await
                .unwrap();
        let key = "cyk_test";
        sqlx::query("INSERT INTO api_keys (user_id, name, key_hash) VALUES ($1,'t',$2)")
            .bind(uid)
            .bind(cyanotype_service::auth::sha256(key.as_bytes()))
            .execute(&t.pool)
            .await
            .unwrap();
        // exif: has taken_at (2020); mtime: only a file mtime (2015); none: neither (sorts by created_at = now).
        for (name, taken, mtime) in [
            ("exif.jpg", Some("2020-06-01T00:00:00Z"), Some("2024-01-01T00:00:00Z")),
            ("mtime.jpg", None, Some("2015-03-04T05:06:07Z")),
            ("none.jpg", None, None),
        ] {
            sqlx::query(
                "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, taken_at, file_modified_at)
                 VALUES ($1, sha256($2::bytea), $3, $3, 'image/jpeg', 1, $4::timestamptz, $5::timestamptz)",
            )
            .bind(uid)
            .bind(name.as_bytes())
            .bind(name)
            .bind(taken)
            .bind(mtime)
            .execute(&t.pool)
            .await
            .unwrap();
        }
        let app = routes().with_state(crate::AppState {
            db: t.pool.clone(),
            storage: dummy_storage(),
            trash_retention_days: 30,
            purge_wake: Default::default(),
            text: std::sync::Arc::new(crate::testutil::FakeText::default()),
            search: crate::testutil::default_search(),
            faces_enabled: false,
        });

        let list = get_json(&app, "/assets", key).await;
        let items = list["items"].as_array().unwrap();
        let order: Vec<_> = items.iter().map(|i| i["original_name"].as_str().unwrap()).collect();
        assert_eq!(order, ["none.jpg", "exif.jpg", "mtime.jpg"]);
        // EXIF taken_at wins over mtime; mtime wins over upload time.
        assert!(items[1]["sort_time"].as_str().unwrap().starts_with("2020-06-01"));
        assert!(
            items[2]["sort_time"]
                .as_str()
                .unwrap()
                .starts_with("2015-03-04T05:06:07")
        );

        // Keyset pagination uses the same expression.
        let page = get_json(&app, "/assets?before=2016-01-01T00:00:00Z", key).await;
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["original_name"], "mtime.jpg");

        assert_eq!(get_json(&app, "/assets/count", key).await["count"], 3);

        // The single-asset view agrees.
        let id = items[2]["id"].as_str().unwrap();
        let one = get_json(&app, &format!("/assets/{id}"), key).await;
        assert!(one["sort_time"].as_str().unwrap().starts_with("2015-03-04T05:06:07"));
        t.teardown().await;
    }

    fn storage() -> Storage {
        Storage::new(&S3Config {
            endpoint: Some("http://localhost:9000".into()),
            public_endpoint: Some("http://localhost:9000".into()),
            region: "us-east-1".into(),
            bucket: "cyanotype".into(),
            access_key: "k".into(),
            secret_key: "s".into(),
            force_path_style: true,
            presign_ttl_secs: 300,
        })
    }

    #[tokio::test]
    async fn thumb_fields_are_null_until_derived_images_exist() {
        let (h, u) = thumb_fields(&storage(), Uuid::new_v4(), Uuid::new_v4(), None)
            .await
            .unwrap();
        assert!(h.is_none() && u.is_none());
    }

    #[tokio::test]
    async fn thumb_fields_give_base64_thumbhash_and_presigned_thumb_url() {
        let (owner, id) = (Uuid::new_v4(), Uuid::new_v4());
        let raw = [0x01u8, 0xfe, 0x80, 0x7f, 0xff];
        let (h, u) = thumb_fields(&storage(), owner, id, Some(&raw)).await.unwrap();
        assert_eq!(B64.decode(h.unwrap()).unwrap(), raw);
        let u = u.unwrap();
        assert!(u.contains(&keys::thumb(owner, id)), "{u}");
        assert!(u.contains("X-Amz-Signature="), "{u}");
        assert!(u.contains("X-Amz-Expires=300"), "{u}");
    }

    #[test]
    fn list_item_json_flattens_summary_and_adds_thumb_fields() {
        let item = ListItem {
            summary: AssetSummary {
                id: Uuid::nil(),
                original_name: "a.png".into(),
                content_type: "image/png".into(),
                size_bytes: 3,
                width: Some(4),
                height: None,
                sort_time: OffsetDateTime::UNIX_EPOCH,
                favorite: false,
                rating: 0,
                archived: false,
                hidden: false,
                edited: false,
            },
            thumbhash: Some("AQ==".into()),
            thumb_url: None,
            paired: vec![],
        };
        let v = serde_json::to_value(item).unwrap();
        assert_eq!(v["original_name"], "a.png");
        assert_eq!(v["sort_time"], "1970-01-01T00:00:00Z");
        assert_eq!(v["thumbhash"], "AQ==");
        assert!(v["thumb_url"].is_null() && v.get("thumb_url").is_some());
    }

    /// A request body that yields scripted frames, then either ends or fails like a connection that went away.
    struct ScriptedBody {
        chunks: std::collections::VecDeque<Bytes>,
        then_fail: bool,
    }

    impl http_body::Body for ScriptedBody {
        type Data = Bytes;
        type Error = std::io::Error;

        fn poll_frame(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<http_body::Frame<Bytes>, std::io::Error>>> {
            std::task::Poll::Ready(match self.chunks.pop_front() {
                Some(c) => Some(Ok(http_body::Frame::data(c))),
                None if self.then_fail => {
                    self.then_fail = false;
                    Some(Err(std::io::Error::other("connection reset")))
                }
                None => None,
            })
        }
    }

    fn scripted(chunks: &[&'static [u8]], then_fail: bool) -> axum::extract::Request {
        let body = ScriptedBody {
            chunks: chunks.iter().map(|c| Bytes::from_static(c)).collect(),
            then_fail,
        };
        Request::builder().body(Body::new(body)).unwrap()
    }

    /// Run `read_part_body` and classify what it returned, as `put_part` does.
    async fn read_and_classify(req: axum::extract::Request, expected: usize) -> (PartFailure, PartProgress) {
        let mut progress = PartProgress::default();
        let err = read_part_body(req, expected, &mut progress).await.unwrap_err();
        (PartFailure::classify(&err, &progress), progress)
    }

    #[tokio::test]
    async fn a_body_that_breaks_mid_part_is_a_client_abort() {
        let (reason, p) = read_and_classify(scripted(&[b"abc", b"de"], true), 10).await;
        assert_eq!(reason, PartFailure::ClientAborted);
        assert_eq!((p.received, p.expected), (5, Some(10)));
    }

    #[tokio::test]
    async fn a_body_that_ends_early_is_too_short() {
        let (reason, p) = read_and_classify(scripted(&[b"abc"], false), 10).await;
        assert_eq!(reason, PartFailure::BodyTooShort);
        assert_eq!((p.received, p.expected), (3, Some(10)));
        // Nothing at all before the end is still a short body, not an abort.
        assert_eq!(
            read_and_classify(scripted(&[], false), 10).await.0,
            PartFailure::BodyTooShort
        );
    }

    #[tokio::test]
    async fn a_body_that_is_too_long_or_complete_is_not_a_short_body() {
        let (reason, _) = read_and_classify(scripted(&[b"abcdef"], false), 4).await;
        assert_eq!(reason, PartFailure::Other);
        let mut progress = PartProgress::default();
        let got = read_part_body(scripted(&[b"ab", b"cd"], false), 4, &mut progress)
            .await
            .unwrap();
        assert_eq!((&got[..], progress.received), (&b"abcd"[..], 4));
    }

    #[test]
    fn part_failures_are_classified_by_stage_and_error() {
        let bad = || ApiError::BadRequest("x".into());
        let at = |stage, received, expected, body_errored| PartProgress {
            stage,
            received,
            expected,
            body_errored,
        };
        let c = PartFailure::classify;

        // Before the body is touched: validation, ownership, unknown upload.
        assert_eq!(c(&bad(), &PartProgress::default()), PartFailure::Other);
        assert_eq!(c(&ApiError::NotFound, &PartProgress::default()), PartFailure::Other);
        // While receiving.
        assert_eq!(
            c(&bad(), &at(PartStage::Receiving, 5, Some(10), true)),
            PartFailure::ClientAborted
        );
        assert_eq!(
            c(&bad(), &at(PartStage::Receiving, 5, Some(10), false)),
            PartFailure::BodyTooShort
        );
        assert_eq!(
            c(&bad(), &at(PartStage::Receiving, 10, Some(10), false)),
            PartFailure::Other
        );
        assert_eq!(
            c(
                &ApiError::RequestTimeout("t".into()),
                &at(PartStage::Receiving, 5, Some(10), false)
            ),
            PartFailure::Timeout
        );
        // Relaying to the bucket: only the gateway error is a storage error.
        let storing = at(PartStage::Storing, 10, Some(10), false);
        assert_eq!(
            c(&ApiError::BadGateway("s".into()), &storing),
            PartFailure::StorageError
        );
        assert_eq!(c(&bad(), &storing), PartFailure::Other);
        // After it, a bad gateway can only be something else.
        let recording = at(PartStage::Recording, 10, Some(10), false);
        assert_eq!(c(&ApiError::BadGateway("s".into()), &recording), PartFailure::Other);
    }

    #[test]
    fn failure_reasons_are_a_fixed_set_of_tag_safe_names() {
        let all = [
            PartFailure::ClientAborted,
            PartFailure::BodyTooShort,
            PartFailure::Timeout,
            PartFailure::StorageError,
            PartFailure::Other,
        ];
        let names: Vec<_> = all.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            names,
            ["client_aborted", "body_too_short", "timeout", "storage_error", "other"]
        );
    }
}
