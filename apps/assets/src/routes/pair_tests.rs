//! RAW + JPEG pairs against a real Postgres (skipped without `DATABASE_URL`, like the other DB tests).
use axum::http::StatusCode;
use cyanotype_core::{
    jobs,
    pairing::{Paired, pair_asset, unpair},
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::{UploadRow, register_asset};
use crate::{
    purge,
    testutil::{FakeObjects, TestDb, app, ids, send, user},
};

/// An asset with metadata as the worker would leave it: capture time (RFC 3339, or none) and camera.
async fn photo(pool: &PgPool, owner: Uuid, name: &str, taken: Option<&str>, camera: Option<(&str, &str)>) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, width, height,
                             taken_at, camera_make, camera_model)
         VALUES ($1, sha256(($2 || $3::text)::bytea), $4, $2, 'image/jpeg', 10, 10, 10, $5::timestamptz, $6, $7)
         RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(taken.unwrap_or("-"))
    .bind(format!("originals/{owner}/{name}-{}", taken.unwrap_or("-")))
    .bind(taken)
    .bind(camera.map(|c| c.0))
    .bind(camera.map(|c| c.1))
    .fetch_one(pool)
    .await
    .unwrap();
    id
}

const T: &str = "2024-05-01T12:00:00Z";

async fn link(pool: &PgPool, id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT primary_id FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn rows(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn pairs_whichever_half_finishes_last() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);

    // JPEG first, RAW second.
    let jpg = photo(&t.pool, uid, "IMG_0001.JPG", Some(T), None).await;
    assert_eq!(pair_asset(&t.pool, jpg).await.unwrap(), Paired::NoMatch);
    let raw = photo(&t.pool, uid, "img_0001.cr3", Some("2024-05-01T12:00:01Z"), None).await;
    assert_eq!(
        pair_asset(&t.pool, raw).await.unwrap(),
        Paired::Linked { raw, primary: jpg }
    );
    // RAW first, HEIC second (different second pair).
    let raw2 = photo(&t.pool, uid, "IMG_0002.DNG", Some(T), None).await;
    assert_eq!(pair_asset(&t.pool, raw2).await.unwrap(), Paired::NoMatch);
    let heic = photo(&t.pool, uid, "IMG_0002.HEIC", Some(T), None).await;
    assert_eq!(
        pair_asset(&t.pool, heic).await.unwrap(),
        Paired::Linked {
            raw: raw2,
            primary: heic
        }
    );
    // Idempotent from both sides.
    for id in [jpg, raw, raw2, heic] {
        assert_eq!(pair_asset(&t.pool, id).await.unwrap(), Paired::Already);
    }

    let (_, lib) = send(&app, "GET", "/assets", &key, None).await;
    let items = lib["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "each pair is one tile");
    let first = items.iter().find(|i| i["id"] == json!(jpg)).unwrap();
    assert_eq!(first["paired"].as_array().unwrap().len(), 1);
    let p = &first["paired"][0];
    assert_eq!(p["id"], json!(raw));
    assert_eq!(p["original_name"], "img_0001.cr3");
    assert_eq!((p["kind"].as_str(), p["size_bytes"].as_i64()), (Some("raw"), Some(10)));
    assert!(p["content_type"].is_string());
    assert_eq!(send(&app, "GET", "/assets/count", &key, None).await.1["count"], 2);
    let (_, one) = send(&app, "GET", &format!("/assets/{heic}"), &key, None).await;
    assert_eq!(one["paired"][0]["id"], json!(raw2));
    // A single photo has an empty list.
    let solo = photo(&t.pool, uid, "solo.jpg", Some(T), None).await;
    assert_eq!(
        send(&app, "GET", &format!("/assets/{solo}"), &key, None).await.1["paired"],
        json!([])
    );
    t.teardown().await;
}

#[tokio::test]
async fn only_a_raw_and_a_primary_with_matching_stem_and_time_pair() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, _) = user(&t.pool, "a@example.com").await;
    let (other, _) = user(&t.pool, "b@example.com").await;
    // Two JPEGs never pair, nor two RAWs.
    let a = photo(&t.pool, uid, "X.JPG", Some(T), None).await;
    let b = photo(&t.pool, uid, "x.jpeg", Some(T), None).await;
    assert_eq!(pair_asset(&t.pool, b).await.unwrap(), Paired::NoMatch);
    let r1 = photo(&t.pool, uid, "x.nef", Some("2024-05-01T12:00:00.5Z"), None).await;
    // The RAW takes one of the two JPEGs (the closest), and a second RAW finds none left in time... it finds the other.
    assert!(matches!(pair_asset(&t.pool, r1).await.unwrap(), Paired::Linked { .. }));
    let r2 = photo(&t.pool, uid, "x.arw", Some(T), None).await;
    assert!(matches!(pair_asset(&t.pool, r2).await.unwrap(), Paired::Linked { .. }));
    let r3 = photo(&t.pool, uid, "x.raf", Some(T), None).await;
    assert_eq!(
        pair_asset(&t.pool, r3).await.unwrap(),
        Paired::NoMatch,
        "no primary left"
    );
    assert_eq!(
        rows(&t.pool, "SELECT count(*) FROM assets WHERE primary_id IS NOT NULL").await,
        2
    );
    let _ = (a, b);

    // Different stem, 2 s apart, another owner, PNG, trashed: none pair.
    let j = photo(&t.pool, uid, "IMG_9.jpg", Some(T), None).await;
    let _far = photo(&t.pool, uid, "IMG_9.cr2", Some("2024-05-01T12:00:02Z"), None).await;
    let _name = photo(&t.pool, uid, "IMG_8.cr2", Some(T), None).await;
    let _theirs = photo(&t.pool, other, "IMG_9.nef", Some(T), None).await;
    let png = photo(&t.pool, uid, "IMG_9.png", Some(T), None).await;
    for id in [j, png] {
        assert!(matches!(
            pair_asset(&t.pool, id).await.unwrap(),
            Paired::NoMatch | Paired::NotApplicable
        ));
    }
    let gone = photo(&t.pool, uid, "IMG_7.jpg", Some(T), None).await;
    let raw = photo(&t.pool, uid, "IMG_7.cr3", Some(T), None).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(gone)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        pair_asset(&t.pool, raw).await.unwrap(),
        Paired::NoMatch,
        "trashed counterpart"
    );
    assert_eq!(
        pair_asset(&t.pool, gone).await.unwrap(),
        Paired::NotApplicable,
        "trashed subject"
    );
    t.teardown().await;
}

#[tokio::test]
async fn missing_capture_time_falls_back_to_the_camera() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, _) = user(&t.pool, "a@example.com").await;
    let cam = Some(("Canon", "EOS R5"));
    let jpg = photo(&t.pool, uid, "IMG_5.JPG", None, cam).await;
    let raw = photo(&t.pool, uid, "IMG_5.CR3", Some(T), cam).await;
    assert!(matches!(pair_asset(&t.pool, raw).await.unwrap(), Paired::Linked { .. }));
    assert_eq!(link(&t.pool, raw).await, Some(jpg));
    // No time and no camera on either side: left alone.
    let a = photo(&t.pool, uid, "IMG_6.JPG", None, None).await;
    let b = photo(&t.pool, uid, "IMG_6.CR3", None, None).await;
    assert_eq!(pair_asset(&t.pool, a).await.unwrap(), Paired::NoMatch);
    assert_eq!(pair_asset(&t.pool, b).await.unwrap(), Paired::NoMatch);
    t.teardown().await;
}

#[tokio::test]
async fn trash_restore_and_purge_act_on_the_whole_pair() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let jpg = photo(&t.pool, uid, "IMG_1.JPG", Some(T), None).await;
    let raw = photo(&t.pool, uid, "IMG_1.CR3", Some(T), None).await;
    let other = photo(&t.pool, uid, "IMG_2.JPG", Some(T), None).await;
    pair_asset(&t.pool, raw).await.unwrap();

    // Trashing the JPEG tile takes the RAW with it; the pair counts once.
    let (_, body) = send(&app, "POST", "/assets/trash", &key, Some(ids(&[jpg]))).await;
    assert_eq!(body["count"], 1);
    assert_eq!(
        rows(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL").await,
        2
    );
    assert_eq!(
        send(&app, "POST", "/assets/trash", &key, Some(ids(&[jpg]))).await.1["count"],
        0
    );
    let (_, tr) = send(&app, "GET", "/assets/trash", &key, None).await;
    assert_eq!(
        (tr["items"].as_array().unwrap().len(), tr["total"].as_i64()),
        (1, Some(1))
    );
    assert_eq!(tr["items"][0]["paired"][0]["id"], json!(raw));
    assert_eq!(send(&app, "GET", "/assets/count", &key, None).await.1["count"], 1);

    // Restore brings both back.
    let (_, body) = send(&app, "POST", "/assets/restore", &key, Some(ids(&[jpg]))).await;
    assert_eq!(body["count"], 1);
    assert_eq!(
        rows(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL").await,
        0
    );
    assert_eq!(link(&t.pool, raw).await, Some(jpg));
    // The single-photo DELETE does the same as trash.
    assert_eq!(
        send(&app, "DELETE", &format!("/assets/{jpg}"), &key, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        rows(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL").await,
        2
    );

    // Delete forever: both rows are marked, and the sweep deletes both originals and both derived prefixes.
    let (_, body) = send(&app, "POST", "/assets/purge", &key, Some(ids(&[jpg, other]))).await;
    assert_eq!(body["count"], 1, "other is not in the trash");
    assert_eq!(purge::pending(&t.pool).await.unwrap(), 2);
    assert_eq!(
        send(&app, "POST", "/assets/restore", &key, Some(ids(&[jpg]))).await.1["count"],
        0
    );
    let objects = FakeObjects::default();
    let swept = purge::sweep(&t.pool, &objects).await.unwrap();
    assert_eq!((swept.purged, swept.failed), (2, 0));
    let deleted = objects.deleted.lock().unwrap().clone();
    assert_eq!(deleted.len(), 2);
    assert!(deleted.iter().any(|d| d.0.contains("IMG_1.JPG")) && deleted.iter().any(|d| d.0.contains("IMG_1.CR3")));
    assert_eq!(
        rows(&t.pool, "SELECT count(*) FROM assets").await,
        1,
        "only the unrelated photo is left"
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_failed_purge_keeps_both_files_marked_and_empty_trash_counts_photos() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let jpg = photo(&t.pool, uid, "IMG_1.JPG", Some(T), None).await;
    let raw = photo(&t.pool, uid, "IMG_1.CR3", Some(T), None).await;
    let solo = photo(&t.pool, uid, "IMG_2.JPG", Some(T), None).await;
    pair_asset(&t.pool, jpg).await.unwrap();
    send(&app, "POST", "/assets/trash", &key, Some(ids(&[jpg, solo]))).await;
    let (_, body) = send(&app, "POST", "/assets/trash/empty", &key, None).await;
    assert_eq!(body["count"], 2, "a pair and a single photo");
    let objects = FakeObjects::default();
    objects.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(purge::sweep(&t.pool, &objects).await.unwrap().failed, 3);
    assert_eq!(purge::pending(&t.pool).await.unwrap(), 3);
    objects.fail.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(purge::sweep(&t.pool, &objects).await.unwrap().purged, 3);
    assert_eq!(rows(&t.pool, "SELECT count(*) FROM assets").await, 0);
    let _ = raw;
    t.teardown().await;
}

#[tokio::test]
async fn retention_expiry_purges_the_pair_together() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, _) = user(&t.pool, "a@example.com").await;
    let jpg = photo(&t.pool, uid, "IMG_1.JPG", Some(T), None).await;
    let raw = photo(&t.pool, uid, "IMG_1.CR3", Some(T), None).await;
    pair_asset(&t.pool, jpg).await.unwrap();
    // Only the primary is old enough (a partial state that must still never purge half a pair).
    sqlx::query("UPDATE assets SET deleted_at = now() - interval '40 days' WHERE id = $1")
        .bind(jpg)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(purge::mark_expired(&t.pool, 30).await.unwrap(), 2);
    assert_eq!(purge::pending(&t.pool).await.unwrap(), 2);
    let _ = raw;
    t.teardown().await;
}

#[tokio::test]
async fn albums_show_a_pair_once_and_operate_on_the_primary() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let jpg = photo(&t.pool, uid, "IMG_1.JPG", Some(T), None).await;
    let raw = photo(&t.pool, uid, "IMG_1.CR3", Some(T), None).await;
    let before = photo(&t.pool, uid, "IMG_0.JPG", Some("2024-01-01T00:00:00Z"), None).await;
    // The RAW was in an album (and its cover) before the pair existed.
    let (_, al) = send(
        &app,
        "POST",
        "/assets/albums",
        &key,
        Some(json!({"title": "Trip", "asset_ids": [raw, before]})),
    )
    .await;
    let album = al["id"].as_str().unwrap().to_string();
    let path = format!("/assets/albums/{album}");
    send(&app, "PATCH", &path, &key, Some(json!({"cover_asset_id": raw}))).await;
    assert_eq!(al["photo_count"], 2);

    pair_asset(&t.pool, jpg).await.unwrap();
    let (_, got) = send(&app, "GET", &path, &key, None).await;
    assert_eq!(got["photo_count"], 2, "the pair counts once: IMG_1 and IMG_0");
    assert_eq!(got["cover"]["id"], json!(jpg), "cover moved to the primary");
    let (_, items) = send(&app, "GET", &format!("{path}/assets"), &key, None).await;
    let list = items["items"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["id"], json!(jpg));
    assert_eq!(list[0]["paired"][0]["id"], json!(raw));
    assert_eq!(
        rows(&t.pool, "SELECT count(*) FROM album_assets").await,
        2,
        "no membership left on the RAW"
    );
    let (_, one) = send(&app, "GET", &format!("/assets/{jpg}"), &key, None).await;
    assert_eq!(one["albums"].as_array().unwrap().len(), 1);

    // Adding the RAW id is a no-op, adding the primary is idempotent; set cover to the RAW is refused.
    assert_eq!(
        send(&app, "POST", &format!("{path}/assets"), &key, Some(ids(&[raw, jpg])))
            .await
            .1["count"],
        0
    );
    let (st, _) = send(&app, "PATCH", &path, &key, Some(json!({"cover_asset_id": raw}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Removing the primary removes the pair's tile.
    assert_eq!(
        send(&app, "POST", &format!("{path}/assets/remove"), &key, Some(ids(&[jpg])))
            .await
            .1["count"],
        1
    );
    assert_eq!(send(&app, "GET", &path, &key, None).await.1["photo_count"], 1);

    // A new album built from the library's tile.
    let (_, al2) = send(
        &app,
        "POST",
        "/assets/albums",
        &key,
        Some(json!({"title": "B", "asset_ids": [jpg, raw]})),
    )
    .await;
    assert_eq!(al2["photo_count"], 1);
    t.teardown().await;
}

#[tokio::test]
async fn unpair_separates_and_is_remembered() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (_, other_key) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    let jpg = photo(&t.pool, uid, "IMG_1.JPG", Some(T), None).await;
    let raw = photo(&t.pool, uid, "IMG_1.CR3", Some(T), None).await;
    pair_asset(&t.pool, jpg).await.unwrap();

    let path = format!("/assets/{jpg}/unpair");
    assert_eq!(
        send(&app, "POST", &path, &other_key, None).await.0,
        StatusCode::NOT_FOUND,
        "someone else's photo"
    );
    assert_eq!(send(&app, "POST", &path, &key, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(
        send(&app, "POST", &path, &key, None).await.0,
        StatusCode::NOT_FOUND,
        "no longer paired"
    );
    let (_, lib) = send(&app, "GET", "/assets", &key, None).await;
    assert_eq!(lib["items"].as_array().unwrap().len(), 2);
    assert_eq!(send(&app, "GET", "/assets/count", &key, None).await.1["count"], 2);
    // The matcher (a re-run of the metadata or backfill job) does not join them again.
    for id in [jpg, raw] {
        assert_eq!(pair_asset(&t.pool, id).await.unwrap(), Paired::NoMatch);
    }
    // Trashing now affects only the one tile.
    assert_eq!(
        send(&app, "POST", "/assets/trash", &key, Some(ids(&[jpg]))).await.1["count"],
        1
    );
    assert_eq!(
        rows(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL").await,
        1
    );

    // Either file's id may be used to unpair; an unpaired or unknown id is false.
    let j2 = photo(&t.pool, uid, "IMG_2.JPG", Some(T), None).await;
    let r2 = photo(&t.pool, uid, "IMG_2.DNG", Some(T), None).await;
    pair_asset(&t.pool, r2).await.unwrap();
    assert!(unpair(&t.pool, uid, r2).await.unwrap());
    assert!(!unpair(&t.pool, uid, j2).await.unwrap());
    assert!(!unpair(&t.pool, uid, Uuid::new_v4()).await.unwrap());
    // Rejections go away with the assets.
    sqlx::query("DELETE FROM assets").execute(&t.pool).await.unwrap();
    assert_eq!(rows(&t.pool, "SELECT count(*) FROM asset_pair_rejections").await, 0);
    t.teardown().await;
}

#[tokio::test]
async fn reuploading_either_file_of_a_trashed_pair_restores_both() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let jpg = photo(&t.pool, uid, "IMG_1.JPG", Some(T), None).await;
    let raw = photo(&t.pool, uid, "IMG_1.CR3", Some(T), None).await;
    pair_asset(&t.pool, jpg).await.unwrap();
    send(&app, "POST", "/assets/trash", &key, Some(ids(&[jpg]))).await;
    sqlx::query(
        "INSERT INTO uploads (upload_id, owner_id, sha256, key, size_bytes, part_size, original_name, content_type)
         SELECT 'u1', owner_id, sha256, 'k', 1, 1, original_name, content_type FROM assets WHERE id = $1",
    )
    .bind(raw)
    .execute(&t.pool)
    .await
    .unwrap();
    let up = UploadRow {
        sha256: sqlx::query_scalar("SELECT sha256 FROM assets WHERE id = $1")
            .bind(raw)
            .fetch_one(&t.pool)
            .await
            .unwrap(),
        key: "k".into(),
        size_bytes: 1,
        original_name: "IMG_1.CR3".into(),
        content_type: "image/x-canon-cr3".into(),
        file_modified_at: None,
    };
    assert_eq!(register_asset(&t.pool, uid, "u1", &up).await.unwrap(), raw);
    assert_eq!(
        rows(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL").await,
        0
    );
    assert_eq!(
        send(&app, "GET", "/assets", &key, None).await.1["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    t.teardown().await;
}

#[tokio::test]
async fn restoring_pairs_a_file_whose_counterpart_arrived_while_it_was_trashed() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let jpg = photo(&t.pool, uid, "IMG_1.JPG", Some(T), None).await;
    send(&app, "POST", "/assets/trash", &key, Some(ids(&[jpg]))).await;
    let raw = photo(&t.pool, uid, "IMG_1.CR3", Some(T), None).await;
    assert_eq!(pair_asset(&t.pool, raw).await.unwrap(), Paired::NoMatch);
    send(&app, "POST", "/assets/restore", &key, Some(ids(&[jpg]))).await;
    assert_eq!(link(&t.pool, raw).await, Some(jpg));
    t.teardown().await;
}

#[tokio::test]
async fn backfill_enqueues_once_and_draining_it_pairs_existing_photos_idempotently() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, _) = user(&t.pool, "a@example.com").await;
    let jpg = photo(&t.pool, uid, "IMG_1.JPG", Some(T), None).await;
    let raw = photo(&t.pool, uid, "IMG_1.CR3", Some(T), None).await;
    let solo = photo(&t.pool, uid, "IMG_2.JPG", Some(T), None).await;
    let trashed = photo(&t.pool, uid, "IMG_3.JPG", Some(T), None).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&t.pool)
        .await
        .unwrap();
    // A photo whose metadata job has not run yet is not enqueued (it pairs when that job finishes).
    let pending = photo(&t.pool, uid, "IMG_4.JPG", Some(T), None).await;
    sqlx::query("UPDATE assets SET width = NULL WHERE id = $1")
        .bind(pending)
        .execute(&t.pool)
        .await
        .unwrap();

    // The statement the migration runs on deploy.
    let migration = include_str!("../../../../crates/cyanotype-core/migrations/0005_raw_jpeg_pairs.sql");
    let backfill = &migration[migration.find("INSERT INTO jobs").unwrap()..];
    for _ in 0..2 {
        sqlx::query(backfill).execute(&t.pool).await.unwrap();
        assert_eq!(
            rows(&t.pool, "SELECT count(*) FROM jobs WHERE kind = 'pair'").await,
            3,
            "re-running adds nothing"
        );
    }
    // Drain like the worker does, twice over.
    let mut linked = 0;
    while let Some(job) = jobs::claim(&t.pool, jobs::KIND_PAIR).await.unwrap() {
        if matches!(pair_asset(&t.pool, job.asset_id).await.unwrap(), Paired::Linked { .. }) {
            linked += 1;
        }
        jobs::complete(&t.pool, job.id).await.unwrap();
    }
    assert_eq!(linked, 1);
    for id in [jpg, raw, solo] {
        let again = pair_asset(&t.pool, id).await.unwrap();
        assert!(matches!(again, Paired::Already | Paired::NoMatch), "{again:?}");
    }
    assert_eq!(link(&t.pool, raw).await, Some(jpg));
    assert_eq!(
        rows(&t.pool, "SELECT count(*) FROM assets WHERE primary_id IS NOT NULL").await,
        1
    );
    t.teardown().await;
}

#[tokio::test]
async fn concurrent_pairing_never_leaves_an_inconsistent_state() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, _) = user(&t.pool, "a@example.com").await;
    for round in 0..15 {
        let stem = format!("IMG_{round}");
        // Two JPEGs and three RAWs of the same moment all finishing at once: exactly two pairs, each file in at
        // most one.
        let mut all = Vec::new();
        for (i, ext) in ["jpg", "heic", "cr3", "dng", "arw"].into_iter().enumerate() {
            let at = format!("2024-05-01T12:00:0{}Z", i % 2);
            all.push(photo(&t.pool, uid, &format!("{stem}.{ext}"), Some(&at), None).await);
        }
        let mut handles = Vec::new();
        for id in all.iter().copied().chain(all.iter().copied()) {
            let pool = t.pool.clone();
            handles.push(tokio::spawn(async move { pair_asset(&pool, id).await.unwrap() }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let linked: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT id, primary_id FROM assets WHERE primary_id IS NOT NULL AND asset_stem(original_name) = lower($1)",
        )
        .bind(&stem)
        .fetch_all(&t.pool)
        .await
        .unwrap();
        assert_eq!(linked.len(), 2, "round {round}: {linked:?}");
        let mut primaries: Vec<Uuid> = linked.iter().map(|l| l.1).collect();
        primaries.sort();
        primaries.dedup();
        assert_eq!(primaries.len(), 2, "each primary has one RAW");
        for (raw, primary) in &linked {
            assert_eq!(link(&t.pool, *primary).await, None, "a primary is never itself hidden");
            assert!(!linked.iter().any(|l| l.0 == *primary) && !primaries.contains(raw));
        }
    }
    t.teardown().await;
}

#[tokio::test]
async fn the_sql_stem_index_expression_agrees_with_the_rust_rule() {
    let Some(t) = TestDb::new().await else { return };
    for name in [
        "IMG_1.CR3",
        "a.b.jpg",
        "noext",
        ".jpg",
        "DCIM/100/IMG.JPG",
        "C:\\x\\Y.heic",
        "Ünï.JPG",
    ] {
        let sql: String = sqlx::query_scalar("SELECT asset_stem($1)")
            .bind(name)
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert_eq!(sql, cyanotype_core::pairing::stem(name), "{name}");
    }
    t.teardown().await;
}
