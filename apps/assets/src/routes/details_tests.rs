//! `GET /assets/{id}/details` over a real Postgres: what it derives from the columns and the stored EXIF dump, that it
//! hides what is not there, and that it answers for the owner's live photos only (like `GET /assets/{id}`).
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::testutil::{TestDb, app, asset, send, user};

async fn details(app: &axum::Router, key: &str, id: Uuid) -> (StatusCode, Value) {
    send(app, "GET", &format!("/assets/{id}/details"), key, None).await
}

fn iphone_dump() -> Value {
    json!({
        "Make": "Apple", "Model": "iPhone 15",
        "DateTimeOriginal": "2023:07:04 13:05:09", "OffsetTimeOriginal": "+02:00",
        "FNumber": "1.78", "ExposureTime": "1/120", "PhotographicSensitivity": "100",
        "FocalLength": "6.765", "FocalLengthIn35mmFilm": "24", "ExposureBiasValue": "0.3333333333333333",
        "Flash": "not fired", "WhiteBalance": "auto white balance", "MeteringMode": "pattern",
        "ExposureProgram": "normal program", "ColorSpace": "uncalibrated", "Software": "17.4.1",
        "GPSAltitude": "37.8", "GPSAltitudeRef": "above sea level",
        "GPSImgDirection": "132.4", "GPSImgDirectionRef": "true direction",
    })
}

/// A fully described HEIC photo taken in Lisbon, 11:05:09 UTC = 13:05:09 at +02:00.
async fn lisbon(pool: &PgPool, owner: Uuid) -> Uuid {
    let id = asset(pool, owner, "IMG_0001.HEIC").await;
    sqlx::query(
        "UPDATE assets SET content_type = 'image/heic', size_bytes = 2500000, width = 4032, height = 3024,
             taken_at = '2023-07-04T11:05:09Z', latitude = 38.7223, longitude = -9.1393,
             camera_make = 'Apple', camera_model = 'iPhone 15', lens_model = 'iPhone 15 back camera 6.765mm f/1.78',
             place_city = 'Lisbon', place_region = 'Lisbon', place_country = 'Portugal', place_country_code = 'PT',
             exif = $2
         WHERE id = $1",
    )
    .bind(id)
    .bind(iphone_dump())
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_fully_described_photo() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = lisbon(&t.pool, alice).await;
    let album: (Uuid,) = sqlx::query_as("INSERT INTO albums (owner_id, title) VALUES ($1, 'Portugal') RETURNING id")
        .bind(alice)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO album_assets (album_id, asset_id) VALUES ($1, $2)")
        .bind(album.0)
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();

    let (st, d) = details(&app, &key, id).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(d["id"], json!(id));

    assert_eq!(d["date"]["local"], "2023-07-04T13:05:09");
    assert_eq!(d["date"]["offset"], "+02:00");
    assert_eq!(d["date"]["zone_known"], true);
    assert_eq!(d["date"]["weekday"], "Tuesday");
    assert_eq!(d["date"]["basis"], "taken");
    assert_eq!(d["date"]["timeline_day"], "2023-07-04");

    assert_eq!(d["location"]["place"]["city"], "Lisbon");
    assert_eq!(d["location"]["latitude"], json!(38.7223));
    assert_eq!(d["location"]["altitude"], "38 m above sea level");
    assert_eq!(d["location"]["direction"], "132° SE");

    assert_eq!(d["camera"]["label"], "Apple iPhone 15");
    assert_eq!(d["camera"]["lens"], "iPhone 15 back camera 6.765mm f/1.78");

    assert_eq!(d["exposure"]["line"], "ISO 100 · 6.8 mm · ƒ/1.8 · 1/120 s · +0.3 ev");
    assert_eq!(d["exposure"]["focal_length_35mm"], "24 mm");
    assert_eq!(d["exposure"]["flash"], "Did not fire");
    assert_eq!(d["exposure"]["white_balance"], "Auto");
    assert_eq!(d["exposure"]["metering"], "Pattern");
    assert_eq!(d["exposure"]["program"], "Normal program");

    assert_eq!(d["file"]["format"], "HEIC");
    assert_eq!(d["file"]["megapixels"], "12 MP");
    assert_eq!(
        (d["file"]["width"].as_i64(), d["file"]["height"].as_i64()),
        (Some(4032), Some(3024))
    );
    assert_eq!(d["file"]["color_space"], "Uncalibrated");
    assert_eq!(d["file"]["software"], "17.4.1");
    assert_eq!(d["files"].as_array().unwrap().len(), 1);

    assert_eq!(d["albums"], json!([{"id": album.0, "title": "Portugal"}]));
    assert!(d["burst"].is_null());
    t.teardown().await;
}

#[tokio::test]
async fn a_bare_photo_has_no_rows_to_show() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = asset(&t.pool, alice, "scan.png").await;

    let (st, d) = details(&app, &key, id).await;
    assert_eq!(st, StatusCode::OK);
    assert!(d["location"].is_null() && d["camera"].is_null() && d["exposure"].is_null() && d["burst"].is_null());
    assert_eq!(d["albums"], json!([]));
    // No capture time: the date is the upload time and says so; no zone.
    assert_eq!(d["date"]["basis"], "added");
    assert_eq!(d["date"]["zone_known"], false);
    assert!(d["date"]["offset"].is_null());
    assert_eq!(d["file"]["format"], "PNG");
    assert!(d["file"]["megapixels"].is_null() && d["file"]["software"].is_null() && d["file"]["color_space"].is_null());
    t.teardown().await;
}

#[tokio::test]
async fn coordinates_without_a_place_and_a_photo_without_an_offset() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = asset(&t.pool, alice, "boat.jpg").await;
    sqlx::query(
        "UPDATE assets SET taken_at = '2020-01-02T03:04:05Z', latitude = 0.5, longitude = -30.5,
             exif = '{\"DateTimeOriginal\": \"2020:01:02 03:04:05\", \"FNumber\": \"2.8\"}' WHERE id = $1",
    )
    .bind(id)
    .execute(&t.pool)
    .await
    .unwrap();

    let (_, d) = details(&app, &key, id).await;
    assert!(d["location"]["place"].is_null(), "open sea: coordinates, no place");
    assert_eq!(d["location"]["longitude"], json!(-30.5));
    assert!(d["location"]["altitude"].is_null() && d["location"]["direction"].is_null());
    assert_eq!(d["date"]["zone_known"], false);
    assert_eq!(d["date"]["local"], "2020-01-02T03:04:05");
    assert_eq!(d["exposure"]["line"], "ƒ/2.8");
    t.teardown().await;
}

#[tokio::test]
async fn a_raw_jpeg_pair_lists_both_files() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let jpg = asset(&t.pool, alice, "DSC_1.JPG").await;
    let raw = asset(&t.pool, alice, "DSC_1.NEF").await;
    sqlx::query("UPDATE assets SET size_bytes = 6000000 WHERE id = $1")
        .bind(jpg)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE assets SET size_bytes = 30000000, content_type = 'image/x-nikon-nef', primary_id = $2 WHERE id = $1",
    )
    .bind(raw)
    .bind(jpg)
    .execute(&t.pool)
    .await
    .unwrap();

    let (_, d) = details(&app, &key, jpg).await;
    let files = d["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(
        (files[0]["format"].as_str(), files[0]["raw"].as_bool()),
        (Some("JPEG"), Some(false))
    );
    assert_eq!(
        (files[1]["format"].as_str(), files[1]["raw"].as_bool()),
        (Some("NEF"), Some(true))
    );
    assert_eq!(files[1]["id"], json!(raw));
    assert_eq!(files[1]["size_bytes"], json!(30_000_000));
    t.teardown().await;
}

#[tokio::test]
async fn burst_position_counts_visible_members_in_capture_order() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let mut ids = Vec::new();
    for i in 0..4 {
        let id = asset(&t.pool, alice, &format!("b{i}.jpg")).await;
        sqlx::query(
            "UPDATE assets SET taken_at = '2024-05-05T10:00:00Z'::timestamptz + make_interval(secs => $2),
                 camera_make = 'ACME', camera_model = 'X1' WHERE id = $1",
        )
        .bind(id)
        .bind(i as f64)
        .execute(&t.pool)
        .await
        .unwrap();
        ids.push(id);
    }
    for id in &ids {
        sqlx::query("UPDATE assets SET burst_id = $2, burst_version = 1 WHERE id = $1")
            .bind(id)
            .bind(ids[0])
            .execute(&t.pool)
            .await
            .unwrap();
    }
    let (_, d) = details(&app, &key, ids[0]).await;
    assert_eq!(d["burst"], json!({"position": 1, "size": 4}));
    let (_, d) = details(&app, &key, ids[2]).await;
    assert_eq!(d["burst"], json!({"position": 3, "size": 4}));
    // A trashed member leaves the count.
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(ids[3])
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, d) = details(&app, &key, ids[2]).await;
    assert_eq!(d["burst"], json!({"position": 3, "size": 3}));
    t.teardown().await;
}

#[tokio::test]
async fn only_the_owners_live_photos_answer() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, akey) = user(&t.pool, "a@example.com").await;
    let (_, bkey) = user(&t.pool, "b@example.com").await;
    let id = lisbon(&t.pool, alice).await;

    assert_eq!(details(&app, &akey, id).await.0, StatusCode::OK);
    // Someone else's photo is a 404, exactly as unknown ids are: no coordinates, no EXIF, no existence leak.
    let (st, body) = details(&app, &bkey, id).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(!body.to_string().contains("Lisbon") && !body.to_string().contains("38.72"));
    assert_eq!(details(&app, &akey, Uuid::new_v4()).await.0, StatusCode::NOT_FOUND);
    // Signed out.
    assert_eq!(details(&app, "cyk_nope", id).await.0, StatusCode::UNAUTHORIZED);

    // In the trash it is gone from here too (restoring brings it back).
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(details(&app, &akey, id).await.0, StatusCode::NOT_FOUND);
    sqlx::query("UPDATE assets SET deleted_at = NULL WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(details(&app, &akey, id).await.0, StatusCode::OK);
    t.teardown().await;
}

#[tokio::test]
async fn the_details_route_does_not_swallow_the_detail_route() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = lisbon(&t.pool, alice).await;
    let (st, one) = send(&app, "GET", &format!("/assets/{id}"), &key, None).await;
    assert_eq!(st, StatusCode::OK);
    // The detail endpoint is unchanged: it carries no coordinates or EXIF.
    assert!(one.get("location").is_none() && one.get("exposure").is_none());
    t.teardown().await;
}
