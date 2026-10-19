//! Share links, owner side: create, list, edit and revoke public links to an album or to a set of photos.
//! The public side (what a visitor of `/s/<token>` calls) is in `share_public`.
//!
//! | Request | Effect |
//! |---|---|
//! | `POST /assets/shares {album_id \| asset_ids, title?, expires_at?, password?, allow_download?, include_originals?}` | Create (`201`). `scope` (+ `except`) in place of `album_id` \| `asset_ids` shares a whole view, up to 1000 photos. |
//! | `GET /assets/shares` | Your links, newest first, each with its token, state and live photo count. |
//! | `GET /assets/shares/{id}` | One link. |
//! | `PATCH /assets/shares/{id}` | `title`, `expires_at` (`null` = never), `password` (`null` removes it), `allow_download`, `include_originals`. |
//! | `DELETE /assets/shares/{id}` | Revoke: the link answers `404` from the next request on. |
//!
//! Another user's link or an unknown id is always `404`.
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    scope::{self, MAX_EXCEPT, Scope},
    share_public,
    trash::MAX_IDS,
};
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser};

const MAX_TITLE: usize = 200;
const MIN_PASSWORD: usize = 4;
const MAX_PASSWORD: usize = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/assets/shares", get(list).post(create))
        .route("/assets/shares/{id}", get(get_one).patch(update).delete(revoke))
}

/// 192 random bits, URL-safe, 32 characters.
pub fn new_token() -> String {
    let mut b = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

/// Distinguishes an absent field (leave alone) from an explicit `null` (clear it).
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

fn clean_title(t: Option<&str>) -> ApiResult<Option<String>> {
    let Some(t) = t.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if t.chars().count() > MAX_TITLE {
        return Err(ApiError::BadRequest(format!("title is at most {MAX_TITLE} characters")));
    }
    Ok(Some(t.to_string()))
}

fn check_password(p: &str) -> ApiResult<()> {
    let n = p.chars().count();
    if !(MIN_PASSWORD..=MAX_PASSWORD).contains(&n) {
        return Err(ApiError::BadRequest(format!(
            "password must be {MIN_PASSWORD} to {MAX_PASSWORD} characters"
        )));
    }
    Ok(())
}

fn check_expiry(t: OffsetDateTime) -> ApiResult<()> {
    if t <= OffsetDateTime::now_utc() {
        return Err(ApiError::BadRequest("the expiry must be in the future".into()));
    }
    Ok(())
}

#[derive(Serialize)]
struct AlbumRef {
    id: Uuid,
    title: String,
}

#[derive(Serialize)]
pub(super) struct Share {
    id: Uuid,
    /// `album` (follows the album) or `selection` (a fixed set of photos).
    kind: String,
    album: Option<AlbumRef>,
    title: Option<String>,
    /// The secret of the link: `https://<host>/s/<token>`.
    token: String,
    has_password: bool,
    allow_download: bool,
    /// Downloads serve the untouched originals, which carry location, instead of a copy without it.
    include_originals: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
    expired: bool,
    /// Photos a visitor sees right now (trashed ones are not counted).
    item_count: i64,
    view_count: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    last_viewed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(FromRow)]
struct ShareRow {
    id: Uuid,
    kind: String,
    album_id: Option<Uuid>,
    album_title: Option<String>,
    title: Option<String>,
    token: String,
    has_password: bool,
    allow_download: bool,
    include_originals: bool,
    expires_at: Option<OffsetDateTime>,
    expired: bool,
    item_count: i64,
    view_count: i64,
    last_viewed_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl From<ShareRow> for Share {
    fn from(r: ShareRow) -> Self {
        Share {
            id: r.id,
            kind: r.kind,
            album: r.album_id.zip(r.album_title).map(|(id, title)| AlbumRef { id, title }),
            title: r.title,
            token: r.token,
            has_password: r.has_password,
            allow_download: r.allow_download,
            include_originals: r.include_originals,
            expires_at: r.expires_at,
            expired: r.expired,
            item_count: r.item_count,
            view_count: r.view_count,
            last_viewed_at: r.last_viewed_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

async fn fetch(db: &sqlx::PgPool, owner: Uuid, id: Option<Uuid>) -> ApiResult<Vec<Share>> {
    let rows: Vec<ShareRow> = sqlx::query_as(
        "SELECT s.id, s.kind, s.album_id, a.title AS album_title, s.title, s.token,
                (s.password_hash IS NOT NULL) AS has_password, s.allow_download, s.include_originals,
                s.expires_at, coalesce(s.expires_at <= now(), false) AS expired,
                CASE WHEN s.kind = 'album' THEN
                    (SELECT count(*) FROM album_assets aa JOIN assets x ON x.id = aa.asset_id
                     WHERE aa.album_id = s.album_id AND x.owner_id = s.owner_id
                       AND x.deleted_at IS NULL AND x.primary_id IS NULL AND NOT x.hidden)
                ELSE
                    (SELECT count(*) FROM share_assets sa JOIN assets x ON x.id = sa.asset_id
                     WHERE sa.share_id = s.id AND x.owner_id = s.owner_id
                       AND x.deleted_at IS NULL AND x.primary_id IS NULL AND NOT x.hidden)
                END AS item_count,
                s.view_count, s.last_viewed_at, s.created_at, s.updated_at
         FROM shares s LEFT JOIN albums a ON a.id = s.album_id
         WHERE s.owner_id = $1 AND ($2::uuid IS NULL OR s.id = $2)
         ORDER BY s.created_at DESC, s.id DESC",
    )
    .bind(owner)
    .bind(id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Share::from).collect())
}

async fn list(State(s): State<AppState>, user: AuthUser) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({"items": fetch(&s.db, user.id, None).await?})))
}

async fn get_one(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<Share>> {
    fetch(&s.db, user.id, Some(id))
        .await?
        .pop()
        .map(Json)
        .ok_or(ApiError::NotFound)
}

#[derive(Deserialize)]
struct CreateBody {
    album_id: Option<Uuid>,
    asset_ids: Option<Vec<Uuid>>,
    /// "Select all": every photo of a view except `except`, resolved here (at most `MAX_IDS` of them).
    scope: Option<Scope>,
    #[serde(default)]
    except: Vec<Uuid>,
    title: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
    password: Option<String>,
    allow_download: Option<bool>,
    include_originals: Option<bool>,
}

async fn create(
    State(s): State<AppState>,
    user: AuthUser,
    Json(req): Json<CreateBody>,
) -> ApiResult<(StatusCode, Json<Share>)> {
    let title = clean_title(req.title.as_deref())?;
    if let Some(t) = req.expires_at {
        check_expiry(t)?;
    }
    let password_hash = match req.password.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => {
            check_password(p)?;
            Some(share_public::hash_password(p).await?)
        }
        None => None,
    };
    let allow_download = req.allow_download.unwrap_or(true);
    let include_originals = allow_download && req.include_originals.unwrap_or(false);
    let (kind, ids) = match (req.album_id, req.asset_ids, req.scope) {
        (Some(_), None, None) => ("album", Vec::new()),
        (None, Some(ids), None) => {
            if ids.is_empty() {
                return Err(ApiError::BadRequest("choose at least one photo to share".into()));
            }
            if ids.len() > MAX_IDS {
                return Err(ApiError::BadRequest(format!("at most {MAX_IDS} photos per link")));
            }
            ("selection", ids)
        }
        (None, None, Some(scope)) => {
            if req.except.len() > MAX_EXCEPT {
                return Err(ApiError::BadRequest(format!("at most {MAX_EXCEPT} except ids")));
            }
            // Never unlocked: a link cannot be made from the Hidden view (nor from hidden photos in the trash).
            if scope == Scope::Hidden {
                return Err(ApiError::BadRequest("hidden photos cannot be shared".into()));
            }
            let mut conn = s.db.acquire().await?;
            let ids = scope::resolve(&mut conn, user.id, &scope, &req.except, false).await?;
            if ids.is_empty() {
                return Err(ApiError::BadRequest("choose at least one photo to share".into()));
            }
            if ids.len() > MAX_IDS {
                return Err(ApiError::BadRequest(format!("at most {MAX_IDS} photos per link")));
            }
            ("selection", ids)
        }
        _ => {
            return Err(ApiError::BadRequest(
                "give either album_id, asset_ids or a scope (the photos to share)".into(),
            ));
        }
    };
    if let Some(album) = req.album_id {
        let owned: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM albums WHERE id = $1 AND owner_id = $2")
            .bind(album)
            .bind(user.id)
            .fetch_optional(&s.db)
            .await?;
        owned.ok_or(ApiError::NotFound)?;
    }

    let mut tx = s.db.begin().await?;
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO shares (owner_id, token, kind, album_id, title, password_hash, allow_download, include_originals, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(user.id)
    .bind(new_token())
    .bind(kind)
    .bind(req.album_id)
    .bind(&title)
    .bind(&password_hash)
    .bind(allow_download)
    .bind(include_originals)
    .bind(req.expires_at)
    .fetch_one(&mut *tx)
    .await?;
    if kind == "selection" {
        // The visible photo of each id (a RAW id maps to its JPEG/HEIC), only your own live photos.
        let added = sqlx::query(
            "INSERT INTO share_assets (share_id, asset_id)
             SELECT DISTINCT $1, p.id FROM assets a JOIN assets p ON p.id = coalesce(a.primary_id, a.id)
             WHERE a.id = ANY($3) AND a.owner_id = $2 AND p.owner_id = $2 AND p.deleted_at IS NULL AND p.purging_at IS NULL
               AND NOT p.hidden",
        )
        .bind(id)
        .bind(user.id)
        .bind(&ids)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if added == 0 {
            return Err(ApiError::BadRequest("none of those photos can be shared".into()));
        }
    }
    tx.commit().await?;
    tracing::info!(share_id = %id, kind, has_password = password_hash.is_some(), allow_download, include_originals, "share created");
    let share = fetch(&s.db, user.id, Some(id)).await?.pop().ok_or(ApiError::NotFound)?;
    Ok((StatusCode::CREATED, Json(share)))
}

#[derive(Deserialize)]
struct UpdateBody {
    #[serde(default, deserialize_with = "present")]
    title: Option<Option<String>>,
    #[serde(default, deserialize_with = "present_time")]
    expires_at: Option<Option<OffsetDateTime>>,
    #[serde(default, deserialize_with = "present")]
    password: Option<Option<String>>,
    allow_download: Option<bool>,
    include_originals: Option<bool>,
}

fn present_time<'de, D>(d: D) -> Result<Option<Option<OffsetDateTime>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    time::serde::rfc3339::option::deserialize(d).map(Some)
}

async fn update(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateBody>,
) -> ApiResult<Json<Share>> {
    if let Some(Some(t)) = req.expires_at {
        check_expiry(t)?;
    }
    // Hash outside the transaction: it is slow on purpose.
    let new_password = match &req.password {
        Some(Some(p)) if !p.is_empty() => {
            check_password(p)?;
            Some(Some(share_public::hash_password(p).await?))
        }
        Some(_) => Some(None),
        None => None,
    };
    let mut tx = s.db.begin().await?;
    let cur: Option<(Option<String>, Option<OffsetDateTime>, bool, bool)> = sqlx::query_as(
        "SELECT title, expires_at, allow_download, include_originals FROM shares
         WHERE id = $1 AND owner_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?;
    let (title, expires_at, allow_download, include_originals) = cur.ok_or(ApiError::NotFound)?;
    let title = match &req.title {
        Some(t) => clean_title(t.as_deref())?,
        None => title,
    };
    let expires_at = req.expires_at.unwrap_or(expires_at);
    let allow_download = req.allow_download.unwrap_or(allow_download);
    let include_originals = allow_download && req.include_originals.unwrap_or(include_originals);
    sqlx::query(
        "UPDATE shares SET title = $2, expires_at = $3, allow_download = $4, include_originals = $5, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(&title)
    .bind(expires_at)
    .bind(allow_download)
    .bind(include_originals)
    .execute(&mut *tx)
    .await?;
    if let Some(hash) = new_password {
        // A new (or removed) password ends every unlock and any lockout of the old one.
        sqlx::query("UPDATE shares SET password_hash = $2, pw_failures = 0, pw_locked_until = NULL WHERE id = $1")
            .bind(id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM share_unlocks WHERE share_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    tracing::info!(share_id = %id, "share updated");
    let share = fetch(&s.db, user.id, Some(id)).await?.pop().ok_or(ApiError::NotFound)?;
    Ok(Json(share))
}

async fn revoke(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    // Deleting the row is the whole revocation: the public side looks the token up on every request.
    let n = sqlx::query("DELETE FROM shares WHERE id = $1 AND owner_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&s.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound);
    }
    tracing::info!(share_id = %id, "share revoked");
    Ok(StatusCode::NO_CONTENT)
}
