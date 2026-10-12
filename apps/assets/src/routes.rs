//! Upload flow: `check` (dedupe + presign) -> client PUTs parts straight to S3 -> `complete`.
//! Bytes never pass through the API.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use cyanotype_core::{jobs, keys, storage::Storage};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser};

const PART_SIZE: i64 = 16 * 1024 * 1024;
const MAX_PARTS: i64 = 10_000; // S3 limit

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/assets/check", post(check))
        .route("/assets/complete", post(complete))
        .route("/assets", get(list))
        .route("/assets/{id}", get(get_one))
        .route("/assets/{id}", delete(remove))
}

#[derive(Deserialize)]
struct CheckRequest {
    sha256: String,
    size_bytes: i64,
    filename: String,
    content_type: Option<String>,
}

#[derive(Serialize)]
struct PartUrl {
    number: i32,
    url: String,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum CheckResponse {
    Exists {
        asset_id: Uuid,
    },
    Upload {
        upload_id: String,
        part_size: i64,
        parts: Vec<PartUrl>,
    },
}

async fn check(
    State(s): State<AppState>,
    user: AuthUser,
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
    if let Some((asset_id,)) = existing {
        return Ok(Json(CheckResponse::Exists { asset_id }));
    }

    let content_type = req.content_type.unwrap_or_else(|| "application/octet-stream".into());
    let key = keys::original(user.id, &sha, &keys::sanitize_ext(&req.filename));
    let upload_id = s.storage.create_multipart(&key, &content_type).await?;
    let n_parts = ((req.size_bytes + PART_SIZE - 1) / PART_SIZE) as i32;
    let parts = s.storage.presign_parts(&key, &upload_id, n_parts).await?;

    sqlx::query(
        "INSERT INTO uploads (upload_id, owner_id, sha256, key, size_bytes, part_size, original_name, content_type)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(&upload_id)
    .bind(user.id)
    .bind(&sha[..])
    .bind(&key)
    .bind(req.size_bytes)
    .bind(PART_SIZE)
    .bind(&req.filename)
    .bind(&content_type)
    .execute(&s.db)
    .await?;

    Ok(Json(CheckResponse::Upload {
        upload_id,
        part_size: PART_SIZE,
        parts: parts
            .into_iter()
            .map(|p| PartUrl {
                number: p.number,
                url: p.url,
            })
            .collect(),
    }))
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
}

async fn complete(
    State(s): State<AppState>,
    user: AuthUser,
    Json(req): Json<CompleteRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let up: UploadRow = sqlx::query_as(
        "SELECT sha256, key, size_bytes, original_name, content_type FROM uploads WHERE upload_id = $1 AND owner_id = $2",
    )
    .bind(&req.upload_id)
    .bind(user.id)
    .fetch_optional(&s.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    let mut parts: Vec<(i32, String)> = req.parts.into_iter().map(|p| (p.number, p.etag)).collect();
    parts.sort_by_key(|p| p.0);
    s.storage.complete_multipart(&up.key, &req.upload_id, &parts).await?;

    // Trust but verify: the stored object must be the size the client declared.
    // (Content hash is re-verified by the metadata job, which has to read the bytes anyway.)
    if s.storage.head_size(&up.key).await? != Some(up.size_bytes) {
        return Err(ApiError::BadRequest(
            "uploaded size does not match declared size".into(),
        ));
    }

    let mut tx = s.db.begin().await?;
    // A concurrent upload of the same content may have won; either way return the single asset.
    let (asset_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
         VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (owner_id, sha256) DO UPDATE SET deleted_at = NULL
         RETURNING id",
    )
    .bind(user.id)
    .bind(&up.sha256)
    .bind(&up.key)
    .bind(&up.original_name)
    .bind(&up.content_type)
    .bind(up.size_bytes)
    .fetch_one(&mut *tx)
    .await?;
    for kind in jobs::DEFAULT_PIPELINE {
        jobs::enqueue(&mut *tx, kind, asset_id, 1).await?;
    }
    sqlx::query("DELETE FROM uploads WHERE upload_id = $1")
        .bind(&req.upload_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(serde_json::json!({"asset_id": asset_id}))))
}

#[derive(Serialize, FromRow)]
struct AssetSummary {
    id: Uuid,
    original_name: String,
    content_type: String,
    size_bytes: i64,
    width: Option<i32>,
    height: Option<i32>,
    #[serde(with = "time::serde::rfc3339")]
    sort_time: OffsetDateTime,
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
    /// Return assets strictly older than this timestamp (RFC 3339).
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
}

#[derive(Serialize)]
struct ListResponse {
    items: Vec<ListItem>,
    /// Pass as `before` to fetch the next page; null when this was the last page.
    #[serde(with = "time::serde::rfc3339::option")]
    next_before: Option<OffsetDateTime>,
}

async fn list(State(s): State<AppState>, user: AuthUser, Query(q): Query<ListQuery>) -> ApiResult<Json<ListResponse>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let rows = sqlx::query_as::<_, ListRow>(
        "SELECT id, original_name, content_type, size_bytes, width, height,
                coalesce(taken_at, created_at) AS sort_time, thumbhash
         FROM assets
         WHERE owner_id = $1 AND deleted_at IS NULL
           AND ($2::timestamptz IS NULL OR coalesce(taken_at, created_at) < $2)
         ORDER BY coalesce(taken_at, created_at) DESC, id
         LIMIT $3",
    )
    .bind(user.id)
    .bind(q.before)
    .bind(limit)
    .fetch_all(&s.db)
    .await?;
    let next_before = if rows.len() as i64 == limit {
        rows.last().map(|r| r.summary.sort_time)
    } else {
        None
    };
    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        let (thumbhash, thumb_url) = thumb_fields(&s.storage, user.id, r.summary.id, r.thumbhash.as_deref()).await?;
        items.push(ListItem {
            summary: r.summary,
            thumbhash,
            thumb_url,
        });
    }
    Ok(Json(ListResponse { items, next_before }))
}

async fn get_one(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let a: AssetSummary = sqlx::query_as(
        "SELECT id, original_name, content_type, size_bytes, width, height, coalesce(taken_at, created_at) AS sort_time
         FROM assets WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&s.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let (key, owner_id, thumbhash): (String, Uuid, Option<Vec<u8>>) =
        sqlx::query_as("SELECT original_key, owner_id, thumbhash FROM assets WHERE id = $1")
            .bind(id)
            .fetch_one(&s.db)
            .await?;
    let mut v = serde_json::to_value(a).map_err(anyhow::Error::from)?;
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
    Ok(Json(v))
}

/// Soft delete; the original stays in the bucket (versioned) until a purge job exists.
async fn remove(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let n = sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL")
        .bind(id)
        .bind(user.id)
        .execute(&s.db)
        .await?
        .rows_affected();
    if n == 0 {
        Err(ApiError::NotFound)
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}

#[cfg(test)]
mod tests {
    use cyanotype_core::config::S3Config;

    use super::*;

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
            },
            thumbhash: Some("AQ==".into()),
            thumb_url: None,
        };
        let v = serde_json::to_value(item).unwrap();
        assert_eq!(v["original_name"], "a.png");
        assert_eq!(v["sort_time"], "1970-01-01T00:00:00Z");
        assert_eq!(v["thumbhash"], "AQ==");
        assert!(v["thumb_url"].is_null() && v.get("thumb_url").is_some());
    }
}
