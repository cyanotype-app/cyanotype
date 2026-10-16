//! The map: clusters, the viewport list (`bbox`), the antimeridian, owner/trash/hidden-RAW filtering and places on a
//! photo, against a real Postgres (skipped without `DATABASE_URL`, like the other DB tests).
use axum::http::StatusCode;
use cyanotype_core::{
    pairing::{Paired, pair_asset},
    places,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::testutil::{TestDb, app, ids, send, user};

/// A live photo at (`lat`, `lon`) taken at `taken`; `thumb` gives it a thumbhash (so it looks processed).
async fn at(pool: &PgPool, owner: Uuid, name: &str, lat: f64, lon: f64, taken: &str, thumb: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, taken_at, latitude, longitude, thumbhash)
         VALUES ($1, sha256($2::bytea), $3, $2, 'image/jpeg', 10, $4::timestamptz, $5, $6, CASE WHEN $7 THEN '\\x0102030405'::bytea END)
         RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(format!("originals/{owner}/{name}"))
    .bind(taken)
    .bind(lat)
    .bind(lon)
    .bind(thumb)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn clusters(app: &axum::Router, key: &str, query: &str) -> Value {
    let (st, v) = send(app, "GET", &format!("/assets/map{query}"), key, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    v
}

fn counts(v: &Value) -> Vec<i64> {
    let mut c: Vec<i64> = v["clusters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["count"].as_i64().unwrap())
        .collect();
    c.sort_unstable();
    c
}

fn names(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["original_name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn clusters_split_as_the_zoom_grows_and_single_photos_carry_a_thumbnail() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    // Three photos within a few hundred metres in Paris (the newest has a thumbnail), one in Lyon, one in Tokyo.
    at(&t.pool, uid, "p1.jpg", 48.8566, 2.3522, "2024-01-01T10:00:00Z", false).await;
    at(&t.pool, uid, "p2.jpg", 48.8570, 2.3530, "2024-01-02T10:00:00Z", true).await;
    at(&t.pool, uid, "p3.jpg", 48.8560, 2.3515, "2024-01-03T10:00:00Z", false).await;
    let lyon = at(&t.pool, uid, "lyon.jpg", 45.764, 4.8357, "2024-02-01T10:00:00Z", false).await;
    let tokyo_id = at(
        &t.pool,
        uid,
        "tokyo.jpg",
        35.6762,
        139.6503,
        "2024-03-01T10:00:00Z",
        true,
    )
    .await;
    // Photos without GPS are not on the map.
    sqlx::query(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
         VALUES ($1, sha256('n'::bytea), 'k-n', 'n.jpg', 'image/jpeg', 1)",
    )
    .bind(uid)
    .execute(&t.pool)
    .await
    .unwrap();

    // The world: Europe (4 photos in one 45-degree cell... Paris and Lyon share it) and Tokyo.
    let w = clusters(&app, &key, "?bbox=-180,-90,180,90&zoom=0").await;
    assert_eq!(counts(&w), [1, 4]);
    assert_eq!((w["total"].as_i64(), w["truncated"].as_bool()), (Some(5), Some(false)));
    let europe = w["clusters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["count"] == 4)
        .unwrap();
    // A cluster carries no thumbnail (it is drawn as a count bubble), and its bounds cover all of its photos.
    assert!(
        europe["thumb_url"].is_null() && europe["thumbhash"].is_null(),
        "{europe}"
    );
    let b = europe["bounds"].as_array().unwrap();
    assert!((b[0].as_f64().unwrap() - 2.3515).abs() < 1e-9 && (b[2].as_f64().unwrap() - 4.8357).abs() < 1e-9);
    assert!((b[1].as_f64().unwrap() - 45.764).abs() < 1e-9 && (b[3].as_f64().unwrap() - 48.857).abs() < 1e-9);
    // A single processed photo has no bounds, but a thumbnail to draw; one still being processed has neither.
    let tokyo = w["clusters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["count"] == 1)
        .unwrap();
    assert!(tokyo["bounds"].is_null());
    assert!(
        tokyo["thumb_url"]
            .as_str()
            .unwrap()
            .contains(&format!("derived/{uid}/{tokyo_id}/")),
        "{tokyo}"
    );
    assert!(tokyo["thumbhash"].is_string());

    // Zoom 6 (cells under a degree): Paris stays one cluster of 3, Lyon is its own photo.
    let z = clusters(&app, &key, "?bbox=-5,40,10,52&zoom=6.4").await;
    assert_eq!(counts(&z), [1, 3]);
    let lone = z["clusters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["count"] == 1)
        .unwrap();
    assert_eq!(lone["id"].as_str().unwrap(), lyon.to_string());
    assert!(lone["thumb_url"].is_null() && lone["thumbhash"].is_null());
    // Zoom 22: even photos 100 m apart are separate markers.
    let deep = clusters(&app, &key, "?bbox=2.35,48.85,2.36,48.86&zoom=22").await;
    assert_eq!(counts(&deep), [1, 1, 1]);
    // The box limits what is returned and counted.
    let tiny = clusters(&app, &key, "?bbox=139,35,140,36&zoom=3").await;
    assert_eq!((counts(&tiny), tiny["total"].as_i64()), (vec![1], Some(1)));
    let none = clusters(&app, &key, "?bbox=-60,-60,-50,-50&zoom=3").await;
    assert_eq!((counts(&none), none["total"].as_i64()), (vec![], Some(0)));
    // Without a box: everything.
    assert_eq!(clusters(&app, &key, "").await["total"], 5);
    t.teardown().await;
}

#[tokio::test]
async fn the_map_skips_trash_hidden_raws_and_other_owners() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (other, other_key) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    at(&t.pool, uid, "keep.jpg", 10.0, 10.0, "2024-01-01T00:00:00Z", false).await;
    let gone = at(&t.pool, uid, "gone.jpg", 10.0, 10.0, "2024-01-02T00:00:00Z", false).await;
    at(&t.pool, other, "theirs.jpg", 10.0, 10.0, "2024-01-03T00:00:00Z", false).await;
    // A RAW + JPEG pair shows once: the RAW is hidden behind its JPEG.
    at(&t.pool, uid, "IMG_1.JPG", 10.0, 10.0, "2024-01-04T00:00:00Z", false).await;
    let raw = at(&t.pool, uid, "IMG_1.CR3", 10.0, 10.0, "2024-01-04T00:00:00Z", false).await;
    assert!(matches!(pair_asset(&t.pool, raw).await.unwrap(), Paired::Linked { .. }));
    let (st, _) = send(&app, "POST", "/assets/trash", &key, Some(ids(&[gone]))).await;
    assert_eq!(st, StatusCode::OK);

    let v = clusters(&app, &key, "?bbox=-180,-90,180,90&zoom=18").await;
    assert_eq!(v["total"], 2, "{v}");
    let list = send(&app, "GET", "/assets?bbox=-180,-90,180,90", &key, None).await.1;
    assert_eq!(names(&list).len(), 2);
    assert_eq!(clusters(&app, &other_key, "").await["total"], 1);
    // Restoring brings it back; favorites-only narrows.
    send(&app, "POST", "/assets/restore", &key, Some(ids(&[gone]))).await;
    assert_eq!(clusters(&app, &key, "").await["total"], 3);
    assert_eq!(clusters(&app, &key, "?favorite=true").await["total"], 0);
    // Needs a login; a malformed box is a 400, not a 500.
    assert_eq!(
        send(&app, "GET", "/assets/map", "cyk_nope", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    for bad in ["1,2,3", "a,b,c,d", "0,50,10,40", "0,0,1,NaN"] {
        assert_eq!(
            send(&app, "GET", &format!("/assets/map?bbox={bad}"), &key, None)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            send(&app, "GET", &format!("/assets?bbox={bad}"), &key, None).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    t.teardown().await;
}

#[tokio::test]
async fn a_box_across_the_antimeridian_finds_both_sides() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    at(&t.pool, uid, "east.jpg", -16.8, 179.6, "2024-01-01T00:00:00Z", false).await; // Fiji, east side
    at(&t.pool, uid, "west.jpg", -16.5, -179.4, "2024-01-02T00:00:00Z", false).await; // Taveuni, west side
    at(&t.pool, uid, "far.jpg", -16.8, 0.0, "2024-01-03T00:00:00Z", false).await;
    at(&t.pool, uid, "near.jpg", -16.8, 170.0, "2024-01-04T00:00:00Z", false).await;

    // 170..-170 in the normal spelling, and 170..190 as a map scrolled past the date line sends it.
    for bbox in ["170,-20,-170,-10", "170,-20,190,-10", "-190,-20,-170,-10"] {
        let v = clusters(&app, &key, &format!("?bbox={bbox}&zoom=4")).await;
        assert_eq!(v["total"], 3, "{bbox}: {v}"); // east, west and near (170.0 is on the edge); not far
    }
    let (_, list) = send(&app, "GET", "/assets?bbox=175,-20,-175,-10", &key, None).await;
    let mut got = names(&list);
    got.sort();
    assert_eq!(got, ["east.jpg", "west.jpg"]);
    // Not crossing: only the east side.
    let (_, list) = send(&app, "GET", "/assets?bbox=175,-20,180,-10", &key, None).await;
    assert_eq!(names(&list), ["east.jpg"]);
    // One cluster per side at world zoom: the grid is anchored at +-180, so a cluster never straddles the line.
    let w = clusters(&app, &key, "?bbox=170,-20,190,-10&zoom=0").await;
    assert_eq!(counts(&w), [1, 2]);
    t.teardown().await;
}

#[tokio::test]
async fn the_viewport_list_pages_through_a_box_by_keyset() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    // 11 photos in the box (some sharing a capture time, as in a burst), 4 outside it, one without GPS.
    for i in 0..11 {
        let taken = if i < 5 {
            "2024-05-05T10:00:00Z".to_string()
        } else {
            format!("2024-05-{:02}T10:00:00Z", 10 + i)
        };
        at(
            &t.pool,
            uid,
            &format!("in{i:02}.jpg"),
            40.0 + f64::from(i) * 0.01,
            -74.0,
            &taken,
            false,
        )
        .await;
    }
    for i in 0..4 {
        at(
            &t.pool,
            uid,
            &format!("out{i}.jpg"),
            41.5,
            -74.0 + f64::from(i),
            "2024-06-01T00:00:00Z",
            false,
        )
        .await;
    }
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let path = match &cursor {
            Some(c) => format!("/assets?bbox=-75,39.9,-73,40.5&limit=4&cursor={c}"),
            None => "/assets?bbox=-75,39.9,-73,40.5&limit=4".to_string(),
        };
        let (st, page) = send(&app, "GET", &path, &key, None).await;
        assert_eq!(st, StatusCode::OK);
        seen.extend(names(&page));
        pages += 1;
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
        assert!(pages < 10);
    }
    assert_eq!(pages, 3);
    assert_eq!(seen.len(), 11, "every photo in the box once: {seen:?}");
    assert!(seen.iter().all(|n| n.starts_with("in")));
    let unique: std::collections::HashSet<_> = seen.iter().collect();
    assert_eq!(unique.len(), 11);
    // Newest first, like the library.
    let (_, all) = send(&app, "GET", "/assets?bbox=-75,39.9,-73,40.5&limit=500", &key, None).await;
    assert_eq!(names(&all), seen);
    assert_eq!(seen[0], "in10.jpg");
    t.teardown().await;
}

#[tokio::test]
async fn a_photo_reports_its_place_once_geocoded() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    sqlx::query(
        "INSERT INTO places (name, region, country, country_code, latitude, longitude, population) VALUES
         ('Paris', 'Ile-de-France', 'France', 'FR', 48.853, 2.349, 2138551)",
    )
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO place_dataset (version, places) VALUES ('test', 1)")
        .execute(&t.pool)
        .await
        .unwrap();
    let a = at(&t.pool, uid, "a.jpg", 48.86, 2.35, "2024-01-01T00:00:00Z", true).await;
    let sea = at(&t.pool, uid, "sea.jpg", 0.0, -30.0, "2024-01-02T00:00:00Z", true).await;
    let before = send(&app, "GET", &format!("/assets/{a}"), &key, None).await.1;
    assert!(before["place"].is_null());
    assert_eq!(
        places::geocode_asset(&t.pool, a).await.unwrap(),
        places::Geocoded::Placed
    );
    assert_eq!(
        places::geocode_asset(&t.pool, sea).await.unwrap(),
        places::Geocoded::NoPlace
    );
    let after = send(&app, "GET", &format!("/assets/{a}"), &key, None).await.1;
    assert_eq!(
        after["place"],
        json!({"city": "Paris", "region": "Ile-de-France", "country": "France", "country_code": "FR"})
    );
    assert!(send(&app, "GET", &format!("/assets/{sea}"), &key, None).await.1["place"].is_null());
    t.teardown().await;
}
