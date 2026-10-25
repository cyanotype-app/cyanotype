//! `GET /assets/slideshow` and `GET /assets/previews` over a real Postgres (skipped without `DATABASE_URL`): the order of a
//! view page by page, the seeded shuffle, and what must never be shown (other people's photos, the trash, hidden photos).
use std::collections::HashSet;

use axum::{Router, http::StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::testutil::{TestDb, app, send, send_cookie, session, user};

async fn many(pool: &PgPool, owner: Uuid, prefix: &str, n: i32) -> Vec<Uuid> {
    // Photo i was taken i seconds before the previous one: `ids[0]` is the newest.
    sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, taken_at, thumbhash)
         SELECT $1, sha256((($2 || i)::text)::bytea), 'originals/' || $2 || i, $2 || i, 'image/jpeg', 1,
                now() - make_interval(secs => i), '\x01'
         FROM generate_series(1, $3) i ORDER BY i RETURNING id",
    )
    .bind(owner)
    .bind(prefix)
    .bind(n)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn url(scope: &Value, extra: &str) -> String {
    let enc: String = scope
        .to_string()
        .bytes()
        .map(|b| match b {
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("/assets/slideshow?scope={enc}{extra}")
}

/// Every page of a view in one order: the ids in order and the `total`.
async fn walk(app: &Router, key: &str, scope: &Value, extra: &str, limit: usize) -> (Vec<String>, i64) {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut u = url(scope, &format!("{extra}&limit={limit}"));
        if let Some(c) = &cursor {
            u.push_str(&format!("&cursor={c}"));
        }
        let (st, b) = send(app, "GET", &u, key, None).await;
        assert_eq!(st, StatusCode::OK, "{u}: {b}");
        let total = b["total"].as_i64().unwrap();
        out.extend(
            b["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["id"].as_str().unwrap().to_string()),
        );
        match b["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => return (out, total),
        }
        assert!(out.len() < 10_000, "paging does not end");
    }
}

#[tokio::test]
async fn orders_page_by_page() {
    let Some(t) = TestDb::new().await else { return };
    let (alpha, ka) = user(&t.pool, "alpha@example.com").await;
    let ids = many(&t.pool, alpha, "a", 23).await;
    let app = app(&t.pool);

    let (listed, total) = walk(&app, &ka, &json!("library"), "&order=listed", 5).await;
    assert_eq!(total, 23);
    assert_eq!(listed, ids.iter().map(|i| i.to_string()).collect::<Vec<_>>());

    let (oldest, _) = walk(&app, &ka, &json!("library"), "&order=oldest", 7).await;
    let mut rev: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    rev.reverse();
    assert_eq!(oldest, rev);

    // Default order is the listed one.
    let (dflt, _) = walk(&app, &ka, &json!("library"), "", 100).await;
    assert_eq!(dflt, listed);
    t.teardown().await;
}

#[tokio::test]
async fn shuffle_is_seeded_stable_across_pages_and_has_no_repeats() {
    let Some(t) = TestDb::new().await else { return };
    let (alpha, ka) = user(&t.pool, "alpha@example.com").await;
    let ids = many(&t.pool, alpha, "a", 61).await;
    let app = app(&t.pool);

    let (a, total) = walk(&app, &ka, &json!("library"), "&order=shuffle&seed=one", 8).await;
    assert_eq!(total, 61);
    assert_eq!(a.len(), 61);
    assert_eq!(a.iter().collect::<HashSet<_>>().len(), 61, "a repeat within one pass");
    assert_eq!(
        a.iter().cloned().collect::<HashSet<_>>(),
        ids.iter().map(|i| i.to_string()).collect::<HashSet<_>>(),
        "every photo once"
    );
    // The page size does not change the order; the same seed gives the same order again.
    let (b, _) = walk(&app, &ka, &json!("library"), "&order=shuffle&seed=one", 25).await;
    assert_eq!(a, b);
    // Another seed is another order, and it is not the listed one.
    let (c, _) = walk(&app, &ka, &json!("library"), "&order=shuffle&seed=two", 25).await;
    assert_ne!(a, c);
    assert_ne!(a, ids.iter().map(|i| i.to_string()).collect::<Vec<_>>());
    // Fair: the first photo is not always the same one across seeds.
    let mut firsts = HashSet::new();
    for n in 0..12 {
        let (st, body) = send(
            &app,
            "GET",
            &url(&json!("library"), &format!("&order=shuffle&seed=s{n}&limit=1")),
            &ka,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        firsts.insert(body["items"][0]["id"].as_str().unwrap().to_string());
    }
    assert!(
        firsts.len() >= 5,
        "twelve seeds gave {} different first photos",
        firsts.len()
    );
    t.teardown().await;
}

#[tokio::test]
async fn never_other_peoples_trashed_archived_hidden_or_paired_photos() {
    let Some(t) = TestDb::new().await else { return };
    let (alpha, ka) = user(&t.pool, "alpha@example.com").await;
    let (bravo, _kb) = user(&t.pool, "bravo@example.com").await;
    let mine = many(&t.pool, alpha, "a", 10).await;
    let theirs = many(&t.pool, bravo, "b", 5).await;
    for (id, sql) in [
        (mine[0], "deleted_at = now()"),
        (mine[1], "hidden = true"),
        (mine[2], "archived = true"),
        (
            mine[3],
            "primary_id = (SELECT id FROM assets WHERE original_name = 'a5' AND owner_id = assets.owner_id)",
        ),
    ] {
        sqlx::query(&format!("UPDATE assets SET {sql} WHERE id = $1"))
            .bind(id)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    let app = app(&t.pool);
    for order in ["listed", "oldest", "shuffle"] {
        let (got, total) = walk(&app, &ka, &json!("library"), &format!("&order={order}&seed=x"), 3).await;
        assert_eq!(total, 6, "{order}");
        let got: HashSet<String> = got.into_iter().collect();
        for gone in &mine[..4] {
            assert!(!got.contains(&gone.to_string()), "{order}: {gone} must not be shown");
        }
        for other in &theirs {
            assert!(!got.contains(&other.to_string()), "{order}: another account's photo");
        }
    }
    // Asking for the Hidden view with a key (which can never unlock it), the trash, or someone else's album is refused.
    for scope in [
        json!("hidden"),
        json!("trash"),
        json!({"trash_batch": "2024-01-01T00:00:00Z"}),
    ] {
        let (st, _) = send(&app, "GET", &url(&scope, ""), &ka, None).await;
        assert!(
            st == StatusCode::FORBIDDEN || st == StatusCode::BAD_REQUEST || st == StatusCode::NOT_FOUND,
            "{scope}: {st}"
        );
    }
    let album: Uuid = sqlx::query_scalar("INSERT INTO albums (owner_id, title) VALUES ($1, 'b') RETURNING id")
        .bind(bravo)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let (st, _) = send(&app, "GET", &url(&json!({ "album": album }), ""), &ka, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[tokio::test]
async fn hidden_view_only_when_unlocked() {
    let Some(t) = TestDb::new().await else { return };
    let (alpha, cookie) = session(&t.pool, "alpha@example.com", "member").await;
    let ids = many(&t.pool, alpha, "a", 4).await;
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(ids[0])
        .execute(&t.pool)
        .await
        .unwrap();
    let app = app(&t.pool);
    let (st, _) = send_cookie(&app, "GET", &url(&json!("hidden"), ""), Some(&cookie), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "locked");
    let (st, b) = send_cookie(&app, "GET", &url(&json!("library"), ""), Some(&cookie), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b["total"], 3, "the library never lists hidden photos");
    let token = cookie.split_once('=').unwrap().1;
    sqlx::query("UPDATE sessions SET hidden_unlocked_until = now() + interval '10 minutes' WHERE token_hash = $1")
        .bind(cyanotype_service::auth::sha256(token.as_bytes()))
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, b) = send_cookie(&app, "GET", &url(&json!("hidden"), ""), Some(&cookie), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b["total"], 1);
    assert_eq!(b["items"][0]["id"], ids[0].to_string());
    t.teardown().await;
}

#[tokio::test]
async fn album_and_favorites_scopes_and_bad_input() {
    let Some(t) = TestDb::new().await else { return };
    let (alpha, ka) = user(&t.pool, "alpha@example.com").await;
    let ids = many(&t.pool, alpha, "a", 12).await;
    let album: Uuid = sqlx::query_scalar("INSERT INTO albums (owner_id, title) VALUES ($1, 'a') RETURNING id")
        .bind(alpha)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    for id in &ids[..5] {
        sqlx::query("INSERT INTO album_assets (album_id, asset_id) VALUES ($1, $2)")
            .bind(album)
            .bind(id)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE assets SET favorite = true WHERE id = ANY($1)")
        .bind(&ids[8..])
        .execute(&t.pool)
        .await
        .unwrap();
    let app = app(&t.pool);
    let (got, total) = walk(&app, &ka, &json!({ "album": album }), "&order=shuffle&seed=q", 2).await;
    assert_eq!((got.len(), total), (5, 5));
    let (got, total) = walk(&app, &ka, &json!("favorites"), "", 2).await;
    assert_eq!((got.len(), total), (4, 4));
    for (extra, why) in [
        ("&order=sideways", "unknown order"),
        ("&seed=has%20space", "seed with a space"),
        ("&cursor=abc", "cursor that is not a number"),
    ] {
        let (st, _) = send(&app, "GET", &url(&json!("library"), extra), &ka, None).await;
        assert!(
            st == StatusCode::BAD_REQUEST || st == StatusCode::UNPROCESSABLE_ENTITY,
            "{why}: {st}"
        );
    }
    let long = "x".repeat(65);
    let (st, _) = send(
        &app,
        "GET",
        &url(&json!("library"), &format!("&seed={long}")),
        &ka,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send(&app, "GET", "/assets/slideshow?scope=nonsense", &ka, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send(&app, "GET", "/assets/slideshow", &ka, None).await;
    assert!(st.is_client_error());
    // Past the end: an empty page, not an error.
    let (st, b) = send(&app, "GET", &url(&json!("library"), "&cursor=999"), &ka, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b["items"].as_array().unwrap().len(), 0);
    assert!(b["next_cursor"].is_null());
    // A page is never larger than the maximum.
    let (_, b) = send(&app, "GET", &url(&json!("library"), "&limit=100000"), &ka, None).await;
    assert!(b["items"].as_array().unwrap().len() <= 100);
    t.teardown().await;
}

#[tokio::test]
async fn previews_are_the_callers_own_visible_and_rendered_photos() {
    let Some(t) = TestDb::new().await else { return };
    let (alpha, ka) = user(&t.pool, "alpha@example.com").await;
    let (bravo, _) = user(&t.pool, "bravo@example.com").await;
    let mine = many(&t.pool, alpha, "a", 6).await;
    let theirs = many(&t.pool, bravo, "b", 1).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(mine[0])
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(mine[1])
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET thumbhash = NULL WHERE id = $1")
        .bind(mine[2])
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET caption = 'A beach' WHERE id = $1")
        .bind(mine[3])
        .execute(&t.pool)
        .await
        .unwrap();
    let app = app(&t.pool);
    let all: Vec<String> = mine.iter().chain(&theirs).map(|i| i.to_string()).collect();
    let (st, b) = send(
        &app,
        "GET",
        &format!("/assets/previews?ids={}", all.join(",")),
        &ka,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let items = b["items"].as_array().unwrap();
    let got: HashSet<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
    let want: HashSet<String> = mine[3..].iter().map(|i| i.to_string()).collect();
    assert_eq!(got, want.iter().map(String::as_str).collect::<HashSet<_>>());
    assert!(
        items
            .iter()
            .all(|i| i["preview_url"].as_str().unwrap().contains("preview"))
    );
    let beach = items.iter().find(|i| i["id"] == mine[3].to_string()).unwrap();
    assert_eq!(beach["caption"], "A beach");

    // Too many ids, and ids that are not ids.
    let many_ids = vec![Uuid::new_v4().to_string(); 25].join(",");
    let (st, _) = send(&app, "GET", &format!("/assets/previews?ids={many_ids}"), &ka, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send(&app, "GET", "/assets/previews?ids=nope", &ka, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, b) = send(&app, "GET", "/assets/previews?ids=", &ka, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b["items"].as_array().unwrap().len(), 0);
    t.teardown().await;
}
