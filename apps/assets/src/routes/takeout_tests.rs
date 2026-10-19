//! Google Takeout metadata through the API and the worker's apply step (`cyanotype_core::takeout`), over a real Postgres and S3
//! API (the `S3_*` variables; the tests return early without them): held by hash, applied as edits over the file's own EXIF,
//! revertible, idempotent, never over an owner's own edit, and the plan endpoint.
use axum::http::StatusCode;
use cyanotype_core::{edits, jobs, keys, takeout};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::testutil::{TestDb, app_with_s3, asset, send, user};

/// A sidecar as Takeout writes it: taken 2019-06-30 12:00:00 UTC, in Lisbon, with a caption, favourited.
fn sidecar(over: Value) -> String {
    let mut v = json!({
        "title": "IMG_1234.JPG",
        "description": "Dinner by the river",
        "photoTakenTime": {"timestamp": "1561896000", "formatted": "Jun 30, 2019, 12:00:00 PM UTC"},
        "geoData": {"latitude": 38.7223, "longitude": -9.1393, "altitude": 12.0},
        "geoDataExif": {"latitude": 0.0, "longitude": 0.0},
        "favorited": true
    });
    for (k, val) in over.as_object().unwrap() {
        v[k] = val.clone();
    }
    v.to_string()
}

async fn sha_hex(pool: &PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT encode(sha256, 'hex') FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn check(app: &axum::Router, key: &str, sha: &str, takeout: Value) -> Value {
    let body = json!({"sha256": sha, "size_bytes": 1, "filename": "IMG_1234.JPG", "takeout": takeout});
    let (st, v) = send(app, "POST", "/assets/check", key, Some(body)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    v
}

async fn jobs_of(pool: &PgPool, id: Uuid) -> Vec<(i32, String)> {
    sqlx::query_as("SELECT version, state FROM jobs WHERE asset_id = $1 AND kind = 'takeout' ORDER BY version")
        .bind(id)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn row(
    pool: &PgPool,
    id: Uuid,
) -> (
    Option<String>,
    Option<f64>,
    Option<f64>,
    Option<String>,
    bool,
    bool,
    bool,
) {
    sqlx::query_as(
        "SELECT taken_at::text, latitude, longitude, caption, favorite, date_edited, location_edited FROM assets WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn albums(pool: &PgPool, owner: Uuid) -> Vec<(String, i64)> {
    sqlx::query_as(
        "SELECT a.title, (SELECT count(*) FROM album_assets x WHERE x.album_id = a.id) FROM albums a WHERE a.owner_id = $1 ORDER BY a.title",
    )
    .bind(owner)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A photo as the metadata job leaves it: the file's own time and no location.
async fn photo(pool: &PgPool, owner: Uuid, name: &str, taken: Option<&str>) -> Uuid {
    let id = asset(pool, owner, name).await;
    sqlx::query("UPDATE assets SET taken_at = $2::timestamptz WHERE id = $1")
        .bind(id)
        .bind(taken)
        .execute(pool)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn takeout_metadata_is_applied_as_an_edit_over_the_files_own_values_and_reverts() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (owner, key) = user(&t.pool, "a@example.com").await;
    // The file says 2019-06-30 12:00 plus a few days: Takeout's time is a correction.
    let id = photo(&t.pool, owner, "one.jpg", Some("2019-07-03T08:15:00Z")).await;
    let sha = sha_hex(&t.pool, id).await;

    let r = check(
        &app,
        &key,
        &sha,
        json!({"sidecar": sidecar(json!({})), "album": "Trip to Lisbon"}),
    )
    .await;
    assert_eq!(r["status"], "exists", "{r}");
    assert_eq!(
        jobs_of(&t.pool, id).await,
        vec![(1, "queued".to_string())],
        "a job is queued for the photo that exists"
    );
    // Nothing is applied by the request itself: that is the worker's job, after processing.
    assert_eq!(row(&t.pool, id).await.3, None);

    let done = takeout::apply(&t.pool, &storage, id).await.unwrap();
    assert!(
        done.found && done.date && done.location && done.caption && done.favorite && done.album,
        "{done:?}"
    );
    let (taken, lat, lon, caption, favorite, date_edited, location_edited) = row(&t.pool, id).await;
    assert_eq!(taken.as_deref(), Some("2019-06-30 12:00:00+00"));
    assert_eq!((lat, lon), (Some(38.7223), Some(-9.1393)));
    assert_eq!(caption.as_deref(), Some("Dinner by the river"));
    assert!(favorite && date_edited && location_edited);
    assert_eq!(albums(&t.pool, owner).await, vec![("Trip to Lisbon".to_string(), 1)]);

    // It is an edit: in the bucket where reindex restores it from, and the file's own time is kept beside it.
    let sha32: [u8; 32] = keys::parse_sha256(&sha).unwrap();
    let stored = edits::parse(&storage.get_bytes(&keys::edits(owner, &sha32)).await.unwrap()).unwrap();
    assert_eq!(stored.caption.as_deref(), Some("Dinner by the river"));
    assert_eq!(stored.latitude, Some(38.7223));
    let exif: Option<String> = sqlx::query_scalar("SELECT taken_at_exif::text FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(exif.as_deref(), Some("2019-07-03 08:15:00+00"));

    // And revertible with the ordinary "Revert to original".
    let (st, v) = send(
        &app,
        "POST",
        "/assets/edits",
        &key,
        Some(json!({"ids": [id], "date": {"revert": true}, "location": {"revert": true}, "caption": {"revert": true}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (taken, lat, _, caption, _, date_edited, location_edited) = row(&t.pool, id).await;
    assert_eq!(taken.as_deref(), Some("2019-07-03 08:15:00+00"));
    assert_eq!((lat, caption, date_edited, location_edited), (None, None, false, false));
    t.teardown().await;
}

#[tokio::test]
async fn running_it_again_changes_nothing_and_new_metadata_applies_once() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (owner, key) = user(&t.pool, "a@example.com").await;
    let id = photo(&t.pool, owner, "one.jpg", Some("2019-07-03T08:15:00Z")).await;
    let sha = sha_hex(&t.pool, id).await;
    let body = json!({"sidecar": sidecar(json!({})), "album": "Trip"});

    check(&app, &key, &sha, body.clone()).await;
    assert!(takeout::apply(&t.pool, &storage, id).await.unwrap().found);
    let before = row(&t.pool, id).await;

    // The same export again: no new job, nothing to apply, nothing changed, no second album.
    check(&app, &key, &sha, body.clone()).await;
    assert_eq!(jobs_of(&t.pool, id).await.len(), 1);
    assert!(!takeout::apply(&t.pool, &storage, id).await.unwrap().found);
    assert_eq!(row(&t.pool, id).await, before);
    assert_eq!(albums(&t.pool, owner).await, vec![("Trip".to_string(), 1)]);

    // Different metadata is a new revision and applies (the caption was Takeout's own, but is an edit now: kept as it is).
    check(
        &app,
        &key,
        &sha,
        json!({"sidecar": sidecar(json!({"favorited": false})), "album": "Other"}),
    )
    .await;
    assert_eq!(jobs_of(&t.pool, id).await.len(), 2);
    let done = takeout::apply(&t.pool, &storage, id).await.unwrap();
    assert!(done.found && done.album && !done.date && !done.caption, "{done:?}");
    assert_eq!(
        albums(&t.pool, owner).await,
        vec![("Other".to_string(), 1), ("Trip".to_string(), 1)],
        "a photo moved to another folder is in both: nothing is ever removed"
    );
    t.teardown().await;
}

#[tokio::test]
async fn an_owners_own_edits_and_a_zone_sized_difference_are_left_alone() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (owner, key) = user(&t.pool, "a@example.com").await;
    // Local time stored as if UTC: two hours before Takeout's instant. A zone, not a correction.
    let zone = photo(&t.pool, owner, "zone.jpg", Some("2019-06-30T10:00:00Z")).await;
    // The owner already captioned and re-dated this one.
    let mine = photo(&t.pool, owner, "mine.jpg", Some("2019-07-03T08:15:00Z")).await;
    let (st, _) = send(
        &app,
        "POST",
        "/assets/edits",
        &key,
        Some(json!({"ids": [mine], "caption": {"text": "My words"}, "date": {"local": "2020-01-01T10:00:00", "offset": "+00:00"}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    for id in [zone, mine] {
        let sha = sha_hex(&t.pool, id).await;
        check(&app, &key, &sha, json!({"sidecar": sidecar(json!({}))})).await;
        takeout::apply(&t.pool, &storage, id).await.unwrap();
    }
    let (taken, lat, _, caption, ..) = row(&t.pool, zone).await;
    assert_eq!(
        taken.as_deref(),
        Some("2019-06-30 10:00:00+00"),
        "the file's own local time stands"
    );
    assert_eq!(
        (lat, caption.as_deref()),
        (Some(38.7223), Some("Dinner by the river")),
        "the rest still applies"
    );
    let (taken, _, _, caption, ..) = row(&t.pool, mine).await;
    assert_eq!(
        taken.as_deref(),
        Some("2020-01-01 10:00:00+00"),
        "the owner's date is not replaced"
    );
    assert_eq!(
        caption.as_deref(),
        Some("My words"),
        "the owner's caption is not replaced"
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_location_the_file_already_has_is_kept_unless_google_photos_moved_it() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (owner, key) = user(&t.pool, "a@example.com").await;
    let same = photo(&t.pool, owner, "same.jpg", Some("2019-06-30T12:00:00Z")).await;
    let moved = photo(&t.pool, owner, "moved.jpg", Some("2019-06-30T12:00:00Z")).await;
    for id in [same, moved] {
        let (la, lo) = if id == same {
            (38.72231, -9.13931)
        } else {
            (51.5, -0.12)
        };
        sqlx::query("UPDATE assets SET latitude = $2, longitude = $3 WHERE id = $1")
            .bind(id)
            .bind(la)
            .bind(lo)
            .execute(&t.pool)
            .await
            .unwrap();
        let sha = sha_hex(&t.pool, id).await;
        check(&app, &key, &sha, json!({"sidecar": sidecar(json!({}))})).await;
        takeout::apply(&t.pool, &storage, id).await.unwrap();
    }
    let (_, lat, lon, _, _, _, edited) = row(&t.pool, same).await;
    assert_eq!((lat, lon, edited), (Some(38.72231), Some(-9.13931), false));
    let (_, lat, lon, _, _, _, edited) = row(&t.pool, moved).await;
    assert_eq!((lat, lon, edited), (Some(38.7223), Some(-9.1393), true));
    t.teardown().await;
}

#[tokio::test]
async fn metadata_held_before_the_photo_exists_waits_for_its_processing() {
    let Some(t) = TestDb::new().await else { return };
    let Some((_app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (owner, _key) = user(&t.pool, "a@example.com").await;
    // The photo is not in the library yet: its metadata is held by hash.
    let name = "late.jpg";
    let sha: Vec<u8> = sqlx::query_scalar("SELECT sha256($1::bytea)")
        .bind(name)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let payload = takeout::payload_of(&takeout::Request {
        sidecar: Some(sidecar(json!({}))),
        album: Some("Trip".into()),
        favorites: true,
    })
    .unwrap();
    assert!(takeout::record(&t.pool, owner, &sha, &payload).await.unwrap());
    assert!(
        !takeout::record(&t.pool, owner, &sha, &payload).await.unwrap(),
        "the same payload is not a new revision"
    );
    assert_eq!(
        takeout::enqueue_ready(&t.pool).await.unwrap(),
        0,
        "no photo, nothing to queue"
    );

    // The upload completes and its metadata job is waiting in the queue: the sweep leaves it alone.
    let id = asset(&t.pool, owner, name).await;
    sqlx::query("INSERT INTO jobs (kind, asset_id) VALUES ('metadata', $1)")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(takeout::enqueue_ready(&t.pool).await.unwrap(), 0);
    assert!(jobs_of(&t.pool, id).await.is_empty());

    // The metadata job finishes (it calls enqueue_for_asset): now the takeout job is queued and applies.
    takeout::enqueue_for_asset(&t.pool, id).await.unwrap();
    assert_eq!(jobs_of(&t.pool, id).await.len(), 1);
    let done = takeout::apply(&t.pool, &storage, id).await.unwrap();
    assert!(done.found && done.favorite && done.album && done.location);
    let (_, applied): (i32, Option<i32>) = sqlx::query_as("SELECT rev, applied_rev FROM takeout_imports")
        .fetch_one(&t.pool)
        .await
        .map(|(r, a): (i32, i32)| (r, Some(a)))
        .unwrap();
    assert_eq!(applied, Some(1));
    t.teardown().await;
}

#[tokio::test]
async fn the_sweep_catches_a_photo_whose_metadata_job_failed_and_drops_what_never_arrived() {
    let Some(t) = TestDb::new().await else { return };
    let (owner, _key) = user(&t.pool, "a@example.com").await;
    let id = asset(&t.pool, owner, "broken.jpg").await;
    sqlx::query("INSERT INTO jobs (kind, asset_id, state, attempts, last_error, finished_at) VALUES ('metadata', $1, 'failed', 5, 'bad file', now())")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    let sha: Vec<u8> = sqlx::query_scalar("SELECT sha256 FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let p = takeout::payload_of(&takeout::Request {
        album: Some("Trip".into()),
        ..Default::default()
    })
    .unwrap();
    takeout::record(&t.pool, owner, &sha, &p).await.unwrap();
    assert_eq!(
        takeout::enqueue_ready(&t.pool).await.unwrap(),
        1,
        "a failed metadata job does not strand the album"
    );
    assert_eq!(jobs::KIND_TAKEOUT, "takeout");

    // A payload whose photo never came is dropped after the stale period; an applied one is kept (it is what makes a re-run a no-op).
    let ghost = vec![7u8; 32];
    takeout::record(&t.pool, owner, &ghost, &p).await.unwrap();
    sqlx::query("UPDATE takeout_imports SET updated_at = now() - interval '31 days'")
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE takeout_imports SET applied_rev = rev WHERE sha256 = $1")
        .bind(&sha)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(takeout::purge_stale(&t.pool).await.unwrap(), 1);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM takeout_imports")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    t.teardown().await;
}

#[tokio::test]
async fn nothing_to_hold_holds_nothing_and_a_foreign_owner_cannot_apply_anothers() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, akey) = user(&t.pool, "a@example.com").await;
    let (bob, _) = user(&t.pool, "b@example.com").await;
    let mine = photo(&t.pool, alice, "x.jpg", None).await;
    let theirs = photo(&t.pool, bob, "x.jpg", None).await; // the same bytes, another account
    let sha = sha_hex(&t.pool, mine).await;
    // A sidecar that is not JSON and no album: nothing held.
    check(&app, &akey, &sha, json!({"sidecar": "garbage"})).await;
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM takeout_imports")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    check(&app, &akey, &sha, json!({"sidecar": sidecar(json!({}))})).await;
    assert!(takeout::apply(&t.pool, &storage, mine).await.unwrap().found);
    assert!(
        !takeout::apply(&t.pool, &storage, theirs).await.unwrap().found,
        "held per owner and hash"
    );
    assert_eq!(row(&t.pool, theirs).await.3, None);
    t.teardown().await;
}

#[tokio::test]
async fn an_archived_photo_is_archived_here_with_its_raw_and_only_once() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (owner, key) = user(&t.pool, "a@example.com").await;
    let id = photo(&t.pool, owner, "one.jpg", Some("2019-07-03T08:15:00Z")).await;
    let raw = asset(&t.pool, owner, "one.cr3").await;
    sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
        .bind(raw)
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    let plain = photo(&t.pool, owner, "plain.jpg", None).await;
    for (a, side) in [(id, sidecar(json!({"archived": true}))), (plain, sidecar(json!({})))] {
        let sha = sha_hex(&t.pool, a).await;
        check(&app, &key, &sha, json!({"sidecar": side})).await;
        takeout::apply(&t.pool, &storage, a).await.unwrap();
    }
    let flags: Vec<(Uuid, bool, bool)> = sqlx::query_as(
        "SELECT id, archived, archived_at IS NOT NULL FROM assets WHERE owner_id = $1 ORDER BY original_name",
    )
    .bind(owner)
    .fetch_all(&t.pool)
    .await
    .unwrap();
    let by = |i: Uuid| flags.iter().find(|f| f.0 == i).map(|f| (f.1, f.2)).unwrap();
    assert_eq!(by(id), (true, true));
    assert_eq!(by(raw), (true, true), "the RAW behind it too");
    assert_eq!(by(plain), (false, false), "not archived unless the sidecar says so");
    t.teardown().await;
}

#[tokio::test]
async fn a_photo_in_a_year_folder_and_two_albums_ends_up_in_both_albums() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (owner, key) = user(&t.pool, "a@example.com").await;
    let id = photo(&t.pool, owner, "one.jpg", Some("2019-07-03T08:15:00Z")).await;
    let sha = sha_hex(&t.pool, id).await;
    // Takeout writes the same photo once per place: the year folder, then each album. The calls may arrive in any order and
    // at the same time; none may replace another's albums.
    let calls = [
        json!({"sidecar": sidecar(json!({}))}),
        json!({"album": "Trip"}),
        json!({"sidecar": sidecar(json!({})), "album": "Beach"}),
    ];
    let mut tasks = Vec::new();
    for c in calls {
        let (app, key, sha) = (app.clone(), key.clone(), sha.clone());
        tasks.push(tokio::spawn(async move { check(&app, &key, &sha, c).await }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    takeout::apply(&t.pool, &storage, id).await.unwrap();
    // A job per revision may still be queued for a revision that was merged before it ran: applying again is a no-op.
    takeout::apply(&t.pool, &storage, id).await.unwrap();
    assert_eq!(
        albums(&t.pool, owner).await,
        vec![("Beach".to_string(), 1), ("Trip".to_string(), 1)]
    );
    assert_eq!(row(&t.pool, id).await.3.as_deref(), Some("Dinner by the river"));
    t.teardown().await;
}

#[tokio::test]
async fn the_plan_endpoint_pairs_names_and_refuses_abuse() {
    let Some(t) = TestDb::new().await else { return };
    let (_, key) = user(&t.pool, "a@example.com").await;
    let app = crate::testutil::app(&t.pool);
    let (st, v) = send(
        &app,
        "POST",
        "/assets/takeout/plan",
        &key,
        Some(json!({"paths": [
            "Takeout/Google Photos/Trip/IMG_1.jpg",
            "Takeout/Google Photos/Trip/IMG_1.jpg.supplemental-metadata.json",
            "Takeout/Google Photos/Trip/IMG_1-edited.jpg",
            "Takeout/Google Photos/Photos from 2019/IMG_2.jpg",
            "Takeout/Google Photos/Trip/clip.mp4"
        ]})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let photos = v["photos"].as_array().unwrap();
    assert_eq!(photos.len(), 3);
    assert_eq!(
        photos[0]["sidecar"],
        "Takeout/Google Photos/Trip/IMG_1.jpg.supplemental-metadata.json"
    );
    assert_eq!(
        (photos[0]["album"].clone(), photos[1]["edited"].clone()),
        (json!("Trip"), json!(true))
    );
    assert!(photos[2]["album"].is_null() && photos[2]["sidecar"].is_null());
    assert_eq!(v["videos"].as_array().unwrap().len(), 1);
    assert_eq!(v["albums"]["Trip"], 2);

    let many: Vec<String> = (0..100_001).map(|i| format!("a/{i}.jpg")).collect();
    let (st, _) = send(&app, "POST", "/assets/takeout/plan", &key, Some(json!({"paths": many}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send(
        &app,
        "POST",
        "/assets/takeout/plan",
        &key,
        Some(json!({"paths": ["x".repeat(2000)]})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    t.teardown().await;
}

#[tokio::test]
async fn an_import_scoped_key_can_send_takeout_metadata_with_its_check() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (owner, _) = user(&t.pool, "a@example.com").await;
    let id = photo(&t.pool, owner, "one.jpg", None).await;
    let sha = sha_hex(&t.pool, id).await;
    let key = format!("cyk_{}", Uuid::new_v4().simple());
    sqlx::query("INSERT INTO api_keys (user_id, name, key_hash, scope) VALUES ($1, 'cli', $2, 'import')")
        .bind(owner)
        .bind(cyanotype_service::auth::sha256(key.as_bytes()))
        .execute(&t.pool)
        .await
        .unwrap();
    check(&app, &key, &sha, json!({"sidecar": sidecar(json!({}))})).await;
    assert_eq!(jobs_of(&t.pool, id).await.len(), 1);
    // The plan is not part of the import path: the command line plans offline, the browser has a session.
    let (st, _) = send(&app, "POST", "/assets/takeout/plan", &key, Some(json!({"paths": []}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    t.teardown().await;
}
