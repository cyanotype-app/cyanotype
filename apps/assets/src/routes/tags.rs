//! Tags (`docs/library.md`, "Tags"): the owner's labels on photos, owner-scoped like everything else.
//!
//! - `GET /assets/tags?q=&sort=name|count&limit=`: the owner's tags with how many photos have each (autocomplete is `q` with a small
//!   `limit`). A photo counts when it is live, not a RAW half, and not hidden unless this session has the Hidden view unlocked; a
//!   tag whose photos are all hidden is not listed to a locked session (its name would say what the hidden photos are).
//! - `POST /assets/tags {name}`: make a tag (the existing one, whatever the case, when there is one).
//! - `PATCH /assets/tags/{id} {name}`: rename. A name another tag has is `409 tag_exists` with that tag's id, for the app to offer
//!   "Merge into it".
//! - `POST /assets/tags/{id}/merge {into}`: every photo of the tag gets the other and the tag goes. Audited with a count.
//! - `DELETE /assets/tags/{id}`: removes the tag from its photos. Audited with a count.
//! - `POST /assets/tags/apply {ids | scope+except, add: [names], remove: [names]}`: bulk add or remove (also over a whole view). New
//!   names create tags; names to remove that are not tags are ignored. `{added, removed}` count (photo, tag) links changed.
//!
//! A RAW + JPEG pair is one photo whose visible primary carries the tags (the id of the hidden RAW means its primary). The ids of
//! another user's tags and photos are `404` / ignored, and a hidden photo is tagged only from an unlocked session.
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use cyanotype_core::{audit, tags};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use uuid::Uuid;

use super::scope::Selector;
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

/// Most names in one `apply` request.
const MAX_NAMES: usize = 20;
/// Most tags a list returns.
const MAX_LIST: i64 = 5_000;

#[derive(Serialize, FromRow)]
pub(super) struct TagItem {
    id: Uuid,
    name: String,
    /// Photos with the tag, as this session may see them.
    count: i64,
}

#[derive(Deserialize, Default)]
pub(super) struct ListQuery {
    /// Names containing this text (case-insensitive), for autocomplete.
    q: Option<String>,
    /// `name` (default, alphabetical) or `count` (most used first).
    sort: Option<String>,
    limit: Option<i64>,
}

/// LIKE's own characters are escaped (backslash is its default escape).
fn like_pattern(s: &str) -> String {
    let escaped: String = s
        .chars()
        .flat_map(|c| match c {
            '%' | '_' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!("%{escaped}%")
}

pub(super) async fn list(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(1_000).clamp(1, MAX_LIST);
    let order = match q.sort.as_deref() {
        None | Some("name") => "lower(t.name), t.id",
        Some("count") => "count DESC, lower(t.name), t.id",
        Some(_) => return Err(ApiError::BadRequest("sort must be name or count".into())),
    };
    let pattern =
        q.q.as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(like_pattern);
    let items: Vec<TagItem> = sqlx::query_as(&format!(
        "SELECT t.id, t.name,
                count(a.id) FILTER (WHERE NOT a.hidden OR $2) AS count
         FROM tags t
         LEFT JOIN asset_tags at ON at.tag_id = t.id
         LEFT JOIN assets a ON a.id = at.asset_id AND a.deleted_at IS NULL AND a.primary_id IS NULL
         WHERE t.owner_id = $1 AND ($3::text IS NULL OR t.name ILIKE $3)
         GROUP BY t.id
         -- A tag only hidden photos have is not for a locked session to see.
         HAVING $2 OR count(a.id) FILTER (WHERE a.hidden) = 0 OR count(a.id) FILTER (WHERE NOT a.hidden) > 0
         ORDER BY {order} LIMIT $4"
    ))
    .bind(user.id)
    .bind(unlock.0)
    .bind(pattern)
    .bind(limit)
    .fetch_all(&s.db)
    .await?;
    Ok(Json(json!({"items": items})))
}

#[derive(Deserialize)]
pub(super) struct NameBody {
    name: String,
}

fn valid(name: &str) -> ApiResult<String> {
    tags::normalize(name).map_err(ApiError::BadRequest)
}

pub(super) async fn create(
    State(s): State<AppState>,
    user: AuthUser,
    Json(req): Json<NameBody>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let name = valid(&req.name)?;
    let mut tx = s.db.begin().await?;
    let before: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM tags WHERE owner_id = $1 AND lower(name) = lower($2)")
            .bind(user.id)
            .bind(&name)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((id, name)) = before {
        return Ok((StatusCode::OK, Json(json!({"id": id, "name": name, "created": false}))));
    }
    let id = tags::ensure(&mut tx, user.id, &name).await?;
    tx.commit().await?;
    super::visibility::manifest_soon(&s, user.id);
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id, "name": name, "created": true})),
    ))
}

/// The tag, if it is the caller's (a lock on its row, so a rename or merge cannot race another).
async fn lock_tag(tx: &mut sqlx::PgConnection, owner: Uuid, id: Uuid) -> ApiResult<String> {
    sqlx::query_scalar::<_, String>("SELECT name FROM tags WHERE id = $1 AND owner_id = $2 FOR UPDATE")
        .bind(id)
        .bind(owner)
        .fetch_optional(tx)
        .await?
        .ok_or(ApiError::NotFound)
}

pub(super) async fn rename(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<NameBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let name = valid(&req.name)?;
    let mut tx = s.db.begin().await?;
    lock_tag(&mut tx, user.id, id).await?;
    let clash: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM tags WHERE owner_id = $1 AND lower(name) = lower($2) AND id <> $3")
            .bind(user.id)
            .bind(&name)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(existing) = clash {
        return Err(ApiError::Limited {
            status: StatusCode::CONFLICT,
            code: "tag_exists",
            message: "you already have a tag with that name".into(),
            details: json!({"existing_id": existing}),
            retry_after_secs: None,
        });
    }
    sqlx::query("UPDATE tags SET name = $2 WHERE id = $1")
        .bind(id)
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    super::visibility::manifest_soon(&s, user.id);
    Ok(Json(json!({"id": id, "name": name})))
}

#[derive(Deserialize)]
pub(super) struct MergeBody {
    into: Uuid,
}

pub(super) async fn merge(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<MergeBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if req.into == id {
        return Err(ApiError::BadRequest("a tag cannot be merged into itself".into()));
    }
    let mut tx = s.db.begin().await?;
    // Both locks in id order, so two merges the other way round cannot wait for each other.
    let (first, second) = if id < req.into { (id, req.into) } else { (req.into, id) };
    lock_tag(&mut tx, user.id, first).await?;
    lock_tag(&mut tx, user.id, second).await?;
    // The owner's mark wins over an imported one, as when a tag is added to a photo.
    let moved = sqlx::query(
        "INSERT INTO asset_tags (asset_id, tag_id, imported, added_at)
         SELECT asset_id, $2, imported, added_at FROM asset_tags WHERE tag_id = $1
         ON CONFLICT (asset_id, tag_id) DO UPDATE SET imported = asset_tags.imported AND EXCLUDED.imported",
    )
    .bind(id)
    .bind(req.into)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query("DELETE FROM tags WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut *tx,
        Some(user.id),
        "tags.merge",
        None,
        None,
        json!({"photos": moved}),
    )
    .await?;
    tx.commit().await?;
    super::visibility::manifest_soon(&s, user.id);
    Ok(Json(json!({"into": req.into, "photos": moved})))
}

pub(super) async fn remove(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let mut tx = s.db.begin().await?;
    lock_tag(&mut tx, user.id, id).await?;
    let (photos,): (i64,) = sqlx::query_as("SELECT count(*) FROM asset_tags WHERE tag_id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM tags WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut *tx,
        Some(user.id),
        "tags.delete",
        None,
        None,
        json!({"photos": photos}),
    )
    .await?;
    tx.commit().await?;
    super::visibility::manifest_soon(&s, user.id);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct ApplyRequest {
    #[serde(flatten)]
    pick: Selector,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

#[derive(Serialize, Default)]
pub(super) struct Applied {
    /// (photo, tag) links made or taken over from an import.
    added: u64,
    /// (photo, tag) links removed.
    removed: u64,
}

/// Unique (ignoring case), normalised names, at most [`MAX_NAMES`].
fn clean_names(names: &[String]) -> ApiResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let n = valid(n)?;
        if !out.iter().any(|o| o.to_lowercase() == n.to_lowercase()) {
            out.push(n);
        }
    }
    if out.len() > MAX_NAMES {
        return Err(ApiError::BadRequest(format!("at most {MAX_NAMES} tags at a time")));
    }
    Ok(out)
}

pub(super) async fn apply(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Json(req): Json<ApplyRequest>,
) -> ApiResult<Json<Applied>> {
    let (add, remove) = (clean_names(&req.add)?, clean_names(&req.remove)?);
    if add.is_empty() && remove.is_empty() {
        return Err(ApiError::BadRequest("nothing to do: send add or remove".into()));
    }
    let pick = req.pick.validated(unlock, s.faces_enabled)?;
    let mut tx = s.db.begin().await?;
    let ids: Vec<Uuid> = pick.resolve(&mut tx, user.id).await?.unwrap_or_default();
    // The visible photos the request names (a RAW id means its primary), not hidden unless unlocked.
    let photos: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT a.id FROM assets a
         WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.primary_id IS NULL AND (NOT a.hidden OR $3)
           AND (a.id = ANY($2) OR a.id IN (SELECT primary_id FROM assets
                                           WHERE owner_id = $1 AND id = ANY($2) AND primary_id IS NOT NULL))",
    )
    .bind(user.id)
    .bind(&ids)
    .bind(unlock.0)
    .fetch_all(&mut *tx)
    .await?;
    let mut done = Applied::default();
    for name in &add {
        let tag = tags::ensure(&mut tx, user.id, name).await?;
        // One statement for all the photos: new links, and imported ones taken over by the owner.
        done.added += sqlx::query(
            "INSERT INTO asset_tags (asset_id, tag_id, imported) SELECT x, $2, false FROM unnest($1::uuid[]) x
             ON CONFLICT (asset_id, tag_id) DO UPDATE SET imported = false WHERE asset_tags.imported",
        )
        .bind(&photos)
        .bind(tag)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    if !remove.is_empty() {
        let lowered: Vec<String> = remove.iter().map(|n| n.to_lowercase()).collect();
        done.removed = sqlx::query(
            "DELETE FROM asset_tags WHERE asset_id = ANY($1)
               AND tag_id IN (SELECT id FROM tags WHERE owner_id = $2 AND lower(name) = ANY($3))",
        )
        .bind(&photos)
        .bind(user.id)
        .bind(&lowered)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    tx.commit().await?;
    if done.added + done.removed > 0 {
        super::visibility::manifest_soon(&s, user.id);
    }
    Ok(Json(done))
}

/// The tags of one photo for its owner, for the viewer: `[{id, name, imported}]`, by name.
#[derive(Serialize, FromRow)]
pub(super) struct PhotoTag {
    id: Uuid,
    name: String,
    imported: bool,
}

pub(super) async fn of_photo(db: &sqlx::PgPool, owner: Uuid, asset: Uuid) -> ApiResult<Vec<PhotoTag>> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.name, at.imported FROM asset_tags at JOIN tags t ON t.id = at.tag_id
         WHERE at.asset_id = $1 AND t.owner_id = $2 ORDER BY lower(t.name), t.id",
    )
    .bind(asset)
    .bind(owner)
    .fetch_all(db)
    .await?)
}
