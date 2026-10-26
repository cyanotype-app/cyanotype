//! Trips ("Recent places"): home inference, runs of away days, titles by place level, GPS-less photos inside a trip,
//! exclusions (hidden, dismissed, stacks), the cover, ongoing trips, the switch, owner isolation and the grid filter
//! (`GET /assets?from&to&memories`), against a real Postgres (skipped without `DATABASE_URL`).
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::testutil::{TestDb, app, send, user};

/// (city, region, country, country code, latitude, longitude)
type Spot = (&'static str, &'static str, &'static str, &'static str, f64, f64);

const LISBON: Spot = ("Lisbon", "Lisbon", "Portugal", "PT", 38.72, -9.14);
const PARIS: Spot = ("Paris", "Ile-de-France", "France", "FR", 48.85, 2.35);
const ROME: Spot = ("Rome", "Lazio", "Italy", "IT", 41.90, 12.50);
const FLORENCE: Spot = ("Florence", "Tuscany", "Italy", "IT", 43.77, 11.25);
const VENICE: Spot = ("Venice", "Veneto", "Italy", "IT", 45.44, 12.33);
const MADRID: Spot = ("Madrid", "Madrid", "Spain", "ES", 40.42, -3.70);
const BERLIN: Spot = ("Berlin", "Berlin", "Germany", "DE", 52.52, 13.40);
const LONDON: Spot = ("London", "England", "United Kingdom", "GB", 51.51, -0.13);

/// A processed photo taken at `taken` (UTC, no zone recorded) at `spot` (None: no GPS).
async fn add(pool: &PgPool, owner: Uuid, name: &str, taken: &str, spot: Option<Spot>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, taken_at, thumbhash,
                             latitude, longitude, place_city, place_region, place_country, place_country_code)
         VALUES ($1, sha256($2::bytea), $3, $2, 'image/jpeg', 10, $4::timestamptz, '\\x0102030405'::bytea, $5, $6, $7, $8, $9, $10)
         RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(format!("originals/{owner}/{name}"))
    .bind(taken)
    .bind(spot.map(|s| s.4))
    .bind(spot.map(|s| s.5))
    .bind(spot.map(|s| s.0))
    .bind(spot.map(|s| s.1))
    .bind(spot.map(|s| s.2))
    .bind(spot.map(|s| s.3))
    .fetch_one(pool)
    .await
    .unwrap()
}

/// `n` photos at `spot` on `day`, named `<tag>-<day>-<i>`; returns their ids.
async fn day(pool: &PgPool, owner: Uuid, tag: &str, day: &str, spot: Option<Spot>, n: usize) -> Vec<Uuid> {
    let mut ids = Vec::new();
    for i in 0..n {
        ids.push(
            add(
                pool,
                owner,
                &format!("{tag}-{day}-{i}"),
                &format!("{day}T12:{i:02}:00Z"),
                spot,
            )
            .await,
        );
    }
    ids
}

async fn set(pool: &PgPool, id: Uuid, column: &str) {
    sqlx::query(&format!("UPDATE assets SET {column} WHERE id = $1"))
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

/// The home city for a user: a photo-day at six dates around the trips.
async fn home(pool: &PgPool, owner: Uuid) {
    for d in [
        "2026-09-01",
        "2026-09-02",
        "2026-09-20",
        "2026-09-21",
        "2026-10-03",
        "2026-10-20",
    ] {
        day(pool, owner, "home", d, Some(LISBON), 2).await;
    }
}

async fn trips(app: &axum::Router, key: &str, query: &str) -> Value {
    let (st, v) = send(app, "GET", &format!("/assets/trips?date=2026-10-26{query}"), key, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    v
}

fn titles(v: &Value) -> Vec<String> {
    v["trips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn trips_are_found_titled_counted_and_covered() {
    let Some(t) = TestDb::new().await else { return };
    let (alpha, ka) = user(&t.pool, "alpha@example.com").await;
    let (bravo, kb) = user(&t.pool, "bravo@example.com").await;
    let app = app(&t.pool);
    home(&t.pool, alpha).await;

    // Paris, 5 to 8 September: one city. Nine singles, a stacked pair, a GPS-less shot inside, plus ones that do not count.
    let mut paris = Vec::new();
    for d in ["2026-09-05", "2026-09-06", "2026-09-07"] {
        paris.extend(day(&t.pool, alpha, "paris", d, Some(PARIS), 3).await);
    }
    set(&t.pool, paris[4], "rating = 5").await; // the best-rated photo is the cover
    let a = add(&t.pool, alpha, "stack-a", "2026-09-08T10:00:00Z", Some(PARIS)).await;
    let b = add(&t.pool, alpha, "stack-b", "2026-09-08T10:00:05Z", Some(PARIS)).await;
    let burst = Uuid::new_v4();
    sqlx::query("UPDATE assets SET burst_id = $3 WHERE id = ANY($1) AND owner_id = $2")
        .bind([a, b])
        .bind(alpha)
        .bind(burst)
        .execute(&t.pool)
        .await
        .unwrap();
    let no_gps = add(&t.pool, alpha, "paris-nogps", "2026-09-06T18:00:00Z", None).await;
    let hidden = add(&t.pool, alpha, "paris-hidden", "2026-09-06T19:00:00Z", Some(PARIS)).await;
    set(&t.pool, hidden, "hidden = true").await;
    let dismissed = add(&t.pool, alpha, "paris-dismissed", "2026-09-05T20:00:00Z", Some(PARIS)).await;
    set(&t.pool, dismissed, "memory_dismissed = true, favorite = true").await;
    // A GPS-less photo days after the trip is not in it.
    add(&t.pool, alpha, "after-nogps", "2026-09-12T12:00:00Z", None).await;

    // Italy, 10 to 14 October with an empty day between: three cities, no one city (or region) names it.
    let rome = day(&t.pool, alpha, "rome", "2026-10-10", Some(ROME), 3).await;
    day(&t.pool, alpha, "rome", "2026-10-11", Some(ROME), 2).await;
    let florence = day(&t.pool, alpha, "florence", "2026-10-12", Some(FLORENCE), 3).await;
    day(&t.pool, alpha, "venice", "2026-10-14", Some(VENICE), 3).await;
    set(&t.pool, florence[1], "favorite = true").await;
    set(&t.pool, rome[0], "memory_dismissed = true").await;

    // Madrid is still going on: yesterday and today.
    day(&t.pool, alpha, "madrid", "2026-10-25", Some(MADRID), 5).await;
    day(&t.pool, alpha, "madrid", "2026-10-26", Some(MADRID), 4).await;
    // Three photos in Berlin are not a trip.
    day(&t.pool, alpha, "berlin", "2026-09-25", Some(BERLIN), 3).await;

    // Another user's London trip is theirs alone.
    home(&t.pool, bravo).await;
    day(&t.pool, bravo, "london", "2026-10-06", Some(LONDON), 10).await;

    let v = trips(&app, &ka, "").await;
    assert_eq!(v["enabled"], true);
    assert_eq!(
        titles(&v),
        [
            "Madrid · October 2026",
            "Italy · October 2026",
            "Paris · September 2026"
        ]
    );
    let [madrid_t, italy_t, paris_t] = &v["trips"].as_array().unwrap()[..] else {
        panic!("{v}")
    };

    assert_eq!(
        (madrid_t["from"].as_str(), madrid_t["to"].as_str()),
        (Some("2026-10-25"), Some("2026-10-26"))
    );
    assert_eq!(madrid_t["ongoing"], true);
    assert_eq!(madrid_t["photos"], 9);
    assert_eq!(madrid_t["place"]["level"], "city");

    assert_eq!(italy_t["id"], "2026-10-10_2026-10-14");
    assert_eq!(italy_t["ongoing"], false);
    assert_eq!(italy_t["place"]["level"], "country");
    assert_eq!(italy_t["place"]["name"], "Italy");
    assert_eq!(italy_t["place"]["country_code"], "IT");
    // 3 + 2 + 3 + 3 minus the dismissed one.
    assert_eq!(italy_t["photos"], 10);
    // The favorite is the cover (the dismissed Rome photo is not a candidate).
    assert_eq!(italy_t["cover_id"], json!(florence[1]));
    assert_eq!(italy_t["cover"]["id"], json!(florence[1]));
    assert!(italy_t["cover"]["thumb_url"].is_string() || italy_t["cover"]["thumbhash"].is_string());

    assert_eq!(
        (paris_t["from"].as_str(), paris_t["to"].as_str()),
        (Some("2026-09-05"), Some("2026-09-08"))
    );
    assert_eq!(paris_t["date_label"], "September 2026");
    assert_eq!(paris_t["place"]["level"], "city");
    assert_eq!(paris_t["place"]["region"], "Ile-de-France");
    // 9 singles + the stack once + the GPS-less photo inside; not the hidden, the dismissed or the later GPS-less ones.
    assert_eq!(paris_t["photos"], 11);
    // No favorite among the candidates (the dismissed one is the only favorite): the best rating.
    assert_eq!(paris_t["cover_id"], json!(paris[4]));

    // The grid: the same photos through the library's own list.
    let (st, grid) = send(
        &app,
        "GET",
        "/assets?from=2026-09-05&to=2026-09-08&memories=true&limit=100",
        &ka,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{grid}");
    let items = grid["items"].as_array().unwrap();
    assert_eq!(items.len(), 11, "{grid}");
    let listed: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert!(listed.contains(&no_gps.to_string().as_str()));
    for out in [hidden, dismissed] {
        assert!(!listed.contains(&out.to_string().as_str()));
    }
    // Without `memories` the dismissed photo is still the library's; the range alone.
    let (_, all) = send(
        &app,
        "GET",
        "/assets?from=2026-09-05&to=2026-09-08&limit=100",
        &ka,
        None,
    )
    .await;
    assert_eq!(all["items"].as_array().unwrap().len(), 12);
    // Expanded stacks list both shots.
    let (_, exp) = send(
        &app,
        "GET",
        "/assets?from=2026-09-05&to=2026-09-08&memories=true&stacks=expanded&limit=100",
        &ka,
        None,
    )
    .await;
    assert_eq!(exp["items"].as_array().unwrap().len(), 12);

    // Bad dates are refused.
    let (st, _) = send(&app, "GET", "/assets?from=nope", &ka, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send(&app, "GET", "/assets/trips?before=2026-13-01", &ka, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Paging: one at a time, newest first, `before` continues.
    let p1 = trips(&app, &ka, "&limit=1").await;
    assert_eq!(titles(&p1), ["Madrid · October 2026"]);
    assert_eq!(p1["next_before"], "2026-10-25");
    let p2 = trips(&app, &ka, "&limit=1&before=2026-10-25").await;
    assert_eq!(titles(&p2), ["Italy · October 2026"]);
    let p3 = trips(&app, &ka, "&limit=5&before=2026-10-10").await;
    assert_eq!(titles(&p3), ["Paris · September 2026"]);
    assert!(p3["next_before"].is_null());

    // Owner isolation.
    let vb = trips(&app, &kb, "").await;
    assert_eq!(titles(&vb), ["London · October 2026"]);

    // The switch.
    let (st, _) = send(
        &app,
        "PUT",
        "/assets/prefs",
        &ka,
        Some(json!({"memories_enabled": false})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let off = trips(&app, &ka, "").await;
    assert_eq!(off["enabled"], false);
    assert!(off["trips"].as_array().unwrap().is_empty());

    t.teardown().await;
}

#[tokio::test]
async fn the_local_day_follows_the_recorded_zone_and_no_home_means_no_trips() {
    let Some(t) = TestDb::new().await else { return };
    let (alpha, ka) = user(&t.pool, "alpha@example.com").await;
    let app = app(&t.pool);
    // Without a home area (too few photo-days) nothing is a trip, however many photos there are.
    day(&t.pool, alpha, "tokyo", "2026-10-01", Some(LONDON), 12).await;
    assert!(trips(&app, &ka, "").await["trips"].as_array().unwrap().is_empty());

    home(&t.pool, alpha).await;
    // Ten photos at 23:30 UTC on the 9th that the camera clock (UTC+2) read as the 10th.
    let late = day(&t.pool, alpha, "late", "2026-10-09", Some(ROME), 10).await;
    for id in &late {
        sqlx::query(
            "UPDATE assets SET taken_at = '2026-10-09T23:30:00Z', exif = '{\"OffsetTimeOriginal\": \"+02:00\"}'::jsonb WHERE id = $1",
        )
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    }
    let v = trips(&app, &ka, "").await;
    assert_eq!(v["trips"][0]["from"], "2026-10-10");
    assert_eq!(v["trips"][0]["to"], "2026-10-10");
    let (_, grid) = send(
        &app,
        "GET",
        "/assets?from=2026-10-10&to=2026-10-10&limit=100",
        &ka,
        None,
    )
    .await;
    assert_eq!(grid["items"].as_array().unwrap().len(), 10);
    let (_, none) = send(
        &app,
        "GET",
        "/assets?from=2026-10-09&to=2026-10-09&limit=100",
        &ka,
        None,
    )
    .await;
    assert!(none["items"].as_array().unwrap().is_empty());

    t.teardown().await;
}
