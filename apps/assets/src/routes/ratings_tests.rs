//! Star ratings over a real Postgres: setting (ids, scope, pairs), who owns what, the search filter and facet, the source of
//! a rating, and the hidden-photo rule (the hidden-photo leak test in `visibility_tests` covers the read side).
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::testutil::{TestDb, app, asset, send, user};

async fn post(app: &axum::Router, key: &str, path: &str, body: Value) -> (StatusCode, Value) {
    send(app, "POST", path, key, Some(body)).await
}

async fn rating_of(pool: &sqlx::PgPool, id: Uuid) -> (i16, Option<String>) {
    sqlx::query_as("SELECT rating, rating_source FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn ids_of(v: &Value) -> Vec<Uuid> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().parse().unwrap())
        .collect()
}

#[tokio::test]
async fn rating_photos_by_ids_and_by_scope_is_idempotent_and_marks_the_owner_as_the_source() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    let (a, b, c) = (
        asset(&t.pool, alice, "a.jpg").await,
        asset(&t.pool, alice, "b.jpg").await,
        asset(&t.pool, alice, "c.jpg").await,
    );
    let theirs = asset(&t.pool, bob, "theirs.jpg").await;

    for bad in [json!({"ids": [a], "rating": 6}), json!({"ids": [a], "rating": -1})] {
        assert_eq!(post(&app, &ka, "/assets/rating", bad).await.0, StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        post(&app, &ka, "/assets/rating", json!({"ids": [a]})).await.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a rating is required"
    );

    // Another user's photo and an unknown id are ignored.
    let (st, r) = post(
        &app,
        &ka,
        "/assets/rating",
        json!({"ids": [a, b, theirs, Uuid::new_v4()], "rating": 4}),
    )
    .await;
    assert_eq!((st, r["count"].as_i64()), (StatusCode::OK, Some(2)), "{r}");
    assert_eq!(rating_of(&t.pool, a).await, (4, Some("user".into())));
    assert_eq!(rating_of(&t.pool, theirs).await, (0, None), "not Alice's to rate");
    // Repeating changes nothing.
    let (_, r) = post(&app, &ka, "/assets/rating", json!({"ids": [a, b], "rating": 4})).await;
    assert_eq!(r["count"], 0);
    // A different photo of Bob's can not be reached with Alice's scope either.
    let (_, r) = post(&app, &kb, "/assets/rating", json!({"ids": [a], "rating": 1})).await;
    assert_eq!(r["count"], 0);
    assert_eq!(rating_of(&t.pool, a).await.0, 4);

    // A scope rates every photo of the view except the excepted: 2 changes (a is already 4... set 2 for all but c).
    let (_, r) = post(
        &app,
        &ka,
        "/assets/rating",
        json!({"scope": "library", "except": [c], "rating": 2}),
    )
    .await;
    assert_eq!(r["count"], 2);
    assert_eq!(rating_of(&t.pool, c).await, (0, None));
    // 0 clears; clearing an unrated photo is not a change and does not claim ownership of "unrated".
    let (_, r) = post(&app, &ka, "/assets/rating", json!({"ids": [a, c], "rating": 0})).await;
    assert_eq!(r["count"], 1);
    assert_eq!(
        rating_of(&t.pool, a).await,
        (0, Some("user".into())),
        "a deliberate 0 is the owner's"
    );
    assert_eq!(rating_of(&t.pool, c).await, (0, None));

    // The list items carry the rating.
    post(&app, &ka, "/assets/rating", json!({"ids": [b], "rating": 5})).await;
    let (_, list) = send(&app, "GET", "/assets", &ka, None).await;
    let item = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!(b))
        .unwrap();
    assert_eq!(item["rating"], 5);
    let (_, one) = send(&app, "GET", &format!("/assets/{b}"), &ka, None).await;
    assert_eq!(one["rating"], 5);
    t.teardown().await;
}

#[tokio::test]
async fn a_file_rating_becomes_the_owners_when_they_rate_the_photo_and_a_raw_takes_its_primarys() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let jpg = asset(&t.pool, alice, "p.jpg").await;
    let raw = asset(&t.pool, alice, "p.dng").await;
    sqlx::query("UPDATE assets SET primary_id = $1 WHERE id = $2")
        .bind(jpg)
        .bind(raw)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET rating = 3, rating_source = 'file' WHERE id = $1")
        .bind(jpg)
        .execute(&t.pool)
        .await
        .unwrap();
    // Same stars as the file says: still a change, because the photo is now the owner's.
    let (_, r) = post(&app, &ka, "/assets/rating", json!({"ids": [raw], "rating": 3})).await;
    assert_eq!(r["count"], 1, "the RAW id means its primary");
    assert_eq!(rating_of(&t.pool, jpg).await, (3, Some("user".into())));
    assert_eq!(
        rating_of(&t.pool, raw).await,
        (0, None),
        "the hidden half carries nothing"
    );
    t.teardown().await;
}

#[tokio::test]
async fn pairing_hands_a_raws_stars_to_the_visible_photo_unless_the_owner_rated_it() {
    use cyanotype_core::pairing::{Paired, pair_asset};
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = user(&t.pool, "a@example.com").await;
    let make = |name: &'static str, stars: i16, source: Option<&'static str>| {
        let pool = t.pool.clone();
        async move {
            let (id,): (Uuid,) = sqlx::query_as(
                "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, width, height,
                                     taken_at, rating, rating_source)
                 VALUES ($1, sha256($2::bytea), $3, $2, 'image/jpeg', 10, 10, 10, '2024-05-01T12:00:00Z', $4, $5) RETURNING id",
            )
            .bind(alice)
            .bind(name)
            .bind(format!("originals/{name}"))
            .bind(stars)
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
            id
        }
    };
    // The RAW says 4 (from its file), the JPEG nothing: the pair has 4.
    let (raw1, jpg1) = (make("A1.cr3", 4, Some("file")).await, make("A1.jpg", 0, None).await);
    assert!(matches!(
        pair_asset(&t.pool, raw1).await.unwrap(),
        Paired::Linked { .. }
    ));
    assert_eq!(rating_of(&t.pool, jpg1).await, (4, Some("file".into())));
    // The owner rated the JPEG 2: a higher rating on the RAW does not override it.
    let (raw2, jpg2) = (
        make("A2.cr3", 5, Some("file")).await,
        make("A2.jpg", 2, Some("user")).await,
    );
    assert!(matches!(
        pair_asset(&t.pool, jpg2).await.unwrap(),
        Paired::Linked { .. }
    ));
    assert_eq!(rating_of(&t.pool, jpg2).await, (2, Some("user".into())));
    // The owner rated the RAW on its own before it was paired: that is the pair's rating, as the owner's.
    let (raw3, jpg3) = (
        make("A3.cr3", 3, Some("user")).await,
        make("A3.jpg", 1, Some("file")).await,
    );
    assert!(matches!(
        pair_asset(&t.pool, raw3).await.unwrap(),
        Paired::Linked { .. }
    ));
    assert_eq!(rating_of(&t.pool, jpg3).await, (3, Some("user".into())));
    let _ = (raw1, raw2);
    t.teardown().await;
}

#[tokio::test]
async fn trashed_photos_are_not_rated() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let a = asset(&t.pool, alice, "a.jpg").await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, r) = post(&app, &ka, "/assets/rating", json!({"ids": [a], "rating": 5})).await;
    assert_eq!(r["count"], 0);
    t.teardown().await;
}

#[tokio::test]
async fn search_filters_by_at_least_n_stars_and_the_facet_counts_the_same_way() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    let mut by_stars = Vec::new();
    for stars in 0..=5i16 {
        let id = asset(&t.pool, alice, &format!("s{stars}.jpg")).await;
        sqlx::query("UPDATE assets SET rating = $2, rating_source = CASE WHEN $2 > 0 THEN 'user' END WHERE id = $1")
            .bind(id)
            .bind(stars)
            .execute(&t.pool)
            .await
            .unwrap();
        by_stars.push(id);
    }
    let b5 = asset(&t.pool, bob, "b5.jpg").await;
    sqlx::query("UPDATE assets SET rating = 5, rating_source = 'user' WHERE id = $1")
        .bind(b5)
        .execute(&t.pool)
        .await
        .unwrap();

    let search = |q: String| {
        let (app, ka) = (app.clone(), ka.clone());
        async move { send(&app, "GET", &format!("/assets/search?{q}"), &ka, None).await }
    };
    for (min, want) in [(1, 5), (3, 3), (5, 1)] {
        let (st, body) = search(format!("rating={min}")).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let got = ids_of(&body);
        assert_eq!(got.len(), want, "rating>={min}");
        assert!(got.iter().all(|id| by_stars[min as usize..].contains(id)));
        assert_eq!(body["total"], want as i64);
    }
    assert!(
        !ids_of(&search("rating=1".into()).await.1).contains(&b5),
        "Bob's photo is not Alice's result"
    );
    // rating=0 is no filter; 6 is refused.
    assert_eq!(ids_of(&search("rating=0".into()).await.1).len(), 6);
    assert_eq!(search("rating=6".into()).await.0, StatusCode::BAD_REQUEST);
    // Combined with another filter, and as a bulk scope ("select all" over a rating filter).
    sqlx::query("UPDATE assets SET favorite = true WHERE id = $1")
        .bind(by_stars[4])
        .execute(&t.pool)
        .await
        .unwrap();
    let both = ids_of(&search("rating=3&favorite=true".into()).await.1);
    assert_eq!(both, [by_stars[4]]);
    let (_, r) = post(
        &app,
        &ka,
        "/assets/favorite",
        json!({"scope": {"search": {"rating": 5}}, "favorite": true}),
    )
    .await;
    assert_eq!(r["count"], 1);

    // The facet: "at least N" counts, the visible library only.
    let (_, f) = send(&app, "GET", "/assets/search/facets", &ka, None).await;
    let facet: Vec<(String, i64)> = f["ratings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["value"].as_str().unwrap().to_string(), x["count"].as_i64().unwrap()))
        .collect();
    assert_eq!(
        facet,
        [
            ("5".into(), 1),
            ("4".into(), 2),
            ("3".into(), 3),
            ("2".into(), 4),
            ("1".into(), 5)
        ]
    );
    let (_, fb) = send(&app, "GET", "/assets/search/facets", &kb, None).await;
    // Bob has one 5-star photo: it counts under every "at least N".
    assert_eq!(fb["ratings"].as_array().unwrap().len(), 5);
    assert!(fb["ratings"].as_array().unwrap().iter().all(|x| x["count"] == 1));
    t.teardown().await;
}
