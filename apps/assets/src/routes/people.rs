//! People (`docs/faces.md`, "People"): the faces the worker found, grouped into people the owner names and corrects. Switched on by
//! `FACES_ENABLED`; off (the default) every route here but the status is a `404`, as if People did not exist. Owner-only like the faces
//! themselves: nothing here is reachable from a share link, a public page, a download or a recovery manifest, and a person's name is
//! never written to the bucket or to the audit trail.
//!
//! - `GET /assets/people/status`: `{enabled}`. The only answer while the feature is off; the web app reads it to show or hide People.
//! - `GET /assets/people`: `{items: [{id, name, hidden, faces, photos, cover}]}`, most photos first. `name` is null for an unnamed person
//!   and `cover` is `{face_id, asset_id, box: [x, y, w, h], preview_url}` (the box is fractions of the preview image the URL serves; the web
//!   app crops it with CSS) or null. Counts are of the photos this session may see (live, not a RAW half, and not hidden unless the Hidden
//!   view is unlocked); a person only hidden photos have is not listed to a locked session. `GET /assets/people/{id}` is one of these.
//! - `PATCH /assets/people/{id} {name?, hidden?, cover_face_id?}`: rename (null or blank clears), hide or show, choose the cover.
//!   `DELETE /assets/people/{id}`: the person goes, its faces are unassigned (they may be grouped again, into a new person).
//! - `POST /assets/people/{id}/merge {into}`: all the faces go to the other person, which keeps its name unless it has none.
//! - `GET /assets/people/{id}/photos?limit&cursor`: the person's photos, same item shape, ordering and keyset pagination as `GET /assets`.
//! - `POST /assets/people/{id}/faces/remove {asset_ids?, face_ids?}`: "not this person": the person's faces in those photos (or those faces)
//!   become unassigned, and a rejection is written so the worker never puts them back and they are never suggested. `{removed}`.
//! - `GET /assets/people/{id}/suggestions?limit`: "Is this X?": unassigned faces that look like the person, best first, each
//!   `{face_id, asset_id, box, preview_url, similarity}`.
//! - `POST /assets/faces/{id}/assign {person_id}` or `{new: true, name?}`: the owner says this face is that person (or a new one). The
//!   worker never moves such a face. Moving it from another person also rejects it from that one. `POST /assets/faces/{id}/reject
//!   {person_id}`: "no, not them" (a refused suggestion).
//! - `GET /assets/{id}/faces`: the faces in one photo, `[{id, box, person: {id, name} | null}]`; faces of hidden people are left out.
//!
//! Ids of someone else's people, faces and photos are `404`. Changing a person asks the worker to group again (`people_runs`), which
//! it does once the owner's faces have been quiet for a while.
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use cyanotype_core::{audit, faces, keys, people};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

use super::{ListQuery, ListResponse, ListRow, decode_cursor, encode_cursor, list_items, trash::MAX_IDS};
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

const MAX_NAME: usize = 100;
/// Most people one list returns.
const MAX_LIST: i64 = 5_000;
const MAX_SUGGESTIONS: i64 = 50;

/// Same expression as the library timeline (`x` is the assets alias).
const SORT: &str = "coalesce(x.taken_at, x.file_modified_at, x.created_at)";

/// A face counts when its photo has a scan by the face's model (after a rotate or crop the old faces wait, unread, for the new scan).
const SCANNED: &str = "EXISTS (SELECT 1 FROM face_scans s WHERE s.asset_id = f.asset_id AND s.model = f.model)";
/// A photo this session may see: live, not a RAW half, not hidden unless the Hidden view is unlocked (`$unlock` is the parameter).
fn seen(alias: &str, unlock: &str) -> String {
    format!(
        "{alias}.deleted_at IS NULL AND {alias}.purging_at IS NULL AND {alias}.primary_id IS NULL AND (NOT {alias}.hidden OR {unlock})"
    )
}

/// While the feature is off nothing about People exists.
fn on(s: &AppState) -> ApiResult<()> {
    if s.faces_enabled {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// A person's name as stored: trimmed, at most [`MAX_NAME`] characters, no control characters; blank means no name.
fn clean_name(n: &str) -> ApiResult<Option<String>> {
    let n = n.trim();
    if n.chars().any(char::is_control) {
        return Err(ApiError::BadRequest("a name cannot hold control characters".into()));
    }
    if n.chars().count() > MAX_NAME {
        return Err(ApiError::BadRequest(format!("a name is at most {MAX_NAME} characters")));
    }
    Ok((!n.is_empty()).then(|| n.to_string()))
}

/// Distinguishes an absent field (leave alone) from an explicit `null` (clear it).
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

pub(super) async fn status(State(s): State<AppState>, _user: AuthUser) -> Json<Value> {
    Json(json!({"enabled": s.faces_enabled}))
}

#[derive(Serialize)]
struct Cover {
    face_id: Uuid,
    asset_id: Uuid,
    #[serde(rename = "box")]
    bbox: [f32; 4],
    preview_url: String,
}

#[derive(Serialize)]
pub(super) struct Person {
    id: Uuid,
    name: Option<String>,
    hidden: bool,
    /// Faces in the photos this session may see, and how many photos they are in.
    faces: i64,
    photos: i64,
    cover: Option<Cover>,
}

#[derive(FromRow)]
struct PersonRow {
    id: Uuid,
    name: Option<String>,
    hidden: bool,
    faces: i64,
    photos: i64,
}

#[derive(FromRow)]
struct CoverRow {
    person_id: Uuid,
    id: Uuid,
    asset_id: Uuid,
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
}

/// The owner's people (`only` one of them), with counts and covers as this session may see them.
async fn load(s: &AppState, owner: Uuid, unlock: bool, only: Option<Uuid>) -> ApiResult<Vec<Person>> {
    let rows: Vec<PersonRow> = sqlx::query_as(&format!(
        "SELECT p.id, p.name, p.hidden,
                count(f.id) FILTER (WHERE a.id IS NOT NULL AND (NOT a.hidden OR $2)) AS faces,
                count(DISTINCT f.asset_id) FILTER (WHERE a.id IS NOT NULL AND (NOT a.hidden OR $2)) AS photos
         FROM people p
         LEFT JOIN faces f ON f.person_id = p.id AND {SCANNED}
         LEFT JOIN assets a ON a.id = f.asset_id AND a.deleted_at IS NULL AND a.purging_at IS NULL AND a.primary_id IS NULL
         WHERE p.owner_id = $1 AND ($3::uuid IS NULL OR p.id = $3)
         GROUP BY p.id
         -- A person only hidden photos have is not for a locked session to see.
         HAVING $2 OR count(f.id) FILTER (WHERE a.hidden) = 0 OR count(f.id) FILTER (WHERE a.id IS NOT NULL AND NOT a.hidden) > 0
         ORDER BY photos DESC, lower(p.name) NULLS LAST, p.created_at, p.id
         LIMIT {MAX_LIST}"
    ))
    .bind(owner)
    .bind(unlock)
    .bind(only)
    .fetch_all(&s.db)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    // The cover: the chosen face when this session may see its photo, else the best face (the most confident, the largest).
    let covers: Vec<CoverRow> = sqlx::query_as(&format!(
        "SELECT DISTINCT ON (f.person_id) f.person_id, f.id, f.asset_id, f.box_x, f.box_y, f.box_w, f.box_h
         FROM faces f JOIN people p ON p.id = f.person_id JOIN assets x ON x.id = f.asset_id
         WHERE f.person_id = ANY($1) AND p.owner_id = $2 AND {SCANNED} AND x.thumbhash IS NOT NULL AND {}
         ORDER BY f.person_id, (f.id = p.cover_face_id) DESC, f.score * f.box_w * f.box_h DESC, f.id",
        seen("x", "$3")
    ))
    .bind(&ids)
    .bind(owner)
    .bind(unlock)
    .fetch_all(&s.db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let cover = match covers.iter().find(|c| c.person_id == r.id) {
            Some(c) => Some(Cover {
                face_id: c.id,
                asset_id: c.asset_id,
                bbox: [c.box_x, c.box_y, c.box_w, c.box_h],
                preview_url: s.storage.presign_get(&keys::preview(owner, c.asset_id)).await?,
            }),
            None => None,
        };
        out.push(Person {
            id: r.id,
            name: r.name,
            hidden: r.hidden,
            faces: r.faces,
            photos: r.photos,
            cover,
        });
    }
    Ok(out)
}

pub(super) async fn list(State(s): State<AppState>, user: AuthUser, unlock: Unlock) -> ApiResult<Json<Value>> {
    on(&s)?;
    Ok(Json(json!({"items": load(&s, user.id, unlock.0, None).await?})))
}

pub(super) async fn get(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Person>> {
    on(&s)?;
    load(&s, user.id, unlock.0, Some(id))
        .await?
        .pop()
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// The person, if it is the caller's (a lock on its row, so a merge or a delete cannot race another change).
async fn lock_person(tx: &mut sqlx::PgConnection, owner: Uuid, id: Uuid) -> ApiResult<Option<String>> {
    sqlx::query_scalar::<_, Option<String>>("SELECT name FROM people WHERE id = $1 AND owner_id = $2 FOR UPDATE")
        .bind(id)
        .bind(owner)
        .fetch_optional(tx)
        .await?
        .ok_or(ApiError::NotFound)
}

#[derive(Deserialize)]
pub(super) struct Patch {
    #[serde(default, deserialize_with = "present")]
    name: Option<Option<String>>,
    hidden: Option<bool>,
    cover_face_id: Option<Uuid>,
}

pub(super) async fn patch(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Json(req): Json<Patch>,
) -> ApiResult<Json<Person>> {
    on(&s)?;
    if req.name.is_none() && req.hidden.is_none() && req.cover_face_id.is_none() {
        return Err(ApiError::BadRequest(
            "nothing to change: send name, hidden or cover_face_id".into(),
        ));
    }
    let mut tx = s.db.begin().await?;
    lock_person(&mut tx, user.id, id).await?;
    if let Some(name) = &req.name {
        let name = match name {
            Some(n) => clean_name(n)?,
            None => None,
        };
        sqlx::query("UPDATE people SET name = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(name)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(hidden) = req.hidden {
        sqlx::query("UPDATE people SET hidden = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(hidden)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(face) = req.cover_face_id {
        // A face of this person, in a photo this session may see.
        let ok: Option<Uuid> = sqlx::query_scalar(&format!(
            "SELECT f.id FROM faces f JOIN assets a ON a.id = f.asset_id
             WHERE f.id = $1 AND f.person_id = $2 AND f.owner_id = $3 AND {SCANNED} AND a.thumbhash IS NOT NULL AND {}",
            seen("a", "$4")
        ))
        .bind(face)
        .bind(id)
        .bind(user.id)
        .bind(unlock.0)
        .fetch_optional(&mut *tx)
        .await?;
        if ok.is_none() {
            return Err(ApiError::BadRequest(
                "the cover must be one of the person's faces".into(),
            ));
        }
        sqlx::query("UPDATE people SET cover_face_id = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(face)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    get(State(s), user, unlock, Path(id)).await
}

pub(super) async fn remove(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    on(&s)?;
    let mut tx = s.db.begin().await?;
    lock_person(&mut tx, user.id, id).await?;
    let (faces_n,): (i64,) = sqlx::query_as("SELECT count(*) FROM faces WHERE person_id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    // Its faces are free again (the foreign key); the decisions about them go with the person.
    sqlx::query("UPDATE faces SET assigned_by = 'auto' WHERE person_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM people WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut *tx,
        Some(user.id),
        "people.delete",
        None,
        None,
        json!({"faces": faces_n}),
    )
    .await?;
    people::request(&mut *tx, user.id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
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
) -> ApiResult<Json<Value>> {
    on(&s)?;
    if req.into == id {
        return Err(ApiError::BadRequest("a person cannot be merged into themselves".into()));
    }
    let mut tx = s.db.begin().await?;
    // Both locks in id order, so two merges the other way round cannot wait for each other.
    let (first, second) = if id < req.into { (id, req.into) } else { (req.into, id) };
    let n1 = lock_person(&mut tx, user.id, first).await?;
    let n2 = lock_person(&mut tx, user.id, second).await?;
    let (from_name, into_name) = if first == id { (n1, n2) } else { (n2, n1) };
    let moved = sqlx::query("UPDATE faces SET person_id = $2 WHERE person_id = $1")
        .bind(id)
        .bind(req.into)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    // "Not the first person" now means "not the second", unless the face is the second's by now.
    sqlx::query(
        "INSERT INTO face_rejections (face_id, person_id)
         SELECT r.face_id, $2 FROM face_rejections r JOIN faces f ON f.id = r.face_id
         WHERE r.person_id = $1 AND f.person_id IS DISTINCT FROM $2
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(req.into)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM face_rejections r USING faces f WHERE f.id = r.face_id AND r.person_id = $1 AND f.person_id = $1",
    )
    .bind(req.into)
    .execute(&mut *tx)
    .await?;
    if into_name.is_none() && from_name.is_some() {
        sqlx::query("UPDATE people SET name = $2 WHERE id = $1")
            .bind(req.into)
            .bind(from_name)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE people SET updated_at = now() WHERE id = $1")
        .bind(req.into)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM people WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut *tx,
        Some(user.id),
        "people.merge",
        None,
        None,
        json!({"faces": moved}),
    )
    .await?;
    people::request(&mut *tx, user.id).await?;
    tx.commit().await?;
    Ok(Json(json!({"into": req.into, "faces": moved})))
}

pub(super) async fn photos(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<ListResponse>> {
    on(&s)?;
    if load(&s, user.id, unlock.0, Some(id)).await?.is_empty() {
        return Err(ApiError::NotFound);
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let after = match &q.cursor {
        Some(c) => Some(decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
        None => None,
    };
    let select = format!(
        "SELECT x.id, x.original_name, x.content_type, x.size_bytes, x.width, x.height, {SORT} AS sort_time, x.favorite, x.rating, x.archived, x.hidden, (x.recipe IS NOT NULL) AS edited, x.thumbhash
         FROM assets x
         WHERE x.owner_id = $2 AND {}
           AND EXISTS (SELECT 1 FROM faces f WHERE f.asset_id = x.id AND f.person_id = $1 AND {SCANNED})",
        seen("x", "$3")
    );
    let rows = match after {
        Some((ts, aid)) => {
            sqlx::query_as::<_, ListRow>(&format!(
                "{select} AND ({SORT}, x.id) < ($4, $5) ORDER BY {SORT} DESC, x.id DESC LIMIT $6"
            ))
            .bind(id)
            .bind(user.id)
            .bind(unlock.0)
            .bind(ts)
            .bind(aid)
            .bind(limit)
            .fetch_all(&s.db)
            .await?
        }
        None => {
            sqlx::query_as::<_, ListRow>(&format!("{select} ORDER BY {SORT} DESC, x.id DESC LIMIT $4"))
                .bind(id)
                .bind(user.id)
                .bind(unlock.0)
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

#[derive(Deserialize)]
pub(super) struct RemoveFaces {
    #[serde(default)]
    asset_ids: Vec<Uuid>,
    #[serde(default)]
    face_ids: Vec<Uuid>,
}

pub(super) async fn remove_faces(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Json(req): Json<RemoveFaces>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    if req.asset_ids.is_empty() && req.face_ids.is_empty() {
        return Err(ApiError::BadRequest("send asset_ids or face_ids".into()));
    }
    if req.asset_ids.len() + req.face_ids.len() > MAX_IDS {
        return Err(ApiError::BadRequest(format!("at most {MAX_IDS} at a time")));
    }
    let mut tx = s.db.begin().await?;
    lock_person(&mut tx, user.id, id).await?;
    // The person's faces among those named, in photos this session may see.
    let gone: Vec<Uuid> = sqlx::query_scalar(&format!(
        "UPDATE faces f SET person_id = NULL, assigned_by = 'auto'
         FROM assets a
         WHERE a.id = f.asset_id AND f.person_id = $1 AND f.owner_id = $2 AND {}
           AND (f.id = ANY($4) OR f.asset_id = ANY($5))
         RETURNING f.id",
        seen("a", "$3")
    ))
    .bind(id)
    .bind(user.id)
    .bind(unlock.0)
    .bind(&req.face_ids)
    .bind(&req.asset_ids)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO face_rejections (face_id, person_id) SELECT x, $2 FROM unnest($1::uuid[]) x ON CONFLICT DO NOTHING")
        .bind(&gone)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE people SET cover_face_id = NULL WHERE id = $1 AND cover_face_id = ANY($2)")
        .bind(id)
        .bind(&gone)
        .execute(&mut *tx)
        .await?;
    if !gone.is_empty() {
        people::request(&mut *tx, user.id).await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"removed": gone.len()})))
}

#[derive(Deserialize, Default)]
pub(super) struct SuggestQuery {
    limit: Option<i64>,
}

pub(super) async fn suggestions(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Query(q): Query<SuggestQuery>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    let known = load(&s, user.id, unlock.0, Some(id)).await?;
    if known.is_empty() {
        return Err(ApiError::NotFound);
    }
    let limit = q.limit.unwrap_or(12).clamp(1, MAX_SUGGESTIONS);
    let found = people::suggestions(&s.db, user.id, id, unlock.0, limit).await?;
    let mut items = Vec::with_capacity(found.len());
    for f in found {
        items.push(json!({
            "face_id": f.face_id,
            "asset_id": f.asset_id,
            "box": f.bbox,
            "preview_url": s.storage.presign_get(&keys::preview(user.id, f.asset_id)).await?,
            "similarity": f.similarity,
        }));
    }
    Ok(Json(json!({"items": items})))
}

#[derive(Deserialize)]
pub(super) struct AssignBody {
    person_id: Option<Uuid>,
    #[serde(default)]
    new: bool,
    name: Option<String>,
}

/// The face, if it is the caller's and in a photo this session may see; locked, with the person it is in now.
async fn lock_face(tx: &mut sqlx::PgConnection, owner: Uuid, unlock: bool, face: Uuid) -> ApiResult<Option<Uuid>> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(&format!(
        "SELECT f.person_id FROM faces f JOIN assets a ON a.id = f.asset_id
         WHERE f.id = $1 AND f.owner_id = $2 AND {SCANNED} AND {} FOR UPDATE OF f",
        seen("a", "$3")
    ))
    .bind(face)
    .bind(owner)
    .bind(unlock)
    .fetch_optional(tx)
    .await?;
    row.map(|r| r.0).ok_or(ApiError::NotFound)
}

pub(super) async fn assign(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(face): Path<Uuid>,
    Json(req): Json<AssignBody>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    if req.person_id.is_some() == req.new {
        return Err(ApiError::BadRequest("send person_id, or new: true".into()));
    }
    let name = match (&req.name, req.new) {
        (Some(n), true) => clean_name(n)?,
        (Some(_), false) => return Err(ApiError::BadRequest("name goes with new: true".into())),
        _ => None,
    };
    let mut tx = s.db.begin().await?;
    let was = lock_face(&mut tx, user.id, unlock.0, face).await?;
    let target = match req.person_id {
        Some(p) => {
            lock_person(&mut tx, user.id, p).await?;
            p
        }
        None => {
            sqlx::query_scalar("INSERT INTO people (owner_id, name) VALUES ($1, $2) RETURNING id")
                .bind(user.id)
                .bind(name)
                .fetch_one(&mut *tx)
                .await?
        }
    };
    if was != Some(target) {
        sqlx::query("UPDATE faces SET person_id = $2, assigned_by = 'user' WHERE id = $1")
            .bind(face)
            .bind(target)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM face_rejections WHERE face_id = $1 AND person_id = $2")
            .bind(face)
            .bind(target)
            .execute(&mut *tx)
            .await?;
        if let Some(from) = was {
            // Moved: it is not the first person.
            sqlx::query("INSERT INTO face_rejections (face_id, person_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(face)
                .bind(from)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE people SET cover_face_id = NULL WHERE id = $1 AND cover_face_id = $2")
                .bind(from)
                .bind(face)
                .execute(&mut *tx)
                .await?;
            // An unnamed person left without faces is not kept.
            sqlx::query(
                "DELETE FROM people p WHERE p.id = $1 AND p.name IS NULL AND NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = p.id)",
            )
            .bind(from)
            .execute(&mut *tx)
            .await?;
        }
        people::request(&mut *tx, user.id).await?;
    }
    tx.commit().await?;
    let person = load(&s, user.id, unlock.0, Some(target)).await?.pop();
    Ok(Json(json!({"person": person})))
}

#[derive(Deserialize)]
pub(super) struct RejectBody {
    person_id: Uuid,
}

pub(super) async fn reject(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(face): Path<Uuid>,
    Json(req): Json<RejectBody>,
) -> ApiResult<StatusCode> {
    on(&s)?;
    let mut tx = s.db.begin().await?;
    let was = lock_face(&mut tx, user.id, unlock.0, face).await?;
    lock_person(&mut tx, user.id, req.person_id).await?;
    if was == Some(req.person_id) {
        return Err(ApiError::BadRequest(
            "the face is in that person: remove it from them instead".into(),
        ));
    }
    sqlx::query("INSERT INTO face_rejections (face_id, person_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(face)
        .bind(req.person_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(FromRow)]
struct PhotoFace {
    id: Uuid,
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
    person_id: Option<Uuid>,
    name: Option<String>,
    hidden: Option<bool>,
}

pub(super) async fn of_photo(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(asset): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    let live: Option<Uuid> = sqlx::query_scalar(&format!(
        "SELECT a.id FROM assets a WHERE a.id = $1 AND a.owner_id = $2 AND {}",
        seen("a", "$3")
    ))
    .bind(asset)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_optional(&s.db)
    .await?;
    live.ok_or(ApiError::NotFound)?;
    // The faces of the model that read the photo last (the current one once it has; an older one's meanwhile).
    let rows: Vec<PhotoFace> = sqlx::query_as(
        "SELECT f.id, f.box_x, f.box_y, f.box_w, f.box_h, f.person_id, p.name, p.hidden
         FROM faces f LEFT JOIN people p ON p.id = f.person_id
         WHERE f.asset_id = $1 AND f.owner_id = $2
           AND f.model = (SELECT s.model FROM face_scans s WHERE s.asset_id = $1 ORDER BY (s.model = $3) DESC, s.scanned_at DESC LIMIT 1)
         ORDER BY f.idx",
    )
    .bind(asset)
    .bind(user.id)
    .bind(faces::MODEL)
    .fetch_all(&s.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .filter(|r| r.hidden != Some(true))
        .map(|r| {
            json!({
                "id": r.id,
                "box": [r.box_x, r.box_y, r.box_w, r.box_h],
                "person": r.person_id.map(|id| json!({"id": id, "name": r.name})),
            })
        })
        .collect();
    Ok(Json(json!({"items": items})))
}
