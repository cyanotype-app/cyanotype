//! Search over a real Postgres (pgvector): ranking, every filter, owner isolation, trash and hidden RAWs, pagination,
//! the exact-versus-HNSW plan, facets and request validation. The text model is a fake that maps words onto embedding
//! axes (`testutil::FakeText`), so these need no ONNX model; the real model is exercised by `cyanotype-clip`'s tests.
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::search::{After, DEPTH, EXACT_MAX_OVERRIDE, Filters, Plan, SearchQuery, ranked_rows, ranked_rows_planned};
use crate::testutil::{TEST_MODEL, TestDb, app, asset, embedding_of, send, set_embedding, user};

/// A visible photo whose image embedding leans toward the given words.
async fn photo(pool: &PgPool, owner: Uuid, name: &str, words: &[(&str, f32)]) -> Uuid {
    let id = asset(pool, owner, name).await;
    set_embedding(pool, id, words).await;
    id
}

async fn set(pool: &PgPool, id: Uuid, column_sql: &str) {
    sqlx::query(&format!("UPDATE assets SET {column_sql} WHERE id = $1"))
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

/// The search as the tests above ask it: the whole ranked list, without the cut-off (`cutoff=false`), so that a test of
/// ranking, filters or pagination sees every result. The cut-off has its own tests (`search_v2_tests.rs`).
async fn get(app: &axum::Router, key: &str, query: &str) -> (StatusCode, Value) {
    send(app, "GET", &format!("/assets/search?{query}&cutoff=false"), key, None).await
}

fn ids(body: &Value) -> Vec<Uuid> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().parse().unwrap())
        .collect()
}

fn enc(s: &str) -> String {
    s.replace(' ', "%20")
}

#[tokio::test]
async fn text_queries_rank_by_similarity_and_report_a_score() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let cat = photo(&t.pool, alice, "cat.jpg", &[("cat", 1.0)]).await;
    let cat_beach = photo(&t.pool, alice, "cat_beach.jpg", &[("cat", 1.0), ("beach", 1.0)]).await;
    photo(&t.pool, alice, "beach.jpg", &[("beach", 1.0)]).await;
    let car = photo(&t.pool, alice, "car.jpg", &[("car", 1.0)]).await;
    set(&t.pool, cat, "favorite = true").await;

    let (st, body) = get(&app, &key, "q=cat").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["ranked"], json!(true));
    assert_eq!(
        ids(&body)[..2],
        [cat, cat_beach],
        "the pure cat photo, then the cat on a beach"
    );
    let scores: Vec<f64> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["score"].as_f64().unwrap())
        .collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "best first: {scores:?}");
    assert!(scores[0] > 0.99 && scores[3] < 0.1, "{scores:?}");
    // The list item shape: favorite, size, thumbs (none yet), paired.
    assert_eq!(body["items"][0]["favorite"], json!(true));
    assert_eq!(body["items"][0]["paired"], json!([]));
    assert!(body["items"][0]["thumb_url"].is_null());

    let (_, body) = get(&app, &key, &format!("q={}", enc("a cat on the beach"))).await;
    assert_eq!(ids(&body)[0], cat_beach);
    let (_, body) = get(&app, &key, "q=car").await;
    assert_eq!(ids(&body)[0], car);
    t.teardown().await;
}

#[tokio::test]
async fn filters_alone_list_in_timeline_order_and_combine_with_text() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let old_cat = photo(&t.pool, alice, "old_cat.jpg", &[("cat", 1.0)]).await;
    let new_cat = photo(&t.pool, alice, "new_cat.heic", &[("cat", 1.0)]).await;
    let new_dog = photo(&t.pool, alice, "new_dog.jpg", &[("dog", 1.0)]).await;
    set(&t.pool, old_cat, "taken_at = '2020-06-15T12:00:00Z', camera_make = 'Canon', camera_model = 'Canon EOS R5', place_city = 'Lisbon', place_region = 'Lisboa', place_country = 'Portugal', place_country_code = 'PT', latitude = 38.7, longitude = -9.1, favorite = true").await;
    set(&t.pool, new_cat, "taken_at = '2024-03-01T08:00:00Z', camera_make = 'Apple', camera_model = 'iPhone 15', latitude = 48.8, longitude = 2.3, place_city = 'Paris', place_country = 'France', place_country_code = 'FR'").await;
    set(&t.pool, new_dog, "taken_at = '2024-03-02T08:00:00Z'").await;

    // No q: newest first, not ranked, no score.
    let (_, body) = get(&app, &key, "favorite=false").await;
    assert_eq!(body["ranked"], json!(false));
    assert_eq!(ids(&body), [new_dog, new_cat, old_cat]);
    assert!(body["items"][0]["score"].is_null());

    let only = |q: &'static str, want: Vec<Uuid>| {
        let app = app.clone();
        let key = key.clone();
        async move {
            let (st, body) = get(&app, &key, q).await;
            assert_eq!(st, StatusCode::OK, "{q}");
            assert_eq!(ids(&body), want, "{q}");
        }
    };
    // Dates are UTC days, both ends inclusive.
    only("from=2024-03-01&to=2024-03-01", vec![new_cat]).await;
    only("from=2024-03-01", vec![new_dog, new_cat]).await;
    only("to=2020-06-15", vec![old_cat]).await;
    only("favorite=true", vec![old_cat]).await;
    only("camera=canon%20eos%20r5", vec![old_cat]).await; // "Canon" is not repeated for a model that starts with it; case-insensitive
    only("camera=Apple%20iPhone%2015", vec![new_cat]).await; // "Apple" + "iPhone 15"
    only("place=lisbon", vec![old_cat]).await;
    only("place=Portugal", vec![old_cat]).await;
    only("place=FR", vec![new_cat]).await;
    only("place=Lisboa", vec![old_cat]).await; // region
    only("has_location=true", vec![new_cat, old_cat]).await;
    only("has_location=false", vec![new_dog]).await;
    only("type=heic", vec![new_cat]).await;
    only("type=jpeg", vec![new_dog, old_cat]).await;
    only("place=nowhere", vec![]).await;

    // With text, the same filters narrow the ranked list.
    let (_, body) = get(&app, &key, "q=cat&from=2024-01-01").await;
    assert_eq!(ids(&body)[0], new_cat);
    assert_eq!(
        ids(&body).len(),
        2,
        "the dog photo is a (weak) match too; the old cat is filtered out"
    );
    assert!(!ids(&body).contains(&old_cat));
    let (_, body) = get(&app, &key, "q=cat&favorite=true&place=Lisbon").await;
    assert_eq!(ids(&body), [old_cat]);
    t.teardown().await;
}

#[tokio::test]
async fn file_types_cover_pairs_raw_only_and_everything_else() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let jpeg_only = photo(&t.pool, alice, "a.jpg", &[("cat", 1.0)]).await;
    let pair = photo(&t.pool, alice, "b.jpg", &[("cat", 1.0)]).await;
    let pair_raw = photo(&t.pool, alice, "b.cr3", &[("cat", 1.0)]).await;
    let raw_only = photo(&t.pool, alice, "c.nef", &[("cat", 1.0)]).await;
    let png = photo(&t.pool, alice, "d.png", &[("cat", 1.0)]).await;
    set(&t.pool, pair_raw, &format!("primary_id = '{pair}'")).await;

    for (ty, want) in [
        ("jpeg", vec![jpeg_only, pair]),
        ("raw", vec![pair, raw_only]), // the JPEG of a RAW + JPEG pair is a photo that has a RAW
        ("other", vec![png]),
        ("heic", vec![]),
    ] {
        let (_, body) = get(&app, &key, &format!("type={ty}")).await;
        let mut got = ids(&body);
        got.sort();
        let mut want = want;
        want.sort();
        assert_eq!(got, want, "type={ty}");
    }
    // The hidden RAW is never a result of its own.
    let (_, body) = get(&app, &key, "q=cat").await;
    assert!(!ids(&body).contains(&pair_raw));
    assert_eq!(ids(&body).len(), 4);
    // And the pair shows its RAW behind it, like the library list.
    let item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!(pair))
        .unwrap();
    assert_eq!(item["paired"][0]["id"], json!(pair_raw));
    t.teardown().await;
}

#[tokio::test]
async fn albums_filter_and_other_peoples_albums_match_nothing() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, ak) = user(&t.pool, "a@example.com").await;
    let (bob, _bk) = user(&t.pool, "b@example.com").await;
    let a1 = photo(&t.pool, alice, "a1.jpg", &[("cat", 1.0)]).await;
    let _a2 = photo(&t.pool, alice, "a2.jpg", &[("cat", 0.5)]).await;
    let b1 = photo(&t.pool, bob, "b1.jpg", &[("cat", 1.0)]).await;
    let album: Uuid = sqlx::query_scalar("INSERT INTO albums (owner_id, title) VALUES ($1, 'x') RETURNING id")
        .bind(alice)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let bobs: Uuid = sqlx::query_scalar("INSERT INTO albums (owner_id, title) VALUES ($1, 'y') RETURNING id")
        .bind(bob)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    for (al, a) in [(album, a1), (bobs, b1)] {
        sqlx::query("INSERT INTO album_assets (album_id, asset_id) VALUES ($1, $2)")
            .bind(al)
            .bind(a)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    let (_, body) = get(&app, &ak, &format!("album={album}")).await;
    assert_eq!(ids(&body), [a1]);
    let (_, body) = get(&app, &ak, &format!("q=cat&album={album}")).await;
    assert_eq!(ids(&body), [a1]);
    let (st, body) = get(&app, &ak, &format!("q=cat&album={bobs}")).await;
    assert_eq!(
        (st, ids(&body)),
        (StatusCode::OK, vec![]),
        "Bob's album gives Alice nothing, and not an error that reveals it"
    );
    t.teardown().await;
}

#[tokio::test]
async fn results_are_the_callers_own_live_visible_photos_only() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, ak) = user(&t.pool, "a@example.com").await;
    let (bob, bk) = user(&t.pool, "b@example.com").await;
    let mine = photo(&t.pool, alice, "mine.jpg", &[("cat", 1.0)]).await;
    let trashed = photo(&t.pool, alice, "trashed.jpg", &[("cat", 1.0)]).await;
    let purging = photo(&t.pool, alice, "purging.jpg", &[("cat", 1.0)]).await;
    let hidden = photo(&t.pool, alice, "hidden.cr2", &[("cat", 1.0)]).await;
    let theirs = photo(&t.pool, bob, "theirs.jpg", &[("cat", 1.0)]).await;
    set(&t.pool, trashed, "deleted_at = now()").await;
    set(&t.pool, purging, "deleted_at = now(), purging_at = now()").await;
    set(&t.pool, hidden, &format!("primary_id = '{mine}'")).await;

    // Both ranking plans: Bob's near-identical vectors are in the same HNSW index and must never reach Alice.
    for exact_max in [None, Some(0)] {
        EXACT_MAX_OVERRIDE.with(|c| c.set(exact_max));
        for q in ["q=cat", "q=cat&favorite=false", "from=2000-01-01", ""] {
            let (st, body) = get(&app, &ak, q).await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(ids(&body), [mine], "Alice, {q:?}, {exact_max:?}");
            let (_, body) = get(&app, &bk, q).await;
            assert_eq!(ids(&body), [theirs], "Bob, {q:?}, {exact_max:?}");
        }
    }
    EXACT_MAX_OVERRIDE.with(|c| c.set(None));
    // Facets obey the same rules.
    let (_, f) = send(&app, "GET", "/assets/search/facets", &ak, None).await;
    assert_eq!(f["total"], json!(1));
    let (st, _) = send(&app, "GET", "/assets/search?q=cat", "cyk_nope", None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

#[tokio::test]
async fn photos_without_a_current_embedding_are_not_text_results_but_do_match_filters() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let indexed = photo(&t.pool, alice, "indexed.jpg", &[("cat", 1.0)]).await;
    let pending = asset(&t.pool, alice, "pending.jpg").await; // testutil's constant vector, replaced below
    let old_version = photo(&t.pool, alice, "old_version.jpg", &[("cat", 1.0)]).await;
    let other_model = photo(&t.pool, alice, "other_model.jpg", &[("cat", 1.0)]).await;
    sqlx::query("DELETE FROM asset_embeddings WHERE asset_id = $1")
        .bind(pending)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE asset_embeddings SET version = version + 1 WHERE asset_id = $1")
        .bind(old_version)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE asset_embeddings SET model = 'another-model' WHERE asset_id = $1")
        .bind(other_model)
        .execute(&t.pool)
        .await
        .unwrap();

    let (_, body) = get(&app, &key, "q=cat").await;
    assert_eq!(ids(&body), [indexed]);
    let (_, body) = get(&app, &key, "favorite=false").await;
    assert_eq!(ids(&body).len(), 4, "filters never need embeddings");
    t.teardown().await;
}

/// 150 photos on a gradient between "cat" and "dog".
async fn gradient(pool: &PgPool, owner: Uuid, n: usize) -> Vec<Uuid> {
    let mut out = Vec::new();
    for i in 0..n {
        let w = i as f32 / n as f32;
        out.push(photo(pool, owner, &format!("g{i:03}.jpg"), &[("cat", 1.0 - w), ("dog", w)]).await);
    }
    out
}

async fn page_through(app: &axum::Router, key: &str, query: &str, limit: usize) -> Vec<Uuid> {
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..50 {
        let mut url = format!("{query}&limit={limit}");
        if let Some(c) = &cursor {
            url.push_str(&format!("&cursor={c}"));
        }
        let (st, body) = get(app, key, &url).await;
        assert_eq!(st, StatusCode::OK);
        all.extend(ids(&body));
        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => return all,
        }
    }
    panic!("pagination did not end");
}

#[tokio::test]
async fn pagination_has_no_gaps_or_duplicates_on_either_plan() {
    // (Runs on one thread: the plan override below is thread-local.)
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let photos = gradient(&t.pool, alice, 150).await;
    // Identical vectors tie on distance: the id breaks the tie, so a page boundary inside a tie loses nothing.
    for tw in 0..6 {
        let id = photo(&t.pool, alice, &format!("tie{tw}.jpg"), &[("cat", 0.5), ("dog", 0.5)]).await;
        set(&t.pool, id, "favorite = true").await;
    }
    let total = photos.len() + 6;

    // A small library is ranked exactly (with or without a filter); lowering the threshold to 0 sends the same queries
    // through the HNSW plan. Both must page without gaps or duplicates.
    for (exact_max, q, expect) in [
        (None, "q=cat", total),
        (None, "q=cat&favorite=true", 6),
        (Some(0), "q=cat", total),
        (Some(0), "q=cat&favorite=true", 6),
    ] {
        EXACT_MAX_OVERRIDE.with(|c| c.set(exact_max));
        let all = page_through(&app, &key, q, 40).await;
        let mut dedup = all.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(
            dedup.len(),
            all.len(),
            "{q} ({exact_max:?}): a photo appeared on two pages"
        );
        assert_eq!(all.len(), expect, "{q} ({exact_max:?}): nothing lost between pages");
        // One big page is the same list in the same order.
        let (_, one) = get(&app, &key, &format!("{q}&limit=200")).await;
        assert_eq!(ids(&one), all, "{q} ({exact_max:?}): paged order equals unpaged order");
    }
    EXACT_MAX_OVERRIDE.with(|c| c.set(None));
    // The ranking itself runs along the gradient.
    let (_, body) = get(&app, &key, "q=cat&limit=3").await;
    assert_eq!(ids(&body), photos[..3]);

    // Filter-only pagination (newest first) is stable too.
    let all = page_through(&app, &key, "favorite=false", 40).await;
    assert_eq!(all.len(), total);
    t.teardown().await;
}

#[tokio::test]
async fn ranked_results_stop_at_the_depth_limit() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = user(&t.pool, "a@example.com").await;
    // Bulk insert beyond DEPTH, all with the same direction.
    sqlx::query(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash)
         SELECT $1, sha256(g::text::bytea), 'k' || g, 'p' || g || '.jpg', 'image/jpeg', 1, '\\x01' FROM generate_series(1, $2) g",
    )
    .bind(alice)
    .bind(DEPTH + 25)
    .execute(&t.pool)
    .await
    .unwrap();
    let lit = cyanotype_core::embeddings::pg_literal(&embedding_of(&[("cat", 1.0)]));
    sqlx::query(
        "INSERT INTO asset_embeddings (asset_id, model, version, embedding)
         SELECT id, $1, $2, $3::text::halfvec FROM assets",
    )
    .bind(TEST_MODEL.id())
    .bind(TEST_MODEL.version())
    .bind(lit)
    .execute(&t.pool)
    .await
    .unwrap();
    let q = embedding_of(&[("cat", 1.0)]);
    for plan in [Plan::Exact, Plan::Hnsw] {
        let f = Filters::default();
        let mut after = None;
        let mut n = 0;
        loop {
            let page = ranked_rows_planned(&t.pool, alice, &f, TEST_MODEL, &q, 100, after, Some(plan))
                .await
                .unwrap();
            n += page.rows.len();
            match page.rows.last() {
                Some(r) if page.rows.len() == 100 => {
                    after = Some(After {
                        dist: r.dist.unwrap(),
                        id: r.row.summary.id,
                        k: page.k,
                    })
                }
                _ => break,
            }
        }
        assert_eq!(n as i64, DEPTH, "{plan:?}");
    }
    t.teardown().await;
}

#[tokio::test]
async fn the_hnsw_plan_looks_further_when_other_photos_crowd_the_nearest_vectors() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ak) = user(&t.pool, "a@example.com").await;
    let (bob, _) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    // Bob has 1,000 photos that are all nearer to "cat" than any of Alice's: the first 900 neighbours are all his. (Each a
    // little different from the others: a thousand identical vectors would leave Alice's unreachable in the HNSW graph,
    // a property of the index that real photos do not test.)
    let near: Vec<String> = (0..1000)
        .map(|g| cyanotype_core::embeddings::pg_literal(&embedding_of(&[("cat", 1.0), ("night", g as f32 * 0.0007)])))
        .collect();
    sqlx::query(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash)
         SELECT $1, sha256(g::text::bytea), 'k' || g, 'b' || g || '.jpg', 'image/jpeg', 1, '\\x01' FROM generate_series(1, 1000) g",
    )
    .bind(bob)
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO asset_embeddings (asset_id, model, version, embedding)
         SELECT a.id, $1, $2, u.v::halfvec
         FROM (SELECT id, row_number() OVER (ORDER BY id) AS rn FROM assets) a
         JOIN unnest($3::text[]) WITH ORDINALITY u(v, rn) ON u.rn = a.rn",
    )
    .bind(TEST_MODEL.id())
    .bind(TEST_MODEL.version())
    .bind(near)
    .execute(&t.pool)
    .await
    .unwrap();
    let mine: Vec<Uuid> = {
        let mut v = Vec::new();
        for i in 0..5 {
            v.push(
                photo(
                    &t.pool,
                    alice,
                    &format!("a{i}.jpg"),
                    &[("cat", 0.3), ("dog", 1.0 + i as f32)],
                )
                .await,
            );
        }
        v
    };
    EXACT_MAX_OVERRIDE.with(|c| c.set(Some(0))); // the HNSW plan
    let (st, body) = get(&app, &ak, "q=cat&limit=3").await;
    assert_eq!(st, StatusCode::OK);
    let first = ids(&body);
    assert_eq!(first.len(), 3, "Alice's photos are found behind Bob's 1,000");
    assert!(first.iter().all(|id| mine.contains(id)));
    // The next page ranks the same list (its cursor carries the widened k).
    let cursor = body["next_cursor"].as_str().unwrap();
    let (_, page2) = get(&app, &ak, &format!("q=cat&limit=3&cursor={cursor}")).await;
    assert_eq!(ids(&page2).len(), 2);
    let mut all: Vec<Uuid> = first.into_iter().chain(ids(&page2)).collect();
    all.sort();
    let mut want = mine.clone();
    want.sort();
    assert_eq!(all, want);
    EXACT_MAX_OVERRIDE.with(|c| c.set(None));
    t.teardown().await;
}

#[tokio::test]
async fn small_searches_are_exact_and_big_ones_use_the_hnsw_index() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = user(&t.pool, "a@example.com").await;
    for i in 0..6 {
        photo(
            &t.pool,
            alice,
            &format!("a{i}.jpg"),
            &[("cat", 1.0 - i as f32 / 10.0), ("dog", i as f32 / 10.0)],
        )
        .await;
    }
    let q = embedding_of(&[("cat", 1.0)]);
    let plain = Filters::default();
    let favs = Filters::parse(&SearchQuery {
        favorite: Some(true),
        ..Default::default()
    })
    .unwrap();
    // Few photos pass: exact, filtered or not.
    assert_eq!(
        ranked_rows(&t.pool, alice, &plain, TEST_MODEL, &q, 10, None)
            .await
            .unwrap()
            .plan,
        Plan::Exact
    );
    assert_eq!(
        ranked_rows(&t.pool, alice, &favs, TEST_MODEL, &q, 10, None)
            .await
            .unwrap()
            .plan,
        Plan::Exact
    );
    // Once more pass than the threshold: HNSW, and it answers the same as the exact plan.
    EXACT_MAX_OVERRIDE.with(|c| c.set(Some(3)));
    let hnsw = ranked_rows(&t.pool, alice, &plain, TEST_MODEL, &q, 10, None)
        .await
        .unwrap();
    assert_eq!(hnsw.plan, Plan::Hnsw);
    let exact = ranked_rows_planned(&t.pool, alice, &plain, TEST_MODEL, &q, 10, None, Some(Plan::Exact))
        .await
        .unwrap();
    let ids = |rows: &[super::search::SearchRow]| rows.iter().map(|r| r.row.summary.id).collect::<Vec<_>>();
    assert_eq!(ids(&hnsw.rows), ids(&exact.rows));
    assert_eq!(hnsw.rows.len(), 6);
    assert_eq!(
        ranked_rows(&t.pool, alice, &favs, TEST_MODEL, &q, 10, None)
            .await
            .unwrap()
            .plan,
        Plan::Exact,
        "no favorites pass: still few"
    );
    EXACT_MAX_OVERRIDE.with(|c| c.set(None));
    t.teardown().await;
}

#[tokio::test]
async fn select_all_over_a_filter_only_search_is_a_scope_and_total_counts_it() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let (bob, bkey) = user(&t.pool, "b@example.com").await;
    let mut want = Vec::new();
    for i in 0..5 {
        let id = photo(&t.pool, alice, &format!("p{i}.jpg"), &[("cat", 1.0)]).await;
        set(&t.pool, id, "camera_make = 'Canon', camera_model = 'EOS R5'").await;
        want.push(id);
    }
    let other = photo(&t.pool, alice, "other.jpg", &[("cat", 1.0)]).await;
    let theirs = photo(&t.pool, bob, "theirs.jpg", &[("cat", 1.0)]).await;
    set(&t.pool, theirs, "camera_make = 'Canon', camera_model = 'EOS R5'").await;

    let (_, body) = get(&app, &key, "camera=Canon%20EOS%20R5&limit=2").await;
    assert_eq!(
        (body["total"].clone(), body["items"].as_array().unwrap().len()),
        (json!(5), 2)
    );
    let cursor = body["next_cursor"].as_str().unwrap();
    let (_, page2) = get(&app, &key, &format!("camera=Canon%20EOS%20R5&limit=2&cursor={cursor}")).await;
    assert!(page2["total"].is_null(), "only the first page pays for the count");
    let (_, ranked) = get(&app, &key, "q=cat&camera=Canon%20EOS%20R5").await;
    assert!(ranked["total"].is_null());

    // Favorite everything the search lists except one photo, by scope: Bob's photo and the other camera are untouched.
    let body = json!({"scope": {"search": {"camera": "Canon EOS R5"}}, "except": [want[0]]});
    let (st, r) = send(
        &app,
        "POST",
        "/assets/favorite",
        &key,
        Some(json!({"scope": body["scope"], "except": body["except"], "favorite": true})),
    )
    .await;
    assert_eq!((st, r["count"].clone()), (StatusCode::OK, json!(4)));
    let favs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM assets WHERE favorite")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(favs.len(), 4);
    assert!(!favs.contains(&want[0]) && !favs.contains(&other) && !favs.contains(&theirs));
    // Bob selecting "the same search" gets only his own photo.
    let (_, r) = send(
        &app,
        "POST",
        "/assets/favorite",
        &bkey,
        Some(json!({"scope": {"search": {"camera": "Canon EOS R5"}}, "except": [], "favorite": true})),
    )
    .await;
    assert_eq!(r["count"], json!(1));
    // A text query cannot be a scope, and bad filters are rejected like in the search itself.
    for scope in [json!({"search": {"q": "cat"}}), json!({"search": {"type": "gif"}})] {
        let (st, _) = send(
            &app,
            "POST",
            "/assets/favorite",
            &key,
            Some(json!({"scope": scope, "except": [], "favorite": true})),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
    t.teardown().await;
}

#[tokio::test]
async fn facets_count_cameras_places_years_types_and_favorites() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let (bob, _) = user(&t.pool, "b@example.com").await;
    let a = photo(&t.pool, alice, "a.jpg", &[("cat", 1.0)]).await;
    let b = photo(&t.pool, alice, "b.heic", &[("cat", 1.0)]).await;
    let c = photo(&t.pool, alice, "c.nef", &[("cat", 1.0)]).await;
    let d = photo(&t.pool, alice, "d.jpg", &[("cat", 1.0)]).await;
    let theirs = photo(&t.pool, bob, "x.jpg", &[("cat", 1.0)]).await;
    set(&t.pool, a, "taken_at = '2023-05-01Z', camera_make = 'Canon', camera_model = 'Canon EOS R5', place_city = 'Lisbon', place_country = 'Portugal', latitude = 1, longitude = 1, favorite = true").await;
    set(&t.pool, b, "taken_at = '2023-06-01Z', camera_make = 'Apple', camera_model = 'iPhone 15', place_city = 'Lisbon', place_country = 'Portugal', latitude = 1, longitude = 1").await;
    set(
        &t.pool,
        c,
        "taken_at = '2021-01-01Z', camera_make = 'Canon', camera_model = 'Canon EOS R5'",
    )
    .await;
    set(
        &t.pool,
        d,
        "deleted_at = now(), camera_make = 'Leica', camera_model = 'Q2'",
    )
    .await; // trashed: not counted
    set(&t.pool, theirs, "camera_make = 'Nikon', camera_model = 'Z6'").await;

    let (st, f) = send(&app, "GET", "/assets/search/facets", &key, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        (f["total"].clone(), f["favorites"].clone(), f["with_location"].clone()),
        (json!(3), json!(1), json!(2))
    );
    assert_eq!(
        f["cameras"],
        json!([{"value": "Canon EOS R5", "count": 2}, {"value": "Apple iPhone 15", "count": 1}])
    );
    let place = |v: &str, kind: &str| {
        f["places"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["value"] == v && p["kind"] == kind)
            .cloned()
    };
    assert_eq!(place("Lisbon", "city").unwrap()["count"], json!(2));
    assert_eq!(place("Portugal", "country").unwrap()["count"], json!(2));
    assert!(place("Nowhere", "city").is_none());
    let years: Vec<(String, i64)> = f["years"]
        .as_array()
        .unwrap()
        .iter()
        .map(|y| (y["value"].as_str().unwrap().to_string(), y["count"].as_i64().unwrap()))
        .collect();
    assert_eq!(years[..2], [("2023".to_string(), 2), ("2021".to_string(), 1)]);
    let types: Vec<(String, i64)> = f["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|y| (y["value"].as_str().unwrap().to_string(), y["count"].as_i64().unwrap()))
        .collect();
    assert_eq!(
        types,
        [("jpeg".to_string(), 1), ("heic".to_string(), 1), ("raw".to_string(), 1)]
    );
    // Every facet value is a working filter value.
    let (_, body) = get(&app, &key, &format!("camera={}", enc("Canon EOS R5"))).await;
    assert_eq!(ids(&body).len(), 2);
    t.teardown().await;
}

#[tokio::test]
async fn bad_requests_are_rejected_and_a_missing_model_is_a_503() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key) = user(&t.pool, "a@example.com").await;
    photo(&t.pool, alice, "a.jpg", &[("cat", 1.0)]).await;
    for q in [
        "from=yesterday",
        "to=2024-13-40",
        "from=2024-05-02&to=2024-05-01",
        "type=gif",
        "album=not-a-uuid",
        "cursor=zz&q=cat",
        "cursor=zz",
    ] {
        let (st, _) = get(&app, &key, q).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{q}");
    }
    let (st, _) = get(&app, &key, &format!("q={}", "x".repeat(201))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Blank q is no query at all.
    let (st, body) = get(&app, &key, "q=%20%20").await;
    assert_eq!((st, body["ranked"].clone()), (StatusCode::OK, json!(false)));

    let no_model = crate::testutil::app_with(
        &t.pool,
        crate::testutil::default_search(),
        Arc::new(crate::testutil::FakeText {
            missing: cyanotype_clip::Model::ALL.to_vec(),
        }),
    );
    let (st, _) = get(&no_model, &key, "q=cat").await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    let (st, _) = get(&no_model, &key, "favorite=false").await;
    assert_eq!(st, StatusCode::OK, "filters work without the model");
    t.teardown().await;
}

/// Latency and recall on the synthetic 50,000-photo library of `scripts/search-perf-seed.sql`. Not part of CI:
///
/// ```text
/// PERF_DATABASE_URL=postgres://cyanotype:cyanotype@127.0.0.1:5432/cy_perf \
///   cargo test --release -p cyanotype-assets search_performance -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "needs the seeded perf database"]
async fn search_performance_on_50k_photos() {
    use std::{sync::Mutex, time::Instant};
    let Ok(url) = std::env::var("PERF_DATABASE_URL") else {
        return;
    };
    let pool = cyanotype_core::db::connect(&url).await.unwrap();
    let owner: Uuid = "00000000-0000-0000-0000-0000000000a1".parse().unwrap();

    // The seed script writes CLIP's 512-d vectors.
    let model = cyanotype_clip::Model::ClipB32;
    struct Fixed(Mutex<Vec<f32>>);
    impl crate::search::TextEmbedder for Fixed {
        fn embed(&self, _: cyanotype_clip::Model, _: &str) -> anyhow::Result<Arc<Vec<f32>>> {
            Ok(Arc::new(self.0.lock().unwrap().clone()))
        }
        fn available(&self, _: cyanotype_clip::Model) -> bool {
            true
        }
    }
    let fixed = Arc::new(Fixed(Mutex::new(vec![0.0; 512])));
    let state = crate::AppState {
        db: pool.clone(),
        storage: crate::testutil::dummy_storage(),
        trash_retention_days: 30,
        purge_wake: Default::default(),
        text: fixed.clone(),
        search: Arc::new(crate::search::SearchState::uncached(model, 95)),
    };
    let mut texts: Vec<String> = Vec::new();
    for _ in 0..40 {
        // A point between photos (the mean of 40 of them), the way a text query sits among the images it describes.
        // (A photo's own vector is the easy case for latency and a noisy one for recall: its neighbours in a topic are
        // near-ties.)
        texts.push(
            sqlx::query_scalar(
                "SELECT l2_normalize(avg(embedding::vector)::halfvec)::text
                 FROM (SELECT embedding FROM asset_embeddings ORDER BY random() LIMIT 40) s",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
        );
    }
    let queries: Vec<Vec<f32>> = texts
        .iter()
        .map(|t| {
            t.trim_matches(['[', ']'])
                .split(',')
                .map(|x| x.parse().unwrap())
                .collect()
        })
        .collect();

    let scenarios: [(&str, &str); 11] = [
        ("no filter (HNSW)", ""),
        ("favorites, 5k (exact)", "&favorite=true"),
        ("one year, ~6k (exact)", "&from=2022-01-01&to=2022-12-31"),
        ("one camera, ~17k (HNSW, filtered)", "&camera=Apple%20iPhone%2014%20Pro"),
        (
            "camera + year, ~2k (exact)",
            "&camera=Apple%20iPhone%2014%20Pro&from=2022-01-01&to=2022-12-31",
        ),
        ("a city, 1.25k (exact)", "&place=City%206"),
        ("an album of 40 (exact)", "&album=00000000-0000-0000-0000-0000000000b1"),
        ("has location, 25k (HNSW, filtered)", "&has_location=true"),
        ("jpeg only, ~25k (HNSW, filtered)", "&type=jpeg"),
        ("heic, ~12k (HNSW, filtered)", "&type=heic"),
        ("favorites + has location (exact)", "&favorite=true&has_location=true"),
    ];
    println!(
        "\n{:<40} {:>8} {:>8} {:>8} {:>10}",
        "scenario (ranked, limit 60)", "p50 ms", "p95 ms", "max ms", "plan"
    );
    let only = std::env::var("PERF_ONLY").unwrap_or_default();
    for (name, extra) in scenarios {
        if !name.contains(&only) {
            continue;
        }
        let q: SearchQuery = serde_urlencoded_like(&format!("q=x{extra}&cutoff=false"));
        let mut lat = Vec::new();
        let mut plan = Plan::Timeline;
        for v in &queries {
            *fixed.0.lock().unwrap() = v.clone();
            let t = Instant::now();
            let (resp, p) = super::search::run(&state, owner, &q).await.unwrap();
            lat.push(t.elapsed().as_secs_f64() * 1000.0);
            plan = p;
            assert!(!resp.items.is_empty() || name.contains("camera + year"));
        }
        lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |p: f64| lat[((lat.len() as f64 * p) as usize).min(lat.len() - 1)];
        println!(
            "{name:<40} {:>8.1} {:>8.1} {:>8.1} {:>10?}",
            pct(0.5),
            pct(0.95),
            lat[lat.len() - 1],
            plan
        );
    }

    // Recall of the HNSW plan against exact distances (the exact plan with the same filters), over the top 10 / 60 / 300,
    // unfiltered and with filters that leave 20,000+ photos (where the HNSW plan is what runs).
    for (label, extra) in [
        ("no filter", ""),
        ("has location (50%)", "&has_location=true"),
        ("one camera (33%)", "&camera=Apple%20iPhone%2014%20Pro"),
    ] {
        let f = Filters::parse(&serde_urlencoded_like(&format!("q=x{extra}"))).unwrap();
        let (mut r10, mut r60, mut r300) = (0.0, 0.0, 0.0);
        for v in &queries {
            let fetch = |plan: Plan| {
                let pool = pool.clone();
                let f = f.clone();
                let v = v.clone();
                async move {
                    let mut got = Vec::new();
                    let mut after = None;
                    loop {
                        let page = ranked_rows_planned(&pool, owner, &f, model, &v, 100, after, Some(plan))
                            .await
                            .unwrap();
                        got.extend(page.rows.iter().map(|r| r.row.summary.id));
                        match page.rows.last() {
                            Some(r) if page.rows.len() == 100 => {
                                after = Some(After {
                                    dist: r.dist.unwrap(),
                                    id: r.row.summary.id,
                                    k: page.k,
                                })
                            }
                            _ => break,
                        }
                    }
                    got
                }
            };
            let (got, exact) = (fetch(Plan::Hnsw).await, fetch(Plan::Exact).await);
            let overlap = |k: usize| {
                got.iter()
                    .take(k)
                    .filter(|id| exact[..k.min(exact.len())].contains(id))
                    .count() as f64
                    / k as f64
            };
            r10 += overlap(10);
            r60 += overlap(60);
            r300 += overlap(exact.len().min(300));
        }
        let n = queries.len() as f64;
        println!(
            "HNSW recall vs exact, {label:<20} {} queries: @10 {:.3}  @60 {:.3}  @300 {:.3}",
            queries.len(),
            r10 / n,
            r60 / n,
            r300 / n
        );
    }
}

/// `GET /assets/{id}/similar` on the same synthetic library: the whole request path for 40 random photos as the source, with
/// and without filters (the text model is not involved). Needs the seeded perf database, like the search measurement above.
#[tokio::test]
#[ignore = "needs the seeded perf database"]
async fn similar_performance_on_50k_photos() {
    use std::time::Instant;
    let Ok(url) = std::env::var("PERF_DATABASE_URL") else {
        return;
    };
    let pool = cyanotype_core::db::connect(&url).await.unwrap();
    let owner: Uuid = "00000000-0000-0000-0000-0000000000a1".parse().unwrap();
    let state = crate::AppState {
        db: pool.clone(),
        storage: crate::testutil::dummy_storage(),
        trash_retention_days: 30,
        purge_wake: Default::default(),
        text: Arc::new(crate::testutil::FakeText::default()),
        // The seed script writes CLIP vectors.
        search: Arc::new(crate::search::SearchState::uncached(cyanotype_clip::Model::ClipB32, 95)),
    };
    let sources: Vec<Uuid> =
        sqlx::query_scalar("SELECT asset_id FROM (SELECT asset_id FROM asset_embeddings ORDER BY random() LIMIT 40) s")
            .fetch_all(&pool)
            .await
            .unwrap();
    println!(
        "\n{:<40} {:>8} {:>8} {:>8} {:>10}",
        "More like this (limit 60)", "p50 ms", "p95 ms", "max ms", "plan"
    );
    for (name, extra) in [
        ("no filter (HNSW)", ""),
        ("favorites, 5k (exact)", "&favorite=true"),
        ("one camera, ~17k (HNSW, filtered)", "&camera=Apple%20iPhone%2014%20Pro"),
        ("one year, ~6k (exact)", "&from=2022-01-01&to=2022-12-31"),
    ] {
        let uri: axum::http::Uri = format!("/x?cutoff=false{extra}").parse().unwrap();
        let q = axum::extract::Query::<super::similar::SimilarQuery>::try_from_uri(&uri)
            .unwrap()
            .0;
        let mut lat = Vec::new();
        let mut plan = Plan::Timeline;
        for id in &sources {
            let t = Instant::now();
            let (_, p) = super::similar::run(&state, owner, *id, &q).await.unwrap();
            lat.push(t.elapsed().as_secs_f64() * 1000.0);
            plan = p;
        }
        lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |p: f64| lat[((lat.len() as f64 * p) as usize).min(lat.len() - 1)];
        println!(
            "{name:<40} {:>8.1} {:>8.1} {:>8.1} {:>10?}",
            pct(0.5),
            pct(0.95),
            lat[lat.len() - 1],
            plan
        );
    }
}

/// `a=1&b=2` into a `SearchQuery`, for tests that build queries as strings.
fn serde_urlencoded_like(qs: &str) -> SearchQuery {
    let uri: axum::http::Uri = format!("/x?{qs}").parse().unwrap();
    axum::extract::Query::<SearchQuery>::try_from_uri(&uri).unwrap().0
}
