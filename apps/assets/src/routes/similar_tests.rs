//! "More like this" (`GET /assets/{id}/similar`) over a real Postgres: ranking by the photo's own vector, what is left out
//! (the source, other people's photos, the trash, the hidden half of a pair), the filters, paging, the tiers, the model the
//! list is answered with, and a photo that has no vector yet. Fake vectors again (`testutil`).
use std::sync::Arc;

use axum::http::StatusCode;
use cyanotype_clip::Model;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::search::EXACT_MAX_OVERRIDE;
use super::search_v2_tests::{ids, photo};
use crate::{
    search::SearchState,
    testutil::{FakeText, TEST_MODEL, TestDb, app, app_with, send, set_embedding_for, user},
};

async fn set(pool: &PgPool, id: Uuid, column_sql: &str) {
    sqlx::query(&format!("UPDATE assets SET {column_sql} WHERE id = $1"))
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

async fn similar(app: &axum::Router, key: &str, id: Uuid, query: &str) -> (StatusCode, Value) {
    send(app, "GET", &format!("/assets/{id}/similar?{query}"), key, None).await
}

/// The whole ranked list, without the cut-off (which has its own tests below), as the search tests do.
async fn all(app: &axum::Router, key: &str, id: Uuid, query: &str) -> (StatusCode, Value) {
    similar(app, key, id, &format!("{query}&cutoff=false")).await
}

/// A source photo and four others at falling similarity to it (cosine about 0.995, 0.86, 0.71 and 0.01).
async fn ladder(t: &TestDb, model: Model) -> (Uuid, String, Uuid, [Uuid; 4]) {
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let source = photo(&t.pool, alice, "source.jpg", model, &[("cat", 1.0)]).await;
    let near = photo(&t.pool, alice, "near.jpg", model, &[("cat", 1.0), ("dog", 0.1)]).await;
    let mid = photo(&t.pool, alice, "mid.jpg", model, &[("cat", 1.0), ("dog", 0.6)]).await;
    let far = photo(&t.pool, alice, "far.jpg", model, &[("cat", 1.0), ("dog", 1.0)]).await;
    let other = photo(&t.pool, alice, "other.jpg", model, &[("dog", 1.0)]).await;
    (alice, key, source, [near, mid, far, other])
}

#[tokio::test]
async fn photos_come_back_by_closeness_to_the_source_and_never_include_it() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, source, [near, mid, far, other]) = ladder(&t, TEST_MODEL).await;

    let (st, body) = all(&app, &key, source, "").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ids(&body), [near, mid, far, other]);
    assert_eq!(body["similar_to"], json!(source.to_string()));
    assert_eq!(body["ranked"], json!(true));
    assert_eq!(body["model"], json!("siglip2-b16"));
    assert_eq!(body["parsed"], Value::Null);
    let score = |i: usize| body["items"][i]["score"].as_f64().unwrap();
    assert!(score(0) > 0.99 && score(1) > 0.82 && score(1) < 0.9 && score(2) > 0.68 && score(2) < 0.74);
    assert!(score(0) > score(1) && score(1) > score(2) && score(2) > score(3));
    // Image to image: the sigmoid head's probability does not apply.
    assert_eq!(body["items"][0]["probability"], Value::Null);
    // The item shape is the library's (thumbhash, thumb_url, paired, favorite) plus score and tier.
    assert!(body["items"][0]["thumbhash"].is_null() && body["items"][0]["paired"] == json!([]));
    t.teardown().await;
}

#[tokio::test]
async fn both_plans_answer_the_same_and_leave_the_source_out() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, source, [near, mid, far, other]) = ladder(&t, TEST_MODEL).await;
    for exact_max in [None, Some(0)] {
        EXACT_MAX_OVERRIDE.with(|c| c.set(exact_max));
        let (_, body) = all(&app, &key, source, "").await;
        assert_eq!(ids(&body), [near, mid, far, other], "exact_max {exact_max:?}");
    }
    EXACT_MAX_OVERRIDE.with(|c| c.set(None));
    t.teardown().await;
}

#[tokio::test]
async fn other_peoples_photos_trash_and_the_hidden_half_of_a_pair_are_left_out() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key, source, [near, ..]) = ladder(&t, TEST_MODEL).await;
    let (bob, bob_key) = user(&t.pool, "b@example.com").await;
    let theirs = photo(&t.pool, bob, "theirs.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    let trashed = photo(&t.pool, alice, "trashed.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    let purging = photo(&t.pool, alice, "purging.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    // The source's own RAW: a hidden half of the pair, with a vector of its own that is identical to the source's.
    let raw = photo(&t.pool, alice, "source.cr2", TEST_MODEL, &[("cat", 1.0)]).await;
    set(&t.pool, trashed, "deleted_at = now()").await;
    set(&t.pool, purging, "deleted_at = now(), purging_at = now()").await;
    set(&t.pool, raw, &format!("primary_id = '{source}'")).await;

    for exact_max in [None, Some(0)] {
        EXACT_MAX_OVERRIDE.with(|c| c.set(exact_max));
        let (_, body) = all(&app, &key, source, "").await;
        let got = ids(&body);
        assert_eq!(got.len(), 4, "{got:?}");
        assert_eq!(got[0], near);
        for hidden in [source, theirs, trashed, purging, raw] {
            assert!(!got.contains(&hidden), "{hidden} must not be listed");
        }
    }
    EXACT_MAX_OVERRIDE.with(|c| c.set(None));

    // Not the caller's photo, a trashed one, a hidden RAW, a photo that does not exist: all 404.
    for id in [theirs, trashed, raw, Uuid::new_v4()] {
        let (st, _) = similar(&app, &key, id, "").await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{id}");
    }
    // And Bob cannot ask about Alice's photo either.
    let (st, _) = similar(&app, &bob_key, source, "").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, body) = all(&app, &bob_key, theirs, "").await;
    assert_eq!((st, ids(&body)), (StatusCode::OK, vec![]), "Bob has nothing else");
    // Unauthenticated.
    let (st, _) = similar(&app, "nope", source, "").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

#[tokio::test]
async fn the_search_filters_narrow_the_list() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key, source, [near, mid, far, other]) = ladder(&t, TEST_MODEL).await;
    set(&t.pool, near, "favorite = true, taken_at = '2020-06-15T12:00:00Z', camera_make = 'Canon', camera_model = 'Canon EOS R5', place_city = 'Lisbon', latitude = 38.7, longitude = -9.1").await;
    set(
        &t.pool,
        mid,
        "taken_at = '2024-03-01T08:00:00Z', camera_make = 'Apple', camera_model = 'iPhone 15'",
    )
    .await;
    set(
        &t.pool,
        far,
        "taken_at = '2024-03-02T08:00:00Z', original_name = 'far.CR3', content_type = 'image/x-canon-cr3'",
    )
    .await;
    set(&t.pool, other, "taken_at = '2019-01-01T00:00:00Z'").await;
    let album: Uuid = sqlx::query_scalar("INSERT INTO albums (owner_id, title) VALUES ($1, 'x') RETURNING id")
        .bind(alice)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO album_assets (album_id, asset_id) VALUES ($1, $2)")
        .bind(album)
        .bind(other)
        .execute(&t.pool)
        .await
        .unwrap();

    let only = |q: &'static str, want: Vec<Uuid>| {
        let (app, key) = (app.clone(), key.clone());
        async move {
            let (st, body) = all(&app, &key, source, q).await;
            assert_eq!(st, StatusCode::OK, "{q}");
            assert_eq!(ids(&body), want, "{q}");
        }
    };
    only("favorite=true", vec![near]).await;
    only("camera=canon%20eos%20r5", vec![near]).await;
    only("place=lisbon", vec![near]).await;
    only("has_location=true", vec![near]).await;
    only("from=2024-01-01", vec![mid, far]).await;
    only("from=2024-01-01&to=2024-03-01", vec![mid]).await;
    only("type=raw", vec![far]).await;
    only("favorite=true&from=2024-01-01", vec![]).await;
    let (_, body) = all(&app, &key, source, &format!("album={album}")).await;
    assert_eq!(ids(&body), [other]);
    // The same with the HNSW plan.
    EXACT_MAX_OVERRIDE.with(|c| c.set(Some(0)));
    only("favorite=true", vec![near]).await;
    only("from=2024-01-01", vec![mid, far]).await;
    EXACT_MAX_OVERRIDE.with(|c| c.set(None));
    // A bad filter is a 400, as in search.
    let (st, _) = similar(&app, &key, source, "from=yesterday").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = similar(&app, &key, source, "cursor=zz").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    t.teardown().await;
}

#[tokio::test]
async fn paging_walks_the_list_one_result_at_a_time() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, source, [near, mid, far, other]) = ladder(&t, TEST_MODEL).await;
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..6 {
        let q = match &cursor {
            Some(c) => format!("limit=1&cursor={c}"),
            None => "limit=1".to_string(),
        };
        let (st, page) = all(&app, &key, source, &q).await;
        assert_eq!(st, StatusCode::OK);
        seen.extend(ids(&page));
        cursor = page["next_cursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, [near, mid, far, other]);
    t.teardown().await;
}

/// The cut-off is the image rules' (`cutoff.rs`): the strong tier is what the list shows, the weaker one is offered.
#[tokio::test]
async fn the_list_is_cut_into_strong_and_weaker_matches() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, source, [near, mid, far, other]) = ladder(&t, TEST_MODEL).await;

    let (_, body) = similar(&app, &key, source, "").await;
    let tiers = |b: &Value| -> Vec<String> {
        b["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["tier"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids(&body), [near], "only the near-identical photo is strong");
    assert_eq!(tiers(&body), ["strong"]);
    assert_eq!(body["strong_count"], json!(1));
    assert_eq!(body["weaker"]["count"], json!(1), "the 0.86 one is weaker");
    let cursor = body["weaker"]["cursor"]
        .as_str()
        .expect("a cursor after the last strong match");

    let (_, more) = similar(&app, &key, source, &format!("weaker=true&cursor={cursor}")).await;
    assert_eq!(ids(&more), [mid]);
    assert_eq!(tiers(&more), ["weak"]);
    let (_, both) = similar(&app, &key, source, "weaker=true").await;
    assert_eq!(ids(&both), [near, mid]);
    assert_eq!(tiers(&both), ["strong", "weak"]);
    // The 0.71 one and the unrelated photo are neither.
    let (_, everything) = all(&app, &key, source, "").await;
    assert_eq!(ids(&everything), [near, mid, far, other]);
    t.teardown().await;
}

#[tokio::test]
async fn a_photo_without_a_vector_for_the_serving_model_answers_409() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alice, key, _, _) = ladder(&t, TEST_MODEL).await;
    // Uploaded, still waiting in the embed queue.
    let fresh = photo(&t.pool, alice, "fresh.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    sqlx::query("DELETE FROM asset_embeddings WHERE asset_id = $1")
        .bind(fresh)
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, body) = similar(&app, &key, fresh, "").await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert!(
        body["error"].as_str().unwrap().contains("still being indexed"),
        "{body}"
    );
    // An out-of-date version of the right model does not count either.
    sqlx::query("INSERT INTO asset_embeddings (asset_id, model, version, embedding) VALUES ($1, $2, 0, array_fill(0.1::real, ARRAY[$3])::vector::halfvec)")
        .bind(fresh)
        .bind(TEST_MODEL.id())
        .bind(TEST_MODEL.dim() as i32)
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, _) = similar(&app, &key, fresh, "").await;
    assert_eq!(st, StatusCode::CONFLICT);
    t.teardown().await;
}

/// A library being re-indexed: CLIP answers until SigLIP 2 covers 95 %. The list is made of CLIP vectors alone, and a photo
/// that only SigLIP 2 has embedded yet (a new upload) is "still being indexed" for the time being.
#[tokio::test]
async fn the_model_that_answers_text_searches_answers_here_too_and_vectors_of_two_models_are_never_mixed() {
    let Some(t) = TestDb::new().await else { return };
    let app = app_with(
        &t.pool,
        Arc::new(SearchState::uncached(Model::Siglip2B16, 95)),
        Arc::new(FakeText::default()),
    );
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let clip = Model::ClipB32;
    let sig = Model::Siglip2B16;
    let source = photo(&t.pool, alice, "s.jpg", clip, &[("cat", 1.0)]).await;
    let a = photo(&t.pool, alice, "a.jpg", clip, &[("cat", 1.0), ("dog", 0.1)]).await;
    let b = photo(&t.pool, alice, "b.jpg", clip, &[("cat", 1.0), ("dog", 0.5)]).await;
    let c = photo(&t.pool, alice, "c.jpg", clip, &[("dog", 1.0)]).await;
    let d = photo(&t.pool, alice, "d.jpg", clip, &[("dog", 1.0), ("car", 0.2)]).await;
    // Uploaded after the switch began: only the new model has embedded it.
    let fresh = photo(&t.pool, alice, "fresh.jpg", sig, &[("cat", 1.0)]).await;
    // SigLIP 2 knows three of the others, and orders them differently (b nearer than a).
    set_embedding_for(&t.pool, source, sig, &[("cat", 1.0)]).await;
    set_embedding_for(&t.pool, a, sig, &[("cat", 1.0), ("dog", 0.9)]).await;
    set_embedding_for(&t.pool, b, sig, &[("cat", 1.0), ("dog", 0.05)]).await;
    for id in [source, a, b, c, d, fresh] {
        set(&t.pool, id, "thumbhash = '\\x01'").await;
    }

    // CLIP 5 of 6, SigLIP 2 4 of 6: CLIP answers, in CLIP's order, over the five photos it knows.
    let (st, body) = all(&app, &key, source, "").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["model"], json!("clip-b32"));
    assert_eq!(ids(&body), [a, b, c, d]);
    assert_eq!(body["reindexing"]["percent"], json!(66));
    // The new upload has no CLIP vector: not yet.
    let (st, _) = similar(&app, &key, fresh, "").await;
    assert_eq!(st, StatusCode::CONFLICT);

    // SigLIP 2 reaches 100 %: it answers, in its own order, and CLIP's vectors are not in the list.
    set_embedding_for(&t.pool, c, sig, &[("dog", 1.0)]).await;
    set_embedding_for(&t.pool, d, sig, &[("dog", 1.0), ("car", 0.2)]).await;
    let (_, body) = all(&app, &key, source, "").await;
    assert_eq!(body["model"], json!("siglip2-b16"));
    assert_eq!(body["reindexing"], Value::Null);
    assert_eq!(ids(&body), [fresh, b, a, c, d]);
    t.teardown().await;
}
