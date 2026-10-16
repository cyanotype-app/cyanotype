//! Share links against a real Postgres (skipped without `DATABASE_URL`, like the other DB tests).
use std::net::SocketAddr;

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::{routes_with, share_public::ShareState};
use crate::{
    AppState,
    testutil::{TestDb, asset, dummy_storage, ids, send, user},
};

fn app_with(pool: &PgPool, st: ShareState) -> Router {
    routes_with(st).with_state(AppState {
        db: pool.clone(),
        storage: dummy_storage(),
        trash_retention_days: 30,
        purge_wake: Default::default(),
    })
}

fn default_state() -> ShareState {
    ShareState::new(false, vec![]).with_limits((10_000, 10_000), (10_000, 10_000), (10_000, 10_000))
}

/// A request without credentials, as a visitor from `ip` (optionally with a cookie header).
async fn visit(
    app: &Router,
    method: &str,
    path: &str,
    ip: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let mut b = Request::builder().method(method).uri(path);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut req = b.body(body).unwrap();
    let addr: SocketAddr = format!("{ip}:1234").parse().unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    let res = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get(app: &Router, path: &str) -> (StatusCode, HeaderMap, Value) {
    visit(app, "GET", path, "203.0.113.1", None, None).await
}

fn base(token: &str) -> String {
    format!("/assets/public/s/{token}")
}

/// Create a selection share as `key`; returns (id, token).
async fn share_of(app: &Router, key: &str, assets: &[Uuid], extra: Value) -> (String, String) {
    let mut body = json!({"asset_ids": assets});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    let (st, v) = send(app, "POST", "/assets/shares", key, Some(body)).await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    (v["id"].as_str().unwrap().into(), v["token"].as_str().unwrap().into())
}

fn item_ids(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}

async fn set_time(pool: &PgPool, id: Uuid, taken: &str) {
    sqlx::query("UPDATE assets SET taken_at = $2::timestamptz WHERE id = $1")
        .bind(id)
        .bind(taken)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn create_list_edit_and_revoke() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (_, other_key) = user(&t.pool, "b@example.com").await;
    let app = app_with(&t.pool, default_state());
    let a = asset(&t.pool, uid, "a.jpg").await;
    let b = asset(&t.pool, uid, "b.jpg").await;

    let (st, v) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"asset_ids": [a, b, a], "title": "  Holiday  "})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["kind"], "selection");
    assert_eq!(v["title"], "Holiday");
    assert_eq!(v["item_count"], 2);
    assert_eq!(v["has_password"], false);
    assert_eq!(v["allow_download"], true);
    assert_eq!(v["include_originals"], false);
    assert_eq!(v["expired"], false);
    assert!(v.get("password_hash").is_none() && v.get("password").is_none());
    let token = v["token"].as_str().unwrap();
    assert!(
        token.len() >= 32
            && token
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    );
    let id = v["id"].as_str().unwrap().to_string();

    // Tokens are unique per link.
    let (id2, token2) = share_of(&app, &key, &[a], json!({})).await;
    assert_ne!(token, token2);

    let (st, list) = send(&app, "GET", "/assets/shares", &key, None).await;
    assert_eq!(st, StatusCode::OK);
    let rows = list["items"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"], id2, "newest first");

    // Edit: title, expiry, password, download settings.
    let future = (time::OffsetDateTime::now_utc() + time::Duration::days(7))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let (st, v) = send(
        &app,
        "PATCH",
        &format!("/assets/shares/{id}"),
        &key,
        Some(json!({"title": null, "expires_at": future, "password": "hunter22", "include_originals": true})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["title"], Value::Null);
    assert_eq!(v["has_password"], true);
    assert_eq!(v["include_originals"], true);
    assert!(v["expires_at"].is_string());
    // Omitted fields stay; null clears.
    let (_, v) = send(
        &app,
        "PATCH",
        &format!("/assets/shares/{id}"),
        &key,
        Some(json!({"expires_at": null, "password": null})),
    )
    .await;
    assert_eq!(v["expires_at"], Value::Null);
    assert_eq!(v["has_password"], false);
    assert_eq!(v["include_originals"], true);
    // Turning downloads off also turns the originals off.
    let (_, v) = send(
        &app,
        "PATCH",
        &format!("/assets/shares/{id}"),
        &key,
        Some(json!({"allow_download": false})),
    )
    .await;
    assert_eq!(
        (v["allow_download"].clone(), v["include_originals"].clone()),
        (json!(false), json!(false))
    );

    // Validation.
    let past = "2001-01-01T00:00:00Z";
    for (body, what) in [
        (json!({"asset_ids": [a], "expires_at": past}), "expiry in the past"),
        (json!({"asset_ids": [a], "password": "abc"}), "short password"),
        (json!({"asset_ids": [a], "title": "x".repeat(201)}), "long title"),
        (json!({"asset_ids": []}), "no photos"),
        (json!({}), "nothing to share"),
        (json!({"asset_ids": [a], "album_id": Uuid::new_v4()}), "both"),
        (json!({"asset_ids": [Uuid::new_v4()]}), "photos that do not exist"),
    ] {
        let (st, _) = send(&app, "POST", "/assets/shares", &key, Some(body)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{what}");
    }
    let (st, _) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"album_id": Uuid::new_v4()})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "unknown album");

    // Owner isolation: nobody else sees, edits or revokes it, and cannot share your photos.
    let (st, list) = send(&app, "GET", "/assets/shares", &other_key, None).await;
    assert_eq!((st, list["items"].as_array().unwrap().len()), (StatusCode::OK, 0));
    for (m, body) in [("GET", None), ("PATCH", Some(json!({"title": "x"}))), ("DELETE", None)] {
        let (st, _) = send(&app, m, &format!("/assets/shares/{id}"), &other_key, body).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{m}");
    }
    let (st, _) = send(
        &app,
        "POST",
        "/assets/shares",
        &other_key,
        Some(json!({"asset_ids": [a]})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Unauthenticated callers cannot use the owner API at all.
    let (st, ..) = visit(&app, "GET", "/assets/shares", "203.0.113.1", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Revoking is instant and final.
    assert_eq!(get(&app, &base(token)).await.0, StatusCode::OK);
    let (st, _) = send(&app, "DELETE", &format!("/assets/shares/{id}"), &key, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(get(&app, &base(token)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        get(&app, &format!("{}/items", base(token))).await.0,
        StatusCode::NOT_FOUND
    );
    let (st, _) = send(&app, "DELETE", &format!("/assets/shares/{id}"), &key, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[tokio::test]
async fn album_links_follow_the_album_and_selection_links_are_fixed() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app_with(&t.pool, default_state());
    let a1 = asset(&t.pool, uid, "1.jpg").await;
    let a2 = asset(&t.pool, uid, "2.jpg").await;
    let a3 = asset(&t.pool, uid, "3.jpg").await;
    for (i, a) in [a1, a2, a3].iter().enumerate() {
        set_time(&t.pool, *a, &format!("2024-01-0{}T00:00:00Z", i + 1)).await;
    }
    let (_, album) = send(
        &app,
        "POST",
        "/assets/albums",
        &key,
        Some(json!({"title": "Trip", "asset_ids": [a1, a2]})),
    )
    .await;
    let album_id = album["id"].as_str().unwrap();
    let (st, album_share) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"album_id": album_id})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(album_share["kind"], "album");
    assert_eq!(album_share["album"]["title"], "Trip");
    let album_token = album_share["token"].as_str().unwrap().to_string();
    let (_, sel_token) = share_of(&app, &key, &[a1, a2], json!({"title": "Two"})).await;

    let list = |token: String| {
        let app = app.clone();
        async move { item_ids(&get(&app, &format!("{}/items", base(&token))).await.2) }
    };
    let (a1s, a2s, a3s) = (a1.to_string(), a2.to_string(), a3.to_string());
    // Newest first.
    assert_eq!(list(album_token.clone()).await, vec![a2s.clone(), a1s.clone()]);
    assert_eq!(list(sel_token.clone()).await, vec![a2s.clone(), a1s.clone()]);
    // The title of an album link is the album's unless the owner set one.
    assert_eq!(get(&app, &base(&album_token)).await.2["title"], "Trip");
    assert_eq!(get(&app, &base(&sel_token)).await.2["title"], "Two");

    // A photo added to the album later shows up on the album link only.
    send(
        &app,
        "POST",
        &format!("/assets/albums/{album_id}/assets"),
        &key,
        Some(ids(&[a3])),
    )
    .await;
    assert_eq!(
        list(album_token.clone()).await,
        vec![a3s.clone(), a2s.clone(), a1s.clone()]
    );
    assert_eq!(list(sel_token.clone()).await, vec![a2s.clone(), a1s.clone()]);
    assert_eq!(get(&app, &base(&album_token)).await.2["item_count"], 3);

    // Trashed photos disappear from both, and come back on restore.
    send(&app, "POST", "/assets/trash", &key, Some(ids(&[a2]))).await;
    assert_eq!(list(album_token.clone()).await, vec![a3s.clone(), a1s.clone()]);
    assert_eq!(list(sel_token.clone()).await, vec![a1s.clone()]);
    assert_eq!(get(&app, &base(&sel_token)).await.2["item_count"], 1);
    assert_eq!(
        get(&app, &format!("{}/items/{a2}", base(&sel_token))).await.0,
        StatusCode::NOT_FOUND
    );
    send(&app, "POST", "/assets/restore", &key, Some(ids(&[a2]))).await;
    assert_eq!(list(sel_token.clone()).await, vec![a2s.clone(), a1s.clone()]);

    // Purged photos are gone for good.
    send(&app, "POST", "/assets/trash", &key, Some(ids(&[a1]))).await;
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(a1)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(list(sel_token.clone()).await, vec![a2s.clone()]);

    // The owner's counts agree.
    let (_, shares) = send(&app, "GET", "/assets/shares", &key, None).await;
    let counts: Vec<(String, i64)> = shares["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["kind"].as_str().unwrap().to_string(),
                s["item_count"].as_i64().unwrap(),
            )
        })
        .collect();
    assert!(
        counts.contains(&("selection".into(), 1)) && counts.contains(&("album".into(), 2)),
        "{counts:?}"
    );

    // Deleting the album revokes its link.
    send(&app, "DELETE", &format!("/assets/albums/{album_id}"), &key, None).await;
    assert_eq!(get(&app, &base(&album_token)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(list(sel_token).await, vec![a2s]);
    t.teardown().await;
}

#[tokio::test]
async fn items_page_with_a_cursor_and_other_photos_never_leak() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (other, _) = user(&t.pool, "b@example.com").await;
    let app = app_with(&t.pool, default_state());
    let mut mine = Vec::new();
    for i in 0..5 {
        let a = asset(&t.pool, uid, &format!("p{i}.jpg")).await;
        set_time(&t.pool, a, "2024-03-01T00:00:00Z").await; // tied times
        mine.push(a);
    }
    let theirs = asset(&t.pool, other, "theirs.jpg").await;
    let (_, token) = share_of(&app, &key, &mine, json!({})).await;
    // Even if a row for someone else's photo sneaks into the join table, it is never shown.
    let (sid,): (Uuid,) = sqlx::query_as("SELECT id FROM shares WHERE token = $1")
        .bind(&token)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO share_assets (share_id, asset_id) VALUES ($1, $2)")
        .bind(sid)
        .bind(theirs)
        .execute(&t.pool)
        .await
        .unwrap();

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..5 {
        let q = cursor.as_ref().map_or(String::new(), |c| format!("&cursor={c}"));
        let (st, _, v) = get(&app, &format!("{}/items?limit=2{q}", base(&token))).await;
        assert_eq!(st, StatusCode::OK);
        seen.extend(item_ids(&v));
        cursor = v["next_cursor"].as_str().map(String::from);
        if cursor.is_none() {
            break;
        }
    }
    let mut want: Vec<String> = mine.iter().map(Uuid::to_string).collect();
    want.sort();
    let mut got = seen.clone();
    got.sort();
    assert_eq!(got, want, "every photo once, none of anyone else's");
    assert!(!seen.contains(&theirs.to_string()));
    assert_eq!(
        get(&app, &format!("{}/items?cursor=zz", base(&token))).await.0,
        StatusCode::BAD_REQUEST
    );
    // A photo that is not in the link is a 404 even if it exists.
    assert_eq!(
        get(&app, &format!("{}/items/{theirs}", base(&token))).await.0,
        StatusCode::NOT_FOUND
    );
    t.teardown().await;
}

#[tokio::test]
async fn unknown_revoked_and_expired_links_look_identical() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app_with(&t.pool, default_state());
    let a = asset(&t.pool, uid, "a.jpg").await;
    let (id_revoked, tok_revoked) = share_of(&app, &key, &[a], json!({})).await;
    let (_, tok_expired) = share_of(&app, &key, &[a], json!({})).await;
    let (_, tok_live) = share_of(&app, &key, &[a], json!({})).await;
    send(&app, "DELETE", &format!("/assets/shares/{id_revoked}"), &key, None).await;
    sqlx::query("UPDATE shares SET expires_at = now() - interval '1 minute' WHERE token = $1")
        .bind(&tok_expired)
        .execute(&t.pool)
        .await
        .unwrap();

    let unknown = "A".repeat(32);
    let weird = "..%2Fetc%2Fpasswd".to_string();
    let too_long = "a".repeat(500);
    let mut responses = Vec::new();
    for tok in [&unknown, &tok_revoked, &tok_expired, &weird, &too_long] {
        for suffix in ["", "/items", &format!("/items/{a}"), &format!("/items/{a}/download")] {
            let (st, h, v) = get(&app, &format!("{}{suffix}", base(tok))).await;
            responses.push((st, v, h.get("content-type").cloned()));
        }
        let (st, _, v) = visit(
            &app,
            "POST",
            &format!("{}/unlock", base(tok)),
            "203.0.113.1",
            None,
            Some(json!({"password": "x"})),
        )
        .await;
        responses.push((st, v, None));
    }
    for (st, v, _) in &responses {
        assert_eq!(*st, StatusCode::NOT_FOUND, "{v}");
        assert_eq!(*v, json!({"error": "not found"}));
    }
    // The live one works, and an expired link is live again once the owner extends it.
    assert_eq!(get(&app, &base(&tok_live)).await.0, StatusCode::OK);
    sqlx::query("UPDATE shares SET expires_at = now() + interval '1 hour' WHERE token = $1")
        .bind(&tok_expired)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(get(&app, &base(&tok_expired)).await.0, StatusCode::OK);
    // The owner's list says which links have expired.
    sqlx::query("UPDATE shares SET expires_at = now() - interval '1 minute' WHERE token = $1")
        .bind(&tok_expired)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, list) = send(&app, "GET", "/assets/shares", &key, None).await;
    let expired: Vec<bool> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["expired"].as_bool().unwrap())
        .collect();
    assert_eq!(expired.iter().filter(|e| **e).count(), 1);
    t.teardown().await;
}

#[tokio::test]
async fn probing_tokens_is_rate_limited_per_ip_but_live_links_are_not_affected() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let st = ShareState::new(false, vec![]).with_limits((10_000, 10_000), (3, 1), (5, 5));
    let app = app_with(&t.pool, st);
    let a = asset(&t.pool, uid, "a.jpg").await;
    let (_, live) = share_of(&app, &key, &[a], json!({})).await;
    let bogus = |i: u32| format!("{}{}", "B".repeat(30), 10 + i);
    for i in 0..3 {
        let (st, ..) = visit(&app, "GET", &base(&bogus(i)), "198.51.100.7", None, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    let (st, h, _) = visit(&app, "GET", &base(&bogus(9)), "198.51.100.7", None, None).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.contains_key(header::RETRY_AFTER));
    // The same IP can still open a live link; another IP can still probe.
    assert_eq!(
        visit(&app, "GET", &base(&live), "198.51.100.7", None, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        visit(&app, "GET", &base(&bogus(9)), "198.51.100.8", None, None).await.0,
        StatusCode::NOT_FOUND
    );

    // The overall per-IP budget.
    let st = ShareState::new(false, vec![]).with_limits((4, 1), (100, 100), (5, 5));
    let app = app_with(&t.pool, st);
    for _ in 0..4 {
        assert_eq!(
            visit(&app, "GET", &base(&live), "198.51.100.9", None, None).await.0,
            StatusCode::OK
        );
    }
    let (status, h, _) = visit(&app, "GET", &base(&live), "198.51.100.9", None, None).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.get("x-robots-tag").is_some(), "even refusals are noindex");
    t.teardown().await;
}

fn cookie_pair(headers: &HeaderMap) -> String {
    let set = headers[header::SET_COOKIE].to_str().unwrap();
    set.split(';').next().unwrap().to_string()
}

#[tokio::test]
async fn password_flow_cookie_scope_and_changes() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app_with(&t.pool, default_state());
    let a = asset(&t.pool, uid, "a.jpg").await;
    let (id, token) = share_of(
        &app,
        &key,
        &[a],
        json!({"password": "correct horse", "title": "Private title"}),
    )
    .await;
    let (_, other) = share_of(&app, &key, &[a], json!({"password": "other pass"})).await;

    // Locked: only the fact that a password is needed; no title, no count, no photos.
    let (st, _, v) = get(&app, &base(&token)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v, json!({"needs_password": true, "unlocked": false}));
    assert_eq!(
        get(&app, &format!("{}/items", base(&token))).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&app, &format!("{}/items/{a}", base(&token))).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&app, &format!("{}/items/{a}/download", base(&token))).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        visit(
            &app,
            "POST",
            &format!("{}/items/{a}/prepare", base(&token)),
            "203.0.113.1",
            None,
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    // A made-up cookie does not help.
    assert_eq!(
        visit(
            &app,
            "GET",
            &format!("{}/items", base(&token)),
            "203.0.113.1",
            Some("cy_share_x=abc"),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    // Wrong password.
    let (st, h, _) = visit(
        &app,
        "POST",
        &format!("{}/unlock", base(&token)),
        "203.0.113.1",
        None,
        Some(json!({"password": "nope"})),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert!(!h.contains_key(header::SET_COOKIE));

    // Right password: a cookie that is HttpOnly, short-lived, and only for this link's own path.
    let (st, h, _) = visit(
        &app,
        "POST",
        &format!("{}/unlock", base(&token)),
        "203.0.113.1",
        None,
        Some(json!({"password": "correct horse"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let set = h[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(set.contains("HttpOnly"), "{set}");
    assert!(set.contains(&format!("Path={}", base(&token))), "{set}");
    assert!(set.contains("Max-Age=21600"), "{set}");
    assert!(set.contains("SameSite=Lax"), "{set}");
    assert!(!set.contains("correct horse"));
    let cookie = cookie_pair(&h);
    let (st, _, v) = visit(&app, "GET", &base(&token), "203.0.113.1", Some(&cookie), None).await;
    assert_eq!(
        (st, v["unlocked"].clone(), v["title"].clone()),
        (StatusCode::OK, json!(true), json!("Private title"))
    );
    let (st, _, v) = visit(
        &app,
        "GET",
        &format!("{}/items", base(&token)),
        "203.0.113.1",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!((st, item_ids(&v)), (StatusCode::OK, vec![a.to_string()]));
    // The cookie opens only the link it was issued for.
    assert_eq!(
        visit(
            &app,
            "GET",
            &format!("{}/items", base(&other)),
            "203.0.113.1",
            Some(&cookie),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    // The cookie value is stored hashed.
    let value = cookie.split_once('=').unwrap().1;
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM share_unlocks WHERE token_hash = $1")
        .bind(cyanotype_service::auth::sha256(value.as_bytes()))
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(stored, 1);
    let raw: i64 = sqlx::query_scalar("SELECT count(*) FROM share_unlocks WHERE token_hash = convert_to($1, 'UTF8')")
        .bind(value)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(raw, 0);

    // Expired unlocks stop working.
    sqlx::query("UPDATE share_unlocks SET expires_at = now() - interval '1 second'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        visit(
            &app,
            "GET",
            &format!("{}/items", base(&token)),
            "203.0.113.1",
            Some(&cookie),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, h, _) = visit(
        &app,
        "POST",
        &format!("{}/unlock", base(&token)),
        "203.0.113.1",
        None,
        Some(json!({"password": "correct horse"})),
    )
    .await;
    let cookie = cookie_pair(&h);

    // Changing the password ends every unlock; removing it opens the link to everyone.
    let (st, _) = send(
        &app,
        "PATCH",
        &format!("/assets/shares/{id}"),
        &key,
        Some(json!({"password": "brand new pw"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        visit(
            &app,
            "GET",
            &format!("{}/items", base(&token)),
            "203.0.113.1",
            Some(&cookie),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (st, ..) = visit(
        &app,
        "POST",
        &format!("{}/unlock", base(&token)),
        "203.0.113.1",
        None,
        Some(json!({"password": "correct horse"})),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "the old password no longer works");
    send(
        &app,
        "PATCH",
        &format!("/assets/shares/{id}"),
        &key,
        Some(json!({"password": null})),
    )
    .await;
    assert_eq!(get(&app, &format!("{}/items", base(&token))).await.0, StatusCode::OK);
    // Unlocking a link without a password is a harmless no-op.
    let (st, h, _) = visit(
        &app,
        "POST",
        &format!("{}/unlock", base(&token)),
        "203.0.113.1",
        None,
        Some(json!({"password": "x"})),
    )
    .await;
    assert_eq!(
        (st, h.contains_key(header::SET_COOKIE)),
        (StatusCode::NO_CONTENT, false)
    );
    // The stored hash is argon2, never the password.
    let (_, tok3) = share_of(&app, &key, &[a], json!({"password": "stored safely"})).await;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM shares WHERE token = $1")
        .bind(&tok3)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(hash.starts_with("$argon2"), "{hash}");
    t.teardown().await;
}

#[tokio::test]
async fn wrong_passwords_are_rate_limited_per_ip_and_lock_the_link() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let a = asset(&t.pool, uid, "a.jpg").await;

    // Per IP: five attempts a minute.
    let app = app_with(
        &t.pool,
        ShareState::new(false, vec![]).with_limits((10_000, 10_000), (10_000, 10_000), (5, 1)),
    );
    let (_, token) = share_of(&app, &key, &[a], json!({"password": "correct horse"})).await;
    let url = format!("{}/unlock", base(&token));
    for _ in 0..5 {
        let (st, ..) = visit(
            &app,
            "POST",
            &url,
            "198.51.100.1",
            None,
            Some(json!({"password": "wrong"})),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
    let (st, h, _) = visit(
        &app,
        "POST",
        &url,
        "198.51.100.1",
        None,
        Some(json!({"password": "correct horse"})),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "even the right password waits");
    assert!(h.contains_key(header::RETRY_AFTER));
    // Someone else is not locked out by it.
    let (st, ..) = visit(
        &app,
        "POST",
        &url,
        "198.51.100.2",
        None,
        Some(json!({"password": "correct horse"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // Per link: ten wrong passwords in a row, from any addresses, lock it for a while.
    let app = app_with(&t.pool, default_state());
    for i in 0..10 {
        let (st, ..) = visit(
            &app,
            "POST",
            &url,
            &format!("198.51.100.{}", 10 + i),
            None,
            Some(json!({"password": "wrong"})),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let (st, h, _) = visit(
        &app,
        "POST",
        &url,
        "198.51.100.99",
        None,
        Some(json!({"password": "correct horse"})),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    let wait: u64 = h[header::RETRY_AFTER].to_str().unwrap().parse().unwrap();
    assert!((1..=600).contains(&wait), "{wait}");
    // The lock ends by itself; a right password then resets the count.
    sqlx::query("UPDATE shares SET pw_locked_until = now() - interval '1 second'")
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, ..) = visit(
        &app,
        "POST",
        &url,
        "198.51.100.99",
        None,
        Some(json!({"password": "correct horse"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let failures: i32 = sqlx::query_scalar("SELECT pw_failures FROM shares")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(failures, 0);
    // The owner changing the password clears a lockout.
    for i in 0..10 {
        visit(
            &app,
            "POST",
            &url,
            &format!("198.51.100.{}", 10 + i),
            None,
            Some(json!({"password": "wrong"})),
        )
        .await;
    }
    let (id,): (Uuid,) = sqlx::query_as("SELECT id FROM shares")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    send(
        &app,
        "PATCH",
        &format!("/assets/shares/{id}"),
        &key,
        Some(json!({"password": "fresh start"})),
    )
    .await;
    let (st, ..) = visit(
        &app,
        "POST",
        &url,
        "198.51.100.50",
        None,
        Some(json!({"password": "fresh start"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    t.teardown().await;
}

/// Everything a visitor can read about a photo with a position, a camera and a person behind it.
#[tokio::test]
async fn nothing_that_identifies_the_owner_or_the_location_is_exposed() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "secret.owner@example.com").await;
    let app = app_with(&t.pool, default_state());
    let a = asset(&t.pool, uid, "SECRETNAME_IMG_0001.jpg").await;
    sqlx::query(
        "UPDATE assets SET latitude = 48.8584, longitude = 2.2945, camera_make = 'SECRETCAM', camera_model = 'SECRETMODEL',
                lens_model = 'SECRETLENS', width = 400, height = 300, thumbhash = '\\x0102030405'::bytea,
                exif = '{\"GPSLatitude\": \"48.8584\", \"Serial\": \"SECRETSERIAL\"}'::jsonb, share_copy_at = now(),
                taken_at = '2024-02-02T10:00:00Z'
         WHERE id = $1",
    )
    .bind(a)
    .execute(&t.pool)
    .await
    .unwrap();
    let (_, token) = share_of(&app, &key, &[a], json!({"title": "Visible title"})).await;
    let (_, album) = send(
        &app,
        "POST",
        "/assets/albums",
        &key,
        Some(json!({"title": "Album", "asset_ids": [a]})),
    )
    .await;
    let (_, atoken) = {
        let (_, v) = send(
            &app,
            "POST",
            "/assets/shares",
            &key,
            Some(json!({"album_id": album["id"]})),
        )
        .await;
        (v["id"].clone(), v["token"].as_str().unwrap().to_string())
    };
    let mut bodies = Vec::new();
    for tok in [&token, &atoken] {
        for path in [
            base(tok),
            format!("{}/items", base(tok)),
            format!("{}/items/{a}", base(tok)),
        ] {
            let (st, _, v) = get(&app, &path).await;
            assert_eq!(st, StatusCode::OK, "{path}");
            bodies.push(v.to_string());
        }
    }
    let all = bodies.join("\n");
    for secret in [
        "48.85",
        "2.2945",
        "latitude",
        "longitude",
        "gps",
        "GPS",
        "SECRET",
        "secret.owner",
        "example.com",
        "exif",
        "camera",
        "lens",
        "owner",
        "original_key",
        "originals/",
        "email",
        "password_hash",
    ] {
        assert!(!all.contains(secret), "a visitor can see {secret:?}:\n{all}");
    }
    // What a visitor does see.
    let (_, _, v) = get(&app, &format!("{}/items/{a}", base(&token))).await;
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "download_pending",
            "downloads",
            "height",
            "id",
            "preview_url",
            "sort_time",
            "thumb_url",
            "thumbhash",
            "width"
        ]
    );
    // The owner's random id is part of the bucket key in a presigned URL (the layout is derived/{owner}/{asset}/...); it
    // identifies nobody and opens nothing without the signature.
    // The image URLs are presigned keys of the derived WebP files, nothing else.
    let thumb = v["thumb_url"].as_str().unwrap();
    let preview = v["preview_url"].as_str().unwrap();
    assert!(thumb.contains(&format!("derived/{uid}/{a}/thumb.webp")), "{thumb}");
    // Short-lived: at most ten minutes (the test storage's own presign TTL is shorter and wins).
    let expires: u64 = thumb
        .split("X-Amz-Expires=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=600).contains(&expires), "{thumb}");
    assert!(preview.contains("preview.webp"));
    assert!(!thumb.contains("originals/") && !preview.contains("originals/"));
    t.teardown().await;
}

async fn jpeg_asset(pool: &PgPool, owner: Uuid, name: &str, copy: bool) -> Uuid {
    let id = asset(pool, owner, name).await;
    sqlx::query(
        "UPDATE assets SET share_copy_at = CASE WHEN $2 THEN now() END, thumbhash = '\\x01'::bytea WHERE id = $1",
    )
    .bind(id)
    .bind(copy)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn downloads_never_hand_out_the_original_unless_the_owner_chose_to() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app_with(&t.pool, default_state());
    let ready = jpeg_asset(&t.pool, uid, "ready.jpg", true).await;
    let waiting = jpeg_asset(&t.pool, uid, "waiting.JPEG", false).await;
    let heic = asset(&t.pool, uid, "phone.heic").await;
    sqlx::query("UPDATE assets SET content_type = 'image/heic' WHERE id = $1")
        .bind(heic)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, token) = share_of(&app, &key, &[ready, waiting, heic], json!({})).await;
    let item = |id: Uuid| {
        let (app, token) = (app.clone(), token.clone());
        async move { get(&app, &format!("{}/items/{id}", base(&token))).await.2 }
    };
    let dl = |id: Uuid, q: String| {
        let (app, token) = (app.clone(), token.clone());
        async move { get(&app, &format!("{}/items/{id}/download{q}", base(&token))).await }
    };

    // Default: a ready JPEG offers its location-free copy; a JPEG still waiting for it says so; HEIC offers nothing.
    let v = item(ready).await;
    assert_eq!(
        (v["downloads"].clone(), v["download_pending"].clone()),
        (
            json!([{"id": ready, "label": "JPEG", "size_bytes": null}]),
            json!(false)
        )
    );
    let v = item(waiting).await;
    assert_eq!(
        (v["downloads"].clone(), v["download_pending"].clone()),
        (json!([]), json!(true))
    );
    let v = item(heic).await;
    assert_eq!(
        (v["downloads"].clone(), v["download_pending"].clone()),
        (json!([]), json!(false))
    );

    let (st, h, _) = dl(ready, String::new()).await;
    assert_eq!(st, StatusCode::TEMPORARY_REDIRECT);
    let loc = h[header::LOCATION].to_str().unwrap();
    assert!(loc.contains(&format!("derived/{uid}/{ready}/share.jpg")), "{loc}");
    assert!(!loc.contains("originals/"), "{loc}");
    assert!(
        loc.contains("response-content-disposition") && loc.contains("ready.jpg"),
        "{loc}"
    );
    assert!(loc.contains("X-Amz-Expires=60"), "{loc}");
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert_eq!(dl(waiting, String::new()).await.0, StatusCode::CONFLICT);
    assert_eq!(
        dl(heic, String::new()).await.0,
        StatusCode::NOT_FOUND,
        "HEIC has no safe copy"
    );
    assert_eq!(
        dl(ready, format!("?file={waiting}")).await.0,
        StatusCode::NOT_FOUND,
        "no other file by the back door"
    );
    assert_eq!(
        dl(ready, format!("?file={}", Uuid::new_v4())).await.0,
        StatusCode::NOT_FOUND
    );

    // prepare queues exactly one copy job, only where one is needed.
    let prep = |id: Uuid| {
        let (app, token) = (app.clone(), token.clone());
        async move {
            visit(
                &app,
                "POST",
                &format!("{}/items/{id}/prepare", base(&token)),
                "203.0.113.1",
                None,
                None,
            )
            .await
        }
    };
    let (st, _, v) = prep(waiting).await;
    assert_eq!((st, v["pending"].clone()), (StatusCode::ACCEPTED, json!(true)));
    prep(waiting).await;
    prep(ready).await;
    prep(heic).await;
    let queued: Vec<(Uuid, String)> = sqlx::query_as("SELECT asset_id, kind FROM jobs WHERE kind = 'share_copy'")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(queued, vec![(waiting, "share_copy".to_string())]);

    // Downloads switched off: nothing is offered and nothing is served, whatever the photo.
    let (_, off) = share_of(
        &app,
        &key,
        &[ready],
        json!({"allow_download": false, "include_originals": true}),
    )
    .await;
    let v = get(&app, &format!("{}/items/{ready}", base(&off))).await.2;
    assert_eq!(
        (v["downloads"].clone(), v["download_pending"].clone()),
        (json!([]), json!(false))
    );
    assert_eq!(
        get(&app, &format!("{}/items/{ready}/download", base(&off))).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(get(&app, &base(&off)).await.2["allow_download"], false);

    // Including originals: the files themselves, with a warning on the owner's side.
    let (_, orig) = share_of(&app, &key, &[heic, ready], json!({"include_originals": true})).await;
    let v = get(&app, &format!("{}/items/{heic}", base(&orig))).await.2;
    assert_eq!(v["downloads"], json!([{"id": heic, "label": "HEIC", "size_bytes": 1}]));
    let (st, h, _) = get(&app, &format!("{}/items/{heic}/download", base(&orig))).await;
    assert_eq!(st, StatusCode::TEMPORARY_REDIRECT);
    assert!(h[header::LOCATION].to_str().unwrap().contains("originals/"));
    t.teardown().await;
}

#[tokio::test]
async fn pairs_offer_the_raw_only_with_originals_and_never_by_id_guessing() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (other, _) = user(&t.pool, "b@example.com").await;
    let app = app_with(&t.pool, default_state());
    let jpg = jpeg_asset(&t.pool, uid, "IMG_1.jpg", true).await;
    let raw = asset(&t.pool, uid, "IMG_1.cr3").await;
    sqlx::query(
        "UPDATE assets SET primary_id = $2, content_type = 'image/x-canon-cr3', size_bytes = 25000000 WHERE id = $1",
    )
    .bind(raw)
    .bind(jpg)
    .execute(&t.pool)
    .await
    .unwrap();
    let foreign = asset(&t.pool, other, "foreign.cr3").await;

    // Sharing by the RAW's id shares the pair once, as its JPEG.
    let (_, token) = share_of(&app, &key, &[raw, jpg], json!({})).await;
    let (_, _, v) = get(&app, &format!("{}/items", base(&token))).await;
    assert_eq!(item_ids(&v), vec![jpg.to_string()], "the RAW is not a photo of its own");
    assert_eq!(get(&app, &base(&token)).await.2["item_count"], 1);
    let it = &v["items"][0];
    assert_eq!(
        it["downloads"],
        json!([{"id": jpg, "label": "JPEG", "size_bytes": null}]),
        "no RAW by default"
    );
    assert_eq!(
        get(&app, &format!("{}/items/{jpg}/download?file={raw}", base(&token)))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &format!("{}/items/{raw}", base(&token))).await.0,
        StatusCode::NOT_FOUND
    );

    let (_, with_orig) = share_of(&app, &key, &[jpg], json!({"include_originals": true})).await;
    let (_, _, v) = get(&app, &format!("{}/items", base(&with_orig))).await;
    assert_eq!(
        v["items"][0]["downloads"],
        json!([{"id": jpg, "label": "JPEG", "size_bytes": 1}, {"id": raw, "label": "RAW", "size_bytes": 25000000}])
    );
    let (st, h, _) = get(&app, &format!("{}/items/{jpg}/download?file={raw}", base(&with_orig))).await;
    assert_eq!(st, StatusCode::TEMPORARY_REDIRECT);
    assert!(h[header::LOCATION].to_str().unwrap().contains("IMG_1.cr3"));
    // Someone else's file cannot be fetched through your link.
    assert_eq!(
        get(
            &app,
            &format!("{}/items/{jpg}/download?file={foreign}", base(&with_orig))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Trashing the pair hides it, restoring brings it back.
    send(&app, "POST", "/assets/trash", &key, Some(ids(&[jpg]))).await;
    assert_eq!(
        get(&app, &format!("{}/items", base(&with_orig))).await.2["items"],
        json!([])
    );
    assert_eq!(
        get(&app, &format!("{}/items/{jpg}/download?file={raw}", base(&with_orig)))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    t.teardown().await;
}

#[tokio::test]
async fn views_are_counted_and_responses_are_not_indexable_or_cacheable() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app_with(&t.pool, default_state());
    let a = asset(&t.pool, uid, "a.jpg").await;
    let (id, token) = share_of(&app, &key, &[a], json!({})).await;
    for _ in 0..3 {
        let (st, h, _) = get(&app, &base(&token)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(h["x-robots-tag"], "noindex, nofollow, noarchive");
        assert_eq!(h[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
    }
    // Listing items is not a view; unknown links answer with the same headers.
    get(&app, &format!("{}/items", base(&token))).await;
    let (st, h, _) = get(&app, &base(&"Z".repeat(32))).await;
    assert_eq!(
        (st, h["x-robots-tag"].to_str().unwrap()),
        (StatusCode::NOT_FOUND, "noindex, nofollow, noarchive")
    );
    let (_, one) = send(&app, "GET", &format!("/assets/shares/{id}"), &key, None).await;
    assert_eq!(one["view_count"], 3);
    assert!(one["last_viewed_at"].is_string());
    t.teardown().await;
}

#[tokio::test]
async fn a_scope_shares_the_whole_view_minus_except_and_never_other_peoples_photos() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (oid, _) = user(&t.pool, "b@example.com").await;
    let app = app_with(&t.pool, default_state());
    let a = asset(&t.pool, uid, "a.jpg").await;
    let b = asset(&t.pool, uid, "b.jpg").await;
    let c = asset(&t.pool, uid, "c.jpg").await;
    let _theirs = asset(&t.pool, oid, "x.jpg").await;
    sqlx::query("UPDATE assets SET favorite = true WHERE id = ANY($1)")
        .bind(vec![a, b])
        .execute(&t.pool)
        .await
        .unwrap();

    let (st, v) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"scope": "library", "except": [c]})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["kind"], "selection");
    assert_eq!(v["item_count"], 2);

    let (st, v) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"scope": "favorites"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["item_count"], 2);

    // An empty result, and a scope mixed with ids, are rejected.
    let all = json!({"scope": "library", "except": [a, b, c]});
    assert_eq!(
        send(&app, "POST", "/assets/shares", &key, Some(all)).await.0,
        StatusCode::BAD_REQUEST
    );
    let mixed = json!({"scope": "library", "asset_ids": [a]});
    assert_eq!(
        send(&app, "POST", "/assets/shares", &key, Some(mixed)).await.0,
        StatusCode::BAD_REQUEST
    );
}
