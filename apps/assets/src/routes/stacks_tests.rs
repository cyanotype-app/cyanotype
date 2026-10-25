//! Stacks over a real Postgres (skipped without `DATABASE_URL`): the timeline and its day counts show one tile per stack
//! (an automatic burst or one made by hand) and agree with each other, the setting and `?stacks=` switch it off, favorites
//! and the archive never collapse, the controls (stack, cover, remove, dissolve) are owner-scoped, and what the owner decided
//! is kept in columns the burst sweep does not write. The sweep itself is `crates/cyanotype-core/tests/bursts.rs`.
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::search_v2_tests::photo as vector_photo;
use crate::testutil::{TEST_MODEL, TestDb, app, send, user};

/// A live photo taken at `taken` (RFC 3339).
async fn photo(pool: &PgPool, owner: Uuid, name: &str, taken: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, width, height, taken_at)
         VALUES ($1, sha256($2::bytea), $3, $2, 'image/jpeg', 10, 10, 10, $4::timestamptz) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(format!("originals/{owner}/{name}"))
    .bind(taken)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// What the worker stores for a burst: the first photo's id on every member.
async fn burst(pool: &PgPool, members: &[Uuid]) {
    for m in members {
        sqlx::query("UPDATE assets SET burst_id = $2, burst_version = 1 WHERE id = $1")
            .bind(m)
            .bind(members[0])
            .execute(pool)
            .await
            .unwrap();
    }
}

async fn get(app: &axum::Router, key: &str, path: &str) -> (StatusCode, Value) {
    send(app, "GET", path, key, None).await
}

fn names(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["original_name"].as_str().unwrap().to_string())
        .collect()
}

/// The day counts' total, which must equal the list's length whatever the setting.
async fn total(app: &axum::Router, key: &str, query: &str) -> i64 {
    get(app, key, &format!("/assets/timeline{query}")).await.1["total"]
        .as_i64()
        .unwrap()
}

/// Owner "alpha": a burst of three (b1 b2 b3, a second apart), and two photos on their own (s1 before, s2 after).
async fn library(t: &TestDb) -> (Uuid, String, [Uuid; 3], [Uuid; 2]) {
    let (alpha, key) = user(&t.pool, "alpha@example.com").await;
    let s1 = photo(&t.pool, alpha, "s1.jpg", "2026-03-01T09:00:00Z").await;
    let b1 = photo(&t.pool, alpha, "b1.jpg", "2026-03-02T10:00:00Z").await;
    let b2 = photo(&t.pool, alpha, "b2.jpg", "2026-03-02T10:00:01Z").await;
    let b3 = photo(&t.pool, alpha, "b3.jpg", "2026-03-02T10:00:02Z").await;
    let s2 = photo(&t.pool, alpha, "s2.jpg", "2026-03-03T09:00:00Z").await;
    burst(&t.pool, &[b1, b2, b3]).await;
    (alpha, key, [b1, b2, b3], [s1, s2])
}

#[tokio::test]
async fn the_timeline_shows_a_burst_as_one_tile_with_its_cover_and_the_counts_agree() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [b1, b2, b3], _) = library(&t).await;

    let (st, list) = get(&app, &key, "/assets").await;
    assert_eq!(st, StatusCode::OK);
    // Newest first: s2, the burst (its earliest photo is the cover), s1.
    assert_eq!(names(&list), ["s2.jpg", "b1.jpg", "s1.jpg"]);
    let tile = &list["items"][1];
    assert_eq!(tile["stack"]["size"], json!(3));
    let members: Vec<Uuid> = tile["stack"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().parse().unwrap())
        .collect();
    let mut sorted = members.clone();
    sorted.sort();
    let mut want = vec![b1, b2, b3];
    want.sort();
    assert_eq!(sorted, want, "every photo of the stack, the cover included");
    assert_eq!(members.len(), 3);
    assert!(list["items"][0].get("stack").is_none());
    assert_eq!(total(&app, &key, "").await, 3, "the day counts count tiles");
    let (_, days) = get(&app, &key, "/assets/timeline").await;
    assert_eq!(days["days"][1], json!({"day": "2026-03-02", "count": 1}));

    // Expanded: every photo, each member still knows its stack (no member list: every photo is listed anyway).
    let (_, all) = get(&app, &key, "/assets?stacks=expanded").await;
    assert_eq!(names(&all), ["s2.jpg", "b3.jpg", "b2.jpg", "b1.jpg", "s1.jpg"]);
    assert_eq!(all["items"][1]["stack"], json!({"size": 3}));
    assert_eq!(total(&app, &key, "?stacks=expanded").await, 5);
    t.teardown().await;
}

#[tokio::test]
async fn paging_a_collapsed_timeline_has_no_gaps_and_no_repeats() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, ..) = library(&t).await;
    let (_, p1) = get(&app, &key, "/assets?limit=2").await;
    assert_eq!(names(&p1), ["s2.jpg", "b1.jpg"]);
    let cur = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = get(&app, &key, &format!("/assets?limit=2&cursor={cur}")).await;
    assert_eq!(names(&p2), ["s1.jpg"]);
    t.teardown().await;
}

#[tokio::test]
async fn the_setting_turns_collapsing_off_and_a_request_can_override_it() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, ..) = library(&t).await;

    let (_, prefs) = get(&app, &key, "/assets/prefs").await;
    assert_eq!(prefs["stacks_collapsed"], json!(true), "collapsed by default");
    let (st, prefs) = send(
        &app,
        "PUT",
        "/assets/prefs",
        &key,
        Some(json!({"stacks_collapsed": false})),
    )
    .await;
    assert_eq!((st, &prefs["stacks_collapsed"]), (StatusCode::OK, &json!(false)));
    assert_eq!(prefs["memories_enabled"], json!(true), "the other settings are kept");

    assert_eq!(total(&app, &key, "").await, 5);
    assert_eq!(get(&app, &key, "/assets").await.1["items"].as_array().unwrap().len(), 5);
    assert_eq!(total(&app, &key, "?stacks=collapsed").await, 3);
    let (_, list) = get(&app, &key, "/assets?stacks=collapsed").await;
    assert_eq!(names(&list), ["s2.jpg", "b1.jpg", "s1.jpg"]);
    t.teardown().await;
}

#[tokio::test]
async fn favorites_the_archive_and_albums_list_every_photo() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [b1, b2, b3], _) = library(&t).await;
    let (st, _) = send(
        &app,
        "POST",
        "/assets/favorite",
        &key,
        Some(json!({"ids": [b1, b2, b3], "favorite": true})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, fav) = get(&app, &key, "/assets?favorite=true").await;
    assert_eq!(fav["items"].as_array().unwrap().len(), 3, "favorites never collapse");
    assert!(fav["items"][0].get("stack").is_none());
    assert_eq!(total(&app, &key, "?favorite=true").await, 3);

    // An archived member leaves the stack's tile count; the archive lists it plainly.
    send(
        &app,
        "POST",
        "/assets/archive",
        &key,
        Some(json!({"ids": [b3], "archived": true})),
    )
    .await;
    let (_, list) = get(&app, &key, "/assets").await;
    assert_eq!(list["items"][1]["stack"]["size"], json!(2));
    let (_, arch) = get(&app, &key, "/assets?archived=true").await;
    assert_eq!(names(&arch), ["b3.jpg"]);
    assert!(arch["items"][0].get("stack").is_none());

    // An album lists all of a stack's photos.
    let (_, album) = send(&app, "POST", "/assets/albums", &key, Some(json!({"title": "Trip"}))).await;
    let aid = album["id"].as_str().unwrap();
    send(
        &app,
        "POST",
        &format!("/assets/albums/{aid}/assets"),
        &key,
        Some(json!({"ids": [b1, b2]})),
    )
    .await;
    let (_, inside) = get(&app, &key, &format!("/assets/albums/{aid}/assets")).await;
    assert_eq!(names(&inside).len(), 2);
    t.teardown().await;
}

#[tokio::test]
async fn stacking_by_hand_makes_one_tile_with_the_first_photo_as_cover() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, _, [s1, s2]) = library(&t).await;

    let (st, r) = send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": [s2, s1]}))).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(
        (r["size"].as_i64(), r["cover_id"].as_str()),
        (Some(2), Some(s2.to_string().as_str()))
    );
    let (_, list) = get(&app, &key, "/assets").await;
    // The burst, then the stack at its cover's date (s2); s1 is behind it.
    assert_eq!(names(&list), ["s2.jpg", "b1.jpg"]);
    assert_eq!(list["items"][0]["stack"]["size"], json!(2));
    assert_eq!(total(&app, &key, "").await, 2);

    // The viewer's list: both photos in capture order, the cover marked, made by hand.
    let (st, stack) = get(&app, &key, &format!("/assets/{s1}/stack")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(names(&stack), ["s1.jpg", "s2.jpg"]);
    assert_eq!(
        (stack["cover_id"].as_str(), &stack["manual"]),
        (Some(s2.to_string().as_str()), &json!(true))
    );
    t.teardown().await;
}

#[tokio::test]
async fn stacking_needs_two_photos_and_a_picked_manual_stack_comes_along_whole() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [b1, b2, b3], [s1, s2]) = library(&t).await;

    for ids in [json!([]), json!([s1]), json!([s1, s1])] {
        let (st, _) = send(&app, "POST", "/assets/stacks", &key, Some(json!({ "ids": ids }))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
    // Two photos, one of them out of a burst: the burst keeps the others, the new stack takes only what was picked.
    let (st, r) = send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": [s1, b2]}))).await;
    assert_eq!((st, r["size"].as_i64()), (StatusCode::OK, Some(2)));
    let (_, list) = get(&app, &key, "/assets").await;
    assert_eq!(names(&list), ["s2.jpg", "b1.jpg", "s1.jpg"]);
    assert_eq!(
        list["items"][1]["stack"]["size"],
        json!(2),
        "b1 and b3 are still a burst"
    );
    assert_eq!(list["items"][2]["stack"]["size"], json!(2), "s1 and b2");
    // Stacking again merges: the stack of a picked photo comes along entirely.
    let (st, r) = send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": [s2, b2]}))).await;
    assert_eq!((st, r["size"].as_i64()), (StatusCode::OK, Some(3)));
    let (_, list) = get(&app, &key, "/assets").await;
    assert_eq!(names(&list), ["s2.jpg", "b1.jpg"]);
    assert_eq!(list["items"][0]["stack"]["size"], json!(3));
    // All of a burst picked: the stack is the burst, and nothing is left of it.
    let (_, r) = send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": [b1, b3]}))).await;
    assert_eq!(r["size"], json!(2));
    t.teardown().await;
}

#[tokio::test]
async fn a_stack_holds_at_most_a_hundred_photos() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alpha, key) = user(&t.pool, "alpha@example.com").await;
    let mut ids = Vec::new();
    for i in 0..101 {
        ids.push(photo(&t.pool, alpha, &format!("p{i}.jpg"), "2026-03-01T09:00:00Z").await);
    }
    let (st, _) = send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": ids}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, r) = send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": &ids[..100]}))).await;
    assert_eq!((st, r["size"].as_i64()), (StatusCode::OK, Some(100)));
    t.teardown().await;
}

#[tokio::test]
async fn the_cover_can_be_chosen_and_changed() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [b1, b2, b3], _) = library(&t).await;

    assert_eq!(
        get(&app, &key, "/assets").await.1["items"][1]["id"],
        json!(b1.to_string()),
        "the earliest by default"
    );
    let (st, r) = send(&app, "POST", &format!("/assets/{b3}/stack/cover"), &key, None).await;
    assert_eq!(
        (st, r["cover_id"].as_str()),
        (StatusCode::OK, Some(b3.to_string().as_str()))
    );
    let (_, list) = get(&app, &key, "/assets").await;
    assert_eq!(list["items"][1]["id"], json!(b3.to_string()));
    assert_eq!(list["items"][1]["stack"]["size"], json!(3));
    assert_eq!(total(&app, &key, "").await, 3);
    // Changing it moves the choice (one cover per stack), and the viewer's list says which.
    send(&app, "POST", &format!("/assets/{b2}/stack/cover"), &key, None).await;
    let (_, stack) = get(&app, &key, &format!("/assets/{b1}/stack")).await;
    assert_eq!(stack["cover_id"], json!(b2.to_string()));
    assert_eq!(names(&stack), ["b1.jpg", "b2.jpg", "b3.jpg"]);
    let flagged: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE stack_cover")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(flagged, 1);

    // A photo on its own has no stack.
    let (st, _) = send(
        &app,
        "POST",
        &format!("/assets/{}/stack/cover", Uuid::new_v4()),
        &key,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[tokio::test]
async fn removing_a_photo_from_a_burst_keeps_it_out_when_the_burst_is_found_again() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [b1, b2, b3], _) = library(&t).await;

    let (st, _) = send(&app, "POST", &format!("/assets/{b2}/stack/remove"), &key, None).await;
    assert_eq!(st, StatusCode::OK);
    let (_, list) = get(&app, &key, "/assets").await;
    // b2 is a tile of its own (a second after b1), b1 covers the two that are left.
    assert_eq!(names(&list), ["s2.jpg", "b2.jpg", "b1.jpg", "s1.jpg"]);
    assert!(list["items"][1].get("stack").is_none());
    assert_eq!(list["items"][2]["stack"]["size"], json!(2));

    // The worker regroups the neighbourhood: it writes the burst again, on all three.
    burst(&t.pool, &[b1, b2, b3]).await;
    let (_, again) = get(&app, &key, "/assets").await;
    assert_eq!(names(&again), ["s2.jpg", "b2.jpg", "b1.jpg", "s1.jpg"], "b2 stays out");
    t.teardown().await;
}

#[tokio::test]
async fn removing_the_second_to_last_photo_dissolves_a_manual_stack() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, _, [s1, s2]) = library(&t).await;
    send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": [s1, s2]}))).await;
    assert_eq!(total(&app, &key, "").await, 2);
    let (st, _) = send(&app, "POST", &format!("/assets/{s1}/stack/remove"), &key, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(total(&app, &key, "").await, 3);
    let (st, _) = get(&app, &key, &format!("/assets/{s2}/stack")).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "one photo is no stack");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE stack_id IS NOT NULL OR stack_cover")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    t.teardown().await;
}

#[tokio::test]
async fn unstacking_all_frees_every_photo_and_a_regroup_does_not_bring_them_back() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [b1, b2, b3], _) = library(&t).await;

    let (st, r) = send(&app, "DELETE", &format!("/assets/{b2}/stack"), &key, None).await;
    assert_eq!((st, r["count"].as_i64()), (StatusCode::OK, Some(3)));
    assert_eq!(total(&app, &key, "").await, 5);
    assert!(
        get(&app, &key, "/assets").await.1["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i.get("stack").is_none())
    );
    burst(&t.pool, &[b1, b2, b3]).await;
    assert_eq!(total(&app, &key, "").await, 5);
    // Nothing is left to dissolve.
    let (st, _) = send(&app, "DELETE", &format!("/assets/{b1}/stack"), &key, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[tokio::test]
async fn a_manual_stack_wins_over_a_burst_and_survives_a_regroup() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [b1, b2, b3], [s1, _]) = library(&t).await;
    // b3 joins s1 by hand; the burst keeps b1 and b2.
    let (_, r) = send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": [s1, b3]}))).await;
    assert_eq!(r["size"], json!(2));
    let (_, list) = get(&app, &key, "/assets").await;
    assert_eq!(names(&list), ["s2.jpg", "b1.jpg", "s1.jpg"]);
    assert_eq!(list["items"][1]["stack"]["size"], json!(2));
    // The worker rewrites the burst (all three, and then none): the manual stack is untouched.
    burst(&t.pool, &[b1, b2, b3]).await;
    assert_eq!(
        names(&get(&app, &key, "/assets").await.1),
        ["s2.jpg", "b1.jpg", "s1.jpg"]
    );
    sqlx::query("UPDATE assets SET burst_id = NULL, burst_version = NULL")
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, list) = get(&app, &key, "/assets").await;
    assert_eq!(
        names(&list),
        ["s2.jpg", "b2.jpg", "b1.jpg", "s1.jpg"],
        "the burst is gone, the stack stays"
    );
    assert_eq!(list["items"][3]["stack"]["size"], json!(2));
    t.teardown().await;
}

#[tokio::test]
async fn nothing_crosses_owners() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [b1, ..], [s1, _]) = library(&t).await;
    let (bravo, bravo_key) = user(&t.pool, "bravo@example.com").await;
    let theirs = photo(&t.pool, bravo, "t1.jpg", "2026-03-02T10:00:00Z").await;
    let theirs2 = photo(&t.pool, bravo, "t2.jpg", "2026-03-02T10:00:01Z").await;

    // Another owner's burst or stack is invisible and untouchable.
    for (method, path) in [
        ("GET", format!("/assets/{b1}/stack")),
        ("POST", format!("/assets/{b1}/stack/cover")),
        ("POST", format!("/assets/{b1}/stack/remove")),
        ("DELETE", format!("/assets/{b1}/stack")),
    ] {
        let (st, _) = send(&app, method, &path, &bravo_key, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {path}");
    }
    let still: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE burst_id = $1 AND NOT stack_unlinked")
        .bind(b1)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(still, 3);

    // Foreign ids in a stacking request are ignored: one of mine and one of theirs is not two photos.
    let (st, _) = send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": [s1, theirs]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send(
        &app,
        "POST",
        "/assets/stacks",
        &bravo_key,
        Some(json!({"ids": [theirs, s1]})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Two of mine and one of theirs: mine only.
    let (_, r) = send(
        &app,
        "POST",
        "/assets/stacks",
        &key,
        Some(json!({"ids": [s1, b1, theirs]})),
    )
    .await;
    assert_eq!(r["size"], json!(2), "s1 and b1, never the other account's photo");
    let owners: i64 = sqlx::query_scalar("SELECT count(DISTINCT owner_id) FROM assets WHERE stack_id IS NOT NULL")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(owners, 1);
    let theirs_stacked: bool = sqlx::query_scalar("SELECT stack_id IS NOT NULL FROM assets WHERE id = $1")
        .bind(theirs)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(!theirs_stacked);

    // Their own photos stack fine, and their list is theirs alone.
    let (st, r) = send(
        &app,
        "POST",
        "/assets/stacks",
        &bravo_key,
        Some(json!({"ids": [theirs, theirs2]})),
    )
    .await;
    assert_eq!((st, r["size"].as_i64()), (StatusCode::OK, Some(2)));
    assert_eq!(names(&get(&app, &bravo_key, "/assets").await.1), ["t1.jpg"]);
    t.teardown().await;
}

#[tokio::test]
async fn trashed_and_hidden_photos_are_not_listed_in_a_stack() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (_, key, [b1, _, b3], _) = library(&t).await;
    send(&app, "POST", "/assets/trash", &key, Some(json!({"ids": [b1]}))).await;
    let (_, list) = get(&app, &key, "/assets").await;
    // The cover was trashed: the next photo covers the stack.
    assert_eq!(names(&list), ["s2.jpg", "b2.jpg", "s1.jpg"]);
    assert_eq!(list["items"][1]["stack"]["size"], json!(2));
    send(
        &app,
        "POST",
        "/assets/hide",
        &key,
        Some(json!({"ids": [b3], "hidden": true})),
    )
    .await;
    let (_, list) = get(&app, &key, "/assets").await;
    assert_eq!(names(&list), ["s2.jpg", "b2.jpg", "s1.jpg"]);
    assert!(list["items"][1].get("stack").is_none(), "one visible photo is no stack");
    let (st, _) = send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": [b1, b3]}))).await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "trashed and hidden photos cannot be stacked"
    );
    // A hidden photo's stack is as hidden as the photo: nothing answers for it while the Hidden view is locked.
    for (method, path) in [
        ("GET", format!("/assets/{b3}/stack")),
        ("POST", format!("/assets/{b3}/stack/cover")),
        ("POST", format!("/assets/{b3}/stack/remove")),
        ("DELETE", format!("/assets/{b3}/stack")),
    ] {
        let (st, _) = send(&app, method, &path, &key, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {path}");
    }
    t.teardown().await;
}

#[tokio::test]
async fn search_groups_a_manual_stack_and_leaves_out_what_the_owner_took_out() {
    let Some(t) = TestDb::new().await else { return };
    let app = app(&t.pool);
    let (alpha, key) = user(&t.pool, "alpha@example.com").await;
    let m = TEST_MODEL;
    let a = vector_photo(&t.pool, alpha, "a.jpg", m, &[("cat", 1.0)]).await;
    let b = vector_photo(&t.pool, alpha, "b.jpg", m, &[("cat", 1.0), ("dog", 0.1)]).await;
    let c = vector_photo(&t.pool, alpha, "c.jpg", m, &[("cat", 1.0), ("dog", 0.2)]).await;
    burst(&t.pool, &[b, c]).await;
    let (_, r) = get(&app, &key, "/assets/search?q=cat&cutoff=false").await;
    assert_eq!(r["items"].as_array().unwrap().len(), 2, "the burst b + c is one result");
    // Taken out of the burst: three results.
    send(&app, "POST", &format!("/assets/{c}/stack/remove"), &key, None).await;
    let (_, r) = get(&app, &key, "/assets/search?q=cat&cutoff=false").await;
    assert_eq!(r["items"].as_array().unwrap().len(), 3);
    // Stacked by hand with a: a and c are one result again.
    send(&app, "POST", "/assets/stacks", &key, Some(json!({"ids": [a, c]}))).await;
    let (_, r) = get(&app, &key, "/assets/search?q=cat&cutoff=false").await;
    assert_eq!(r["items"].as_array().unwrap().len(), 2);
    assert_eq!(r["items"][0]["burst"]["size"], json!(2));
    t.teardown().await;
}

#[test]
fn the_sql_helpers_name_one_rule() {
    use super::stacks::{collapse_sql, in_stack_sql, key_sql};
    assert_eq!(
        key_sql("a"),
        "coalesce(a.stack_id, CASE WHEN a.stack_unlinked THEN NULL ELSE a.burst_id END)"
    );
    assert!(in_stack_sql("m", "$2").contains("m.stack_id = $2"));
    let c = collapse_sql(" AND NOT hidden AND NOT archived");
    assert!(c.contains("NOT hidden AND NOT archived"), "{c}");
    assert!(c.contains("m.owner_id = assets.owner_id"), "{c}");
}
