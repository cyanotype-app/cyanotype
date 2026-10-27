//! Trash: bulk trash / restore, the trash listing, and permanent deletion ("purge").
//!
//! All endpoints are owner-scoped (ids belonging to someone else, or in the wrong state, are simply not
//! counted) and idempotent: repeating a request matches nothing and returns `{"count": 0}`. `count` is always the
//! number of rows the request actually changed.
//!
//! - `POST /assets/trash {ids}`: soft delete (`deleted_at = now()`).
//! - `POST /assets/restore {ids}`: back to the library; not possible once a purge has been queued.
//! - `GET /assets/trash`: trashed items, newest deletion first, keyset-paged like `GET /assets`.
//! - `POST /assets/purge {ids}` and `POST /assets/trash/empty`: queue permanent deletion of items already in the
//!   trash; the janitor removes the S3 objects and rows (`purge`), the items disappear from the trash at once.
use axum::{
    Json,
    extract::{Query, State},
};
use cyanotype_core::pairing;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use super::scope::{Pick, Selector};
use super::{AssetSummary, ListItem, decode_cursor, encode_cursor, paired_files, thumb_fields};
use crate::AppState;
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

/// Most ids one bulk request may carry.
pub const MAX_IDS: usize = 1000;

#[derive(Serialize)]
pub(super) struct Count {
    count: u64,
    /// For a scope-based trash: the batch to pass back as `{"scope": {"trash_batch": ...}}` to undo exactly it.
    #[serde(skip_serializing_if = "Option::is_none", with = "time::serde::rfc3339::option")]
    batch: Option<OffsetDateTime>,
}

impl Count {
    pub(super) fn new(count: u64) -> Self {
        Count { count, batch: None }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Op {
    Trash,
    Restore,
    Purge,
}

pub(super) struct Applied {
    /// Photos changed. A RAW + JPEG pair is one photo.
    pub photos: u64,
    /// Every row changed, hidden RAW files included.
    pub changed: Vec<Uuid>,
}

#[derive(FromRow)]
struct Changed {
    id: Uuid,
    primary_id: Option<Uuid>,
}

/// Apply `op` to the given ids (all eligible rows when `None`, as "empty trash" does), together with the paired RAW
/// of every photo it changes (or the primary, if a RAW id was given), in one transaction so no reader sees a pair
/// half-trashed. Ids of other owners, unknown ids and rows in the wrong state are ignored.
pub(super) async fn apply(db: &sqlx::PgPool, owner: Uuid, op: Op, ids: Option<&[Uuid]>) -> ApiResult<Applied> {
    let mut tx = db.begin().await?;
    let applied = apply_in(&mut tx, owner, op, ids).await?;
    tx.commit().await?;
    Ok(applied)
}

/// Like [`apply`] for a request's [`Pick`]: a scope is resolved to ids inside the same transaction as the update.
/// Also returns the transaction's timestamp (what `deleted_at` was set to), which identifies a trash batch.
pub(super) async fn apply_pick(
    db: &sqlx::PgPool,
    owner: Uuid,
    op: Op,
    pick: &Pick,
) -> ApiResult<(Applied, OffsetDateTime)> {
    let mut tx = db.begin().await?;
    let (now,): (OffsetDateTime,) = sqlx::query_as("SELECT now()").fetch_one(&mut *tx).await?;
    let ids = pick.resolve(&mut tx, owner).await?;
    let applied = apply_in(&mut tx, owner, op, ids.as_deref()).await?;
    tx.commit().await?;
    Ok((applied, now))
}

async fn apply_in(tx: &mut sqlx::PgConnection, owner: Uuid, op: Op, ids: Option<&[Uuid]>) -> ApiResult<Applied> {
    // (SET clause, state a row must be in)
    let (set, state, partner_state) = match op {
        Op::Trash => ("deleted_at = now()", "deleted_at IS NULL", "deleted_at IS NULL"),
        Op::Restore => (
            "deleted_at = NULL",
            "deleted_at IS NOT NULL AND purging_at IS NULL",
            "deleted_at IS NOT NULL AND purging_at IS NULL",
        ),
        Op::Purge => (
            "purging_at = now(), deleted_at = coalesce(deleted_at, now())",
            "deleted_at IS NOT NULL AND purging_at IS NULL",
            "purging_at IS NULL",
        ),
    };
    let first: Vec<Changed> = sqlx::query_as(&format!(
        "UPDATE assets SET {set}
         WHERE owner_id = $1 AND ($2::uuid[] IS NULL OR id = ANY($2)) AND {state}
         RETURNING id, primary_id"
    ))
    .bind(owner)
    .bind(ids)
    .fetch_all(&mut *tx)
    .await?;
    let done: Vec<Uuid> = first.iter().map(|c| c.id).collect();
    let primaries: Vec<Uuid> = first.iter().filter_map(|c| c.primary_id).collect();
    let partners: Vec<Changed> = sqlx::query_as(&format!(
        "UPDATE assets SET {set}
         WHERE owner_id = $1 AND (primary_id = ANY($2) OR id = ANY($3)) AND {partner_state}
         RETURNING id, primary_id"
    ))
    .bind(owner)
    .bind(&done)
    .bind(&primaries)
    .fetch_all(&mut *tx)
    .await?;
    let mut photos = std::collections::HashSet::new();
    let mut changed = Vec::new();
    for c in first.iter().chain(&partners) {
        photos.insert(c.primary_id.unwrap_or(c.id));
        changed.push(c.id);
    }
    if matches!(op, Op::Trash) {
        // A stack that is down to one live photo is no stack (its trashed members leave it too).
        cyanotype_core::stacks::dissolve_small_around(&mut *tx, owner, &changed).await?;
    }
    Ok(Applied {
        photos: photos.len() as u64,
        changed,
    })
}

pub(super) async fn trash(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Json(req): Json<Selector>,
) -> ApiResult<Json<Count>> {
    let pick = req.validated(unlock, s.features())?;
    let (applied, now) = apply_pick(&s.db, user.id, Op::Trash, &pick).await?;
    let batch = (pick.is_scope() && applied.photos > 0).then_some(now);
    Ok(Json(Count {
        count: applied.photos,
        batch,
    }))
}

pub(super) async fn restore(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Json(req): Json<Selector>,
) -> ApiResult<Json<Count>> {
    let pick = req.validated(unlock, s.features())?;
    let (applied, _) = apply_pick(&s.db, user.id, Op::Restore, &pick).await?;
    // Files that were trashed while their counterpart was uploaded were not paired then; they can be now.
    // Pairing is per file, so only look at files that have a live, unpaired namesake to pair with (a restore of
    // tens of thousands of photos has almost none).
    let candidates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT a.id FROM assets a
         WHERE a.id = ANY($1) AND a.primary_id IS NULL AND a.deleted_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM assets r WHERE r.primary_id = a.id)
           AND EXISTS (SELECT 1 FROM assets b
                       WHERE b.owner_id = a.owner_id AND asset_stem(b.original_name) = asset_stem(a.original_name)
                         AND b.id <> a.id AND b.deleted_at IS NULL AND b.purging_at IS NULL AND b.primary_id IS NULL)",
    )
    .bind(&applied.changed)
    .fetch_all(&s.db)
    .await?;
    for id in &candidates {
        if let Err(e) = pairing::pair_asset(&s.db, *id).await {
            tracing::warn!(asset_id = %id, error = ?e, "pairing after restore failed");
        }
    }
    Ok(Json(Count::new(applied.photos)))
}

pub(super) async fn purge(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Json(req): Json<Selector>,
) -> ApiResult<Json<Count>> {
    let pick = req.validated(unlock, s.features())?;
    let count = apply_pick(&s.db, user.id, Op::Purge, &pick).await?.0.photos;
    if count > 0 {
        s.purge_wake.notify_one();
    }
    Ok(Json(Count::new(count)))
}

pub(super) async fn empty(State(s): State<AppState>, user: AuthUser) -> ApiResult<Json<Count>> {
    let count = apply(&s.db, user.id, Op::Purge, None).await?.photos;
    if count > 0 {
        s.purge_wake.notify_one();
    }
    Ok(Json(Count::new(count)))
}

#[derive(Deserialize)]
pub(super) struct TrashQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

#[derive(FromRow)]
struct TrashRow {
    #[sqlx(flatten)]
    summary: AssetSummary,
    thumbhash: Option<Vec<u8>>,
    deleted_at: OffsetDateTime,
    purge_at: OffsetDateTime,
}

#[derive(Serialize)]
struct TrashItem {
    #[serde(flatten)]
    item: ListItem,
    #[serde(with = "time::serde::rfc3339")]
    deleted_at: OffsetDateTime,
    /// When the janitor will delete it for good (`deleted_at` + `TRASH_RETENTION_DAYS`).
    #[serde(with = "time::serde::rfc3339")]
    purge_at: OffsetDateTime,
}

#[derive(Serialize)]
pub(super) struct TrashResponse {
    items: Vec<TrashItem>,
    /// Pass as `cursor` for the next page; null on the last page.
    next_cursor: Option<String>,
    /// Everything currently in the trash (not just this page), for "Empty trash (N)".
    total: i64,
    retention_days: i32,
}

const TRASH_SELECT: &str = "SELECT id, original_name, content_type, size_bytes, width, height,
        coalesce(taken_at, file_modified_at, created_at) AS sort_time, favorite, rating, archived, hidden, (recipe IS NOT NULL) AS edited, thumbhash, deleted_at,
        deleted_at + make_interval(days => $2) AS purge_at
 FROM assets WHERE owner_id = $1 AND deleted_at IS NOT NULL AND purging_at IS NULL AND primary_id IS NULL";

/// A hidden photo that was trashed is listed (and counted) only for a session that has the Hidden view unlocked.
const NOT_HIDDEN: &str = " AND NOT hidden";

pub(super) async fn list(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Query(q): Query<TrashQuery>,
) -> ApiResult<Json<TrashResponse>> {
    let select = if unlock.0 {
        TRASH_SELECT.to_string()
    } else {
        format!("{TRASH_SELECT}{NOT_HIDDEN}")
    };
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let after = q
        .cursor
        .as_deref()
        .map(|c| decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into())))
        .transpose()?;
    let rows = match after {
        Some((ts, id)) => {
            sqlx::query_as::<_, TrashRow>(&format!(
                "{select} AND (deleted_at, id) < ($3, $4) ORDER BY deleted_at DESC, id DESC LIMIT $5"
            ))
            .bind(user.id)
            .bind(s.trash_retention_days)
            .bind(ts)
            .bind(id)
            .bind(limit)
            .fetch_all(&s.db)
            .await?
        }
        None => {
            sqlx::query_as::<_, TrashRow>(&format!("{select} ORDER BY deleted_at DESC, id DESC LIMIT $3"))
                .bind(user.id)
                .bind(s.trash_retention_days)
                .bind(limit)
                .fetch_all(&s.db)
                .await?
        }
    };
    let next_cursor = if rows.len() as i64 == limit {
        rows.last().map(|r| encode_cursor(r.deleted_at, r.summary.id))
    } else {
        None
    };
    let ids: Vec<Uuid> = rows.iter().map(|r| r.summary.id).collect();
    let mut paired = paired_files(&s.db, &ids).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        let (thumbhash, thumb_url) = thumb_fields(&s.storage, user.id, r.summary.id, r.thumbhash.as_deref()).await?;
        items.push(TrashItem {
            item: ListItem {
                paired: paired.remove(&r.summary.id).unwrap_or_default(),
                summary: r.summary,
                thumbhash,
                thumb_url,
                stack: None,
            },
            deleted_at: r.deleted_at,
            purge_at: r.purge_at,
        });
    }
    let (total,): (i64,) = sqlx::query_as(&format!(
        "SELECT count(*) FROM assets WHERE owner_id = $1 AND deleted_at IS NOT NULL AND purging_at IS NULL AND primary_id IS NULL{}",
        if unlock.0 { "" } else { NOT_HIDDEN }
    ))
    .bind(user.id)
    .fetch_one(&s.db)
    .await?;
    Ok(Json(TrashResponse {
        items,
        next_cursor,
        total,
        retention_days: s.trash_retention_days,
    }))
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

    async fn count(pool: &sqlx::PgPool, sql: &str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql).bind(id).fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn trash_restore_and_list_round_trip() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let a = asset(&t.pool, uid, "a.jpg").await;
        let b = asset(&t.pool, uid, "b.jpg").await;
        let c = asset(&t.pool, uid, "c.jpg").await;
        let app = app(&t.pool);

        // Unknown ids and repeats are fine: only real, live items count.
        let (st, body) = send(
            &app,
            "POST",
            "/assets/trash",
            &key,
            Some(ids(&[a, b, b, Uuid::new_v4()])),
        )
        .await;
        assert_eq!((st, body["count"].as_u64()), (StatusCode::OK, Some(2)));
        let (_, body) = send(&app, "POST", "/assets/trash", &key, Some(ids(&[a, b]))).await;
        assert_eq!(body["count"], 0, "idempotent");

        let (_, lib) = send(&app, "GET", "/assets", &key, None).await;
        assert_eq!(lib["items"].as_array().unwrap().len(), 1);
        assert_eq!(send(&app, "GET", "/assets/count", &key, None).await.1["count"], 1);
        assert_eq!(
            send(&app, "GET", &format!("/assets/{a}"), &key, None).await.0,
            StatusCode::NOT_FOUND
        );

        let (_, tr) = send(&app, "GET", "/assets/trash", &key, None).await;
        let items = tr["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(tr["retention_days"], 30);
        assert_eq!(tr["total"], 2);
        let item = &items[0];
        // Same shape as the library list, plus deleted_at / purge_at (30 days apart).
        for field in [
            "id",
            "original_name",
            "content_type",
            "size_bytes",
            "sort_time",
            "thumbhash",
            "thumb_url",
        ] {
            assert!(item.get(field).is_some(), "missing {field}");
        }
        let fmt = &time::format_description::well_known::Rfc3339;
        let deleted = time::OffsetDateTime::parse(item["deleted_at"].as_str().unwrap(), fmt).unwrap();
        let purge_at = time::OffsetDateTime::parse(item["purge_at"].as_str().unwrap(), fmt).unwrap();
        assert_eq!(purge_at - deleted, time::Duration::days(30));

        // Restore brings them back; restoring a live item or repeating counts nothing.
        let (_, body) = send(&app, "POST", "/assets/restore", &key, Some(ids(&[a, c]))).await;
        assert_eq!(body["count"], 1, "only a was in the trash");
        assert_eq!(
            send(&app, "POST", "/assets/restore", &key, Some(ids(&[a]))).await.1["count"],
            0
        );
        assert_eq!(send(&app, "GET", "/assets/count", &key, None).await.1["count"], 2);
        let (_, tr) = send(&app, "GET", "/assets/trash", &key, None).await;
        assert_eq!(tr["items"].as_array().unwrap().len(), 1);

        // The single-item endpoint still works and lands in the same trash.
        let (st, _) = send(&app, "DELETE", &format!("/assets/{c}"), &key, None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        assert_eq!(
            send(&app, "DELETE", &format!("/assets/{c}"), &key, None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(&app, "GET", "/assets/trash", &key, None).await.1["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        t.teardown().await;
    }

    #[tokio::test]
    async fn bulk_requests_are_capped_and_validated() {
        let Some(t) = TestDb::new().await else { return };
        let (_, key) = user(&t.pool, "a@example.com").await;
        let app = app(&t.pool);
        let many: Vec<Uuid> = (0..1001).map(|_| Uuid::new_v4()).collect();
        for path in ["/assets/trash", "/assets/restore", "/assets/purge"] {
            let (st, _) = send(&app, "POST", path, &key, Some(ids(&many))).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{path}");
            let (st, body) = send(&app, "POST", path, &key, Some(ids(&many[..1000]))).await;
            assert_eq!((st, body["count"].as_u64()), (StatusCode::OK, Some(0)), "{path}");
            let (st, _) = send(&app, "POST", path, &key, Some(json!({"ids": ["nope"]}))).await;
            assert!(st.is_client_error(), "{path}");
        }
        let (st, _) = send(&app, "POST", "/assets/trash", "cyk_wrong", Some(ids(&[]))).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        t.teardown().await;
    }

    #[tokio::test]
    async fn other_users_items_are_untouchable() {
        let Some(t) = TestDb::new().await else { return };
        let (alice, ka) = user(&t.pool, "alice@example.com").await;
        let (_, kb) = user(&t.pool, "bob@example.com").await;
        let a = asset(&t.pool, alice, "a.jpg").await;
        let app = app(&t.pool);

        assert_eq!(
            send(&app, "POST", "/assets/trash", &kb, Some(ids(&[a]))).await.1["count"],
            0
        );
        assert_eq!(send(&app, "GET", "/assets/count", &ka, None).await.1["count"], 1);
        assert_eq!(
            send(&app, "POST", "/assets/trash", &ka, Some(ids(&[a]))).await.1["count"],
            1
        );

        // Bob can neither see, restore, purge nor empty Alice's trash.
        assert_eq!(
            send(&app, "GET", "/assets/trash", &kb, None).await.1["items"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            send(&app, "POST", "/assets/restore", &kb, Some(ids(&[a]))).await.1["count"],
            0
        );
        assert_eq!(
            send(&app, "POST", "/assets/purge", &kb, Some(ids(&[a]))).await.1["count"],
            0
        );
        assert_eq!(send(&app, "POST", "/assets/trash/empty", &kb, None).await.1["count"], 0);
        let purging: Option<time::OffsetDateTime> = sqlx::query_scalar("SELECT purging_at FROM assets WHERE id = $1")
            .bind(a)
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert!(purging.is_none());
        assert_eq!(
            send(&app, "GET", "/assets/trash", &ka, None).await.1["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        t.teardown().await;
    }

    #[tokio::test]
    async fn trash_pagination_is_gapless_when_deletion_times_tie() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let mut all = Vec::new();
        for i in 0..23 {
            all.push(asset(&t.pool, uid, &format!("f{i}.jpg")).await);
        }
        let app = app(&t.pool);
        // One statement => one now() => every row shares deleted_at; only the id tiebreak orders them.
        assert_eq!(
            send(&app, "POST", "/assets/trash", &key, Some(ids(&all))).await.1["count"],
            23
        );

        let (mut seen, mut cursor, mut pages) = (Vec::new(), None::<String>, 0);
        loop {
            let path = match &cursor {
                Some(c) => format!("/assets/trash?limit=5&cursor={c}"),
                None => "/assets/trash?limit=5".to_string(),
            };
            let (st, page) = send(&app, "GET", &path, &key, None).await;
            assert_eq!(st, StatusCode::OK);
            seen.extend(
                page["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["id"].as_str().unwrap().to_string()),
            );
            pages += 1;
            match page["next_cursor"].as_str() {
                Some(c) => cursor = Some(c.into()),
                None => break,
            }
            assert!(pages < 20);
        }
        assert_eq!(pages, 5);
        assert_eq!(seen.iter().collect::<std::collections::HashSet<_>>().len(), 23);
        let mut expected: Vec<String> = all.iter().map(|i| i.to_string()).collect();
        expected.sort();
        expected.reverse();
        assert_eq!(seen, expected, "ordered by (deleted_at, id) descending");
        assert_eq!(
            send(&app, "GET", "/assets/trash?cursor=zz", &key, None).await.0,
            StatusCode::BAD_REQUEST
        );
        t.teardown().await;
    }

    #[tokio::test]
    async fn purge_only_takes_trashed_items_and_removes_everything_in_the_sweep() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let live = asset(&t.pool, uid, "live.jpg").await;
        let gone = asset(&t.pool, uid, "gone.jpg").await;
        let kept = asset(&t.pool, uid, "kept.jpg").await;
        let app = app(&t.pool);
        send(&app, "POST", "/assets/trash", &key, Some(ids(&[gone, kept]))).await;
        // The faces found in the photo (docs/faces.md) must not outlive it.
        for id in [gone, kept] {
            cyanotype_core::faces::store(
                &t.pool,
                id,
                cyanotype_core::faces::MODEL,
                cyanotype_core::faces::VERSION,
                &[cyanotype_core::faces::NewFace {
                    bbox: [0.1, 0.1, 0.2, 0.2],
                    landmarks: [0.1; 10],
                    score: 0.9,
                    embedding: Some(vec![0.1; cyanotype_core::faces::DIM]),
                }],
            )
            .await
            .unwrap();
        }
        // ... and the dogs and cats found in it (docs/pets.md).
        for id in [gone, kept] {
            cyanotype_core::pets::store(
                &t.pool,
                id,
                cyanotype_core::pets::MODEL,
                cyanotype_core::pets::VERSION,
                &cyanotype_core::pets::Scan {
                    dog_share: 0.9,
                    cat_share: 0.0,
                    found: vec![cyanotype_core::pets::NewDetection {
                        kind: "dog".into(),
                        bbox: [0.1, 0.1, 0.5, 0.5],
                        score: 0.8,
                        embedding: None,
                    }],
                    fingerprinted: false,
                },
            )
            .await
            .unwrap();
        }
        // ... and so must the people and rejections: the unnamed person who only had that face goes, the one the owner named stays
        // (without the photo), and the "not this person" decisions about the purged photo's face go with it.
        let face_of = |a: Uuid| {
            let pool = t.pool.clone();
            async move {
                sqlx::query_scalar::<_, Uuid>("SELECT id FROM faces WHERE asset_id = $1")
                    .bind(a)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        let (gone_face, kept_face) = (face_of(gone).await, face_of(kept).await);
        let unnamed: Uuid = sqlx::query_scalar("INSERT INTO people (owner_id) VALUES ($1) RETURNING id")
            .bind(uid)
            .fetch_one(&t.pool)
            .await
            .unwrap();
        let named: Uuid =
            sqlx::query_scalar("INSERT INTO people (owner_id, name) VALUES ($1, 'Person A') RETURNING id")
                .bind(uid)
                .fetch_one(&t.pool)
                .await
                .unwrap();
        sqlx::query("UPDATE faces SET person_id = $2 WHERE id = $1")
            .bind(gone_face)
            .bind(unnamed)
            .execute(&t.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE faces SET person_id = $2 WHERE id = $1")
            .bind(kept_face)
            .bind(named)
            .execute(&t.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO face_rejections (face_id, person_id) VALUES ($1, $2), ($3, $2)")
            .bind(gone_face)
            .bind(named)
            .bind(kept_face)
            .execute(&t.pool)
            .await
            .unwrap();

        // A live item is not purgeable ("only items already in trash").
        assert_eq!(
            send(&app, "POST", "/assets/purge", &key, Some(ids(&[live, gone])))
                .await
                .1["count"],
            1
        );
        assert_eq!(
            send(&app, "POST", "/assets/purge", &key, Some(ids(&[gone]))).await.1["count"],
            0
        );
        // Queued items leave the trash right away and cannot be restored.
        let (_, tr) = send(&app, "GET", "/assets/trash", &key, None).await;
        assert_eq!(tr["items"].as_array().unwrap().len(), 1);
        assert_eq!(
            send(&app, "POST", "/assets/restore", &key, Some(ids(&[gone]))).await.1["count"],
            0
        );

        // The sweep: storage first, then jobs + embeddings + row. A storage failure leaves everything for a retry.
        let objects = FakeObjects::default();
        objects.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            purge::sweep(&t.pool, &objects).await.unwrap(),
            purge::Swept { purged: 0, failed: 1 }
        );
        for sql in [
            "SELECT count(*) FROM assets WHERE id = $1",
            "SELECT count(*) FROM jobs WHERE asset_id = $1",
            "SELECT count(*) FROM asset_embeddings WHERE asset_id = $1",
            "SELECT count(*) FROM faces WHERE asset_id = $1",
            "SELECT count(*) FROM face_scans WHERE asset_id = $1",
            "SELECT count(*) FROM pet_detections WHERE asset_id = $1",
            "SELECT count(*) FROM pet_scans WHERE asset_id = $1",
        ] {
            assert_eq!(count(&t.pool, sql, gone).await, 1, "{sql}");
        }
        objects.fail.store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            purge::sweep(&t.pool, &objects).await.unwrap(),
            purge::Swept { purged: 1, failed: 0 }
        );
        for sql in [
            "SELECT count(*) FROM assets WHERE id = $1",
            "SELECT count(*) FROM jobs WHERE asset_id = $1",
            "SELECT count(*) FROM asset_embeddings WHERE asset_id = $1",
            "SELECT count(*) FROM faces WHERE asset_id = $1",
            "SELECT count(*) FROM face_scans WHERE asset_id = $1",
            "SELECT count(*) FROM pet_detections WHERE asset_id = $1",
            "SELECT count(*) FROM pet_scans WHERE asset_id = $1",
        ] {
            assert_eq!(count(&t.pool, sql, gone).await, 0, "{sql}");
        }
        assert_eq!(
            *objects.deleted.lock().unwrap(),
            vec![(format!("originals/{uid}/gone.jpg"), format!("derived/{uid}/{gone}/"))]
        );
        let people: Vec<Option<String>> = sqlx::query_scalar("SELECT name FROM people ORDER BY name")
            .fetch_all(&t.pool)
            .await
            .unwrap();
        assert_eq!(
            people,
            vec![Some("Person A".to_string())],
            "the unnamed person with no face left went, the named one stayed"
        );
        assert_eq!(
            count(
                &t.pool,
                "SELECT count(*) FROM face_rejections WHERE person_id = $1",
                named
            )
            .await,
            1,
            "only the kept photo's rejection is left"
        );
        // Re-running is a no-op; the live and the merely trashed item are untouched.
        assert_eq!(purge::sweep(&t.pool, &objects).await.unwrap(), purge::Swept::default());
        assert_eq!(
            count(&t.pool, "SELECT count(*) FROM assets WHERE id = $1", live).await,
            1
        );
        assert_eq!(
            count(&t.pool, "SELECT count(*) FROM jobs WHERE asset_id = $1", kept).await,
            1
        );
        assert_eq!(
            count(&t.pool, "SELECT count(*) FROM faces WHERE asset_id = $1", kept).await,
            1,
            "the faces of a photo that is only in the trash stay"
        );
        assert_eq!(
            count(&t.pool, "SELECT count(*) FROM pet_detections WHERE asset_id = $1", kept).await,
            1,
            "so do its dogs and cats"
        );
        t.teardown().await;
    }

    #[tokio::test]
    async fn empty_trash_queues_everything_in_the_trash_and_only_that() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let (other, _) = user(&t.pool, "b@example.com").await;
        let live = asset(&t.pool, uid, "live.jpg").await;
        let t1 = asset(&t.pool, uid, "t1.jpg").await;
        let t2 = asset(&t.pool, uid, "t2.jpg").await;
        let theirs = asset(&t.pool, other, "theirs.jpg").await;
        sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = ANY($1)")
            .bind(vec![t1, t2, theirs])
            .execute(&t.pool)
            .await
            .unwrap();
        let app = app(&t.pool);

        assert_eq!(
            send(&app, "POST", "/assets/trash/empty", &key, None).await.1["count"],
            2
        );
        assert_eq!(
            send(&app, "POST", "/assets/trash/empty", &key, None).await.1["count"],
            0
        );
        assert_eq!(purge::pending(&t.pool).await.unwrap(), 2);
        assert_eq!(purge::sweep(&t.pool, &FakeObjects::default()).await.unwrap().purged, 2);
        assert_eq!(
            count(&t.pool, "SELECT count(*) FROM assets WHERE id = $1", live).await,
            1
        );
        assert_eq!(
            count(&t.pool, "SELECT count(*) FROM assets WHERE id = $1", theirs).await,
            1
        );
        assert_eq!(count(&t.pool, "SELECT count(*) FROM assets WHERE id = $1", t1).await, 0);
        t.teardown().await;
    }

    #[tokio::test]
    async fn trash_past_retention_is_purged_and_younger_trash_is_kept() {
        let Some(t) = TestDb::new().await else { return };
        let (uid, key) = user(&t.pool, "a@example.com").await;
        let old = asset(&t.pool, uid, "old.jpg").await;
        let young = asset(&t.pool, uid, "young.jpg").await;
        let live = asset(&t.pool, uid, "live.jpg").await;
        sqlx::query("UPDATE assets SET deleted_at = now() - interval '31 days' WHERE id = $1")
            .bind(old)
            .execute(&t.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE assets SET deleted_at = now() - interval '29 days' WHERE id = $1")
            .bind(young)
            .execute(&t.pool)
            .await
            .unwrap();

        assert_eq!(purge::mark_expired(&t.pool, 30).await.unwrap(), 1);
        assert_eq!(purge::mark_expired(&t.pool, 30).await.unwrap(), 0);
        let objects = FakeObjects::default();
        assert_eq!(purge::sweep(&t.pool, &objects).await.unwrap().purged, 1);
        assert_eq!(
            count(&t.pool, "SELECT count(*) FROM assets WHERE id = $1", old).await,
            0
        );
        assert_eq!(
            count(&t.pool, "SELECT count(*) FROM assets WHERE id = $1", young).await,
            1
        );
        assert_eq!(
            count(&t.pool, "SELECT count(*) FROM assets WHERE id = $1", live).await,
            1
        );

        // purge_at in the listing counts down from the (shortened) retention.
        let app = app(&t.pool);
        let (_, tr) = send(&app, "GET", "/assets/trash", &key, None).await;
        let item = &tr["items"][0];
        let fmt = &time::format_description::well_known::Rfc3339;
        let left = time::OffsetDateTime::parse(item["purge_at"].as_str().unwrap(), fmt).unwrap()
            - time::OffsetDateTime::now_utc();
        assert!(
            left > time::Duration::hours(23) && left < time::Duration::hours(25),
            "{left}"
        );
        t.teardown().await;
    }
}
