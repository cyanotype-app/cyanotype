//! Albums: owner-scoped collections of photos, served under `/assets/albums` (the gateway sends `/assets/*` to
//! this service, while `/albums` is the web app's own page).
//!
//! - `GET /assets/albums`: every album with photo count, cover, date range; most recently updated first.
//! - `POST /assets/albums {title, description?, asset_ids?}`; `GET|PATCH|DELETE /assets/albums/{id}`.
//! - `GET /assets/albums/{id}/assets?limit&cursor`: the album's photos, same item shape, ordering and keyset
//!   pagination as `GET /assets`.
//! - `POST /assets/albums/{id}/assets {ids}` adds, `POST /assets/albums/{id}/assets/remove {ids}` removes; both
//!   are idempotent and answer `{count}` with the number of memberships actually changed.
//!
//! Albums reference photos. Trashed photos keep their membership but are invisible in album views and counts
//! (restoring brings them back); so do hidden photos (the album reports `hidden_count` for its owner and its cover is never
//! a hidden photo), while archived photos stay; a purge removes the membership through the foreign key cascade. Someone else's
//! album, or an unknown id, is always a 404, and only the caller's own live photos can be added.
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use super::scope::Selector;
use super::{ListQuery, ListResponse, ListRow, decode_cursor, encode_cursor, list_items, thumb_fields, trash::MAX_IDS};
use crate::{AppState, Features};
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

const MAX_TITLE: usize = 200;
const MAX_DESCRIPTION: usize = 5000;

/// Same expression as the library timeline (`x` is the assets alias).
const SORT: &str = "coalesce(x.taken_at, x.file_modified_at, x.created_at)";

fn clean_title(t: &str) -> ApiResult<String> {
    let t = t.trim();
    if t.is_empty() {
        return Err(ApiError::BadRequest("title must not be empty".into()));
    }
    if t.chars().count() > MAX_TITLE {
        return Err(ApiError::BadRequest(format!("title is at most {MAX_TITLE} characters")));
    }
    Ok(t.to_string())
}

/// Blank means no description.
fn clean_description(d: Option<&str>) -> ApiResult<Option<String>> {
    let Some(d) = d.map(str::trim).filter(|d| !d.is_empty()) else {
        return Ok(None);
    };
    if d.chars().count() > MAX_DESCRIPTION {
        return Err(ApiError::BadRequest(format!(
            "description is at most {MAX_DESCRIPTION} characters"
        )));
    }
    Ok(Some(d.to_string()))
}

fn clean_ids(mut ids: Vec<Uuid>) -> ApiResult<Vec<Uuid>> {
    if ids.len() > MAX_IDS {
        return Err(ApiError::BadRequest(format!("at most {MAX_IDS} ids per request")));
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// Distinguishes an absent field (leave alone) from an explicit `null` (clear it).
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Serialize)]
struct Cover {
    id: Uuid,
    thumbhash: Option<String>,
    thumb_url: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Album {
    id: Uuid,
    title: String,
    description: Option<String>,
    /// A smart album: its photos are what its `rules` match when it is looked at, not members.
    smart: bool,
    /// The rules of a smart album (`smart.rs`); null for an ordinary one.
    rules: Option<serde_json::Value>,
    /// Photos in the album, not counting trashed ones or hidden ones.
    photo_count: i64,
    /// A smart album with more matches than are counted (`photo_count` is then 10,000: "10,000 or more").
    photo_count_capped: bool,
    /// Members that are hidden: they stay in the album but no album view lists them ("N hidden" for the owner).
    hidden_count: i64,
    /// The chosen cover if it is still in the album and not trashed, else the most recent photo; null when empty.
    cover: Option<Cover>,
    /// Earliest / latest photo time (taken_at, file mtime or upload time); null when empty.
    #[serde(with = "time::serde::rfc3339::option")]
    first_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    last_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(FromRow)]
struct AlbumRow {
    id: Uuid,
    title: String,
    description: Option<String>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
    rules: Option<serde_json::Value>,
    photo_count: i64,
    hidden_count: i64,
    first_at: Option<OffsetDateTime>,
    last_at: Option<OffsetDateTime>,
    cover_id: Option<Uuid>,
    cover_thumbhash: Option<Vec<u8>>,
}

fn summary_sql(tail: &str) -> String {
    format!(
        "SELECT a.id, a.title, a.description, a.created_at, a.updated_at, a.rules,
                st.photo_count, st.hidden_count, st.first_at, st.last_at, cv.id AS cover_id, cv.thumbhash AS cover_thumbhash
         FROM albums a
         CROSS JOIN LATERAL (
             SELECT count(*) FILTER (WHERE NOT x.hidden) AS photo_count, count(*) FILTER (WHERE x.hidden) AS hidden_count,
                    min({SORT}) FILTER (WHERE NOT x.hidden) AS first_at, max({SORT}) FILTER (WHERE NOT x.hidden) AS last_at
             FROM album_assets aa JOIN assets x ON x.id = aa.asset_id
             WHERE aa.album_id = a.id AND x.deleted_at IS NULL AND x.primary_id IS NULL
         ) st
         LEFT JOIN LATERAL (
             SELECT x.id, x.thumbhash FROM album_assets aa JOIN assets x ON x.id = aa.asset_id
             WHERE aa.album_id = a.id AND x.deleted_at IS NULL AND x.primary_id IS NULL AND NOT x.hidden
             ORDER BY (x.id = a.cover_asset_id) DESC, {SORT} DESC, x.id DESC LIMIT 1
         ) cv ON true
         WHERE a.owner_id = $1 {tail}"
    )
}

/// The most photos a smart album counts; beyond it the count says "this many or more" (`photo_count_capped`). Counting every match
/// of a broad rule costs far more than reading the 10,001st (measured at 50,000 photos: 411 ms for 20,000 matches, 26 ms capped).
const SMART_COUNT_CAP: i64 = 10_000;

/// What a smart album shows now: how many photos match (up to [`SMART_COUNT_CAP`]), the oldest and the newest (two index reads
/// of the timeline), and the newest one as the cover.
async fn smart_summary(
    db: &sqlx::PgPool,
    features: Features,
    owner: Uuid,
    rules: &super::smart::Rules,
) -> ApiResult<(
    i64,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
    Option<(Uuid, Option<Vec<u8>>)>,
)> {
    let filters = rules.filters(features);
    let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT count(*) FROM (SELECT 1 FROM assets a WHERE");
    filters.push_where(&mut qb, owner);
    qb.push(" LIMIT ").push_bind(SMART_COUNT_CAP + 1).push(") c");
    let n: i64 = qb.build_query_scalar().persistent(false).fetch_one(db).await?;
    if n == 0 {
        return Ok((0, None, None, None));
    }
    let end = |dir: &str| {
        let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new(format!(
            "SELECT a.id, a.thumbhash, {SORT_A} AS t FROM assets a WHERE"
        ));
        filters.push_where(&mut qb, owner);
        qb.push(format!(" ORDER BY {SORT_A} {dir}, a.id {dir} LIMIT 1"));
        qb
    };
    type EndRow = (Uuid, Option<Vec<u8>>, OffsetDateTime);
    let newest: Option<EndRow> = end("DESC")
        .build_query_as()
        .persistent(false)
        .fetch_optional(db)
        .await?;
    let oldest: Option<EndRow> = end("ASC").build_query_as().persistent(false).fetch_optional(db).await?;
    Ok((
        n,
        oldest.map(|r| r.2),
        newest.as_ref().map(|r| r.2),
        newest.map(|r| (r.0, r.1)),
    ))
}

/// The same expression as the library timeline, for `assets a`.
const SORT_A: &str = "coalesce(a.taken_at, a.file_modified_at, a.created_at)";

async fn to_albums(s: &AppState, owner: Uuid, rows: Vec<AlbumRow>) -> ApiResult<Vec<Album>> {
    let mut out = Vec::with_capacity(rows.len());
    for mut r in rows {
        // A smart album has no members: what it shows is looked up from its rules each time.
        if let Some(rules) = r.rules.as_ref().and_then(|v| super::smart::Rules::from_json(v).ok()) {
            let (n, first, last, cover) = smart_summary(&s.db, s.features(), owner, &rules).await?;
            (r.photo_count, r.hidden_count, r.first_at, r.last_at) = (n, 0, first, last);
            (r.cover_id, r.cover_thumbhash) = cover.map_or((None, None), |(id, th)| (Some(id), th));
        }
        let cover = match r.cover_id {
            Some(id) => {
                let (thumbhash, thumb_url) = thumb_fields(&s.storage, owner, id, r.cover_thumbhash.as_deref()).await?;
                Some(Cover {
                    id,
                    thumbhash,
                    thumb_url,
                })
            }
            None => None,
        };
        out.push(Album {
            id: r.id,
            title: r.title,
            description: r.description,
            smart: r.rules.is_some(),
            rules: r.rules,
            photo_count: r.photo_count.min(SMART_COUNT_CAP),
            photo_count_capped: r.photo_count > SMART_COUNT_CAP,
            hidden_count: r.hidden_count,
            cover,
            first_at: r.first_at,
            last_at: r.last_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        });
    }
    Ok(out)
}

async fn load_album(s: &AppState, owner: Uuid, id: Uuid) -> ApiResult<Album> {
    let row: AlbumRow = sqlx::query_as(&summary_sql("AND a.id = $2"))
        .bind(owner)
        .bind(id)
        .fetch_optional(&s.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(to_albums(s, owner, vec![row]).await?.remove(0))
}

/// 404 unless the album exists and is the caller's. Takes a row lock so a concurrent delete cannot race the insert.
async fn lock_album(tx: &mut sqlx::PgConnection, owner: Uuid, id: Uuid) -> ApiResult<()> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM albums WHERE id = $1 AND owner_id = $2 FOR UPDATE")
        .bind(id)
        .bind(owner)
        .fetch_optional(tx)
        .await?
        .map(|_| ())
        .ok_or(ApiError::NotFound)
}

async fn is_smart(tx: &mut sqlx::PgConnection, id: Uuid) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT rules IS NOT NULL FROM albums WHERE id = $1")
        .bind(id)
        .fetch_one(tx)
        .await?)
}

/// A smart album's photos come from its rules: adding or removing one by hand makes no sense.
async fn refuse_smart(tx: &mut sqlx::PgConnection, id: Uuid) -> ApiResult<()> {
    if is_smart(tx, id).await? {
        return Err(ApiError::BadRequest(
            "this is a smart album: its photos come from its rules, change the rules instead".into(),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
pub(super) struct PreviewBody {
    rules: serde_json::Value,
}

/// The people a smart album's rules name must be the caller's own, and People must be on: anything else is a `404`, as for an
/// unknown person anywhere else (another owner's id is not told apart from one that never existed).
async fn check_people(s: &AppState, owner: Uuid, rules: &super::smart::Rules) -> ApiResult<()> {
    // A pets rule needs Pets (its kind is not an id: there is nothing of anyone else's to check); a pet rule's ids must be the owner's.
    if rules.has_pets() && !s.pets_enabled {
        return Err(ApiError::NotFound);
    }
    let pet_ids = rules.named_pets();
    if !pet_ids.is_empty() {
        let mut unique = pet_ids.clone();
        unique.sort_unstable();
        unique.dedup();
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM pets WHERE owner_id = $1 AND id = ANY($2)")
            .bind(owner)
            .bind(&unique)
            .fetch_one(&s.db)
            .await?;
        if n != unique.len() as i64 {
            return Err(ApiError::NotFound);
        }
    }
    let ids = rules.people();
    if ids.is_empty() {
        return Ok(());
    }
    if !s.faces_enabled {
        return Err(ApiError::NotFound);
    }
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM people WHERE owner_id = $1 AND id = ANY($2)")
        .bind(owner)
        .bind(&ids)
        .fetch_one(&s.db)
        .await?;
    // (`ids` has no duplicates: the rules were normalised, but one rule per person can repeat across rules)
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    if n != unique.len() as i64 {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// `POST /assets/albums/preview {rules}`: how many photos the rules would match now and a few of them, for the editor's live
/// preview. Nothing is saved.
pub(super) async fn preview(
    State(s): State<AppState>,
    user: AuthUser,
    Json(req): Json<PreviewBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let rules = super::smart::Rules::from_json(&req.rules)?;
    check_people(&s, user.id, &rules).await?;
    let filters = rules.filters(s.features());
    let (count, _, _, _) = smart_summary(&s.db, s.features(), user.id, &rules).await?;
    let rows = super::search::timeline_rows(&s.db, user.id, &filters, 8, None).await?;
    let items = list_items(&s, user.id, rows.into_iter().map(|r| r.row).collect()).await?;
    Ok(Json(serde_json::json!({
        "count": count.min(SMART_COUNT_CAP),
        "capped": count > SMART_COUNT_CAP,
        "items": items,
        "rules": rules.to_json()
    })))
}

pub(super) async fn list(State(s): State<AppState>, user: AuthUser) -> ApiResult<Json<serde_json::Value>> {
    let rows: Vec<AlbumRow> = sqlx::query_as(&summary_sql("ORDER BY a.updated_at DESC, a.id DESC"))
        .bind(user.id)
        .fetch_all(&s.db)
        .await?;
    Ok(Json(serde_json::json!({"items": to_albums(&s, user.id, rows).await?})))
}

pub(super) async fn get(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<Album>> {
    Ok(Json(load_album(&s, user.id, id).await?))
}

#[derive(Deserialize)]
pub(super) struct CreateBody {
    title: String,
    description: Option<String>,
    #[serde(default)]
    asset_ids: Vec<Uuid>,
    /// Rules make a smart album (no `asset_ids` then).
    rules: Option<serde_json::Value>,
}

/// Insert the caller's live photos among `ids` into the album; returns how many were new.
async fn insert_members(tx: &mut sqlx::PgConnection, owner: Uuid, album: Uuid, ids: &[Uuid]) -> ApiResult<u64> {
    Ok(sqlx::query(
        "INSERT INTO album_assets (album_id, asset_id)
         SELECT $1, id FROM assets WHERE owner_id = $2 AND id = ANY($3) AND deleted_at IS NULL AND primary_id IS NULL
         ON CONFLICT DO NOTHING",
    )
    .bind(album)
    .bind(owner)
    .bind(ids)
    .execute(tx)
    .await?
    .rows_affected())
}

pub(super) async fn create(
    State(s): State<AppState>,
    user: AuthUser,
    Json(req): Json<CreateBody>,
) -> ApiResult<(StatusCode, Json<Album>)> {
    let title = clean_title(&req.title)?;
    let description = clean_description(req.description.as_deref())?;
    let ids = clean_ids(req.asset_ids)?;
    let rules = match &req.rules {
        Some(v) => {
            if !ids.is_empty() {
                return Err(ApiError::BadRequest(
                    "a smart album has rules, not photos: send one or the other".into(),
                ));
            }
            let rules = super::smart::Rules::from_json(v)?;
            check_people(&s, user.id, &rules).await?;
            Some(rules.to_json())
        }
        None => None,
    };
    let mut tx = s.db.begin().await?;
    let (id,): (Uuid,) =
        sqlx::query_as("INSERT INTO albums (owner_id, title, description, rules) VALUES ($1,$2,$3,$4) RETURNING id")
            .bind(user.id)
            .bind(&title)
            .bind(&description)
            .bind(&rules)
            .fetch_one(&mut *tx)
            .await?;
    if !ids.is_empty() {
        insert_members(&mut tx, user.id, id, &ids).await?;
    }
    tx.commit().await?;
    if rules.is_some() {
        super::visibility::manifest_soon(&s, user.id);
    }
    Ok((StatusCode::CREATED, Json(load_album(&s, user.id, id).await?)))
}

#[derive(Deserialize)]
pub(super) struct UpdateBody {
    title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    description: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    cover_asset_id: Option<Option<Uuid>>,
    /// New rules for a smart album (an ordinary album cannot become smart or the other way round).
    rules: Option<serde_json::Value>,
}

pub(super) async fn update(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateBody>,
) -> ApiResult<Json<Album>> {
    let title = req.title.as_deref().map(clean_title).transpose()?;
    let description = req
        .description
        .as_ref()
        .map(|d| clean_description(d.as_deref()))
        .transpose()?;
    let mut new_rules = None;
    if let Some(v) = &req.rules {
        let rules = super::smart::Rules::from_json(v)?;
        check_people(&s, user.id, &rules).await?;
        new_rules = Some(rules.to_json());
    }
    let mut tx = s.db.begin().await?;
    lock_album(&mut tx, user.id, id).await?;
    let smart = is_smart(&mut tx, id).await?;
    if new_rules.is_some() && !smart {
        return Err(ApiError::BadRequest("only a smart album has rules".into()));
    }
    if smart && matches!(req.cover_asset_id, Some(Some(_))) {
        return Err(ApiError::BadRequest("a smart album's cover is its newest photo".into()));
    }
    if let Some(Some(cover)) = req.cover_asset_id {
        let ok: Option<(Uuid,)> = sqlx::query_as(
            "SELECT aa.asset_id FROM album_assets aa JOIN assets x ON x.id = aa.asset_id
             WHERE aa.album_id = $1 AND aa.asset_id = $2 AND x.owner_id = $3 AND x.deleted_at IS NULL AND x.primary_id IS NULL
               AND NOT x.hidden",
        )
        .bind(id)
        .bind(cover)
        .bind(user.id)
        .fetch_optional(&mut *tx)
        .await?;
        if ok.is_none() {
            return Err(ApiError::BadRequest("the cover must be a photo in this album".into()));
        }
    }
    if title.is_some() || description.is_some() || req.cover_asset_id.is_some() || new_rules.is_some() {
        sqlx::query(
            "UPDATE albums SET
                rules = coalesce($7, rules),
                title = coalesce($2, title),
                description = CASE WHEN $3 THEN $4 ELSE description END,
                cover_asset_id = CASE WHEN $5 THEN $6 ELSE cover_asset_id END,
                updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(&title)
        .bind(description.is_some())
        .bind(description.flatten())
        .bind(req.cover_asset_id.is_some())
        .bind(req.cover_asset_id.flatten())
        .bind(&new_rules)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    if new_rules.is_some() {
        super::visibility::manifest_soon(&s, user.id);
    }
    Ok(Json(load_album(&s, user.id, id).await?))
}

/// Deletes the album and its memberships only; the photos are untouched.
pub(super) async fn remove(State(s): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<StatusCode> {
    let n = sqlx::query("DELETE FROM albums WHERE id = $1 AND owner_id = $2")
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

pub(super) async fn assets(
    State(s): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<ListResponse>> {
    let owned: Option<(Option<serde_json::Value>,)> =
        sqlx::query_as("SELECT rules FROM albums WHERE id = $1 AND owner_id = $2")
            .bind(id)
            .bind(user.id)
            .fetch_optional(&s.db)
            .await?;
    let (rules,) = owned.ok_or(ApiError::NotFound)?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    if let Some(rules) = rules {
        // A smart album lists what its rules match now, newest first, with the search's own SQL and cursor.
        let filters = super::smart::Rules::from_json(&rules)?.filters(s.features());
        let after = match &q.cursor {
            Some(c) => Some(decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
            None => None,
        };
        let rows = super::search::timeline_rows(&s.db, user.id, &filters, limit, after).await?;
        let last = if rows.len() as i64 == limit { rows.last() } else { None };
        let next_cursor = last.map(|r| encode_cursor(r.row.summary.sort_time, r.row.summary.id));
        let next_before = last.map(|r| r.row.summary.sort_time);
        let items = list_items(&s, user.id, rows.into_iter().map(|r| r.row).collect()).await?;
        return Ok(Json(ListResponse {
            items,
            next_cursor,
            next_before,
        }));
    }
    let after = match (&q.cursor, q.before) {
        (Some(c), _) => Some(decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
        (None, Some(t)) => Some((t, Uuid::nil())),
        (None, None) => None,
    };
    let select = format!(
        "SELECT x.id, x.original_name, x.content_type, x.size_bytes, x.width, x.height, {SORT} AS sort_time, x.favorite, x.rating, x.archived, x.hidden, (x.recipe IS NOT NULL) AS edited, x.thumbhash
         FROM album_assets aa JOIN assets x ON x.id = aa.asset_id
         WHERE aa.album_id = $1 AND x.owner_id = $2 AND x.deleted_at IS NULL AND x.primary_id IS NULL AND NOT x.hidden"
    );
    let rows = match after {
        Some((ts, aid)) => {
            sqlx::query_as::<_, ListRow>(&format!(
                "{select} AND ({SORT}, x.id) < ($3, $4) ORDER BY {SORT} DESC, x.id DESC LIMIT $5"
            ))
            .bind(id)
            .bind(user.id)
            .bind(ts)
            .bind(aid)
            .bind(limit)
            .fetch_all(&s.db)
            .await?
        }
        None => {
            sqlx::query_as::<_, ListRow>(&format!("{select} ORDER BY {SORT} DESC, x.id DESC LIMIT $3"))
                .bind(id)
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

#[derive(Serialize)]
pub(super) struct Count {
    count: u64,
}

/// Add photos. Unknown, foreign, trashed and already-present ids are skipped; `count` is how many were added.
pub(super) async fn add(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Json(req): Json<Selector>,
) -> ApiResult<Json<Count>> {
    let pick = req.validated(unlock, s.features())?;
    let mut tx = s.db.begin().await?;
    lock_album(&mut tx, user.id, id).await?;
    refuse_smart(&mut tx, id).await?;
    let ids = pick.resolve(&mut tx, user.id).await?.unwrap_or_default();
    let count = insert_members(&mut tx, user.id, id, &ids).await?;
    if count > 0 {
        sqlx::query("UPDATE albums SET updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(Count { count }))
}

/// Remove photos from the album (they stay in the library). `count` is how many memberships were removed.
pub(super) async fn remove_assets(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
    Json(req): Json<Selector>,
) -> ApiResult<Json<Count>> {
    let pick = req.validated(unlock, s.features())?;
    let mut tx = s.db.begin().await?;
    lock_album(&mut tx, user.id, id).await?;
    refuse_smart(&mut tx, id).await?;
    let ids = pick.resolve(&mut tx, user.id).await?.unwrap_or_default();
    let count = sqlx::query("DELETE FROM album_assets WHERE album_id = $1 AND asset_id = ANY($2)")
        .bind(id)
        .bind(&ids)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if count > 0 {
        // A removed photo can no longer be the cover.
        sqlx::query(
            "UPDATE albums SET updated_at = now(),
                cover_asset_id = CASE WHEN cover_asset_id = ANY($2) THEN NULL ELSE cover_asset_id END
             WHERE id = $1",
        )
        .bind(id)
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Json(Count { count }))
}

/// The caller's albums containing `asset`, for the photo detail view.
pub(super) async fn containing(db: &sqlx::PgPool, owner: Uuid, asset: Uuid) -> ApiResult<serde_json::Value> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT al.id, al.title FROM album_assets aa JOIN albums al ON al.id = aa.album_id
         WHERE aa.asset_id = $1 AND al.owner_id = $2 ORDER BY lower(al.title), al.id",
    )
    .bind(asset)
    .bind(owner)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, title)| serde_json::json!({"id": id, "title": title}))
        .collect())
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;
    use uuid::Uuid;

    use crate::{
        purge,
        testutil::{FakeObjects, TestDb, app, asset, ids, send, user},
    };

    #[tokio::test]
    async fn crud_and_validation() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let a = asset(&t.pool, uid, "a.jpg").await;
        let b = asset(&t.pool, uid, "b.jpg").await;
        let app = app(&t.pool);

        for bad in [json!({"title": "  "}), json!({"title": "x".repeat(201)})] {
            let (st, _) = send(&app, "POST", "/assets/albums", &key, Some(bad)).await;
            assert_eq!(st, StatusCode::BAD_REQUEST);
        }
        let (st, al) = send(
            &app,
            "POST",
            "/assets/albums",
            &key,
            Some(json!({"title": " Trip ", "description": "  ", "asset_ids": [a, b, a, Uuid::new_v4()]})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        assert_eq!(al["title"], "Trip");
        assert!(al["description"].is_null());
        assert_eq!(al["photo_count"], 2);
        let id = al["id"].as_str().unwrap().to_string();
        let path = format!("/assets/albums/{id}");

        let (_, got) = send(&app, "GET", &path, &key, None).await;
        assert_eq!(got["photo_count"], 2);
        assert!(got["first_at"].is_string() && got["last_at"].is_string());

        // Absent fields are left alone, null clears, a blank title is refused.
        let (st, up) = send(&app, "PATCH", &path, &key, Some(json!({"description": "Sea"}))).await;
        assert_eq!(
            (st, up["title"].as_str(), up["description"].as_str()),
            (StatusCode::OK, Some("Trip"), Some("Sea"))
        );
        let (_, up) = send(&app, "PATCH", &path, &key, Some(json!({"title": "Holiday"}))).await;
        assert_eq!(
            (up["title"].as_str(), up["description"].as_str()),
            (Some("Holiday"), Some("Sea"))
        );
        let (_, up) = send(&app, "PATCH", &path, &key, Some(json!({"description": null}))).await;
        assert!(up["description"].is_null());
        assert_eq!(
            send(&app, "PATCH", &path, &key, Some(json!({"title": ""}))).await.0,
            StatusCode::BAD_REQUEST
        );

        // Delete removes the album and nothing else.
        let (_, list) = send(&app, "GET", "/assets/albums", &key, None).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        assert_eq!(send(&app, "DELETE", &path, &key, None).await.0, StatusCode::NO_CONTENT);
        assert_eq!(send(&app, "DELETE", &path, &key, None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(send(&app, "GET", &path, &key, None).await.0, StatusCode::NOT_FOUND);
        assert_eq!(send(&app, "GET", "/assets/count", &key, None).await.1["count"], 2);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM album_assets")
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        t.teardown().await;
    }

    #[tokio::test]
    async fn albums_are_private_to_their_owner() {
        let Some(t) = TestDb::new().await else { return };
        let (alice, ka) = user(&t.pool, "alice@example.com").await;
        let (bob, kb) = user(&t.pool, "bob@example.com").await;
        let a = asset(&t.pool, alice, "a.jpg").await;
        let b = asset(&t.pool, bob, "b.jpg").await;
        let app = app(&t.pool);
        let (_, al) = send(
            &app,
            "POST",
            "/assets/albums",
            &ka,
            Some(json!({"title": "Mine", "asset_ids": [a]})),
        )
        .await;
        let id = al["id"].as_str().unwrap();
        let path = format!("/assets/albums/{id}");

        // Bob sees nothing of it and cannot change it.
        assert_eq!(
            send(&app, "GET", "/assets/albums", &kb, None).await.1["items"],
            json!([])
        );
        for (m, p, body) in [
            ("GET", path.clone(), None),
            ("GET", format!("{path}/assets"), None),
            ("PATCH", path.clone(), Some(json!({"title": "Hacked"}))),
            ("DELETE", path.clone(), None),
            ("POST", format!("{path}/assets"), Some(ids(&[b]))),
            ("POST", format!("{path}/assets/remove"), Some(ids(&[a]))),
        ] {
            assert_eq!(send(&app, m, &p, &kb, body).await.0, StatusCode::NOT_FOUND, "{m} {p}");
        }
        let (_, still) = send(&app, "GET", &path, &ka, None).await;
        assert_eq!(
            (still["title"].as_str(), still["photo_count"].as_i64()),
            (Some("Mine"), Some(1))
        );

        // Alice cannot pull Bob's photo into her album, at creation or later; Bob's photo is not hers to cover.
        let (_, add) = send(&app, "POST", &format!("{path}/assets"), &ka, Some(ids(&[b]))).await;
        assert_eq!(add["count"], 0);
        let (_, al2) = send(
            &app,
            "POST",
            "/assets/albums",
            &ka,
            Some(json!({"title": "X", "asset_ids": [b]})),
        )
        .await;
        assert_eq!(al2["photo_count"], 0);
        assert_eq!(
            send(&app, "PATCH", &path, &ka, Some(json!({"cover_asset_id": b})))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        // Bob's photo does not reveal her albums either.
        assert_eq!(
            send(&app, "GET", &format!("/assets/{a}"), &kb, None).await.0,
            StatusCode::NOT_FOUND
        );
        t.teardown().await;
    }

    #[tokio::test]
    async fn add_and_remove_are_idempotent_and_capped() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let a = asset(&t.pool, uid, "a.jpg").await;
        let b = asset(&t.pool, uid, "b.jpg").await;
        let c = asset(&t.pool, uid, "c.jpg").await;
        let app = app(&t.pool);
        let (_, al) = send(&app, "POST", "/assets/albums", &key, Some(json!({"title": "T"}))).await;
        let path = format!("/assets/albums/{}", al["id"].as_str().unwrap());
        let add = format!("{path}/assets");
        let rm = format!("{path}/assets/remove");

        let (st, r) = send(&app, "POST", &add, &key, Some(ids(&[a, b, b, Uuid::new_v4()]))).await;
        assert_eq!((st, r["count"].as_u64()), (StatusCode::OK, Some(2)));
        assert_eq!(send(&app, "POST", &add, &key, Some(ids(&[a, b]))).await.1["count"], 0);
        assert_eq!(send(&app, "POST", &add, &key, Some(ids(&[a, c]))).await.1["count"], 1);
        assert_eq!(send(&app, "GET", &path, &key, None).await.1["photo_count"], 3);

        // A trashed photo cannot be added.
        let d = asset(&t.pool, uid, "d.jpg").await;
        send(&app, "POST", "/assets/trash", &key, Some(ids(&[d]))).await;
        assert_eq!(send(&app, "POST", &add, &key, Some(ids(&[d]))).await.1["count"], 0);

        assert_eq!(
            send(&app, "POST", &rm, &key, Some(ids(&[a, d, Uuid::new_v4()])))
                .await
                .1["count"],
            1
        );
        assert_eq!(send(&app, "POST", &rm, &key, Some(ids(&[a]))).await.1["count"], 0);
        assert_eq!(send(&app, "GET", &path, &key, None).await.1["photo_count"], 2);
        // Removed photos stay in the library.
        assert_eq!(send(&app, "GET", "/assets/count", &key, None).await.1["count"], 3);

        let many: Vec<Uuid> = (0..1001).map(|_| Uuid::new_v4()).collect();
        for p in [&add, &rm] {
            assert_eq!(
                send(&app, "POST", p, &key, Some(ids(&many))).await.0,
                StatusCode::BAD_REQUEST
            );
            let (st, r) = send(&app, "POST", p, &key, Some(ids(&many[..1000]))).await;
            assert_eq!((st, r["count"].as_u64()), (StatusCode::OK, Some(0)));
        }
        let (st, _) = send(
            &app,
            "POST",
            "/assets/albums",
            &key,
            Some(json!({"title": "Big", "asset_ids": many})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        t.teardown().await;
    }

    #[tokio::test]
    async fn album_photos_paginate_in_timeline_order() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let app = app(&t.pool);
        let mut all = Vec::new();
        // 12 photos, several sharing a time, to exercise the id tiebreak; 3 more outside the album.
        for i in 0..15 {
            let id = asset(&t.pool, uid, &format!("p{i}.jpg")).await;
            sqlx::query("UPDATE assets SET taken_at = $2::timestamptz WHERE id = $1")
                .bind(id)
                .bind(["2020-01-01T00:00:00Z", "2021-01-01T00:00:00Z", "2021-01-01T00:00:00Z"][i % 3])
                .execute(&t.pool)
                .await
                .unwrap();
            all.push(id);
        }
        let (_, al) = send(
            &app,
            "POST",
            "/assets/albums",
            &key,
            Some(json!({"title": "T", "asset_ids": &all[..12]})),
        )
        .await;
        let path = format!("/assets/albums/{}/assets", al["id"].as_str().unwrap());

        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let p = match &cursor {
                Some(c) => format!("{path}?limit=5&cursor={c}"),
                None => format!("{path}?limit=5"),
            };
            let (st, page) = send(&app, "GET", &p, &key, None).await;
            assert_eq!(st, StatusCode::OK);
            pages += 1;
            for i in page["items"].as_array().unwrap() {
                assert!(i.get("thumb_url").is_some() && i.get("sort_time").is_some());
                seen.push(i["id"].as_str().unwrap().to_string());
            }
            match page["next_cursor"].as_str() {
                Some(c) => cursor = Some(c.to_string()),
                None => break,
            }
            assert!(pages < 10);
        }
        assert_eq!((pages, seen.len()), (3, 12));
        assert_eq!(seen.iter().collect::<std::collections::HashSet<_>>().len(), 12);
        let (_, big) = send(&app, "GET", &path, &key, None).await;
        let one: Vec<_> = big["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(one, seen, "same order as one big page");
        // Newest first.
        let times: Vec<_> = big["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["sort_time"].as_str().unwrap().to_string())
            .collect();
        assert!(times.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(
            send(&app, "GET", &format!("{path}?cursor=zz"), &key, None).await.0,
            StatusCode::BAD_REQUEST
        );
        t.teardown().await;
    }

    #[tokio::test]
    async fn trashed_photos_vanish_from_albums_and_return_on_restore() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let a = asset(&t.pool, uid, "a.jpg").await;
        let b = asset(&t.pool, uid, "b.jpg").await;
        let app = app(&t.pool);
        sqlx::query("UPDATE assets SET taken_at = '2020-01-01T00:00:00Z' WHERE id = $1")
            .bind(a)
            .execute(&t.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE assets SET taken_at = '2021-01-01T00:00:00Z' WHERE id = $1")
            .bind(b)
            .execute(&t.pool)
            .await
            .unwrap();
        let (_, al) = send(
            &app,
            "POST",
            "/assets/albums",
            &key,
            Some(json!({"title": "T", "asset_ids": [a, b]})),
        )
        .await;
        let path = format!("/assets/albums/{}", al["id"].as_str().unwrap());
        // Automatic cover is the most recent photo.
        assert_eq!(al["cover"]["id"], b.to_string());

        send(&app, "POST", "/assets/trash", &key, Some(ids(&[b]))).await;
        let (_, got) = send(&app, "GET", &path, &key, None).await;
        assert_eq!(got["photo_count"], 1);
        assert_eq!(
            got["cover"]["id"],
            a.to_string(),
            "automatic cover skips the trashed photo"
        );
        assert!(got["last_at"].as_str().unwrap().starts_with("2020-01-01"));
        let (_, items) = send(&app, "GET", &format!("{path}/assets"), &key, None).await;
        assert_eq!(items["items"].as_array().unwrap().len(), 1);
        let (_, list) = send(&app, "GET", "/assets/albums", &key, None).await;
        assert_eq!(list["items"][0]["photo_count"], 1);

        send(&app, "POST", "/assets/restore", &key, Some(ids(&[b]))).await;
        let (_, got) = send(&app, "GET", &path, &key, None).await;
        assert_eq!(
            (got["photo_count"].as_i64(), got["cover"]["id"].as_str()),
            (Some(2), Some(b.to_string().as_str()))
        );

        // Purge removes the membership for good.
        send(&app, "POST", "/assets/trash", &key, Some(ids(&[b]))).await;
        send(&app, "POST", "/assets/purge", &key, Some(ids(&[b]))).await;
        purge::sweep(&t.pool, &FakeObjects::default()).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM album_assets WHERE asset_id = $1")
            .bind(b)
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        assert_eq!(send(&app, "GET", &path, &key, None).await.1["photo_count"], 1);
        t.teardown().await;
    }

    #[tokio::test]
    async fn cover_choice_falls_back_when_trashed_or_removed() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let a = asset(&t.pool, uid, "a.jpg").await;
        let b = asset(&t.pool, uid, "b.jpg").await;
        let outside = asset(&t.pool, uid, "o.jpg").await;
        let app = app(&t.pool);
        sqlx::query("UPDATE assets SET taken_at = '2020-01-01T00:00:00Z' WHERE id = $1")
            .bind(a)
            .execute(&t.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE assets SET taken_at = '2021-01-01T00:00:00Z' WHERE id = $1")
            .bind(b)
            .execute(&t.pool)
            .await
            .unwrap();
        let (_, al) = send(
            &app,
            "POST",
            "/assets/albums",
            &key,
            Some(json!({"title": "T", "asset_ids": [a, b]})),
        )
        .await;
        let path = format!("/assets/albums/{}", al["id"].as_str().unwrap());

        assert_eq!(
            send(&app, "PATCH", &path, &key, Some(json!({"cover_asset_id": outside})))
                .await
                .0,
            StatusCode::BAD_REQUEST,
            "not in the album"
        );
        let (_, up) = send(&app, "PATCH", &path, &key, Some(json!({"cover_asset_id": a}))).await;
        assert_eq!(
            up["cover"]["id"],
            a.to_string(),
            "explicit cover beats the newest photo"
        );

        send(&app, "POST", "/assets/trash", &key, Some(ids(&[a]))).await;
        assert_eq!(
            send(&app, "GET", &path, &key, None).await.1["cover"]["id"],
            b.to_string()
        );
        send(&app, "POST", "/assets/restore", &key, Some(ids(&[a]))).await;
        assert_eq!(
            send(&app, "GET", &path, &key, None).await.1["cover"]["id"],
            a.to_string(),
            "choice survives the trash"
        );

        // Removing the cover from the album clears the choice; null clears it explicitly.
        send(&app, "POST", &format!("{path}/assets/remove"), &key, Some(ids(&[a]))).await;
        let cover: Option<Uuid> = sqlx::query_scalar("SELECT cover_asset_id FROM albums")
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert!(cover.is_none());
        send(&app, "POST", &format!("{path}/assets"), &key, Some(ids(&[a]))).await;
        assert_eq!(
            send(&app, "GET", &path, &key, None).await.1["cover"]["id"],
            b.to_string()
        );
        send(&app, "PATCH", &path, &key, Some(json!({"cover_asset_id": a}))).await;
        let (_, up) = send(&app, "PATCH", &path, &key, Some(json!({"cover_asset_id": null}))).await;
        assert_eq!(up["cover"]["id"], b.to_string());
        t.teardown().await;
    }

    #[tokio::test]
    async fn list_is_sorted_by_update_and_asset_detail_names_its_albums() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let a = asset(&t.pool, uid, "a.jpg").await;
        let app = app(&t.pool);
        let (_, one) = send(&app, "POST", "/assets/albums", &key, Some(json!({"title": "One"}))).await;
        let (_, two) = send(&app, "POST", "/assets/albums", &key, Some(json!({"title": "Two"}))).await;
        let titles = |v: &serde_json::Value| -> Vec<String> {
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["title"].as_str().unwrap().to_string())
                .collect()
        };
        let (_, list) = send(&app, "GET", "/assets/albums", &key, None).await;
        assert_eq!(titles(&list), ["Two", "One"]);
        // Empty albums have no cover and no date range.
        assert!(list["items"][0]["cover"].is_null() && list["items"][0]["first_at"].is_null());

        // Touching the older album moves it to the top.
        let add = format!("/assets/albums/{}/assets", one["id"].as_str().unwrap());
        send(&app, "POST", &add, &key, Some(ids(&[a]))).await;
        assert_eq!(
            titles(&send(&app, "GET", "/assets/albums", &key, None).await.1),
            ["One", "Two"]
        );
        send(
            &app,
            "POST",
            &format!("/assets/albums/{}/assets", two["id"].as_str().unwrap()),
            &key,
            Some(ids(&[a])),
        )
        .await;

        let (st, detail) = send(&app, "GET", &format!("/assets/{a}"), &key, None).await;
        assert_eq!(st, StatusCode::OK);
        let names: Vec<_> = detail["albums"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["title"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["One", "Two"]);
        assert!(detail["albums"][0]["id"].is_string());
        t.teardown().await;
    }
}
