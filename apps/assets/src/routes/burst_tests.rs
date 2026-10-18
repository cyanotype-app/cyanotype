//! Bursts in search and "More like this" over a real Postgres: a burst's photos collapse into one result that ranks by its
//! best photo, the tiers and the pages count groups, a favorite is the cover, members are listed behind the cover, filters
//! apply to the members too, "More like this" leaves out the source's own burst, and a filter-only search lists every photo.
//! `burst_id` is set by hand here: the grouping itself is `crates/cyanotype-core/tests/bursts.rs`.
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::search_v2_tests::{ids, photo};
use crate::testutil::{TEST_MODEL, TestDb, app, send, user};

async fn set(pool: &PgPool, id: Uuid, column_sql: &str) {
    sqlx::query(&format!("UPDATE assets SET {column_sql} WHERE id = $1"))
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

/// Put the photos in one burst (the first one's id names it, as the worker does).
async fn burst(pool: &PgPool, members: &[Uuid]) -> Uuid {
    let id = members[0];
    for m in members {
        sqlx::query("UPDATE assets SET burst_id = $2, burst_version = 1 WHERE id = $1")
            .bind(m)
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }
    id
}

async fn get(app: &axum::Router, key: &str, path: &str) -> (StatusCode, Value) {
    send(app, "GET", path, key, None).await
}

fn member_ids(item: &Value) -> Vec<Uuid> {
    item["burst"]["members"]
        .as_array()
        .map(|m| m.iter().map(|i| i["id"].as_str().unwrap().parse().unwrap()).collect())
        .unwrap_or_default()
}

/// Query "cat". Photos: `a` the best cat, a burst of three slightly worse cats `b1 > b2 > b3`, `c` a weaker cat, `d` a dog.
/// Cosines to the query: a 1.0, b* about 0.995 to 0.98, c about 0.95, d about 0.
async fn library(t: &TestDb) -> (String, Uuid, [Uuid; 3], Uuid, Uuid) {
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let m = TEST_MODEL;
    let a = photo(&t.pool, alice, "a.jpg", m, &[("cat", 1.0)]).await;
    let b1 = photo(&t.pool, alice, "b1.jpg", m, &[("cat", 1.0), ("dog", 0.08)]).await;
    let b2 = photo(&t.pool, alice, "b2.jpg", m, &[("cat", 1.0), ("dog", 0.12)]).await;
    let b3 = photo(&t.pool, alice, "b3.jpg", m, &[("cat", 1.0), ("dog", 0.16)]).await;
    let c = photo(&t.pool, alice, "c.jpg", m, &[("cat", 1.0), ("dog", 0.4)]).await;
    let d = photo(&t.pool, alice, "d.jpg", m, &[("dog", 1.0)]).await;
    burst(&t.pool, &[b1, b2, b3]).await;
    (key, a, [b1, b2, b3], c, d)
}

#[tokio::test]
async fn a_burst_is_one_result_ranked_by_its_best_photo_with_the_others_behind_it() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (key, a, [b1, b2, b3], c, d) = library(&t).await;

    let (st, body) = get(&app, &key, "/assets/search?q=cat&cutoff=false").await;
    assert_eq!(st, StatusCode::OK);
    // Five results for six photos: a, the burst (at the place of b1), c, d.
    assert_eq!(ids(&body), [a, b1, c, d]);
    let items = body["items"].as_array().unwrap();
    assert!(items[0].get("burst").is_none() && items[2].get("burst").is_none());
    assert_eq!(items[1]["burst"]["size"], json!(3));
    assert_eq!(
        member_ids(&items[1]),
        [b2, b3],
        "the other photos of the burst, best first"
    );
    assert!(items[1]["burst"]["members"][0]["score"].as_f64().unwrap() > 0.97);
    assert!(
        items[1]["burst"]["members"][0]["thumb_url"].is_null()
            || items[1]["burst"]["members"][0]["thumb_url"].is_string()
    );
    t.teardown().await;
}

#[tokio::test]
async fn tiers_and_pages_count_groups_not_photos() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (key, a, [b1, ..], c, _) = library(&t).await;

    // Strong: within 0.06 of the best, so a and the burst (three photos, one group); c (0.95) is weaker.
    let (_, body) = get(&app, &key, "/assets/search?q=cat").await;
    assert_eq!(ids(&body), [a, b1]);
    assert_eq!(body["strong_count"], json!(2), "groups");
    assert_eq!(body["weaker"]["count"], json!(1));
    assert_eq!(body["items"][1]["tier"], json!("strong"));

    // One group per page.
    let (_, p1) = get(&app, &key, "/assets/search?q=cat&limit=1&weaker=true").await;
    assert_eq!(ids(&p1), [a]);
    let cur = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = get(
        &app,
        &key,
        &format!("/assets/search?q=cat&limit=1&weaker=true&cursor={cur}"),
    )
    .await;
    assert_eq!(ids(&p2), [b1]);
    assert_eq!(p2["items"][0]["burst"]["size"], json!(3));
    let cur = p2["next_cursor"].as_str().unwrap();
    let (_, p3) = get(
        &app,
        &key,
        &format!("/assets/search?q=cat&limit=1&weaker=true&cursor={cur}"),
    )
    .await;
    assert_eq!(ids(&p3), [c]);
    assert_eq!(p3["items"][0]["tier"], json!("weak"));
    assert_eq!(p3["next_cursor"], Value::Null);

    // The weaker cursor after the last strong group.
    let (_, s) = get(&app, &key, "/assets/search?q=cat&limit=2").await;
    let cur = s["weaker"]["cursor"].as_str().expect("after the last strong group");
    let (_, w) = get(&app, &key, &format!("/assets/search?q=cat&weaker=true&cursor={cur}")).await;
    assert_eq!(ids(&w), [c]);
    t.teardown().await;
}

#[tokio::test]
async fn a_favorite_is_the_cover_and_a_burst_keeps_the_tier_of_its_best_photo() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (key, a, [b1, b2, b3], ..) = library(&t).await;
    set(&t.pool, b3, "favorite = true").await;

    let (_, body) = get(&app, &key, "/assets/search?q=cat").await;
    assert_eq!(ids(&body), [a, b3], "the favorite is shown for its burst");
    assert_eq!(member_ids(&body["items"][1]), [b1, b2]);
    assert_eq!(body["items"][1]["tier"], json!("strong"));
    assert_eq!(body["items"][1]["favorite"], json!(true));
    t.teardown().await;
}

#[tokio::test]
async fn filters_apply_to_the_members_of_a_burst_and_hidden_or_trashed_ones_are_not_shown() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (key, _, [b1, b2, b3], ..) = library(&t).await;

    set(&t.pool, b2, "favorite = true").await;
    let (_, body) = get(&app, &key, "/assets/search?q=cat&favorite=true&cutoff=false").await;
    assert_eq!(
        ids(&body),
        [b2],
        "one favorite is left of the burst: a result of its own"
    );
    assert!(body["items"][0].get("burst").is_none());

    set(&t.pool, b3, "deleted_at = now()").await;
    let (_, body) = get(&app, &key, "/assets/search?q=cat&cutoff=false").await;
    // b2 is still a favorite, so it covers what is left of the burst.
    let burst_item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!(b2.to_string()))
        .unwrap();
    assert_eq!(
        burst_item["burst"]["size"],
        json!(2),
        "the trashed photo is not counted"
    );
    assert_eq!(member_ids(burst_item), [b1]);
    t.teardown().await;
}

#[tokio::test]
async fn more_like_this_leaves_out_the_sources_own_burst() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (key, a, [b1, b2, b3], c, _) = library(&t).await;
    // A second burst of look-alikes of `a`.
    let (alice,): (Uuid,) = sqlx::query_as("SELECT owner_id FROM assets WHERE id = $1")
        .bind(a)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let e1 = photo(&t.pool, alice, "e1.jpg", TEST_MODEL, &[("cat", 1.0), ("dog", 0.05)]).await;
    let e2 = photo(&t.pool, alice, "e2.jpg", TEST_MODEL, &[("cat", 1.0), ("dog", 0.06)]).await;
    burst(&t.pool, &[e1, e2]).await;

    // Similar to b2: its own burst (b1, b3) is not listed, another burst is one result.
    let (st, body) = get(&app, &key, &format!("/assets/{b2}/similar?cutoff=false")).await;
    assert_eq!(st, StatusCode::OK);
    let got = ids(&body);
    for left_out in [b1, b2, b3] {
        assert!(!got.contains(&left_out), "{left_out} is the source or in its burst");
    }
    assert!(got.contains(&a) && got.contains(&c));
    let e_item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["burst"]["size"] == json!(2))
        .expect("the other burst");
    let mut e = vec![e_item["id"].as_str().unwrap().parse::<Uuid>().unwrap()];
    e.extend(member_ids(e_item));
    e.sort();
    let mut want = vec![e1, e2];
    want.sort();
    assert_eq!(e, want);
    t.teardown().await;
}

#[tokio::test]
async fn a_filter_only_search_lists_every_photo_and_never_collapses() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (key, ..) = library(&t).await;
    let (_, body) = get(&app, &key, "/assets/search?favorite=false").await;
    assert_eq!(body["items"].as_array().unwrap().len(), 6);
    assert_eq!(body["total"], json!(6));
    assert!(
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i.get("burst").is_none())
    );
    t.teardown().await;
}
