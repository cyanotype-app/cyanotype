//! Stacks: photos shown as one tile (docs/library.md, "Stacks"). A stack is an automatic burst (`burst_id`, the worker's) or
//! one the owner made by hand (`stack_id`); what the owner decided is kept in columns the worker never writes (migration
//! 0033), so no regroup or sweep undoes it.
//!
//! - `GET /assets/{id}/stack`: the photos of the photo's stack in capture order, and which one is the cover (the viewer steps
//!   through them). `404` when the photo is not in a stack of two or more, and for a hidden photo unless the Hidden view is
//!   unlocked (the other routes of a photo's own stack are `404` for it as well).
//! - `POST /assets/stacks {ids}`: stack the photos by hand (at least two photos); the first id is the new cover. A photo that
//!   is already in a stack made by hand brings that whole stack along (stacks merge); the other photos of a picked photo's
//!   burst do not (the web app sends all of a collapsed burst's photos when its tile is picked). `400` for fewer than two
//!   photos or more than [`MAX_STACK`].
//! - `POST /assets/{id}/stack/cover`: make the photo its stack's cover.
//! - `POST /assets/{id}/stack/remove`: take the photo out of its stack. It stays out of automatic bursts too.
//! - `DELETE /assets/{id}/stack`: dissolve the photo's stack; its photos stay out of automatic bursts.
//!
//! Everything is owner-scoped: an id of another account, an unknown id and a trashed photo are `404` (or ignored in the
//! bulk call), and a stack never holds photos of two owners. Timeline and search collapse a stack to its cover; favorites,
//! the archive, the Hidden view, albums, the map and people list every photo.
//!
//! The SQL helpers here are the one place that says what a stack is, so the list, the day counts, search and the details
//! agree.
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::{ListItem, ListRow, list_items, list_select, trash::MAX_IDS, visibility::Sort};
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

/// The most photos a stack made by hand holds (the list sends every member's id with the cover).
pub(super) const MAX_STACK: usize = 100;

/// The stack of a photo of table alias `a`: a manual stack wins over the burst, and a photo the owner took out has none.
pub(super) fn key_sql(a: &str) -> String {
    format!("coalesce({a}.stack_id, CASE WHEN {a}.stack_unlinked THEN NULL ELSE {a}.burst_id END)")
}

/// Is row `m` in the stack `key` (a SQL expression)? Written so that `assets_stack_idx` and `assets_burst_idx` serve it.
pub(super) fn in_stack_sql(m: &str, key: &str) -> String {
    format!("({m}.stack_id = {key} OR ({m}.stack_id IS NULL AND NOT {m}.stack_unlinked AND {m}.burst_id = {key}))")
}

/// The order that picks a stack's cover among the rows `m`: the owner's choice, then a favorite, then the earliest photo.
pub(super) fn cover_order_sql(m: &str) -> String {
    format!(
        "{m}.stack_cover DESC, {m}.favorite DESC, coalesce({m}.taken_at, {m}.file_modified_at, {m}.created_at), {m}.id"
    )
}

/// The extra WHERE term (starting with ` AND`) that makes a list show one tile per stack, on the unaliased `assets` of the
/// list and the day counts: a photo in a stack is listed only when it is the stack's cover among the stack's photos of the
/// same view (`view` is `visibility::view_sql`'s text, which names `hidden` and `archived` unqualified, so inside the
/// subquery it reads the member's own).
pub(super) fn collapse_sql(view: &str) -> String {
    let key = key_sql("assets");
    format!(
        " AND ({key} IS NULL OR assets.id = (SELECT m.id FROM assets m
            WHERE m.owner_id = assets.owner_id AND {} AND m.deleted_at IS NULL AND m.primary_id IS NULL{view}
            ORDER BY {} LIMIT 1))",
        in_stack_sql("m", &key),
        cover_order_sql("m")
    )
}

/// What a list entry says about the stack its photo is in.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub(super) struct StackInfo {
    /// Photos in the stack that the view lists, the cover included.
    pub(super) size: i64,
    /// Their ids (the cover included) when the list shows the stack as one tile, so that selecting the tile means all of them;
    /// absent when every photo is listed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) members: Vec<Uuid>,
}

/// Stack info for the listed photos: `keys[i]` is the stack of photo `i` (if any). One query for the whole page.
pub(super) async fn infos(
    db: &PgPool,
    owner: Uuid,
    keys: &[Option<Uuid>],
    view: &str,
    collapsed: bool,
) -> ApiResult<Vec<Option<StackInfo>>> {
    let wanted: Vec<Uuid> = {
        let mut k: Vec<Uuid> = keys.iter().flatten().copied().collect();
        k.sort();
        k.dedup();
        k
    };
    if wanted.is_empty() {
        return Ok(keys.iter().map(|_| None).collect());
    }
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(&format!(
        "SELECT k.key, m.id FROM unnest($2::uuid[]) AS k(key)
         JOIN assets m ON m.owner_id = $1 AND {} AND m.deleted_at IS NULL AND m.primary_id IS NULL{view}
         ORDER BY m.id",
        in_stack_sql("m", "k.key")
    ))
    .bind(owner)
    .bind(&wanted)
    .fetch_all(db)
    .await?;
    let mut by_key: std::collections::HashMap<Uuid, Vec<Uuid>> = std::collections::HashMap::new();
    for (k, id) in rows {
        by_key.entry(k).or_default().push(id);
    }
    Ok(keys
        .iter()
        .map(|k| {
            let members = by_key.get(&(*k)?)?;
            (members.len() > 1).then(|| StackInfo {
                size: members.len() as i64,
                members: if collapsed { members.clone() } else { Vec::new() },
            })
        })
        .collect())
}

#[derive(Serialize)]
pub(super) struct StackResponse {
    /// The cover (the photo the timeline shows for the stack).
    cover_id: Uuid,
    /// Whether the owner made the stack (rather than the worker finding a burst).
    manual: bool,
    /// The stack's photos in capture order.
    items: Vec<ListItem>,
}

/// The stack of a live photo of the caller's: `(key, manual)`, or `404` when it is in none. A hidden photo is a `404` too
/// unless this session has the Hidden view unlocked (the same rule as its details).
async fn key_of(db: &PgPool, owner: Uuid, id: Uuid, unlock: Unlock) -> ApiResult<(Uuid, bool)> {
    let row: Option<(Option<Uuid>, bool)> = sqlx::query_as(&format!(
        "SELECT {}, stack_id IS NOT NULL FROM assets a
         WHERE a.id = $1 AND a.owner_id = $2 AND a.deleted_at IS NULL AND a.primary_id IS NULL AND (NOT a.hidden OR $3)",
        key_sql("a")
    ))
    .bind(id)
    .bind(owner)
    .bind(unlock.0)
    .fetch_optional(db)
    .await?;
    match row {
        Some((Some(key), manual)) => Ok((key, manual)),
        _ => Err(ApiError::NotFound),
    }
}

pub(super) async fn get(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<StackResponse>> {
    let (key, manual) = key_of(&s.db, user.id, id, unlock).await?;
    // The photos the timeline would list for the stack, and the photo asked about even when it is archived or hidden.
    let view = " AND NOT hidden AND NOT archived";
    let rows: Vec<ListRow> = sqlx::query_as(&format!(
        "{} AND (id = $3 OR (NOT hidden AND NOT archived)) AND {} ORDER BY coalesce(taken_at, file_modified_at, created_at), id",
        list_select(Sort::Taken),
        in_stack_sql("assets", "$2")
    ))
    .bind(user.id)
    .bind(key)
    .bind(id)
    .fetch_all(&s.db)
    .await?;
    if rows.len() < 2 {
        return Err(ApiError::NotFound);
    }
    let cover: Option<Uuid> = sqlx::query_scalar(&format!(
        "SELECT m.id FROM assets m WHERE m.owner_id = $1 AND {} AND m.deleted_at IS NULL AND m.primary_id IS NULL{view}
         ORDER BY {} LIMIT 1",
        in_stack_sql("m", "$2"),
        cover_order_sql("m")
    ))
    .bind(user.id)
    .bind(key)
    .fetch_optional(&s.db)
    .await?;
    let cover_id = cover.unwrap_or(rows[0].summary.id);
    let items = list_items(&s, user.id, rows).await?;
    Ok(Json(StackResponse {
        cover_id,
        manual,
        items,
    }))
}

#[derive(Deserialize)]
pub(super) struct StackRequest {
    /// The photos to stack; the first becomes the cover.
    ids: Vec<Uuid>,
}

#[derive(Serialize)]
pub(super) struct Stacked {
    /// Photos in the new stack (the whole stacks of the picked photos included).
    size: usize,
    cover_id: Uuid,
}

pub(super) async fn create(
    State(s): State<AppState>,
    user: AuthUser,
    Json(req): Json<StackRequest>,
) -> ApiResult<Json<Stacked>> {
    if req.ids.len() > MAX_IDS {
        return Err(ApiError::BadRequest(format!("at most {MAX_IDS} photos at a time")));
    }
    let mut tx = s.db.begin().await?;
    // Runs of one owner are serialised (two stackings that share a photo would otherwise interleave).
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('stack:' || $1::text, 0))")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    // The picked photos that can be stacked (live, visible, the caller's), in the order asked.
    let picked: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT a.id, a.stack_id FROM unnest($2::uuid[]) WITH ORDINALITY AS p(id, n)
         JOIN assets a ON a.id = p.id WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.primary_id IS NULL AND NOT a.hidden
         ORDER BY p.n",
    )
    .bind(user.id)
    .bind(&req.ids)
    .fetch_all(&mut *tx)
    .await?;
    let Some(&(cover, _)) = picked.first() else {
        return Err(ApiError::BadRequest("pick at least two photos to stack".into()));
    };
    let picked_ids: Vec<Uuid> = picked.iter().map(|p| p.0).collect();
    let mut keys: Vec<Uuid> = picked.iter().filter_map(|p| p.1).collect();
    keys.sort();
    keys.dedup();
    // Manual stacks move whole: the picked photos and every photo of the stacks they are in (trashed ones too, so a
    // restored photo does not return to a stack that is gone).
    let all: Vec<(Uuid, bool)> = sqlx::query_as(
        "SELECT m.id, m.deleted_at IS NULL FROM assets m
         WHERE m.owner_id = $1 AND m.primary_id IS NULL AND (m.id = ANY($2) OR m.stack_id = ANY($3))",
    )
    .bind(user.id)
    .bind(&picked_ids)
    .bind(&keys)
    .fetch_all(&mut *tx)
    .await?;
    let live = all.iter().filter(|(_, live)| *live).count();
    if live < 2 {
        return Err(ApiError::BadRequest("pick at least two photos to stack".into()));
    }
    if live > MAX_STACK {
        return Err(ApiError::BadRequest(format!(
            "a stack holds at most {MAX_STACK} photos"
        )));
    }
    let members: Vec<Uuid> = all.into_iter().map(|(id, _)| id).collect();
    let stack = Uuid::new_v4();
    sqlx::query("UPDATE assets SET stack_id = $2, stack_cover = (id = $3) WHERE owner_id = $1 AND id = ANY($4)")
        .bind(user.id)
        .bind(stack)
        .bind(cover)
        .bind(&members)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    // The stack is in the recovery manifest (as are the cover, a photo taken out and the dissolved stack below).
    super::visibility::manifest_soon(&s, user.id);
    Ok(Json(Stacked {
        size: live,
        cover_id: cover,
    }))
}

pub(super) async fn set_cover(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let (key, _) = key_of(&s.db, user.id, id, unlock).await?;
    // The cover is one of the photos the timeline lists.
    let done = sqlx::query(&format!(
        "UPDATE assets m SET stack_cover = (m.id = $3)
         WHERE m.owner_id = $1 AND {} AND (m.id = $3 OR m.stack_cover)
           AND EXISTS (SELECT 1 FROM assets c WHERE c.id = $3 AND NOT c.hidden AND NOT c.archived)",
        in_stack_sql("m", "$2")
    ))
    .bind(user.id)
    .bind(key)
    .bind(id)
    .execute(&s.db)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::BadRequest(
            "restore the photo before making it the cover".into(),
        ));
    }
    super::visibility::manifest_soon(&s, user.id);
    Ok(Json(serde_json::json!({"cover_id": id})))
}

pub(super) async fn remove(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let (key, manual) = key_of(&s.db, user.id, id, unlock).await?;
    let mut tx = s.db.begin().await?;
    sqlx::query(
        "UPDATE assets SET stack_id = NULL, stack_unlinked = true, stack_cover = false WHERE id = $1 AND owner_id = $2",
    )
    .bind(id)
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    if manual {
        // A manual stack down to one photo is no stack: free the last one (it keeps its cover flag cleared).
        sqlx::query(
            "UPDATE assets SET stack_id = NULL, stack_cover = false
             WHERE owner_id = $1 AND stack_id = $2
               AND (SELECT count(*) FROM assets WHERE owner_id = $1 AND stack_id = $2 AND deleted_at IS NULL) < 2",
        )
        .bind(user.id)
        .bind(key)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    super::visibility::manifest_soon(&s, user.id);
    Ok(Json(serde_json::json!({"removed": id})))
}

pub(super) async fn dissolve(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let (key, _) = key_of(&s.db, user.id, id, unlock).await?;
    let done = sqlx::query(&format!(
        "UPDATE assets m SET stack_id = NULL, stack_unlinked = true, stack_cover = false
         WHERE m.owner_id = $1 AND {}",
        in_stack_sql("m", "$2")
    ))
    .bind(user.id)
    .bind(key)
    .execute(&s.db)
    .await?;
    super::visibility::manifest_soon(&s, user.id);
    Ok(Json(serde_json::json!({"count": done.rows_affected()})))
}
