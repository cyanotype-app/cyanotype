//! Search v2 over a real Postgres: the strong and weaker tiers (and paging through them), the calibrated probability, the
//! dates and places read out of the query (and turned off again), and the model swap (the old model answers until the new one
//! has indexed enough). Fake text and image vectors again (`testutil`): the real models are `cyanotype-clip`'s tests.
use std::sync::Arc;

use axum::http::StatusCode;
use cyanotype_clip::Model;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    search::SearchState,
    testutil::{FakeText, TEST_MODEL, TestDb, app, app_with, asset, send, set_embedding_for, user},
};

pub(super) async fn photo(pool: &PgPool, owner: Uuid, name: &str, model: Model, words: &[(&str, f32)]) -> Uuid {
    let id = asset(pool, owner, name).await;
    // `asset` writes a constant vector for the test model; start from a clean slate.
    sqlx::query("DELETE FROM asset_embeddings WHERE asset_id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    set_embedding_for(pool, id, model, words).await;
    id
}

async fn get(app: &axum::Router, key: &str, query: &str) -> (StatusCode, Value) {
    send(app, "GET", &format!("/assets/search?{query}"), key, None).await
}

pub(super) fn ids(body: &Value) -> Vec<Uuid> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().parse().unwrap())
        .collect()
}

fn tiers(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["tier"].as_str().unwrap().to_string())
        .collect()
}

/// Four photos for the query "cat": A and B are strong (within 0.06 of the best), C is weaker (within 0.12), D is neither.
async fn tiered_library(t: &TestDb, model: Model) -> (Uuid, String, [Uuid; 4]) {
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let a = photo(&t.pool, alice, "a.jpg", model, &[("cat", 1.0)]).await;
    let b = photo(&t.pool, alice, "b.jpg", model, &[("cat", 0.97), ("dog", 0.2)]).await;
    let c = photo(&t.pool, alice, "c.jpg", model, &[("cat", 0.95), ("dog", 0.4)]).await;
    let d = photo(&t.pool, alice, "d.jpg", model, &[("dog", 1.0)]).await;
    (alice, key, [a, b, c, d])
}

#[tokio::test]
async fn the_search_shows_the_strong_matches_and_offers_the_weaker_ones() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [a, b, c, d]) = tiered_library(&t, TEST_MODEL).await;

    let (st, body) = get(&app, &key, "q=cat").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ids(&body), [a, b], "only the strong matches");
    assert_eq!(tiers(&body), ["strong", "strong"]);
    assert_eq!(body["strong_count"], json!(2));
    assert_eq!(body["weaker"]["count"], json!(1));
    assert_eq!(body["next_cursor"], Value::Null);
    assert_eq!(body["model"], json!("siglip2-b16"));
    // The model's own probability accompanies the cosine: sigmoid(112.67 * cos - 16.77).
    let (score, p) = (
        body["items"][0]["score"].as_f64().unwrap(),
        body["items"][0]["probability"].as_f64().unwrap(),
    );
    assert!(
        (p - 1.0 / (1.0 + (-(112.6689 * score - 16.771723)).exp())).abs() < 1e-3,
        "{score} {p}"
    );

    // "Show weaker matches": continue right after the last strong one.
    let cursor = body["weaker"]["cursor"]
        .as_str()
        .expect("a cursor after the last strong match");
    let (_, more) = get(&app, &key, &format!("q=cat&weaker=true&cursor={cursor}")).await;
    assert_eq!(ids(&more), [c]);
    assert_eq!(tiers(&more), ["weak"]);
    assert_eq!(
        more["weaker"],
        Value::Null,
        "nothing further to offer once they are shown"
    );
    assert_eq!(more["next_cursor"], Value::Null);

    // The same list in one request, and with the cut-off off.
    let (_, all) = get(&app, &key, "q=cat&weaker=true").await;
    assert_eq!(ids(&all), [a, b, c]);
    assert_eq!(tiers(&all), ["strong", "strong", "weak"]);
    let (_, plain) = get(&app, &key, "q=cat&cutoff=false").await;
    assert_eq!(ids(&plain), [a, b, c, d]);
    assert_eq!(plain["weaker"], Value::Null);
    t.teardown().await;
}

#[tokio::test]
async fn paging_through_the_tiers_is_stable_one_result_at_a_time() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [a, b, c, _]) = tiered_library(&t, TEST_MODEL).await;

    // Strong only, one per page: A, B, and then the last page carries the cursor for the weaker ones.
    let (_, p1) = get(&app, &key, "q=cat&limit=1").await;
    assert_eq!(ids(&p1), [a]);
    assert_eq!(
        p1["weaker"]["cursor"],
        Value::Null,
        "more strong pages to come: no weaker cursor yet"
    );
    let cur = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = get(&app, &key, &format!("q=cat&limit=1&cursor={cur}")).await;
    assert_eq!(ids(&p2), [b]);
    assert_eq!(p2["next_cursor"], Value::Null, "the strong list ends here");
    let weaker = p2["weaker"]["cursor"]
        .as_str()
        .expect("the cursor right after the last strong match");

    // With weaker, the same cursors walk through everything, tier by tier.
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..5 {
        let q = match &cursor {
            Some(c) => format!("q=cat&limit=1&weaker=true&cursor={c}"),
            None => "q=cat&limit=1&weaker=true".to_string(),
        };
        let (st, page) = get(&app, &key, &q).await;
        assert_eq!(st, StatusCode::OK);
        seen.push((ids(&page)[0], tiers(&page)[0].clone()));
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen,
        [(a, "strong".into()), (b, "strong".into()), (c, "weak".into())],
        "no gaps, no repeats"
    );
    // The cursor the strong list handed out continues into the same weak result.
    let (_, from_weaker) = get(&app, &key, &format!("q=cat&limit=1&weaker=true&cursor={weaker}")).await;
    assert_eq!(
        (ids(&from_weaker), tiers(&from_weaker)),
        (vec![c], vec!["weak".to_string()])
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_query_nothing_answers_has_no_strong_matches_and_says_what_it_has() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    // Cosine about 0.0945 and 0.0896 to "cat": above the weak floor, under the strong one.
    let near = photo(&t.pool, alice, "near.jpg", TEST_MODEL, &[("cat", 0.095), ("dog", 1.0)]).await;
    let nearer = photo(&t.pool, alice, "nearer.jpg", TEST_MODEL, &[("cat", 0.09), ("dog", 1.0)]).await;
    photo(&t.pool, alice, "far.jpg", TEST_MODEL, &[("cat", 0.05), ("dog", 1.0)]).await;
    let (_, body) = get(&app, &key, "q=cat").await;
    assert_eq!(ids(&body), Vec::<Uuid>::new());
    assert_eq!(body["strong_count"], json!(0));
    assert_eq!(body["weaker"], json!({"count": 2, "cursor": null}));
    let (_, weaker) = get(&app, &key, "q=cat&weaker=true").await;
    assert_eq!(ids(&weaker), [near, nearer]);
    assert_eq!(tiers(&weaker), ["weak", "weak"]);

    // A subject absent from the library altogether (all vectors far from the query): nothing, and nothing to offer.
    let (_, body) = get(&app, &key, "q=beach").await;
    assert_eq!(ids(&body), Vec::<Uuid>::new());
    assert_eq!(body["weaker"], Value::Null);
    t.teardown().await;
}

#[tokio::test]
async fn clip_has_relative_tiers_only_and_no_probability() {
    let Some(t) = TestDb::new().await else { return };
    let app = app_with(
        &t.pool,
        Arc::new(SearchState::uncached(Model::ClipB32, 95)),
        Arc::new(FakeText::default()),
    );
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let m = Model::ClipB32;
    let a = photo(&t.pool, alice, "a.jpg", m, &[("cat", 1.0)]).await;
    let b = photo(&t.pool, alice, "b.jpg", m, &[("cat", 0.99), ("dog", 0.1)]).await;
    // 0.032 below the best: past the 0.03 of the strong tier, inside the 0.04 of the weaker one.
    let c = photo(&t.pool, alice, "c.jpg", m, &[("cat", 0.97), ("dog", 0.25)]).await;
    photo(&t.pool, alice, "d.jpg", m, &[("dog", 1.0)]).await;
    let (_, body) = get(&app, &key, "q=cat").await;
    assert_eq!(body["model"], json!("clip-b32"));
    assert_eq!(ids(&body), [a, b]);
    assert_eq!(body["weaker"]["count"], json!(1));
    assert!(
        body["items"][0]["probability"].is_null(),
        "CLIP's scores have no probability reading"
    );
    let (_, more) = get(&app, &key, "q=cat&weaker=true").await;
    assert_eq!(
        (ids(&more), tiers(&more)),
        (
            vec![a, b, c],
            vec!["strong".to_string(), "strong".into(), "weak".into()]
        )
    );
    // No absolute floor: a query nothing answers still has "strong" nearest photos (documented limitation).
    let (_, body) = get(&app, &key, "q=beach").await;
    assert!(!ids(&body).is_empty(), "{body}");
    t.teardown().await;
}

#[tokio::test]
async fn dates_and_places_in_the_query_become_chips_and_filters() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let lisbon_june = photo(&t.pool, alice, "lj.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    let lisbon_2022 = photo(&t.pool, alice, "l22.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    let paris_june = photo(&t.pool, alice, "pj.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    for (id, when, city) in [
        (lisbon_june, "2023-06-10T10:00:00Z", "Lisbon"),
        (lisbon_2022, "2022-06-10T10:00:00Z", "Lisbon"),
        (paris_june, "2023-06-12T10:00:00Z", "Paris"),
    ] {
        sqlx::query("UPDATE assets SET taken_at = $2::timestamptz, place_city = $3, place_country = 'X' WHERE id = $1")
            .bind(id)
            .bind(when)
            .bind(city)
            .execute(&t.pool)
            .await
            .unwrap();
    }

    let (st, body) = get(&app, &key, "q=cat%20in%20Lisbon%20june%202023").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ids(&body), [lisbon_june]);
    assert_eq!(body["parsed"]["text"], json!("cat"));
    assert_eq!(
        body["parsed"]["chips"],
        json!([
            {"kind": "date", "label": "June 2023", "matched": "june 2023", "from": "2023-06-01", "to": "2023-06-30"},
            {"kind": "place", "label": "Lisbon", "matched": "in Lisbon", "value": "Lisbon", "place_kind": "city"},
        ])
    );

    // Taking a chip off puts its words back into the query and drops its filter.
    let (_, body) = get(&app, &key, "q=cat%20in%20Lisbon%20june%202023&skip=date").await;
    assert_eq!(ids(&body).len(), 2, "both Lisbon photos");
    assert_eq!(body["parsed"]["text"], json!("cat june 2023"));
    assert_eq!(body["parsed"]["chips"].as_array().unwrap().len(), 1);
    let (_, body) = get(&app, &key, "q=cat%20in%20Lisbon%20june%202023&skip=date,place").await;
    assert_eq!(body["parsed"]["chips"], json!([]));
    assert_eq!(body["parsed"]["text"], json!("cat in Lisbon june 2023"));

    // Nothing but filters left: a filter-only search (the timeline of the filter), not ranked.
    let (_, body) = get(&app, &key, "q=photos%20from%202023").await;
    assert_eq!(body["ranked"], json!(false));
    assert_eq!(ids(&body), [paris_june, lisbon_june]);
    assert_eq!(body["total"], json!(2));
    assert_eq!(body["parsed"]["text"], json!(""));
    assert_eq!(body["parsed"]["chips"][0]["label"], json!("2023"));
    assert!(body["items"][0]["tier"].is_null());

    // A place the library does not have is just words; an ambiguous word needs "in".
    let (_, body) = get(&app, &key, "q=cat%20in%20Tokyo").await;
    assert_eq!(body["parsed"]["chips"], json!([]));
    // Filters chosen in the sheet win over words of the same kind in the query: "2023" stays in the text.
    let (_, body) = get(&app, &key, "q=cat%202023&from=2022-01-01&to=2022-12-31").await;
    assert_eq!(ids(&body), [lisbon_2022]);
    assert_eq!(body["parsed"]["text"], json!("cat 2023"));
    assert_eq!(body["parsed"]["chips"], json!([]));
    // A request without q has nothing to parse.
    let (_, body) = get(&app, &key, "favorite=false").await;
    assert!(body["parsed"].is_null());
    t.teardown().await;
}

#[tokio::test]
async fn places_come_from_the_callers_own_photos_only() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, ak) = user(&t.pool, "a@example.com").await;
    let (bob, _) = user(&t.pool, "b@example.com").await;
    let a = photo(&t.pool, alice, "a.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    let b = photo(&t.pool, bob, "b.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    for (id, city) in [(a, "Lisbon"), (b, "Maui")] {
        sqlx::query("UPDATE assets SET place_city = $2 WHERE id = $1")
            .bind(id)
            .bind(city)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    let (_, body) = get(&app, &ak, "q=cat%20in%20Maui").await;
    assert_eq!(body["parsed"]["chips"], json!([]), "Maui is Bob's, not Alice's");
    let (_, body) = get(&app, &ak, "q=cat%20in%20Lisbon").await;
    assert_eq!(body["parsed"]["chips"][0]["value"], json!("Lisbon"));
    t.teardown().await;
}

/// Ten photos, each with a CLIP vector (the model being replaced); the first `n` also have a SigLIP 2 vector.
async fn swapping_library(t: &TestDb, total: usize, n: usize) -> (String, Vec<Uuid>) {
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let mut ids = Vec::new();
    for i in 0..total {
        let id = photo(&t.pool, alice, &format!("p{i}.jpg"), Model::ClipB32, &[("cat", 1.0)]).await;
        sqlx::query("UPDATE assets SET thumbhash = '\\x01' WHERE id = $1")
            .bind(id)
            .execute(&t.pool)
            .await
            .unwrap();
        if i < n {
            set_embedding_for(&t.pool, id, Model::Siglip2B16, &[("cat", 1.0)]).await;
        }
        ids.push(id);
    }
    (key, ids)
}

#[tokio::test]
async fn the_old_model_answers_until_the_new_one_covers_enough_of_the_library() {
    let Some(t) = TestDb::new().await else { return };
    let app = app_with(
        &t.pool,
        Arc::new(SearchState::uncached(Model::Siglip2B16, 95)),
        Arc::new(FakeText::default()),
    );
    let (key, ids_) = swapping_library(&t, 20, 5).await;

    // 25 %: CLIP answers (all 20 photos have its vector), and the response says a re-index is under way.
    let (_, body) = get(&app, &key, "q=cat").await;
    assert_eq!(body["model"], json!("clip-b32"));
    assert_eq!(ids(&body).len(), 20);
    assert_eq!(
        body["reindexing"],
        json!({"model": "siglip2-b16", "indexed": 5, "total": 20, "percent": 25, "switch_at_percent": 95})
    );
    let (_, p) = send(&app, "GET", "/assets/processing", &key, None).await;
    assert_eq!(p["embedding"]["reindexing"], json!(true));
    assert_eq!(p["embedding"]["percent"], json!(25));
    assert_eq!(p["embedding"]["serving_model"], json!("clip-b32"));
    assert_eq!(p["embedding"]["model"], json!("siglip2-b16"));

    // 90 %: still CLIP.
    for id in &ids_[5..18] {
        set_embedding_for(&t.pool, *id, Model::Siglip2B16, &[("cat", 1.0)]).await;
    }
    let (_, body) = get(&app, &key, "q=cat").await;
    assert_eq!(body["model"], json!("clip-b32"));
    assert_eq!(body["reindexing"]["percent"], json!(90));

    // 95 %: SigLIP 2 takes over (19 of 20), without the re-index banner; the stragglers are not searchable by it yet.
    set_embedding_for(&t.pool, ids_[18], Model::Siglip2B16, &[("cat", 1.0)]).await;
    let (_, body) = get(&app, &key, "q=cat").await;
    assert_eq!(body["model"], json!("siglip2-b16"));
    assert_eq!(body["reindexing"], Value::Null);
    assert_eq!(ids(&body).len(), 19);
    let (_, p) = send(&app, "GET", "/assets/processing", &key, None).await;
    assert_eq!(p["embedding"]["reindexing"], json!(false));
    assert_eq!(p["embedding"]["serving_model"], json!("siglip2-b16"));
    t.teardown().await;
}

#[tokio::test]
async fn a_fresh_library_and_a_missing_old_model_use_the_configured_model_at_once() {
    let Some(t) = TestDb::new().await else { return };
    // No old vectors at all: the new model answers whatever its coverage.
    let app = app_with(
        &t.pool,
        Arc::new(SearchState::uncached(Model::Siglip2B16, 95)),
        Arc::new(FakeText::default()),
    );
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = photo(&t.pool, alice, "a.jpg", Model::Siglip2B16, &[("cat", 1.0)]).await;
    sqlx::query("UPDATE assets SET thumbhash = '\\x01' WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    photo(&t.pool, alice, "b.jpg", Model::Siglip2B16, &[("cat", 1.0)]).await;
    let (_, body) = get(&app, &key, "q=cat").await;
    assert_eq!(
        (body["model"].clone(), body["reindexing"].clone()),
        (json!("siglip2-b16"), Value::Null)
    );

    // The old model's text tower is not installed: it cannot answer, so the new one does even at 50 %.
    let (key2, _) = swapping_library_for(&t, "c@example.com", 10, 5).await;
    let no_clip = app_with(
        &t.pool,
        Arc::new(SearchState::uncached(Model::Siglip2B16, 95)),
        Arc::new(FakeText {
            missing: vec![Model::ClipB32],
        }),
    );
    let (_, body) = get(&no_clip, &key2, "q=cat").await;
    assert_eq!(body["model"], json!("siglip2-b16"));
    assert_eq!(ids(&body).len(), 5);
    t.teardown().await;
}

async fn swapping_library_for(t: &TestDb, email: &str, total: usize, n: usize) -> (String, Vec<Uuid>) {
    let (owner, key) = user(&t.pool, email).await;
    let mut out = Vec::new();
    for i in 0..total {
        let id = photo(
            &t.pool,
            owner,
            &format!("{email}{i}.jpg"),
            Model::ClipB32,
            &[("cat", 1.0)],
        )
        .await;
        sqlx::query("UPDATE assets SET thumbhash = '\\x01' WHERE id = $1")
            .bind(id)
            .execute(&t.pool)
            .await
            .unwrap();
        if i < n {
            set_embedding_for(&t.pool, id, Model::Siglip2B16, &[("cat", 1.0)]).await;
        }
        out.push(id);
    }
    (key, out)
}
