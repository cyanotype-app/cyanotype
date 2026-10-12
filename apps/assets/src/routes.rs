//! Upload flow: `check` (dedupe + presign) -> client PUTs parts straight to S3 -> `complete`.
//! Bytes never pass through the API.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use cyanotype_core::{jobs, keys, storage::UploadedPart};
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
        .route("/assets/count", get(count))
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
async fn resume_upload(s: &AppState, owner: Uuid, sha: &[u8], req: &CheckRequest) -> ApiResult<Option<CheckResponse>> {
    let Some(p) = sqlx::query_as::<_, PendingUpload>(
        "SELECT upload_id, key, part_size FROM uploads
         WHERE owner_id = $1 AND sha256 = $2 AND size_bytes = $3 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(owner)
    .bind(sha)
    .bind(req.size_bytes)
    .fetch_optional(&s.db)
    .await?
    else {
        return Ok(None);
    };
    let Some(found) = s.storage.list_parts(&p.key, &p.upload_id).await? else {
        // S3 forgot the upload (aborted or expired): drop the stale row and start over.
        sqlx::query("DELETE FROM uploads WHERE upload_id = $1")
            .bind(&p.upload_id)
            .execute(&s.db)
            .await?;
        return Ok(None);
    };
    let done = usable_parts(found, req.size_bytes, p.part_size);
    let n_parts = ((req.size_bytes + p.part_size - 1) / p.part_size) as i32;
    let missing: Vec<i32> = (1..=n_parts).filter(|n| !done.iter().any(|d| d.number == *n)).collect();
    let parts = s.storage.presign_parts(&p.key, &p.upload_id, missing).await?;
    // Keep an upload that is being actively resumed away from the stale-upload janitor.
    sqlx::query(
        "UPDATE uploads SET created_at = now(), file_modified_at = coalesce($2, file_modified_at) WHERE upload_id = $1",
    )
    .bind(&p.upload_id)
    .bind(req.file_modified_at)
    .execute(&s.db)
    .await?;
    Ok(Some(CheckResponse::Upload {
        upload_id: p.upload_id,
        part_size: p.part_size,
        parts: parts
            .into_iter()
            .map(|p| PartUrl {
                number: p.number,
                url: p.url,
            })
            .collect(),
        uploaded: done
            .into_iter()
            .map(|p| DonePart {
                number: p.number,
                etag: p.etag,
            })
            .collect(),
    }))
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

    if let Some(resp) = resume_upload(&s, user.id, &sha[..], &req).await? {
        return Ok(Json(resp));
    }

    let content_type = req
        .content_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".into());
    let key = keys::original(user.id, &sha, &keys::sanitize_ext(&req.filename));
    let upload_id = s.storage.create_multipart(&key, &content_type).await?;
    let n_parts = ((req.size_bytes + PART_SIZE - 1) / PART_SIZE) as i32;
    let parts = s.storage.presign_parts(&key, &upload_id, 1..=n_parts).await?;

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
        uploaded: Vec::new(),
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
    file_modified_at: Option<OffsetDateTime>,
}

async fn complete(
    State(s): State<AppState>,
    user: AuthUser,
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
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, file_modified_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (owner_id, sha256) DO UPDATE
            SET deleted_at = NULL, file_modified_at = coalesce(assets.file_modified_at, EXCLUDED.file_modified_at)
         RETURNING id",
    )
    .bind(user.id)
    .bind(&up.sha256)
    .bind(&up.key)
    .bind(&up.original_name)
    .bind(&up.content_type)
    .bind(up.size_bytes)
    .bind(up.file_modified_at)
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

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    /// Return assets strictly older than this timestamp (RFC 3339).
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
}

#[derive(Serialize)]
struct ListResponse {
    items: Vec<AssetSummary>,
    /// Pass as `before` to fetch the next page; null when this was the last page.
    #[serde(with = "time::serde::rfc3339::option")]
    next_before: Option<OffsetDateTime>,
}

async fn list(State(s): State<AppState>, user: AuthUser, Query(q): Query<ListQuery>) -> ApiResult<Json<ListResponse>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let rows = sqlx::query_as::<_, AssetSummary>(
        "SELECT id, original_name, content_type, size_bytes, width, height,
                coalesce(taken_at, file_modified_at, created_at) AS sort_time
         FROM assets
         WHERE owner_id = $1 AND deleted_at IS NULL
           AND ($2::timestamptz IS NULL OR coalesce(taken_at, file_modified_at, created_at) < $2)
         ORDER BY coalesce(taken_at, file_modified_at, created_at) DESC, id
         LIMIT $3",
    )
    .bind(user.id)
    .bind(q.before)
    .bind(limit)
    .fetch_all(&s.db)
    .await?;
    let next_before = if rows.len() as i64 == limit {
        rows.last().map(|r| r.sort_time)
    } else {
        None
    };
    Ok(Json(ListResponse {
        items: rows,
        next_before,
    }))
}

/// Number of live assets, for `cyanotype status`.
async fn count(State(s): State<AppState>, user: AuthUser) -> ApiResult<Json<serde_json::Value>> {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM assets WHERE owner_id = $1 AND deleted_at IS NULL")
        .bind(user.id)
        .fetch_one(&s.db)
        .await?;
    Ok(Json(serde_json::json!({"count": n})))
}

async fn get_one(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let a: AssetSummary = sqlx::query_as(
        "SELECT id, original_name, content_type, size_bytes, width, height, coalesce(taken_at, file_modified_at, created_at) AS sort_time
         FROM assets WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&s.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let (key, owner_id, derived_ready): (String, Uuid, bool) =
        sqlx::query_as("SELECT original_key, owner_id, thumbhash IS NOT NULL FROM assets WHERE id = $1")
            .bind(id)
            .fetch_one(&s.db)
            .await?;
    let mut v = serde_json::to_value(a).map_err(anyhow::Error::from)?;
    v["original_url"] = s.storage.presign_get(&key).await?.into();
    // The worker sets `thumbhash` only after both derived objects are uploaded, so it doubles
    // as the "derived images exist" flag. Until then the URLs are null rather than 404s.
    if derived_ready {
        v["thumb_url"] = s.storage.presign_get(&keys::thumb(owner_id, id)).await?.into();
        v["preview_url"] = s.storage.presign_get(&keys::preview(owner_id, id)).await?.into();
    } else {
        v["thumb_url"] = serde_json::Value::Null;
        v["preview_url"] = serde_json::Value::Null;
    }
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
}
