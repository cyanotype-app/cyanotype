//! Editing photos (`POST /assets/edits`, `GET /assets/places`) over a real Postgres and a real S3 API (the `S3_*`
//! variables; the tests return early without them): overrides next to the file's own values, revert, the object in the
//! bucket that `reindex` restores from, bulk shifts, owner isolation, bursts and geocoding re-queued, caption search.
use axum::http::StatusCode;
use cyanotype_core::{edits, keys, reindex, storage::Storage};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::search_v2_tests::{ids, photo};
use crate::testutil::{TEST_MODEL, TestDb, app_with_s3, asset, send, user};

async fn post(app: &axum::Router, key: &str, body: Value) -> (StatusCode, Value) {
    send(app, "POST", "/assets/edits", key, Some(body)).await
}

async fn details(app: &axum::Router, key: &str, id: Uuid) -> Value {
    let (st, d) = send(app, "GET", &format!("/assets/{id}/details"), key, None).await;
    assert_eq!(st, StatusCode::OK, "{d}");
    d
}

async fn sha(pool: &PgPool, id: Uuid) -> [u8; 32] {
    let v: Vec<u8> = sqlx::query_scalar("SELECT sha256 FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    v.try_into().unwrap()
}

async fn stored(storage: &Storage, owner: Uuid, pool: &PgPool, id: Uuid) -> edits::Edits {
    let bytes = storage
        .get_bytes(&keys::edits(owner, &sha(pool, id).await))
        .await
        .unwrap();
    edits::parse(&bytes).unwrap()
}

/// A Lisbon photo taken 11:05:09 UTC = 13:05:09 at +02:00, with a place.
async fn lisbon(pool: &PgPool, owner: Uuid, name: &str) -> Uuid {
    let id = asset(pool, owner, name).await;
    sqlx::query(
        "UPDATE assets SET taken_at = '2023-07-04T11:05:09Z', latitude = 38.7223, longitude = -9.1393,
             place_city = 'Lisbon', place_region = 'Lisbon', place_country = 'Portugal', place_country_code = 'PT',
             camera_make = 'Apple', camera_model = 'iPhone 15',
             exif = '{\"DateTimeOriginal\": \"2023:07:04 13:05:09\", \"OffsetTimeOriginal\": \"+02:00\"}'
         WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn an_edit_overrides_next_to_the_files_values_and_reverts() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = lisbon(&t.pool, alice, "edit-one.jpg").await;

    let (st, r) = post(
        &app,
        &key,
        json!({"ids": [id],
               "date": {"local": "2022-01-02T03:04:05", "offset": "-05:00"},
               "location": {"latitude": 48.8566, "longitude": 2.3522},
               "caption": {"text": "  Dinner by the river "}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r, json!({"updated": 1, "unchanged": 0, "missing": 0, "failed": []}));

    // The row: effective values in the columns everything reads, the file's own next to them, the place forgotten and
    // looked up again.
    let (taken, exif, off, lat, lat_exif, lon_exif, place, edited): (String, String, Option<i32>, f64, f64, f64, Option<String>, bool) =
        sqlx::query_as(
            "SELECT taken_at::text, taken_at_exif::text, taken_offset_edit, latitude, latitude_exif, longitude_exif, place_city,
                    date_edited AND location_edited FROM assets WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(taken, "2022-01-02 08:04:05+00");
    assert_eq!(exif, "2023-07-04 11:05:09+00");
    assert_eq!(
        (off, lat, lat_exif, lon_exif, place, edited),
        (Some(-18000), 48.8566, 38.7223, -9.1393, None, true)
    );
    let geocode: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'geocode' AND asset_id = $1 AND state = 'queued'")
            .bind(id)
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert_eq!(geocode, 1, "the new coordinates are geocoded again");

    // The details say what was edited and what the file says.
    let d = details(&app, &key, id).await;
    assert_eq!(d["date"]["local"], "2022-01-02T03:04:05");
    assert_eq!(
        (d["date"]["offset"].as_str(), d["date"]["zone_known"].as_bool()),
        (Some("-05:00"), Some(true))
    );
    assert_eq!(d["date"]["weekday"], "Sunday");
    assert_eq!(d["caption"], "Dinner by the river");
    assert_eq!(d["location"]["latitude"], json!(48.8566));
    assert!(d["location"]["place"].is_null());
    assert_eq!(
        (
            d["edits"]["date_edited"].as_bool(),
            d["edits"]["location_edited"].as_bool()
        ),
        (Some(true), Some(true))
    );
    assert_eq!(d["edits"]["original_date"]["local"], "2023-07-04T13:05:09");
    assert_eq!(d["edits"]["original_date"]["offset"], "+02:00");
    assert_eq!(d["edits"]["original_location"]["latitude"], json!(38.7223));
    // The timeline and the viewer read the edited time; the caption is in the viewer's detail too.
    let (_, one) = send(&app, "GET", &format!("/assets/{id}"), &key, None).await;
    assert_eq!(one["sort_time"], "2022-01-02T08:04:05Z");
    assert_eq!(one["caption"], "Dinner by the river");

    // The bucket's copy, written before the database changed.
    let s = stored(&storage, alice, &t.pool, id).await;
    assert_eq!(s.taken_at.map(|x| x.unix_timestamp()), Some(1_641_110_645));
    assert_eq!(
        (s.taken_offset, s.latitude, s.longitude),
        (Some(-18000), Some(48.8566), Some(2.3522))
    );
    assert_eq!(s.caption.as_deref(), Some("Dinner by the river"));

    // The same request again changes nothing (and writes nothing).
    let updated_at = s.updated_at;
    let (_, again) = post(
        &app,
        &key,
        json!({"ids": [id], "caption": {"text": "Dinner by the river"}}),
    )
    .await;
    assert_eq!(
        (again["updated"].clone(), again["unchanged"].clone()),
        (json!(0), json!(1))
    );
    assert_eq!(stored(&storage, alice, &t.pool, id).await.updated_at, updated_at);

    // Revert one part at a time; the file's values come back, the others stay.
    let (_, r) = post(&app, &key, json!({"ids": [id], "date": {"revert": true}})).await;
    assert_eq!(r["updated"], json!(1));
    let d = details(&app, &key, id).await;
    assert_eq!(
        (d["date"]["local"].as_str(), d["date"]["offset"].as_str()),
        (Some("2023-07-04T13:05:09"), Some("+02:00"))
    );
    assert_eq!(d["edits"]["date_edited"], json!(false));
    assert!(d["edits"]["original_date"].is_null());
    assert_eq!(d["caption"], "Dinner by the river");
    assert_eq!(d["edits"]["location_edited"], json!(true));

    let (_, r) = post(
        &app,
        &key,
        json!({"ids": [id], "location": {"revert": true}, "caption": {"revert": true}}),
    )
    .await;
    assert_eq!(r["updated"], json!(1));
    let (lat, taken, edited, caption): (f64, String, bool, Option<String>) = sqlx::query_as(
        "SELECT latitude, taken_at::text, date_edited OR location_edited, caption FROM assets WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(
        (lat, taken.as_str(), edited, caption),
        (38.7223, "2023-07-04 11:05:09+00", false, None)
    );
    // Reverted: an empty object stays in the bucket, so a restored older database is corrected too.
    let s = stored(&storage, alice, &t.pool, id).await;
    assert!(s.is_empty() && s.updated_at > updated_at);
    t.teardown().await;
}

#[tokio::test]
async fn a_zone_less_edit_reads_as_utc_and_says_the_zone_is_unknown() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = lisbon(&t.pool, alice, "edit-zone.jpg").await;
    let (st, _) = post(
        &app,
        &key,
        json!({"ids": [id], "date": {"local": "2020-02-29T23:59", "offset": null}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let d = details(&app, &key, id).await;
    assert_eq!(d["date"]["local"], "2020-02-29T23:59:00");
    assert_eq!(
        (d["date"]["zone_known"].as_bool(), d["date"]["offset"].is_null()),
        (Some(false), true)
    );
    assert_eq!(d["date"]["timeline_day"], "2020-02-29");
    t.teardown().await;
}

#[tokio::test]
async fn a_bulk_shift_moves_every_photo_keeping_its_zone_and_one_location_goes_to_all() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let a = lisbon(&t.pool, alice, "shift-a.jpg").await;
    let b = asset(&t.pool, alice, "shift-b.jpg").await;
    sqlx::query("UPDATE assets SET taken_at = '2021-12-31T23:30:00Z' WHERE id = $1")
        .bind(b)
        .execute(&t.pool)
        .await
        .unwrap();
    // No capture time at all: the shift starts from the time it is filed under (the upload time).
    let c = asset(&t.pool, alice, "shift-c.jpg").await;
    sqlx::query("UPDATE assets SET file_modified_at = '2020-01-01T00:00:00Z' WHERE id = $1")
        .bind(c)
        .execute(&t.pool)
        .await
        .unwrap();

    // The camera clock was an hour and a half slow and a day behind.
    let secs = 86_400 + 5_400;
    let (st, r) = post(
        &app,
        &key,
        json!({"ids": [a, b, c, Uuid::new_v4()], "date": {"shift_seconds": secs}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r, json!({"updated": 3, "unchanged": 0, "missing": 1, "failed": []}));
    let d = details(&app, &key, a).await;
    assert_eq!(d["date"]["local"], "2023-07-05T14:35:09", "still at +02:00");
    assert_eq!(d["date"]["offset"], "+02:00");
    let d = details(&app, &key, b).await;
    assert_eq!(d["date"]["local"], "2022-01-02T01:00:00");
    assert_eq!(d["date"]["zone_known"], json!(false));
    let d = details(&app, &key, c).await;
    assert_eq!(
        (d["date"]["basis"].as_str(), d["date"]["local"].as_str()),
        (Some("taken"), Some("2020-01-02T01:30:00"))
    );
    assert_eq!(d["edits"]["original_date"]["basis"], "file");

    // Shifting back by the same amount restores the times exactly.
    post(&app, &key, json!({"ids": [a, b], "date": {"shift_seconds": -secs}})).await;
    assert_eq!(details(&app, &key, a).await["date"]["local"], "2023-07-04T13:05:09");
    assert_eq!(details(&app, &key, b).await["date"]["local"], "2021-12-31T23:30:00");

    // One location for all, and one caption.
    let (_, r) = post(
        &app,
        &key,
        json!({"ids": [a, b, c], "location": {"latitude": -33.8688, "longitude": 151.2093}, "caption": {"text": "Sydney"}}),
    )
    .await;
    assert_eq!(r["updated"], json!(3));
    for id in [a, b, c] {
        let d = details(&app, &key, id).await;
        assert_eq!(
            (d["location"]["latitude"].clone(), d["caption"].clone()),
            (json!(-33.8688), json!("Sydney"))
        );
    }
    // (c and b had no location of their own: reverting gives them none again.)
    post(&app, &key, json!({"ids": [b], "location": {"revert": true}})).await;
    assert!(details(&app, &key, b).await["location"].is_null());
    t.teardown().await;
}

#[tokio::test]
async fn only_your_own_live_photos_can_be_edited() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, akey) = user(&t.pool, "a@example.com").await;
    let (bob, bkey) = user(&t.pool, "b@example.com").await;
    let mine = lisbon(&t.pool, alice, "iso-mine.jpg").await;
    let theirs = lisbon(&t.pool, bob, "iso-theirs.jpg").await;
    let trashed = lisbon(&t.pool, alice, "iso-trashed.jpg").await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&t.pool)
        .await
        .unwrap();
    let raw = lisbon(&t.pool, alice, "iso-raw.cr3").await;
    sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
        .bind(raw)
        .bind(mine)
        .execute(&t.pool)
        .await
        .unwrap();

    // Alice names Bob's photo, her trashed one and a hidden RAW: all "missing", and nothing is written anywhere.
    let (st, r) = post(
        &app,
        &akey,
        json!({"ids": [theirs, trashed, raw], "caption": {"text": "mine now"}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(r, json!({"updated": 0, "unchanged": 0, "missing": 3, "failed": []}));
    for (owner, id) in [(bob, theirs), (alice, trashed), (alice, raw)] {
        assert!(
            storage
                .head(&keys::edits(owner, &sha(&t.pool, id).await))
                .await
                .unwrap()
                .is_none()
        );
        let c: Option<String> = sqlx::query_scalar("SELECT caption FROM assets WHERE id = $1")
            .bind(id)
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert!(c.is_none());
    }
    // A scope reaches only the caller's photos too.
    let (_, r) = post(
        &app,
        &akey,
        json!({"scope": "library", "caption": {"text": "all of mine"}}),
    )
    .await;
    assert_eq!(r["updated"], json!(1), "alice has one live visible photo");
    let c: Option<String> = sqlx::query_scalar("SELECT caption FROM assets WHERE id = $1")
        .bind(theirs)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(c.is_none(), "bob's photo was not touched by alice's scope");
    // Bob sees nothing of Alice's edits, and the other endpoints keep their 404s.
    assert_eq!(
        send(&app, "GET", &format!("/assets/{mine}/details"), &bkey, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(details(&app, &akey, mine).await["caption"], "all of mine");
    // Signed out.
    assert_eq!(
        post(&app, "cyk_nope", json!({"ids": [mine], "caption": {"text": "x"}}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    t.teardown().await;
}

#[tokio::test]
async fn bad_requests_are_refused_before_anything_is_written() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = lisbon(&t.pool, alice, "bad.jpg").await;
    for body in [
        json!({"ids": [id]}),
        json!({"ids": [id], "location": {"latitude": 91, "longitude": 0}}),
        json!({"ids": [id], "location": {"latitude": 0, "longitude": 181}}),
        json!({"ids": [id], "date": {"local": "last Tuesday"}}),
        json!({"ids": [id], "date": {"local": "2020-01-01T00:00", "offset": "+25:00"}}),
        json!({"ids": [id], "date": {"local": "1800-01-01T00:00"}}),
        json!({"ids": [id], "caption": {"text": "x".repeat(2001)}}),
        json!({"ids": [id], "caption": {"revert": false}}),
        json!({"ids": (0..1001).map(|_| Uuid::new_v4()).collect::<Vec<_>>(), "caption": {"text": "x"}}),
        json!({"ids": [id], "scope": "library", "caption": {"text": "x"}}),
    ] {
        let (st, r) = post(&app, &key, body.clone()).await;
        assert!(
            st == StatusCode::BAD_REQUEST || st == StatusCode::UNPROCESSABLE_ENTITY,
            "{st} {r} for {}",
            body.to_string().chars().take(100).collect::<String>()
        );
    }
    assert!(
        storage
            .head(&keys::edits(alice, &sha(&t.pool, id).await))
            .await
            .unwrap()
            .is_none()
    );
    // An empty caption is no caption, not an error.
    post(&app, &key, json!({"ids": [id], "caption": {"text": "hello"}})).await;
    let (_, r) = post(&app, &key, json!({"ids": [id], "caption": {"text": "  "}})).await;
    assert_eq!(r["updated"], json!(1));
    assert!(details(&app, &key, id).await["caption"].is_null());
    t.teardown().await;
}

#[tokio::test]
async fn editing_a_time_regroups_the_burst_it_leaves_and_the_one_it_joins() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let mut shots = Vec::new();
    for i in 0..3 {
        let id = asset(&t.pool, alice, &format!("burst-{i}.jpg")).await;
        sqlx::query(
            "UPDATE assets SET taken_at = '2024-05-05T10:00:00Z'::timestamptz + make_interval(secs => $2), camera_make = 'ACME',
                 camera_model = 'X1' WHERE id = $1",
        )
        .bind(id)
        .bind(i as f64)
        .execute(&t.pool)
        .await
        .unwrap();
        shots.push(id);
    }
    sqlx::query("UPDATE assets SET burst_id = $1, burst_version = 1")
        .bind(shots[0])
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM jobs WHERE kind = 'burst'")
        .execute(&t.pool)
        .await
        .unwrap();

    // The middle shot moves a day: it is "to be computed", so are the shots it left, and each place has a job.
    let (_, r) = post(
        &app,
        &key,
        json!({"ids": [shots[1]], "date": {"shift_seconds": 86_400}}),
    )
    .await;
    assert_eq!(r["updated"], json!(1));
    let versions: Vec<Option<i32>> = sqlx::query_scalar("SELECT burst_version FROM assets ORDER BY taken_at")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(versions, vec![None, None, None]);
    let queued: Vec<Uuid> =
        sqlx::query_scalar("SELECT asset_id FROM jobs WHERE kind = 'burst' AND state = 'queued' ORDER BY asset_id")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert!(queued.contains(&shots[1]), "regroup at the new time");
    assert!(
        queued.contains(&shots[0]) || queued.contains(&shots[2]),
        "regroup what it left"
    );
    // A caption alone moves nothing in time and queues nothing.
    sqlx::query("DELETE FROM jobs WHERE kind = 'burst'")
        .execute(&t.pool)
        .await
        .unwrap();
    post(&app, &key, json!({"ids": [shots[0]], "caption": {"text": "first"}})).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'burst'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    t.teardown().await;
}

#[tokio::test]
async fn captions_are_found_by_text_search_in_front_of_the_ranking() {
    let Some(t) = TestDb::new().await else { return };
    let app = crate::testutil::app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let (bob, _) = user(&t.pool, "b@example.com").await;
    let m = TEST_MODEL;
    let cat = photo(&t.pool, alice, "cat.jpg", m, &[("cat", 1.0)]).await;
    let dog = photo(&t.pool, alice, "dog.jpg", m, &[("dog", 1.0)]).await;
    let captioned = photo(&t.pool, alice, "plain.jpg", m, &[("car", 1.0)]).await;
    let theirs = photo(&t.pool, bob, "theirs.jpg", m, &[("car", 1.0)]).await;
    let trashed = photo(&t.pool, alice, "trashed.jpg", m, &[("car", 1.0)]).await;
    for (id, c) in [
        (captioned, "Our wedding cake, Lisbon"),
        (theirs, "Our wedding cake"),
        (trashed, "wedding cake in the trash"),
        (dog, "Rex at the wedding"),
    ] {
        sqlx::query("UPDATE assets SET caption = $2 WHERE id = $1")
            .bind(id)
            .bind(c)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET taken_at = '2020-01-01T00:00:00Z' WHERE id = $1")
        .bind(dog)
        .execute(&t.pool)
        .await
        .unwrap();

    // Every word must be in the caption (case-insensitive, short and common words do not count), the model's matches follow.
    let (st, r) = send(&app, "GET", "/assets/search?q=Wedding%20CAKE", &key, None).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(
        ids(&r)[0],
        captioned,
        "the caption match is first; others' captions and the trash do not match: {r}"
    );
    assert!(!ids(&r).contains(&theirs) && !ids(&r).contains(&trashed));
    assert!(r["items"][0]["score"].is_null() && r["items"][0]["tier"] == "strong");
    assert!(r["strong_count"].as_i64().unwrap() >= 1);

    // One word of two is not a match; a word in the middle of another is.
    let (_, r) = send(&app, "GET", "/assets/search?q=wedding%20cake%20rex", &key, None).await;
    assert!(!ids(&r).contains(&captioned) || ids(&r)[0] != captioned);
    let (_, r) = send(&app, "GET", "/assets/search?q=wedding", &key, None).await;
    assert_eq!(ids(&r)[..2], [captioned, dog].map(|x| x), "newest caption match first");

    // The words are searched for with the model too: a caption match that is also a model match is shown once, in front, and
    // the model's own cut-off is not disturbed by it ("cat" has no caption).
    sqlx::query("UPDATE assets SET caption = 'my cat' WHERE id = $1")
        .bind(cat)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, r) = send(&app, "GET", "/assets/search?q=cat&cutoff=false", &key, None).await;
    assert_eq!(ids(&r).iter().filter(|i| **i == cat).count(), 1);
    assert_eq!(ids(&r)[0], cat);
    // The filters apply to caption matches too.
    let (_, r) = send(&app, "GET", "/assets/search?q=wedding&from=2024-01-01", &key, None).await;
    assert!(!ids(&r).contains(&dog), "{r}");
    // LIKE's own characters in a query are plain characters.
    let (_, r) = send(&app, "GET", "/assets/search?q=%25%25%25", &key, None).await;
    assert!(
        ids(&r).iter().all(|i| ![captioned, dog, cat].contains(i))
            || r["items"].as_array().unwrap().is_empty()
            || r["items"][0]["score"].is_number()
    );
    // Paging goes through the caption matches into the model's results without gaps or repeats.
    let (_, p1) = send(&app, "GET", "/assets/search?q=wedding&limit=1&cutoff=false", &key, None).await;
    let cursor = p1["next_cursor"].as_str().unwrap().to_string();
    let (_, p2) = send(
        &app,
        "GET",
        &format!("/assets/search?q=wedding&limit=1&cutoff=false&cursor={cursor}"),
        &key,
        None,
    )
    .await;
    assert_eq!(ids(&p1)[0], captioned);
    assert_eq!(ids(&p2)[0], dog);
    t.teardown().await;
}

#[tokio::test]
async fn places_lists_your_own_places_with_a_point_to_put_a_photo_on() {
    let Some(t) = TestDb::new().await else { return };
    let app = crate::testutil::app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let (bob, bkey) = user(&t.pool, "b@example.com").await;
    for (owner, name, city, country, lat, lon) in [
        (alice, "p1.jpg", "Lisbon", "Portugal", 38.70, -9.10),
        (alice, "p2.jpg", "Lisbon", "Portugal", 38.74, -9.18),
        (alice, "p3.jpg", "Paris", "France", 48.85, 2.35),
        (alice, "p4.jpg", "San Lisbon", "Nowhere", 1.0, 1.0),
        (bob, "p5.jpg", "Oslo", "Norway", 59.9, 10.7),
    ] {
        let id = asset(&t.pool, owner, name).await;
        sqlx::query(
            "UPDATE assets SET place_city = $2, place_country = $3, latitude = $4, longitude = $5 WHERE id = $1",
        )
        .bind(id)
        .bind(city)
        .bind(country)
        .bind(lat)
        .bind(lon)
        .execute(&t.pool)
        .await
        .unwrap();
    }
    let (st, r) = send(&app, "GET", "/assets/places", &key, None).await;
    assert_eq!(st, StatusCode::OK);
    let cities: Vec<&str> = r
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["city"].as_str().unwrap())
        .collect();
    assert_eq!(
        cities,
        ["Lisbon", "Paris", "San Lisbon"],
        "most photographed first, and only alice's"
    );
    assert_eq!(r[0]["count"], json!(2));
    assert!(
        (r[0]["latitude"].as_f64().unwrap() - 38.72).abs() < 1e-9
            && (r[0]["longitude"].as_f64().unwrap() + 9.14).abs() < 1e-9
    );
    // The start of a word of the city, region or country, case-insensitively.
    let (_, r) = send(&app, "GET", "/assets/places?q=lis", &key, None).await;
    assert_eq!(r.as_array().unwrap().len(), 2, "Lisbon and San Lisbon");
    let (_, r) = send(&app, "GET", "/assets/places?q=fra", &key, None).await;
    assert_eq!(r[0]["city"], "Paris");
    let (_, r) = send(&app, "GET", "/assets/places?q=%25", &key, None).await;
    assert!(r.as_array().unwrap().is_empty(), "a % is not a wildcard");
    let (_, r) = send(&app, "GET", "/assets/places?q=oslo", &key, None).await;
    assert!(r.as_array().unwrap().is_empty(), "bob's place is not alice's");
    assert_eq!(
        send(&app, "GET", "/assets/places?q=oslo", &bkey, None).await.1[0]["city"],
        "Oslo"
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("/assets/places?q={}", "x".repeat(101)),
            &key,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&app, "GET", "/assets/places", "cyk_nope", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    t.teardown().await;
}

#[tokio::test]
async fn reindex_puts_edits_back_on_a_database_restored_from_before_them() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let edited = lisbon(&t.pool, alice, "re-edited.jpg").await;
    let reverted = lisbon(&t.pool, alice, "re-reverted.jpg").await;
    let current = lisbon(&t.pool, alice, "re-current.jpg").await;
    post(&app, &key, json!({"ids": [edited], "date": {"local": "2000-01-01T00:00", "offset": "+01:00"}, "caption": {"text": "kept"}})).await;
    post(
        &app,
        &key,
        json!({"ids": [reverted], "caption": {"text": "will be reverted"}}),
    )
    .await;
    post(
        &app,
        &key,
        json!({"ids": [current], "caption": {"text": "in the dump"}}),
    )
    .await;
    // The database as a dump taken now has it: `reverted` still has its caption; then everything after happens.
    let dump_state: (Option<String>,) = sqlx::query_as("SELECT caption FROM assets WHERE id = $1")
        .bind(reverted)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(dump_state.0.as_deref(), Some("will be reverted"));
    post(&app, &key, json!({"ids": [reverted], "caption": {"revert": true}})).await;
    // Roll the rows back to what the older dump had: `edited` was not edited yet, `reverted` still had its caption (and an
    // older stamp), `current` is as the bucket says.
    sqlx::query(
        "UPDATE assets SET date_edited = false, taken_at = '2023-07-04T11:05:09Z', taken_at_exif = NULL, taken_offset_edit = NULL,
                caption = NULL, edited_at = NULL WHERE id = $1",
    )
    .bind(edited)
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE assets SET caption = 'will be reverted', edited_at = now() - interval '1 hour' WHERE id = $1")
        .bind(reverted)
        .execute(&t.pool)
        .await
        .unwrap();

    let opts = |apply: bool| reindex::Options {
        apply,
        min_age: std::time::Duration::ZERO,
        ..Default::default()
    };
    // A dry run says what it would restore and writes nothing.
    let dry = reindex::run(&t.pool, &storage, &opts(false)).await.unwrap();
    assert_eq!((dry.edits_restored, dry.edits_current), (2, 1), "{}", dry.text());
    let c: Option<String> = sqlx::query_scalar("SELECT caption FROM assets WHERE id = $1")
        .bind(edited)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(c.is_none());

    let done = reindex::run(&t.pool, &storage, &opts(true)).await.unwrap();
    // (The shared test bucket also holds other tests' edits, of users this database does not have: not counted as ours.)
    assert!(
        done.edits_objects >= 3 && done.edits_without_photo + 3 == done.edits_objects,
        "{}",
        done.text()
    );
    assert_eq!((done.edits_restored, done.edits_current, done.edits_bad), (2, 1, 0));
    let d = details(&app, &key, edited).await;
    assert_eq!(d["date"]["local"], "2000-01-01T00:00:00");
    assert_eq!(d["date"]["offset"], "+01:00");
    assert_eq!(d["caption"], "kept");
    assert_eq!(d["edits"]["original_date"]["local"], "2023-07-04T13:05:09");
    assert!(
        details(&app, &key, reverted).await["caption"].is_null(),
        "the empty object corrects the older dump"
    );
    assert_eq!(details(&app, &key, current).await["caption"], "in the dump");

    // Idempotent: nothing left to restore.
    let again = reindex::run(&t.pool, &storage, &opts(true)).await.unwrap();
    assert_eq!((again.edits_restored, again.edits_current), (0, 3), "{}", again.text());
    t.teardown().await;
}

#[tokio::test]
async fn purging_a_photo_deletes_its_edits_from_the_bucket() {
    use crate::purge::Objects;
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    // A photo whose key is the real layout (the purge derives the edits key from it).
    let sha: [u8; 32] = [7; 32];
    let okey = keys::original(alice, &sha, "jpg");
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
         VALUES ($1, $2, $3, 'p.jpg', 'image/jpeg', 1) RETURNING id",
    )
    .bind(alice)
    .bind(&sha[..])
    .bind(&okey)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    post(&app, &key, json!({"ids": [id], "caption": {"text": "private words"}})).await;
    let ekey = keys::edits(alice, &sha);
    assert!(storage.head(&ekey).await.unwrap().is_some());

    storage
        .delete_asset_objects(&okey, &keys::derived_prefix(alice, id))
        .await
        .unwrap();
    assert!(
        storage.head(&ekey).await.unwrap().is_none(),
        "the caption does not outlive the photo"
    );
    assert!(
        storage.list_versions(&ekey).await.unwrap().is_empty(),
        "no old version of it is left either"
    );
    t.teardown().await;
}
