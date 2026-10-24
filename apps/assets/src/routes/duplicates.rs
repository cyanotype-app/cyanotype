//! Near-duplicate review (`docs/library.md`, "Near-duplicates"): the groups the worker found (`cyanotype_core::duplicates`), side by side,
//! with the best photo of each preselected; keep one and trash the rest, or say they are not duplicates.
//!
//! - `GET /assets/duplicates?limit=&cursor=`: the owner's groups, newest first, each with its photos' resolution, size, date, how much
//!   metadata they carry and which one is `best` (largest resolution, then most metadata, then a favorite, then the larger file). Also
//!   `scan`: when the library was last checked and whether a check is wanted or running.
//! - `POST /assets/duplicates/scan`: check the library now (the worker does it within seconds). `POST /assets/admin/duplicates/scan`:
//!   the same for every account (admins, audited).
//! - `POST /assets/duplicates/{id}/keep {keep}`: the other photos of the group go to the trash (restorable: the answer lists their ids
//!   for an Undo) after what they carry has been merged onto the one kept: albums, tags, the highest rating, the favorite flag, the
//!   captions (every distinct one, the kept photo's first), a location and a capture time the kept photo lacks. **The merge stays when
//!   the trashed photos are restored**: it is additive (nothing is taken from the kept photo), and undoing it would need a copy of what
//!   was there.
//! - `POST /assets/duplicates/{id}/dismiss`: "Not duplicates": no pair of the group is grouped again, ever.
//!
//! **Hidden photos.** A session that has not unlocked the Hidden view sees neither hidden members nor a group that has fewer than two
//! visible ones, and `keep` acts on visible members only (a hidden photo is never trashed or merged by a locked session). API keys never
//! have the unlock. Every request is for the caller's own groups; another owner's group is a 404. Keep and dismiss are audited with
//! counts (no photo ids, no names).
use axum::{
    Json,
    extract::{Path, Query, State},
};
use cyanotype_core::{audit, duplicates, edits};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    AssetSummary, decode_cursor, encode_cursor, thumb_fields,
    trash::{self, Op},
};
use crate::AppState;
use cyanotype_service::{AdminUser, ApiError, ApiResult, AuthUser, Unlock};

const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 50;

#[derive(Deserialize, Default)]
pub(super) struct ListQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

/// What `best` is chosen from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Candidate {
    pub pixels: i64,
    pub richness: i64,
    pub favorite: bool,
    pub size_bytes: i64,
    pub id: Uuid,
}

/// The photo to keep by default: the most pixels, then the most metadata, then a favorite, then the larger file, then the lowest id
/// (so the answer never depends on the order the rows came in).
pub(super) fn best_of(c: &[Candidate]) -> Option<Uuid> {
    c.iter()
        .max_by(|a, b| {
            (a.pixels, a.richness, a.favorite, a.size_bytes)
                .cmp(&(b.pixels, b.richness, b.favorite, b.size_bytes))
                .then(b.id.cmp(&a.id))
        })
        .map(|c| c.id)
}

#[derive(FromRow)]
struct Row {
    group_id: Uuid,
    #[sqlx(flatten)]
    summary: AssetSummary,
    thumbhash: Option<Vec<u8>>,
    exif_fields: i64,
    has_location: bool,
    has_taken: bool,
    has_caption: bool,
    tag_count: i64,
}

#[derive(Serialize)]
struct Member {
    #[serde(flatten)]
    summary: AssetSummary,
    thumbhash: Option<String>,
    thumb_url: Option<String>,
    /// Fields in the EXIF dump of the file.
    exif_fields: i64,
    has_location: bool,
    has_caption: bool,
    tag_count: i64,
    /// The one to keep unless the user chooses another.
    best: bool,
}

#[derive(Serialize)]
struct Group {
    id: Uuid,
    members: Vec<Member>,
}

#[derive(Serialize, FromRow)]
struct ScanState {
    #[serde(with = "time::serde::rfc3339::option")]
    requested_at: Option<OffsetDateTime>,
    manual: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    finished_at: Option<OffsetDateTime>,
    groups: Option<i32>,
    photos: Option<i32>,
    error: Option<String>,
}

/// A scan is `wanted` (asked for, not started), `running`, or neither; `idle` also when nothing was ever asked.
fn scan_json(s: Option<ScanState>) -> serde_json::Value {
    let Some(s) = s else {
        return json!({"state": "never"});
    };
    let state = match (s.requested_at, s.started_at, s.finished_at) {
        (_, Some(st), f) if f.is_none_or(|f| st > f) => "running",
        (Some(_), _, _) => "wanted",
        (None, _, Some(_)) => "idle",
        _ => "never",
    };
    json!({"state": state, "requested_at": s.requested_at, "started_at": s.started_at, "finished_at": s.finished_at,
           "groups": s.groups, "photos": s.photos, "error": s.error, "manual": s.manual})
}

async fn scan_state(db: &sqlx::PgPool, owner: Uuid) -> ApiResult<serde_json::Value> {
    let s: Option<ScanState> = sqlx::query_as(
        "SELECT requested_at, manual, started_at, finished_at, groups, photos, error FROM duplicate_scans WHERE owner_id = $1",
    )
    .bind(owner)
    .fetch_optional(db)
    .await?;
    Ok(scan_json(s))
}

/// The members a session may see and act on: live, not a hidden RAW half, and not hidden unless unlocked.
fn visible(owner: &str, unlocked: &str) -> String {
    format!("a.owner_id = {owner} AND a.deleted_at IS NULL AND a.primary_id IS NULL AND (NOT a.hidden OR {unlocked})")
}

pub(super) async fn list(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let after = match q.cursor.as_deref().filter(|c| !c.is_empty()) {
        Some(c) => decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?,
        // The far future: every group sorts before it.
        None => (
            OffsetDateTime::from_unix_timestamp(32_503_680_000).unwrap_or(OffsetDateTime::UNIX_EPOCH),
            Uuid::max(),
        ),
    };
    // Groups with two or more members this session may see.
    let groups: Vec<(Uuid, OffsetDateTime)> = sqlx::query_as(&format!(
        "SELECT g.id, g.sort_time FROM duplicate_groups g
         WHERE g.owner_id = $1 AND (g.sort_time, g.id) < ($3, $4)
           AND (SELECT count(*) FROM duplicate_members m JOIN assets a ON a.id = m.asset_id
                WHERE m.group_id = g.id AND {}) >= 2
         ORDER BY g.sort_time DESC, g.id DESC LIMIT $5",
        visible("$1", "$2")
    ))
    .bind(user.id)
    .bind(unlock.0)
    .bind(after.0)
    .bind(after.1)
    .bind(limit)
    .fetch_all(&s.db)
    .await?;
    let ids: Vec<Uuid> = groups.iter().map(|g| g.0).collect();
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT m.group_id, a.id, a.original_name, a.content_type, a.size_bytes, a.width, a.height,
                coalesce(a.taken_at, a.file_modified_at, a.created_at) AS sort_time, a.favorite, a.rating, a.archived, a.hidden,
                a.thumbhash,
                CASE WHEN jsonb_typeof(a.exif) = 'object' THEN (SELECT count(*) FROM jsonb_object_keys(a.exif)) ELSE 0 END AS exif_fields,
                (a.latitude IS NOT NULL AND a.longitude IS NOT NULL) AS has_location,
                (a.taken_at IS NOT NULL) AS has_taken, (a.caption IS NOT NULL) AS has_caption,
                (SELECT count(*) FROM asset_tags at WHERE at.asset_id = a.id) AS tag_count
         FROM duplicate_members m JOIN assets a ON a.id = m.asset_id
         WHERE m.group_id = ANY($1) AND {}
         ORDER BY m.group_id, a.id",
        visible("$2", "$3")
    ))
    .bind(&ids)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_all(&s.db)
    .await?;

    let mut out: Vec<Group> = Vec::with_capacity(groups.len());
    for (gid, _) in &groups {
        let mine: Vec<&Row> = rows.iter().filter(|r| r.group_id == *gid).collect();
        let cands: Vec<Candidate> = mine
            .iter()
            .map(|r| Candidate {
                pixels: i64::from(r.summary.width.unwrap_or(0)) * i64::from(r.summary.height.unwrap_or(0)),
                // The EXIF fields, and the facts a user would hate to lose: a place, a time, a caption, tags.
                richness: r.exif_fields
                    + 4 * i64::from(r.has_location)
                    + 2 * i64::from(r.has_taken)
                    + 2 * i64::from(r.has_caption)
                    + r.tag_count,
                favorite: r.summary.favorite,
                size_bytes: r.summary.size_bytes,
                id: r.summary.id,
            })
            .collect();
        let best = best_of(&cands);
        let mut members = Vec::with_capacity(mine.len());
        for r in mine {
            let (thumbhash, thumb_url) =
                thumb_fields(&s.storage, user.id, r.summary.id, r.thumbhash.as_deref()).await?;
            members.push(Member {
                summary: r.summary.clone(),
                thumbhash,
                thumb_url,
                exif_fields: r.exif_fields,
                has_location: r.has_location,
                has_caption: r.has_caption,
                tag_count: r.tag_count,
                best: Some(r.summary.id) == best,
            });
        }
        out.push(Group { id: *gid, members });
    }
    let (total,): (i64,) = sqlx::query_as(&format!(
        "SELECT count(*) FROM duplicate_groups g WHERE g.owner_id = $1
           AND (SELECT count(*) FROM duplicate_members m JOIN assets a ON a.id = m.asset_id WHERE m.group_id = g.id AND {}) >= 2",
        visible("$1", "$2")
    ))
    .bind(user.id)
    .bind(unlock.0)
    .fetch_one(&s.db)
    .await?;
    let next_cursor = (groups.len() as i64 == limit)
        .then(|| groups.last())
        .flatten()
        .map(|(id, t)| encode_cursor(*t, *id));
    Ok(Json(json!({
        "groups": out,
        "total": total,
        "next_cursor": next_cursor,
        "scan": scan_state(&s.db, user.id).await?,
    })))
}

pub(super) async fn scan(State(s): State<AppState>, user: AuthUser) -> ApiResult<Json<serde_json::Value>> {
    duplicates::request(&s.db, user.id, true)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(json!({"scan": scan_state(&s.db, user.id).await?})))
}

/// Admins: check every account's library (each runs in the worker in turn). Audited with a count.
pub(super) async fn scan_all(State(s): State<AppState>, admin: AdminUser) -> ApiResult<Json<serde_json::Value>> {
    let n = sqlx::query(
        "INSERT INTO duplicate_scans (owner_id, requested_at, manual)
         SELECT u.id, now(), true FROM users u WHERE EXISTS (SELECT 1 FROM assets a WHERE a.owner_id = u.id AND a.deleted_at IS NULL)
         ON CONFLICT (owner_id) DO UPDATE SET requested_at = now(), manual = true",
    )
    .execute(&s.db)
    .await?
    .rows_affected();
    audit::record(
        &s.db,
        Some(admin.id),
        "duplicates.scan_all",
        None,
        None,
        json!({"accounts": n}),
    )
    .await?;
    Ok(Json(json!({"accounts": n})))
}

#[derive(Deserialize)]
pub(super) struct KeepBody {
    keep: Uuid,
}

#[derive(FromRow)]
struct Mem {
    id: Uuid,
    favorite: bool,
    rating: i16,
    caption: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    taken_at: Option<OffsetDateTime>,
    date_edited: bool,
    taken_offset_edit: Option<i32>,
}

/// Every distinct caption (trimmed, exact), the kept photo's first, joined by blank lines, cut to the longest a caption can be.
pub(super) fn merged_caption(captions: &[Option<String>]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for c in captions.iter().flatten() {
        let c = c.trim();
        if !c.is_empty() && !parts.iter().any(|p| p == c) {
            parts.push(c.to_string());
        }
    }
    if parts.is_empty() {
        return None;
    }
    let all = parts.join("\n\n");
    Some(
        all.chars()
            .take(edits::MAX_CAPTION_CHARS)
            .collect::<String>()
            .trim_end()
            .to_string(),
    )
}

pub(super) async fn keep(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(group): Path<Uuid>,
    Json(req): Json<KeepBody>,
) -> ApiResult<Json<serde_json::Value>> {
    // The members this session may act on (the group is the caller's, or it is a 404).
    let owned: Option<Uuid> =
        sqlx::query_scalar("SELECT owner_id FROM duplicate_groups WHERE id = $1 AND owner_id = $2")
            .bind(group)
            .bind(user.id)
            .fetch_optional(&s.db)
            .await?;
    owned.ok_or(ApiError::NotFound)?;
    let mut members: Vec<Mem> = sqlx::query_as(&format!(
        "SELECT a.id, a.favorite, a.rating, a.caption, a.latitude, a.longitude, a.taken_at, a.date_edited,
                a.taken_offset_edit
         FROM duplicate_members m JOIN assets a ON a.id = m.asset_id
         WHERE m.group_id = $1 AND {}
         ORDER BY a.created_at, a.id",
        visible("$2", "$3")
    ))
    .bind(group)
    .bind(user.id)
    .bind(unlock.0)
    .fetch_all(&s.db)
    .await?;
    let Some(at) = members.iter().position(|m| m.id == req.keep) else {
        return Err(ApiError::BadRequest("the photo to keep is not in this group".into()));
    };
    let keeper = members.remove(at);
    if members.is_empty() {
        return Err(ApiError::BadRequest(
            "there is nothing to merge: the group has one photo".into(),
        ));
    }
    let loser_ids: Vec<Uuid> = members.iter().map(|m| m.id).collect();

    // Captions, a place and a time the kept photo lacks go through the edits mechanism (written to the bucket first, as every edit is).
    let mut summary = json!({"trashed": loser_ids.len()});
    let merged = merged_caption(
        &std::iter::once(keeper.caption.clone())
            .chain(members.iter().map(|m| m.caption.clone()))
            .collect::<Vec<_>>(),
    );
    let new_place = keeper
        .latitude
        .is_none()
        .then(|| members.iter().find_map(|m| m.latitude.zip(m.longitude)))
        .flatten();
    let new_time = keeper
        .taken_at
        .is_none()
        .then(|| {
            members.iter().find_map(|m| {
                m.taken_at
                    .map(|t| (t, m.date_edited.then_some(m.taken_offset_edit).flatten()))
            })
        })
        .flatten();
    if merged != keeper.caption || new_place.is_some() || new_time.is_some() {
        let (owner_id, sha, mut e) = edits::current(&s.db, keeper.id)
            .await
            .map_err(ApiError::Internal)?
            .ok_or(ApiError::NotFound)?;
        if merged != keeper.caption {
            e.caption.clone_from(&merged);
            summary["caption"] = json!(true);
        }
        if let Some((lat, lon)) = new_place {
            (e.latitude, e.longitude) = (Some(lat), Some(lon));
            summary["location"] = json!(true);
        }
        if let Some((t, offset)) = new_time {
            e.taken_at = Some(t);
            e.taken_offset = offset;
            summary["date"] = json!(true);
        }
        let e = edits::Edits {
            updated_at: edits::now(),
            ..e.validated().map_err(ApiError::BadRequest)?
        };
        edits::put(&s.storage, owner_id, &sha, &e)
            .await
            .map_err(ApiError::Internal)?;
        edits::apply(&s.db, owner_id, keeper.id, &e)
            .await
            .map_err(ApiError::Internal)?;
    }

    let mut tx = s.db.begin().await?;
    // Lock the keeper so two merges onto it do not interleave.
    sqlx::query("SELECT id FROM assets WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL FOR UPDATE")
        .bind(keeper.id)
        .bind(user.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    // Favorite and the highest rating.
    let any_favorite = members.iter().any(|m| m.favorite);
    let best_rating = members.iter().map(|m| m.rating).max().unwrap_or(0);
    if (any_favorite && !keeper.favorite) || best_rating > keeper.rating {
        sqlx::query(
            "UPDATE assets SET favorite = favorite OR $2,
                    rating = greatest(rating, $3),
                    rating_source = CASE WHEN $3 > rating THEN 'user' ELSE rating_source END
             WHERE id = $1",
        )
        .bind(keeper.id)
        .bind(any_favorite)
        .bind(best_rating)
        .execute(&mut *tx)
        .await?;
        if any_favorite && !keeper.favorite {
            summary["favorite"] = json!(true);
        }
        if best_rating > keeper.rating {
            summary["rating"] = json!(best_rating);
        }
    }
    // Albums (and a cover that was one of the others), tags (the owner's mark wins over an imported one).
    let albums = sqlx::query(
        "INSERT INTO album_assets (album_id, asset_id, added_at)
         SELECT album_id, $1, added_at FROM album_assets WHERE asset_id = ANY($2) ON CONFLICT DO NOTHING",
    )
    .bind(keeper.id)
    .bind(&loser_ids)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query("UPDATE albums SET cover_asset_id = $1 WHERE cover_asset_id = ANY($2)")
        .bind(keeper.id)
        .bind(&loser_ids)
        .execute(&mut *tx)
        .await?;
    let tags = sqlx::query(
        "INSERT INTO asset_tags (asset_id, tag_id, imported, added_at)
         SELECT $1, tag_id, imported, added_at FROM asset_tags WHERE asset_id = ANY($2)
         ON CONFLICT (asset_id, tag_id) DO UPDATE SET imported = asset_tags.imported AND EXCLUDED.imported
         WHERE asset_tags.imported AND NOT EXCLUDED.imported",
    )
    .bind(keeper.id)
    .bind(&loser_ids)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    summary["albums"] = json!(albums);
    summary["tags"] = json!(tags);
    tx.commit().await?;

    // The others to the trash (a pair goes with its RAW), restorable from there.
    let trashed = trash::apply(&s.db, user.id, Op::Trash, Some(&loser_ids)).await?;
    summary["trashed"] = json!(trashed.photos);

    // The group is done: the merged photos leave it; what is left (hidden photos this session cannot see) stays a group.
    sqlx::query("DELETE FROM duplicate_members WHERE group_id = $1 AND asset_id = ANY($2)")
        .bind(group)
        .bind(&loser_ids)
        .execute(&s.db)
        .await?;
    sqlx::query(
        "DELETE FROM duplicate_groups g WHERE g.id = $1 AND (SELECT count(*) FROM duplicate_members m WHERE m.group_id = g.id) < 2",
    )
    .bind(group)
    .execute(&s.db)
    .await?;
    audit::record(&s.db, Some(user.id), "duplicates.keep", None, None, summary.clone()).await?;
    super::visibility::manifest_soon(&s, user.id);
    Ok(Json(
        json!({"kept": keeper.id, "trashed": loser_ids, "merged": summary}),
    ))
}

pub(super) async fn dismiss(
    State(s): State<AppState>,
    user: AuthUser,
    Path(group): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut tx = s.db.begin().await?;
    let owned: Option<Uuid> =
        sqlx::query_scalar("SELECT owner_id FROM duplicate_groups WHERE id = $1 AND owner_id = $2 FOR UPDATE")
            .bind(group)
            .bind(user.id)
            .fetch_optional(&mut *tx)
            .await?;
    owned.ok_or(ApiError::NotFound)?;
    let members: Vec<Uuid> =
        sqlx::query_scalar("SELECT asset_id FROM duplicate_members WHERE group_id = $1 ORDER BY asset_id")
            .bind(group)
            .fetch_all(&mut *tx)
            .await?;
    // Every pair, smaller id first (`a < b`).
    sqlx::query(
        "INSERT INTO duplicate_dismissals (owner_id, a, b)
         SELECT $1, x, y FROM unnest($2::uuid[]) x, unnest($2::uuid[]) y WHERE x < y ON CONFLICT DO NOTHING",
    )
    .bind(user.id)
    .bind(&members)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM duplicate_groups WHERE id = $1")
        .bind(group)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut *tx,
        Some(user.id),
        "duplicates.dismiss",
        None,
        None,
        json!({"photos": members.len()}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"dismissed": members.len()})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(n: u128, pixels: i64, richness: i64, favorite: bool, size: i64) -> Candidate {
        Candidate {
            pixels,
            richness,
            favorite,
            size_bytes: size,
            id: Uuid::from_u128(n),
        }
    }

    #[test]
    fn the_best_photo_is_the_largest_then_the_richest_then_a_favorite() {
        let id = Uuid::from_u128;
        assert_eq!(best_of(&[]), None);
        // Resolution first, whatever else the smaller one has.
        assert_eq!(
            best_of(&[c(1, 12_000_000, 5, false, 1), c(2, 2_000_000, 90, true, 9)]),
            Some(id(1))
        );
        // Same resolution: more metadata.
        assert_eq!(best_of(&[c(1, 12, 5, true, 9), c(2, 12, 40, false, 1)]), Some(id(2)));
        // Same resolution and metadata: the favorite, then the larger file, then the lowest id.
        assert_eq!(best_of(&[c(1, 12, 5, false, 9), c(2, 12, 5, true, 1)]), Some(id(2)));
        assert_eq!(best_of(&[c(1, 12, 5, false, 1), c(2, 12, 5, false, 9)]), Some(id(2)));
        assert_eq!(best_of(&[c(2, 12, 5, false, 1), c(1, 12, 5, false, 1)]), Some(id(1)));
    }

    #[test]
    fn captions_are_merged_distinctly_with_the_kept_ones_first() {
        let s = |x: &str| Some(x.to_string());
        assert_eq!(merged_caption(&[None, None]), None);
        assert_eq!(
            merged_caption(&[s("Beach"), s("Beach "), None, s("Sunset")]).as_deref(),
            Some("Beach\n\nSunset")
        );
        assert_eq!(
            merged_caption(&[None, s("From the copy")]).as_deref(),
            Some("From the copy")
        );
        assert_eq!(merged_caption(&[s("  "), s("")]), None);
        let long = "x".repeat(edits::MAX_CAPTION_CHARS);
        let out = merged_caption(&[Some(long.clone()), s("more")]).unwrap();
        assert_eq!(
            out.chars().count(),
            edits::MAX_CAPTION_CHARS,
            "cut to the longest caption"
        );
        assert!(out.starts_with(&long));
    }
}
