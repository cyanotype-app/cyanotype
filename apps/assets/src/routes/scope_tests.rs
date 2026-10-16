//! Scope-based bulk operations ("select all" over a whole view) against a real Postgres.
use std::time::{Duration, Instant};

use axum::{Router, http::StatusCode};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::testutil::{TestDb, app, asset, send, user};

async fn many(pool: &PgPool, owner: Uuid, prefix: &str, n: i32) -> Vec<Uuid> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, taken_at)
         SELECT $1, sha256((($2 || i)::text)::bytea), 'originals/' || $2 || i, $2 || i, 'image/jpeg', 1,
                now() - make_interval(secs => i)
         FROM generate_series(1, $3) i RETURNING id",
    )
    .bind(owner)
    .bind(prefix)
    .bind(n)
    .fetch_all(pool)
    .await
    .unwrap();
    // Like a database that autovacuum has looked at: planner statistics for the freshly inserted rows.
    sqlx::query("ANALYZE assets").execute(pool).await.unwrap();
    ids
}

async fn n(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

async fn post(app: &Router, key: &str, path: &str, body: serde_json::Value) -> serde_json::Value {
    let (st, b) = send(app, "POST", path, key, Some(body)).await;
    assert_eq!(st, StatusCode::OK, "{path}: {b}");
    b
}

async fn album(app: &Router, key: &str, title: &str) -> String {
    let (st, b) = send(app, "POST", "/assets/albums", key, Some(json!({"title": title}))).await;
    assert_eq!(st, StatusCode::CREATED);
    b["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn scope_trash_and_undo_respect_except_owner_and_pairs() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "alice@example.com").await;
    let (bob, kb) = user(&t.pool, "bob@example.com").await;
    let app = app(&t.pool);
    let mine = many(&t.pool, alice, "a", 30).await;
    let theirs = many(&t.pool, bob, "b", 5).await;
    // A RAW + JPEG pair: the RAW is hidden and moves with its primary.
    let jpg = asset(&t.pool, alice, "p.jpg").await;
    let raw = asset(&t.pool, alice, "p.dng").await;
    sqlx::query("UPDATE assets SET primary_id = $1 WHERE id = $2")
        .bind(jpg)
        .bind(raw)
        .execute(&t.pool)
        .await
        .unwrap();
    // Already trashed before: must not be part of the batch, nor restored by its Undo.
    sqlx::query("UPDATE assets SET deleted_at = now() - interval '1 day' WHERE id = $1")
        .bind(mine[0])
        .execute(&t.pool)
        .await
        .unwrap();

    // 30 + the pair = 31 photos, 1 trashed already, 2 excepted => 28.
    let b = post(
        &app,
        &ka,
        "/assets/trash",
        json!({"scope": "library", "except": [mine[1], mine[2], Uuid::new_v4()]}),
    )
    .await;
    assert_eq!(b["count"], 28);
    let batch = b["batch"].as_str().unwrap().to_string();
    assert_eq!(
        n(
            &t.pool,
            &format!("SELECT count(*) FROM assets WHERE owner_id='{alice}' AND deleted_at IS NULL")
        )
        .await,
        2
    );
    assert_eq!(
        n(
            &t.pool,
            &format!("SELECT count(*) FROM assets WHERE owner_id='{bob}' AND deleted_at IS NULL")
        )
        .await,
        5
    );
    assert!(
        n(
            &t.pool,
            &format!("SELECT count(*) FROM assets WHERE id='{raw}' AND deleted_at IS NOT NULL")
        )
        .await
            == 1
    );
    // Bob cannot reach Alice's photos with the same call.
    assert_eq!(
        post(&app, &kb, "/assets/trash", json!({"scope": "library"})).await["count"],
        5
    );
    sqlx::query("UPDATE assets SET deleted_at = NULL WHERE owner_id = $1")
        .bind(bob)
        .execute(&t.pool)
        .await
        .unwrap();
    let _ = theirs;

    // Undo restores exactly the batch (not the one trashed yesterday), the pair as one.
    let b = post(&app, &ka, "/assets/restore", json!({"scope": {"trash_batch": batch}})).await;
    assert_eq!(b["count"], 28);
    assert_eq!(
        n(
            &t.pool,
            &format!("SELECT count(*) FROM assets WHERE owner_id='{alice}' AND deleted_at IS NULL")
        )
        .await,
        31
    );
    t.teardown().await;
}

#[tokio::test]
async fn scope_validation() {
    let Some(t) = TestDb::new().await else { return };
    let (_, k) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    for body in [
        json!({"scope": "library", "ids": [Uuid::new_v4()]}),
        json!({"except": [Uuid::new_v4()]}),
        json!({"scope": "nonsense"}),
        json!({"scope": {"album": Uuid::new_v4()}}),
    ] {
        let (st, _) = send(&app, "POST", "/assets/trash", &k, Some(body.clone())).await;
        assert!(st.is_client_error(), "{body}");
    }
    let too_many: Vec<Uuid> = (0..20_001).map(|_| Uuid::new_v4()).collect();
    let (st, _) = send(
        &app,
        "POST",
        "/assets/trash",
        &k,
        Some(json!({"scope": "library", "except": too_many})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Someone else's album is a 404.
    let (_, other) = user(&t.pool, "o@example.com").await;
    let al = album(&app, &other, "theirs").await;
    let (st, _) = send(&app, "POST", "/assets/trash", &k, Some(json!({"scope": {"album": al}}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[tokio::test]
async fn scope_favorites_albums_trash_purge() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let ids = many(&t.pool, uid, "x", 20).await;
    let jpg = asset(&t.pool, uid, "p.jpg").await;
    let raw = asset(&t.pool, uid, "p.dng").await;
    sqlx::query("UPDATE assets SET primary_id = $1 WHERE id = $2")
        .bind(jpg)
        .bind(raw)
        .execute(&t.pool)
        .await
        .unwrap();

    // Favorite everything but two (21 photos incl. the pair, 19 changed); a repeat changes nothing.
    let b = post(
        &app,
        &key,
        "/assets/favorite",
        json!({"scope": "library", "except": [ids[0], ids[1]], "favorite": true}),
    )
    .await;
    assert_eq!(b["count"], 19);
    assert_eq!(
        post(
            &app,
            &key,
            "/assets/favorite",
            json!({"scope": "library", "except": [ids[0], ids[1]], "favorite": true})
        )
        .await["count"],
        0
    );
    assert_eq!(
        n(
            &t.pool,
            "SELECT count(*) FROM assets WHERE favorite AND primary_id IS NULL"
        )
        .await,
        19
    );
    assert_eq!(
        n(
            &t.pool,
            "SELECT count(*) FROM assets WHERE favorite AND primary_id IS NOT NULL"
        )
        .await,
        0,
        "RAW stays unflagged"
    );
    // Unfavorite within the favorites view, keeping one.
    let b = post(
        &app,
        &key,
        "/assets/favorite",
        json!({"scope": "favorites", "except": [ids[2]], "favorite": false}),
    )
    .await;
    assert_eq!(b["count"], 18);

    // Albums: add the whole library, then remove the whole album minus one.
    let al = album(&app, &key, "all").await;
    let b = post(
        &app,
        &key,
        &format!("/assets/albums/{al}/assets"),
        json!({"scope": "library"}),
    )
    .await;
    assert_eq!(b["count"], 22 - 1, "21 photos + ... pair counted once");
    let b = post(
        &app,
        &key,
        &format!("/assets/albums/{al}/assets"),
        json!({"scope": "library"}),
    )
    .await;
    assert_eq!(b["count"], 0);
    let b = post(
        &app,
        &key,
        &format!("/assets/albums/{al}/assets/remove"),
        json!({"scope": {"album": al}, "except": [ids[5]]}),
    )
    .await;
    assert_eq!(b["count"], 20);
    assert_eq!(n(&t.pool, "SELECT count(*) FROM album_assets").await, 1);
    // Copy one album's content into another.
    let al2 = album(&app, &key, "copy").await;
    assert_eq!(
        post(
            &app,
            &key,
            &format!("/assets/albums/{al2}/assets"),
            json!({"scope": {"album": al}})
        )
        .await["count"],
        1
    );

    // Trash an album's photos, then purge the trash view minus one.
    post(&app, &key, "/assets/trash", json!({"scope": "library"})).await;
    let b = post(
        &app,
        &key,
        "/assets/purge",
        json!({"scope": "trash", "except": [ids[5], ids[6]]}),
    )
    .await;
    assert_eq!(b["count"], 19);
    assert_eq!(
        n(&t.pool, "SELECT count(*) FROM assets WHERE purging_at IS NOT NULL").await,
        20,
        "pair purged together"
    );
    let b = post(&app, &key, "/assets/restore", json!({"scope": "trash"})).await;
    assert_eq!(b["count"], 2);
    t.teardown().await;
}

#[tokio::test]
async fn five_thousand_photos_in_one_request() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    many(&t.pool, uid, "big", 5000).await;
    let al = album(&app, &key, "big").await;
    let start = Instant::now();
    assert_eq!(
        post(
            &app,
            &key,
            "/assets/favorite",
            json!({"scope": "library", "favorite": true})
        )
        .await["count"],
        5000
    );
    assert_eq!(
        post(
            &app,
            &key,
            &format!("/assets/albums/{al}/assets"),
            json!({"scope": "favorites"})
        )
        .await["count"],
        5000
    );
    let b = post(&app, &key, "/assets/trash", json!({"scope": {"album": al}})).await;
    assert_eq!(b["count"], 5000);
    assert_eq!(
        post(
            &app,
            &key,
            "/assets/restore",
            json!({"scope": {"trash_batch": b["batch"]}})
        )
        .await["count"],
        5000
    );
    assert_eq!(
        post(
            &app,
            &key,
            &format!("/assets/albums/{al}/assets/remove"),
            json!({"scope": {"album": al}})
        )
        .await["count"],
        5000
    );
    assert_eq!(
        post(&app, &key, "/assets/trash", json!({"scope": "library"})).await["count"],
        5000
    );
    assert_eq!(
        post(&app, &key, "/assets/purge", json!({"scope": "trash"})).await["count"],
        5000
    );
    assert!(start.elapsed() < Duration::from_secs(30), "took {:?}", start.elapsed());
    t.teardown().await;
}
