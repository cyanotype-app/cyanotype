//! Named pets (`docs/pets.md`, "Named pets"): the owner makes a pet ("Biscuit"), tags a few photos of it, the app proposes more by how much
//! the picture of the animal looks like the tagged ones, and the owner confirms or refuses. Nothing is automatic: an animal found in a photo
//! (`pet_detections`) joins a pet only when the owner says so. Under `PETS_ENABLED` like the rest of Pets (off: every route is a `404`),
//! owner-only, and never reachable from a share link, a public page, a download or a recovery manifest. A pet's name is personal data:
//! it is never written to the bucket, the audit trail or a log.
//!
//! The routes live under `/assets/pets/named` (`/assets/pets` itself is the summary of the kinds):
//!
//! - `GET /assets/pets/named`: `{items: [{id, name, kind, hidden, photos, detections, pending, cover}]}`, most photos first. `kind` is `dog` or
//!   `cat`; `cover` is `{detection_id, asset_id, box: [x, y, w, h], preview_url}` (the box is fractions of the preview image the URL serves;
//!   the web app crops it with CSS) or null. Counts are of the photos this session may see (live, not a RAW half, not hidden unless the
//!   Hidden view is unlocked); a pet only hidden photos have is not listed to a locked session. `pending` is how many proposals ("Is this
//!   Biscuit?") wait for a shown pet (0 for a hidden one), counted for all the pets in one query. `GET /assets/pets/named/{id}` is one.
//! - `POST /assets/pets/named {name, kind, asset_ids?, detection_ids?}`: make a pet from photos: `{pet, assigned, ambiguous, skipped}`. See
//!   [`add_in`] for what the lists say. The pet is made even when no photo could be resolved (the caller then picks among `ambiguous`).
//! - `PATCH /assets/pets/named/{id} {name?, hidden?, cover_detection_id?}`, `DELETE` (its animals are free again; the photos stay),
//!   `POST .../merge {into}` (same kind only; the first pet's animals go to the second, which keeps its name).
//!
//! The owner's word wins over the detector's kind: an animal put in a pet of the other kind takes the pet's kind (`pet_detections.kind_by_owner`).
//! - `GET .../detections?limit&cursor`: the animals a cover can be chosen from (`{detection_id, asset_id, box, preview_url}`).
//! - `GET .../photos?limit&cursor|before` and `GET .../timeline`: the pet's photos, like `GET /assets` and its timeline. A
//!   `{"pet": "<id>"}` scope (`scope.rs`), the `pet=<id>,<id>` filter of search and the `pet` smart-album rule name the same photos.
//! - `POST .../photos {asset_ids?, detection_ids?}`: add photos (or animals) to the pet: `{pet, assigned, ambiguous, skipped}`.
//!   `POST .../photos/remove {asset_ids?, detection_ids?}`: "not this pet": `{removed}`, and the animals are never proposed for it again.
//! - `GET .../suggestions?limit&tier=likely|maybe`: "Is this Biscuit?": `{items: [{detection_id, asset_id, box, preview_url, similarity,
//!   kind, tier}], counts: {likely, maybe}}`, best first, free animals (of either kind) that are not refused for it. `POST
//!   .../suggestions/answer {confirm?: [detection_id], reject?: [detection_id]}`: answer many at once: `{confirmed, rejected, skipped}`.
//! - `POST /assets/pets/detections/{id}/assign {pet_id}`, `POST /assets/pets/detections/{id}/reject {pet_id}`: the same for one animal.
//! - `POST /assets/pets/detections/{id}/kind {kind}`: the owner says an animal is a dog or a cat (see [`set_kind`]).
//!
//! Ids of someone else's pets, animals and photos are `404`. Moving an animal from one pet to another refuses it for the first.
use std::collections::{BTreeMap, HashMap, HashSet};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use cyanotype_core::{
    audit, keys,
    named_pets::{self, Tier},
    pets,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    ListQuery, ListResponse, ListRow, decode_cursor, encode_cursor, list_items,
    people::{clean_name, seen},
    timeline::{Day, TimelineResponse},
    trash::MAX_IDS,
};
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

/// Most pets one owner can have: every pet is compared with all the owner's free animals.
const MAX_PETS: i64 = 500;
const MAX_SUGGESTIONS: i64 = 100;
/// Most animals one page of a pet's cover choices holds.
const MAX_COVERS: i64 = 60;

/// Same expression as the library timeline (`x` is the assets alias).
const SORT: &str = "coalesce(x.taken_at, x.file_modified_at, x.created_at)";

/// While the feature is off nothing about Pets exists.
fn on(s: &AppState) -> ApiResult<()> {
    if s.pets_enabled {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// A name is required: a pet exists because the owner named it.
fn pet_name(n: &str) -> ApiResult<String> {
    clean_name(n)?.ok_or_else(|| ApiError::BadRequest("a pet needs a name".into()))
}

#[derive(Serialize)]
struct Cover {
    detection_id: Uuid,
    asset_id: Uuid,
    #[serde(rename = "box")]
    bbox: [f32; 4],
    preview_url: String,
}

#[derive(Serialize)]
pub(super) struct Pet {
    id: Uuid,
    name: String,
    kind: String,
    hidden: bool,
    /// The pet's animals in the photos this session may see, and how many photos they are in.
    detections: i64,
    photos: i64,
    /// Proposals waiting for an answer: 0 for a hidden pet.
    pending: i64,
    cover: Option<Cover>,
}

#[derive(FromRow)]
struct PetRow {
    id: Uuid,
    name: String,
    kind: String,
    hidden: bool,
    detections: i64,
    photos: i64,
}

#[derive(FromRow)]
struct CoverRow {
    pet_id: Uuid,
    id: Uuid,
    asset_id: Uuid,
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
}

/// The owner's pets (`only` one of them), with counts and covers as this session may see them.
async fn load(s: &AppState, owner: Uuid, unlock: bool, only: Option<Uuid>) -> ApiResult<Vec<Pet>> {
    let rows: Vec<PetRow> = sqlx::query_as(&format!(
        "SELECT p.id, p.name, p.kind, p.hidden,
                count(d.id) FILTER (WHERE a.id IS NOT NULL AND (NOT a.hidden OR $2)) AS detections,
                count(DISTINCT d.asset_id) FILTER (WHERE a.id IS NOT NULL AND (NOT a.hidden OR $2)) AS photos
         FROM pets p
         LEFT JOIN pet_detections d ON d.pet_id = p.id AND {}
         LEFT JOIN assets a ON a.id = d.asset_id AND a.deleted_at IS NULL AND a.purging_at IS NULL AND a.primary_id IS NULL
         WHERE p.owner_id = $1 AND ($3::uuid IS NULL OR p.id = $3)
         GROUP BY p.id
         -- A pet only hidden photos have is not for a locked session to see.
         HAVING $2 OR count(d.id) FILTER (WHERE a.hidden) = 0 OR count(d.id) FILTER (WHERE a.id IS NOT NULL AND NOT a.hidden) > 0
         ORDER BY photos DESC, lower(p.name), p.created_at, p.id",
        pets::SCANNED
    ))
    .bind(owner)
    .bind(unlock)
    .bind(only)
    .fetch_all(&s.db)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let pending = named_pets::pending_counts(&s.db, owner, only, unlock)
        .await
        .map_err(ApiError::Internal)?;
    // The cover: the chosen animal when this session may see its photo, else the clearest one (big and sure of itself).
    let covers: Vec<CoverRow> = sqlx::query_as(&format!(
        "SELECT DISTINCT ON (d.pet_id) d.pet_id, d.id, d.asset_id, d.box_x, d.box_y, d.box_w, d.box_h
         FROM pet_detections d JOIN pets p ON p.id = d.pet_id JOIN assets x ON x.id = d.asset_id
         WHERE d.pet_id = ANY($1) AND p.owner_id = $2 AND {} AND x.thumbhash IS NOT NULL AND {}
         ORDER BY d.pet_id, (d.id = p.cover_detection_id) DESC, d.score * d.box_w * d.box_h DESC, d.id",
        pets::SCANNED,
        seen("x", "$3")
    ))
    .bind(&ids)
    .bind(owner)
    .bind(unlock)
    .fetch_all(&s.db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let cover = match covers.iter().find(|c| c.pet_id == r.id) {
            Some(c) => Some(Cover {
                detection_id: c.id,
                asset_id: c.asset_id,
                bbox: [c.box_x, c.box_y, c.box_w, c.box_h],
                preview_url: s.storage.presign_get(&keys::preview(owner, c.asset_id)).await?,
            }),
            None => None,
        };
        out.push(Pet {
            pending: pending.get(&r.id).copied().unwrap_or(0),
            id: r.id,
            name: r.name,
            kind: r.kind,
            hidden: r.hidden,
            detections: r.detections,
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
) -> ApiResult<Json<Pet>> {
    on(&s)?;
    load(&s, user.id, unlock.0, Some(id))
        .await?
        .pop()
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// The pet, if it is the caller's (a lock on its row, so a merge or a delete cannot race another change): its name and kind.
async fn lock_pet(tx: &mut sqlx::PgConnection, owner: Uuid, id: Uuid) -> ApiResult<(String, String)> {
    sqlx::query_as::<_, (String, String)>("SELECT name, kind FROM pets WHERE id = $1 AND owner_id = $2 FOR UPDATE")
        .bind(id)
        .bind(owner)
        .fetch_optional(tx)
        .await?
        .ok_or(ApiError::NotFound)
}

fn too_many(n: usize) -> ApiResult<()> {
    if n > MAX_IDS {
        return Err(ApiError::BadRequest(format!("at most {MAX_IDS} at a time")));
    }
    Ok(())
}

/// Why a photo of `add_in`'s `asset_ids` joined no pet.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum Skip {
    /// No animal was found in the photo.
    NoAnimal,
    /// The photo has not been looked at yet (or is being looked at again after an edit).
    NotScanned,
    /// The photo already has an animal of this pet in it.
    Already,
    /// Every animal in the photo is another pet's.
    InOtherPet,
}

/// A photo with several free animals (or none of the pet's kind): the photo and each animal's id, kind and box.
type Ambiguous = (Uuid, Vec<(Uuid, String, [f32; 4])>);

#[derive(Default)]
struct Added {
    assigned: Vec<Uuid>,
    /// `(asset, candidates)`: photos with several free animals of the kind, or only free ones of the other kind, for the caller to choose from.
    ambiguous: Vec<Ambiguous>,
    skipped: Vec<(Uuid, Skip)>,
}

#[derive(FromRow)]
struct Det {
    id: Uuid,
    asset_id: Uuid,
    kind: String,
    pet_id: Option<Uuid>,
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
}

/// Put animals in `pet` (locked, owner-checked, of `kind` by the caller), and say what happened to each photo asked for.
///
/// `detection_ids` are animals the caller chose: each must be the owner's and in a photo this session may see (`404` otherwise), of either
/// kind (one of the other kind becomes the pet's kind); one that is another pet's moves (and is refused for that one). `asset_ids` are photos: the photo's free animals of the pet's
/// kind decide. One: it joins. Several, or none of the kind but free ones of the other kind (maybe a mislabeled dog): none joins, and the photo comes back in `ambiguous` with the candidates, for the caller to pick
/// from (a second call with `detection_ids`). None found, not looked at yet, already in this pet, or only other pets' animals: it
/// is in `skipped` with the reason. A photo that is not the owner's or that this session may not see is a `404` and nothing changes.
async fn add_in(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    unlock: bool,
    pet: Uuid,
    kind: &str,
    asset_ids: &[Uuid],
    detection_ids: &[Uuid],
) -> ApiResult<Added> {
    let mut out = Added::default();
    let mut taken: HashSet<Uuid> = HashSet::new();
    if !detection_ids.is_empty() {
        let mut ids = detection_ids.to_vec();
        ids.sort();
        ids.dedup();
        let rows: Vec<Det> = sqlx::query_as(&format!(
            "SELECT d.id, d.asset_id, d.kind, d.pet_id, d.box_x, d.box_y, d.box_w, d.box_h
             FROM pet_detections d JOIN assets a ON a.id = d.asset_id
             WHERE d.id = ANY($1) AND d.owner_id = $2 AND {} AND {}
             ORDER BY d.id FOR UPDATE OF d",
            pets::SCANNED,
            seen("a", "$3")
        ))
        .bind(&ids)
        .bind(owner)
        .bind(unlock)
        .fetch_all(&mut *tx)
        .await?;
        if rows.len() != ids.len() {
            return Err(ApiError::NotFound);
        }
        for r in &rows {
            taken.insert(r.asset_id);
            if r.pet_id == Some(pet) {
                continue;
            }
            assign_in(&mut *tx, r.id, r.pet_id, pet, kind).await?;
            out.assigned.push(r.id);
        }
    }
    if !asset_ids.is_empty() {
        let mut ids = asset_ids.to_vec();
        ids.sort();
        ids.dedup();
        let live: Vec<Uuid> = sqlx::query_scalar(&format!(
            "SELECT a.id FROM assets a WHERE a.id = ANY($1) AND a.owner_id = $2 AND {}",
            seen("a", "$3")
        ))
        .bind(&ids)
        .bind(owner)
        .bind(unlock)
        .fetch_all(&mut *tx)
        .await?;
        if live.len() != ids.len() {
            return Err(ApiError::NotFound);
        }
        let rows: Vec<Det> = sqlx::query_as(&format!(
            "SELECT d.id, d.asset_id, d.kind, d.pet_id, d.box_x, d.box_y, d.box_w, d.box_h
             FROM pet_detections d
             WHERE d.asset_id = ANY($1) AND d.owner_id = $2 AND {}
             ORDER BY d.asset_id, d.idx FOR UPDATE OF d",
            pets::SCANNED
        ))
        .bind(&ids)
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        let scanned: HashSet<Uuid> = sqlx::query_scalar("SELECT asset_id FROM pet_scans WHERE asset_id = ANY($1)")
            .bind(&ids)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
        let mut by_asset: BTreeMap<Uuid, Vec<&Det>> = BTreeMap::new();
        for r in &rows {
            by_asset.entry(r.asset_id).or_default().push(r);
        }
        for asset in ids {
            let dets = by_asset.get(&asset).map(Vec::as_slice).unwrap_or(&[]);
            if taken.contains(&asset) || dets.iter().any(|d| d.pet_id == Some(pet)) {
                out.skipped.push((asset, Skip::Already));
                continue;
            }
            let free: Vec<&&Det> = dets.iter().filter(|d| d.pet_id.is_none()).collect();
            // The photo's free animals of the pet's kind decide; with none of them, the owner picks among the others (the detector may have
            // called the pet the other kind, and the owner's word wins: `assign_in`).
            let of_kind: Vec<&&Det> = free.iter().copied().filter(|d| d.kind == kind).collect();
            match (of_kind.len(), free.len()) {
                (0, 0) if dets.is_empty() => out.skipped.push((
                    asset,
                    if scanned.contains(&asset) {
                        Skip::NoAnimal
                    } else {
                        Skip::NotScanned
                    },
                )),
                (0, 0) => out.skipped.push((asset, Skip::InOtherPet)),
                (1, _) => {
                    assign_in(&mut *tx, of_kind[0].id, None, pet, kind).await?;
                    out.assigned.push(of_kind[0].id);
                }
                _ => out.ambiguous.push((
                    asset,
                    free.iter()
                        .map(|d| (d.id, d.kind.clone(), [d.box_x, d.box_y, d.box_w, d.box_h]))
                        .collect(),
                )),
            }
        }
    }
    sqlx::query("UPDATE pets SET updated_at = now() WHERE id = $1")
        .bind(pet)
        .execute(&mut *tx)
        .await?;
    Ok(out)
}

/// The owner says animal `det` (in pet `was`, if any) is `pet` (a `pet_kind`): it moves (and is refused for the pet it left), and
/// an earlier refusal for `pet` is forgotten. An animal the detector called the other kind becomes the pet's kind, as the owner's word.
async fn assign_in(
    tx: &mut sqlx::PgConnection,
    det: Uuid,
    was: Option<Uuid>,
    pet: Uuid,
    pet_kind: &str,
) -> ApiResult<()> {
    sqlx::query(
        "UPDATE pet_detections SET pet_id = $2, kind = $3, kind_by_owner = kind_by_owner OR kind <> $3 WHERE id = $1",
    )
    .bind(det)
    .bind(pet)
    .bind(pet_kind)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM pet_rejections WHERE detection_id = $1 AND pet_id = $2")
        .bind(det)
        .bind(pet)
        .execute(&mut *tx)
        .await?;
    if let Some(from) = was {
        sqlx::query("INSERT INTO pet_rejections (detection_id, pet_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(det)
            .bind(from)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE pets SET cover_detection_id = NULL WHERE id = $1 AND cover_detection_id = $2")
            .bind(from)
            .bind(det)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

/// What `add_in` did, as the routes answer it: the photos with several animals come with their candidates and a preview to choose on.
async fn added_json(s: &AppState, owner: Uuid, added: &Added) -> ApiResult<Value> {
    let mut ambiguous = Vec::with_capacity(added.ambiguous.len());
    for (asset, cands) in &added.ambiguous {
        ambiguous.push(json!({
            "asset_id": asset,
            "preview_url": s.storage.presign_get(&keys::preview(owner, *asset)).await?,
            "candidates": cands.iter().map(|(id, kind, b)| json!({"detection_id": id, "kind": kind, "box": b})).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({
        "assigned": added.assigned.len(),
        "ambiguous": ambiguous,
        "skipped": added.skipped.iter().map(|(a, why)| json!({"asset_id": a, "reason": why})).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
pub(super) struct CreateBody {
    name: String,
    kind: String,
    #[serde(default)]
    asset_ids: Vec<Uuid>,
    #[serde(default)]
    detection_ids: Vec<Uuid>,
}

pub(super) async fn create(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Json(req): Json<CreateBody>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    let name = pet_name(&req.name)?;
    let kind = req.kind.trim().to_ascii_lowercase();
    if !pets::is_kind(&kind) {
        return Err(ApiError::BadRequest("kind must be dog or cat".into()));
    }
    too_many(req.asset_ids.len() + req.detection_ids.len())?;
    let mut tx = s.db.begin().await?;
    // Serialises two creates of one owner, so the cap holds.
    sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let (have,): (i64,) = sqlx::query_as("SELECT count(*) FROM pets WHERE owner_id = $1")
        .bind(user.id)
        .fetch_one(&mut *tx)
        .await?;
    if have >= MAX_PETS {
        return Err(ApiError::BadRequest(format!("at most {MAX_PETS} pets")));
    }
    let id: Uuid = sqlx::query_scalar("INSERT INTO pets (owner_id, name, kind) VALUES ($1, $2, $3) RETURNING id")
        .bind(user.id)
        .bind(&name)
        .bind(&kind)
        .fetch_one(&mut *tx)
        .await?;
    let added = add_in(
        &mut tx,
        user.id,
        unlock.0,
        id,
        &kind,
        &req.asset_ids,
        &req.detection_ids,
    )
    .await?;
    tx.commit().await?;
    let pet = load(&s, user.id, unlock.0, Some(id)).await?.pop();
    let mut body = added_json(&s, user.id, &added).await?;
    body["pet"] = json!(pet);
    Ok(Json(body))
}

#[derive(Deserialize)]
pub(super) struct Patch {
    name: Option<String>,
    hidden: Option<bool>,
    cover_detection_id: Option<Uuid>,
}

pub(super) async fn patch(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Json(req): Json<Patch>,
) -> ApiResult<Json<Pet>> {
    on(&s)?;
    if req.name.is_none() && req.hidden.is_none() && req.cover_detection_id.is_none() {
        return Err(ApiError::BadRequest(
            "nothing to change: send name, hidden or cover_detection_id".into(),
        ));
    }
    let name = req.name.as_deref().map(pet_name).transpose()?;
    let mut tx = s.db.begin().await?;
    lock_pet(&mut tx, user.id, id).await?;
    if let Some(name) = name {
        sqlx::query("UPDATE pets SET name = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(name)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(hidden) = req.hidden {
        sqlx::query("UPDATE pets SET hidden = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(hidden)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(det) = req.cover_detection_id {
        // An animal of this pet, in a photo this session may see.
        let ok: Option<Uuid> = sqlx::query_scalar(&format!(
            "SELECT d.id FROM pet_detections d JOIN assets a ON a.id = d.asset_id
             WHERE d.id = $1 AND d.pet_id = $2 AND d.owner_id = $3 AND {} AND a.thumbhash IS NOT NULL AND {}",
            pets::SCANNED,
            seen("a", "$4")
        ))
        .bind(det)
        .bind(id)
        .bind(user.id)
        .bind(unlock.0)
        .fetch_optional(&mut *tx)
        .await?;
        if ok.is_none() {
            return Err(ApiError::BadRequest(
                "the cover must be one of the pet's animals".into(),
            ));
        }
        sqlx::query("UPDATE pets SET cover_detection_id = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(det)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    get(State(s), user, unlock, Path(id)).await
}

pub(super) async fn remove(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    on(&s)?;
    let mut tx = s.db.begin().await?;
    lock_pet(&mut tx, user.id, id).await?;
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM pet_detections WHERE pet_id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    // Its animals are free again (the foreign key); the refusals for it go with it.
    sqlx::query("DELETE FROM pets WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut *tx,
        Some(user.id),
        "pets.delete",
        None,
        None,
        json!({"detections": n}),
    )
    .await?;
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
        return Err(ApiError::BadRequest("a pet cannot be merged into itself".into()));
    }
    let mut tx = s.db.begin().await?;
    // Both locks in id order, so two merges the other way round cannot wait for each other.
    let (first, second) = if id < req.into { (id, req.into) } else { (req.into, id) };
    let a = lock_pet(&mut tx, user.id, first).await?;
    let b = lock_pet(&mut tx, user.id, second).await?;
    let (from, into) = if first == id { (a, b) } else { (b, a) };
    if from.1 != into.1 {
        return Err(ApiError::BadRequest("only pets of one kind can be merged".into()));
    }
    let moved = sqlx::query("UPDATE pet_detections SET pet_id = $2 WHERE pet_id = $1")
        .bind(id)
        .bind(req.into)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    // "Not the first pet" now means "not the second", unless the animal is the second's by now.
    sqlx::query(
        "INSERT INTO pet_rejections (detection_id, pet_id)
         SELECT r.detection_id, $2 FROM pet_rejections r JOIN pet_detections d ON d.id = r.detection_id
         WHERE r.pet_id = $1 AND d.pet_id IS DISTINCT FROM $2
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(req.into)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM pet_rejections r USING pet_detections d WHERE d.id = r.detection_id AND r.pet_id = $1 AND d.pet_id = $1",
    )
    .bind(req.into)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE pets SET updated_at = now() WHERE id = $1")
        .bind(req.into)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM pets WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut *tx,
        Some(user.id),
        "pets.merge",
        None,
        None,
        json!({"detections": moved}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"into": req.into, "detections": moved})))
}

#[derive(Deserialize, Default)]
pub(super) struct CoverQuery {
    limit: Option<i64>,
    /// How many to skip (what `next_cursor` of the page before says).
    cursor: Option<String>,
}

#[derive(FromRow)]
struct CoverDet {
    id: Uuid,
    asset_id: Uuid,
    box_x: f32,
    box_y: f32,
    box_w: f32,
    box_h: f32,
}

/// `GET .../detections?limit&cursor`: the pet's animals a cover can be chosen from (in photos this session may see, with a preview), best
/// first, `{items: [{detection_id, asset_id, box, preview_url}], next_cursor}`.
pub(super) async fn cover_detections(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Query(q): Query<CoverQuery>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    if load(&s, user.id, unlock.0, Some(id)).await?.is_empty() {
        return Err(ApiError::NotFound);
    }
    let limit = q.limit.unwrap_or(24).clamp(1, MAX_COVERS);
    let skip: i64 = match &q.cursor {
        Some(c) => c
            .parse()
            .ok()
            .filter(|n| *n >= 0)
            .ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?,
        None => 0,
    };
    // One more than asked: whether there is a next page.
    let mut rows: Vec<CoverDet> = sqlx::query_as(&format!(
        "SELECT d.id, d.asset_id, d.box_x, d.box_y, d.box_w, d.box_h
         FROM pet_detections d JOIN assets x ON x.id = d.asset_id
         WHERE d.pet_id = $1 AND d.owner_id = $2 AND {} AND x.thumbhash IS NOT NULL AND {}
         ORDER BY d.score * d.box_w * d.box_h DESC, d.id LIMIT $4 OFFSET $5",
        pets::SCANNED,
        seen("x", "$3")
    ))
    .bind(id)
    .bind(user.id)
    .bind(unlock.0)
    .bind(limit + 1)
    .bind(skip)
    .fetch_all(&s.db)
    .await?;
    let more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let mut items = Vec::new();
    for r in &rows {
        items.push(json!({
            "detection_id": r.id,
            "asset_id": r.asset_id,
            "box": [r.box_x, r.box_y, r.box_w, r.box_h],
            "preview_url": s.storage.presign_get(&keys::preview(user.id, r.asset_id)).await?,
        }));
    }
    let next = more.then(|| (skip + limit).to_string());
    Ok(Json(json!({"items": items, "next_cursor": next})))
}

/// SQL: photo `x` has an animal of pet `$1` in it.
fn with_pet(pet: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM pet_detections d WHERE d.asset_id = x.id AND d.pet_id = {pet} AND {})",
        pets::SCANNED
    )
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
    let after = match (&q.cursor, q.before) {
        (Some(c), _) => Some(decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
        (None, Some(t)) => Some((t, Uuid::nil())),
        (None, None) => None,
    };
    let select = format!(
        "SELECT x.id, x.original_name, x.content_type, x.size_bytes, x.width, x.height, {SORT} AS sort_time, x.favorite, x.rating, x.archived, x.hidden, (x.recipe IS NOT NULL) AS edited, x.thumbhash
         FROM assets x
         WHERE x.owner_id = $2 AND {} AND {}",
        seen("x", "$3"),
        with_pet("$1")
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

pub(super) async fn timeline(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TimelineResponse>> {
    on(&s)?;
    if load(&s, user.id, unlock.0, Some(id)).await?.is_empty() {
        return Err(ApiError::NotFound);
    }
    let days: Vec<Day> = sqlx::query_as(&format!(
        "SELECT to_char(d, 'YYYY-MM-DD') AS day, count(*) AS count FROM (
         SELECT ({SORT} AT TIME ZONE 'UTC')::date AS d FROM assets x
         WHERE x.owner_id = $2 AND {} AND {}) t
         GROUP BY d ORDER BY d DESC",
        seen("x", "$3"),
        with_pet("$1")
    ))
    .bind(id)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_all(&s.db)
    .await?;
    let total = days.iter().map(|d| d.count).sum();
    Ok(Json(TimelineResponse { total, days }))
}

#[derive(Deserialize)]
pub(super) struct PhotosBody {
    #[serde(default)]
    asset_ids: Vec<Uuid>,
    #[serde(default)]
    detection_ids: Vec<Uuid>,
}

pub(super) async fn add_photos(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Json(req): Json<PhotosBody>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    if req.asset_ids.is_empty() && req.detection_ids.is_empty() {
        return Err(ApiError::BadRequest("send asset_ids or detection_ids".into()));
    }
    too_many(req.asset_ids.len() + req.detection_ids.len())?;
    let mut tx = s.db.begin().await?;
    let (_, kind) = lock_pet(&mut tx, user.id, id).await?;
    let added = add_in(
        &mut tx,
        user.id,
        unlock.0,
        id,
        &kind,
        &req.asset_ids,
        &req.detection_ids,
    )
    .await?;
    tx.commit().await?;
    let pet = load(&s, user.id, unlock.0, Some(id)).await?.pop();
    let mut body = added_json(&s, user.id, &added).await?;
    body["pet"] = json!(pet);
    Ok(Json(body))
}

pub(super) async fn remove_photos(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Json(req): Json<PhotosBody>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    if req.asset_ids.is_empty() && req.detection_ids.is_empty() {
        return Err(ApiError::BadRequest("send asset_ids or detection_ids".into()));
    }
    too_many(req.asset_ids.len() + req.detection_ids.len())?;
    let mut tx = s.db.begin().await?;
    lock_pet(&mut tx, user.id, id).await?;
    // The pet's animals among those named, in photos this session may see.
    let gone: Vec<Uuid> = sqlx::query_scalar(&format!(
        "UPDATE pet_detections d SET pet_id = NULL
         FROM assets a
         WHERE a.id = d.asset_id AND d.pet_id = $1 AND d.owner_id = $2 AND {}
           AND (d.id = ANY($4) OR d.asset_id = ANY($5))
         RETURNING d.id",
        seen("a", "$3")
    ))
    .bind(id)
    .bind(user.id)
    .bind(unlock.0)
    .bind(&req.detection_ids)
    .bind(&req.asset_ids)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO pet_rejections (detection_id, pet_id) SELECT x, $2 FROM unnest($1::uuid[]) x ON CONFLICT DO NOTHING",
    )
    .bind(&gone)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE pets SET cover_detection_id = NULL WHERE id = $1 AND cover_detection_id = ANY($2)")
        .bind(id)
        .bind(&gone)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"removed": gone.len()})))
}

#[derive(Deserialize, Default)]
pub(super) struct SuggestQuery {
    limit: Option<i64>,
    tier: Option<String>,
}

pub(super) async fn suggestions(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Query(q): Query<SuggestQuery>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    let tier = match q.tier.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        None => None,
        Some(t) => Some(Tier::parse(t).ok_or_else(|| ApiError::BadRequest("tier must be likely or maybe".into()))?),
    };
    let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM pets WHERE id = $1 AND owner_id = $2")
        .bind(id)
        .bind(user.id)
        .fetch_optional(&s.db)
        .await?;
    exists.ok_or(ApiError::NotFound)?;
    let limit = q.limit.unwrap_or(24).clamp(1, MAX_SUGGESTIONS);
    // Everything the pet's centre reaches (at most 300), so the counts are of all of it; the list is cut afterwards.
    let all = named_pets::suggestions(&s.db, user.id, id, unlock.0, named_pets::CONSIDERED)
        .await
        .map_err(ApiError::Internal)?;
    let counts = json!({
        "likely": all.iter().filter(|f| f.tier == Tier::Likely).count(),
        "maybe": all.iter().filter(|f| f.tier == Tier::Maybe).count(),
    });
    let mut items = Vec::new();
    for f in all
        .iter()
        .filter(|f| tier.is_none_or(|t| f.tier == t))
        .take(limit as usize)
    {
        items.push(json!({
            "detection_id": f.detection_id,
            "asset_id": f.asset_id,
            "box": f.bbox,
            "preview_url": s.storage.presign_get(&keys::preview(user.id, f.asset_id)).await?,
            "kind": f.kind,
            "similarity": f.similarity,
            "tier": f.tier.name(),
        }));
    }
    Ok(Json(json!({"items": items, "counts": counts})))
}

#[derive(Deserialize)]
pub(super) struct AnswerBody {
    #[serde(default)]
    confirm: Vec<Uuid>,
    #[serde(default)]
    reject: Vec<Uuid>,
}

/// Answer many proposals at once: `confirm` puts the animals in the pet, `reject` refuses them for it. Only free animals
/// change: one that is another pet's or already this one's (to confirm), or already refused for it or in it (to reject), is `skipped`
/// (nothing changes for it); an id that is not the owner's, not in a photo this session may see is a `404` (and
/// nothing changes at all); a confirmed animal of the other kind takes the pet's kind. `{confirmed, rejected, skipped}`.
pub(super) async fn answer(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Json(req): Json<AnswerBody>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    if req.confirm.is_empty() && req.reject.is_empty() {
        return Err(ApiError::BadRequest("send confirm or reject".into()));
    }
    too_many(req.confirm.len() + req.reject.len())?;
    if req.confirm.iter().any(|c| req.reject.contains(c)) {
        return Err(ApiError::BadRequest(
            "an animal cannot be both confirmed and rejected".into(),
        ));
    }
    let mut tx = s.db.begin().await?;
    let (_, kind) = lock_pet(&mut tx, user.id, id).await?;
    let mut ids: Vec<Uuid> = req.confirm.iter().chain(&req.reject).copied().collect();
    ids.sort();
    ids.dedup();
    let rows: Vec<Det> = sqlx::query_as(&format!(
        "SELECT d.id, d.asset_id, d.kind, d.pet_id, d.box_x, d.box_y, d.box_w, d.box_h
         FROM pet_detections d JOIN assets a ON a.id = d.asset_id
         WHERE d.id = ANY($1) AND d.owner_id = $2 AND {} AND {}
         ORDER BY d.id FOR UPDATE OF d",
        pets::SCANNED,
        seen("a", "$3")
    ))
    .bind(&ids)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_all(&mut *tx)
    .await?;
    if rows.len() != ids.len() {
        return Err(ApiError::NotFound);
    }
    let by_id: HashMap<Uuid, &Det> = rows.iter().map(|r| (r.id, r)).collect();
    let (mut confirmed, mut rejected, mut skipped) = (0, 0, 0);
    let mut ids_ok = req.confirm.clone();
    ids_ok.sort();
    ids_ok.dedup();
    for det in ids_ok {
        let r = by_id[&det];
        if r.pet_id.is_some() {
            skipped += 1;
            continue;
        }
        assign_in(&mut tx, det, None, id, &kind).await?;
        confirmed += 1;
    }
    let mut rej = req.reject.clone();
    rej.sort();
    rej.dedup();
    for det in rej {
        let r = by_id[&det];
        if r.pet_id == Some(id) {
            skipped += 1;
            continue;
        }
        let new =
            sqlx::query("INSERT INTO pet_rejections (detection_id, pet_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(det)
                .bind(id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        if new == 0 {
            skipped += 1;
        } else {
            rejected += 1;
        }
    }
    sqlx::query("UPDATE pets SET updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"confirmed": confirmed, "rejected": rejected, "skipped": skipped}),
    ))
}

#[derive(Deserialize)]
pub(super) struct PetBody {
    pet_id: Uuid,
}

/// `POST /assets/pets/detections/{id}/assign {pet_id}`: the owner says this animal is that pet. Another pet's animal moves (and is refused for
/// the first). `{pet}`.
pub(super) async fn assign(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(det): Path<Uuid>,
    Json(req): Json<PetBody>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    let mut tx = s.db.begin().await?;
    let (_, kind) = lock_pet(&mut tx, user.id, req.pet_id).await?;
    add_in(&mut tx, user.id, unlock.0, req.pet_id, &kind, &[], &[det]).await?;
    tx.commit().await?;
    let pet = load(&s, user.id, unlock.0, Some(req.pet_id)).await?.pop();
    Ok(Json(json!({"pet": pet})))
}

/// `POST /assets/pets/detections/{id}/reject {pet_id}`: "no, not that pet" (a refused proposal).
pub(super) async fn reject(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(det): Path<Uuid>,
    Json(req): Json<PetBody>,
) -> ApiResult<StatusCode> {
    on(&s)?;
    let mut tx = s.db.begin().await?;
    lock_pet(&mut tx, user.id, req.pet_id).await?;
    let row: Option<Option<Uuid>> = sqlx::query_scalar(&format!(
        "SELECT d.pet_id FROM pet_detections d JOIN assets a ON a.id = d.asset_id
         WHERE d.id = $1 AND d.owner_id = $2 AND {} AND {} FOR UPDATE OF d",
        pets::SCANNED,
        seen("a", "$3")
    ))
    .bind(det)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_optional(&mut *tx)
    .await?;
    let was = row.ok_or(ApiError::NotFound)?;
    if was == Some(req.pet_id) {
        return Err(ApiError::BadRequest(
            "the animal is in that pet: remove it from them instead".into(),
        ));
    }
    sqlx::query("INSERT INTO pet_rejections (detection_id, pet_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(det)
        .bind(req.pet_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct KindBody {
    kind: String,
}

/// `POST /assets/pets/detections/{id}/kind {kind}`: the owner says what the animal is ("It's a dog" / "It's a cat"), without a pet. The
/// detector's guess is replaced and the correction is kept through a rescan (`kind_by_owner`). An animal in a pet of the other kind leaves
/// it and is refused for it, like "Not this pet" (the pet's kind is the pet's own). `{id, kind}`.
pub(super) async fn set_kind(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(det): Path<Uuid>,
    Json(req): Json<KindBody>,
) -> ApiResult<Json<Value>> {
    on(&s)?;
    let kind = req.kind.trim().to_ascii_lowercase();
    if !pets::is_kind(&kind) {
        return Err(ApiError::BadRequest("kind must be dog or cat".into()));
    }
    let mut tx = s.db.begin().await?;
    let row: Option<(Option<Uuid>, String)> = sqlx::query_as(&format!(
        "SELECT d.pet_id, d.kind FROM pet_detections d JOIN assets a ON a.id = d.asset_id
         WHERE d.id = $1 AND d.owner_id = $2 AND {} AND {} FOR UPDATE OF d",
        pets::SCANNED,
        seen("a", "$3")
    ))
    .bind(det)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_optional(&mut *tx)
    .await?;
    let (pet, was) = row.ok_or(ApiError::NotFound)?;
    if was != kind {
        sqlx::query("UPDATE pet_detections SET kind = $2, kind_by_owner = true WHERE id = $1")
            .bind(det)
            .bind(&kind)
            .execute(&mut *tx)
            .await?;
        if let Some(pet) = pet {
            sqlx::query("UPDATE pet_detections SET pet_id = NULL WHERE id = $1")
                .bind(det)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO pet_rejections (detection_id, pet_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(det)
                .bind(pet)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE pets SET cover_detection_id = NULL, updated_at = now() WHERE id = $1 AND cover_detection_id = $2")
                .bind(pet)
                .bind(det)
                .execute(&mut *tx)
                .await?;
        }
    } else {
        // Saying what it already is still makes it the owner's word, which a rescan keeps.
        sqlx::query("UPDATE pet_detections SET kind_by_owner = true WHERE id = $1")
            .bind(det)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"id": det, "kind": kind})))
}
