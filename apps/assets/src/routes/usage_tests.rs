//! Usage through the API: the counters under every request that changes them (upload, trash, restore, delete forever, empty
//! trash, the janitor's purge), the caller's own numbers, owner isolation, and the admin endpoints (admins only, counts
//! only). The upload test needs an S3 API (`S3_ENDPOINT`...); the others need only Postgres.
use axum::http::StatusCode;
use cyanotype_core::usage;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    purge,
    testutil::{FakeObjects, TestDb, app, app_with_s3, asset, ids, send, send_cookie, session, user},
};

async fn mine(app: &axum::Router, key: &str) -> Value {
    let (s, v) = send(app, "GET", "/assets/usage", key, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

/// The maintained numbers equal the recomputed ones.
async fn in_step(pool: &sqlx::PgPool, owner: uuid::Uuid, what: &str) {
    assert_eq!(
        usage::of(pool, owner).await.unwrap(),
        usage::truth_of(pool, owner).await.unwrap(),
        "{what}"
    );
}

#[tokio::test]
async fn trash_restore_delete_forever_and_empty_trash_move_the_numbers() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let a = asset(&t.pool, uid, "a.jpg").await;
    let b = asset(&t.pool, uid, "b.jpg").await;
    let c = asset(&t.pool, uid, "c.jpg").await;
    let d = asset(&t.pool, uid, "d.jpg").await;
    let app = app(&t.pool);

    let u = mine(&app, &key).await;
    assert_eq!(
        (
            u["originals_bytes"].as_i64(),
            u["trash_bytes"].as_i64(),
            u["photo_count"].as_i64()
        ),
        (Some(4), Some(0), Some(4))
    );

    send(&app, "POST", "/assets/trash", &key, Some(ids(&[a, b, c]))).await;
    let u = mine(&app, &key).await;
    assert_eq!(
        (
            u["originals_bytes"].as_i64(),
            u["trash_bytes"].as_i64(),
            u["trash_count"].as_i64()
        ),
        (Some(1), Some(3), Some(3))
    );
    // What is in the trash still counts toward what the account stores.
    assert_eq!(u["stored_bytes"], 4);
    in_step(&t.pool, uid, "trash").await;

    send(&app, "POST", "/assets/restore", &key, Some(ids(&[a]))).await;
    let u = mine(&app, &key).await;
    assert_eq!(
        (u["originals_bytes"].as_i64(), u["trash_bytes"].as_i64()),
        (Some(2), Some(2))
    );
    in_step(&t.pool, uid, "restore").await;

    // Delete forever: the photo leaves the trash at once, but its bytes count until the janitor has removed it.
    send(&app, "POST", "/assets/purge", &key, Some(ids(&[b]))).await;
    in_step(&t.pool, uid, "purge queued").await;
    purge::sweep(&t.pool, &FakeObjects::default()).await.unwrap();
    let u = mine(&app, &key).await;
    assert_eq!(
        (
            u["originals_bytes"].as_i64(),
            u["trash_bytes"].as_i64(),
            u["file_count"].as_i64()
        ),
        (Some(2), Some(1), Some(2))
    );
    in_step(&t.pool, uid, "purge swept").await;

    // Empty trash.
    send(&app, "POST", "/assets/trash/empty", &key, None).await;
    purge::sweep(&t.pool, &FakeObjects::default()).await.unwrap();
    let u = mine(&app, &key).await;
    assert_eq!(
        (
            u["originals_bytes"].as_i64(),
            u["trash_bytes"].as_i64(),
            u["photo_count"].as_i64()
        ),
        (Some(2), Some(0), Some(2))
    );
    in_step(&t.pool, uid, "emptied").await;

    // The janitor's automatic purge of old trash.
    send(&app, "POST", "/assets/trash", &key, Some(ids(&[d]))).await;
    sqlx::query("UPDATE assets SET deleted_at = now() - interval '31 days' WHERE id = $1")
        .bind(d)
        .execute(&t.pool)
        .await
        .unwrap();
    purge::mark_expired(&t.pool, 30).await.unwrap();
    purge::sweep(&t.pool, &FakeObjects::default()).await.unwrap();
    let u = mine(&app, &key).await;
    assert_eq!(
        (u["originals_bytes"].as_i64(), u["trash_bytes"].as_i64()),
        (Some(1), Some(0))
    );
    in_step(&t.pool, uid, "retention purge").await;
    t.teardown().await;
}

#[tokio::test]
async fn an_upload_counts_the_file_and_logs_the_bytes_and_a_re_upload_of_trash_counts_again() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        t.teardown().await;
        return;
    };
    let (uid, key) = user(&t.pool, "up@example.com").await;
    let body = format!("photo bytes {}", uuid::Uuid::new_v4()).into_bytes();
    let sha: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();

    let upload = |label: &'static str| {
        let (app, storage, key, body, sha) = (app.clone(), storage.clone(), key.clone(), body.clone(), sha.clone());
        let pool = t.pool.clone();
        async move {
            let (status, check) = send(
                &app,
                "POST",
                "/assets/check",
                &key,
                Some(json!({"sha256": sha, "size_bytes": body.len(), "filename": "IMG_9.CR3", "content_type": "image/x-canon-cr3"})),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{label}: {check}");
            let upload_id = check["upload_id"].as_str().unwrap().to_string();
            let (okey,): (String,) = sqlx::query_as("SELECT key FROM uploads WHERE upload_id = $1")
                .bind(&upload_id)
                .fetch_one(&pool)
                .await
                .unwrap();
            let etag = storage
                .upload_part(&okey, &upload_id, 1, body.clone().into())
                .await
                .unwrap();
            let (status, done) = send(
                &app,
                "POST",
                "/assets/complete",
                &key,
                Some(json!({"upload_id": upload_id, "parts": [{"number": 1, "etag": etag}]})),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{label}: {done}");
            done["asset_id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap()
        }
    };

    let id = upload("first").await;
    let u = mine(&app, &key).await;
    let n = body.len() as i64;
    assert_eq!(
        (
            u["originals_bytes"].as_i64(),
            u["photo_count"].as_i64(),
            u["raw_count"].as_i64()
        ),
        (Some(n), Some(1), Some(1))
    );
    assert_eq!(
        (u["uploads"]["count_24h"].as_i64(), u["uploads"]["bytes_24h"].as_i64()),
        (Some(1), Some(n))
    );
    in_step(&t.pool, uid, "uploaded").await;

    // Trash it, upload the same content again: the trashed copy comes back (no second file) and the bytes were uploaded.
    send(&app, "POST", "/assets/trash", &key, Some(ids(&[id]))).await;
    assert_eq!(mine(&app, &key).await["trash_bytes"], n);
    // `check` sees the trashed file as absent, so the client uploads again.
    let again = upload("again").await;
    assert_eq!(again, id);
    let u = mine(&app, &key).await;
    assert_eq!(
        (
            u["originals_bytes"].as_i64(),
            u["trash_bytes"].as_i64(),
            u["file_count"].as_i64()
        ),
        (Some(n), Some(0), Some(1))
    );
    assert_eq!(
        (u["uploads"]["count_24h"].as_i64(), u["uploads"]["bytes_24h"].as_i64()),
        (Some(2), Some(2 * n))
    );
    in_step(&t.pool, uid, "re-uploaded").await;
    t.teardown().await;
}

#[tokio::test]
async fn usage_is_per_account_and_the_upload_windows_are_rolling() {
    let Some(t) = TestDb::new().await else { return };
    let (a, ka) = user(&t.pool, "a@example.com").await;
    let (b, kb) = user(&t.pool, "b@example.com").await;
    asset(&t.pool, a, "a1.jpg").await;
    asset(&t.pool, a, "a2.jpg").await;
    asset(&t.pool, b, "b1.jpg").await;
    sqlx::query(
        "INSERT INTO upload_log (owner_id, at, bytes) VALUES
           ($1, now() - interval '1 hour', 10), ($1, now() - interval '25 hours', 20), ($1, now() - interval '29 days', 40),
           ($1, now() - interval '31 days', 80), ($2, now(), 5)",
    )
    .bind(a)
    .bind(b)
    .execute(&t.pool)
    .await
    .unwrap();
    let app = app(&t.pool);
    let ua = mine(&app, &ka).await;
    let ub = mine(&app, &kb).await;
    assert_eq!(
        (ua["photo_count"].as_i64(), ub["photo_count"].as_i64()),
        (Some(2), Some(1))
    );
    assert_eq!(
        (
            ua["uploads"]["count_24h"].as_i64(),
            ua["uploads"]["bytes_24h"].as_i64(),
            ua["uploads"]["count_30d"].as_i64(),
            ua["uploads"]["bytes_30d"].as_i64()
        ),
        (Some(1), Some(10), Some(3), Some(70))
    );
    assert_eq!(
        (ub["uploads"]["count_24h"].as_i64(), ub["uploads"]["bytes_30d"].as_i64()),
        (Some(1), Some(5))
    );
    // Needs credentials.
    let (s, _) = send_cookie(&app, "GET", "/assets/usage", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

#[tokio::test]
async fn the_admin_endpoints_are_for_admin_sessions_only() {
    let Some(t) = TestDb::new().await else { return };
    let (_, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (member_id, member) = session(&t.pool, "member@example.com", "member").await;
    let app = app(&t.pool);
    let paths = [
        ("GET", "/assets/admin/accounts"),
        ("GET", "/assets/admin/bucket-check"),
        ("POST", "/assets/admin/bucket-check"),
    ];
    for (m, p) in paths {
        let (s, _) = send_cookie(&app, m, p, None, None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{m} {p}");
        let (s, _) = send_cookie(&app, m, p, Some(&member), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{m} {p}");
    }
    // An API key does not open them, not even an admin's.
    let key = format!("cyk_{}", uuid::Uuid::new_v4().simple());
    let (admin_id,): (uuid::Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = 'admin@example.com'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO api_keys (user_id, name, key_hash) VALUES ($1,'k',$2)")
        .bind(admin_id)
        .bind(cyanotype_service::auth::sha256(key.as_bytes()))
        .execute(&t.pool)
        .await
        .unwrap();
    for (m, p) in paths {
        let (s, _) = send(&app, m, p, &key, None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{m} {p} with a key");
    }
    // A member's refused request queued nothing and logged nothing.
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM admin_tasks")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let (s, _) = send_cookie(&app, "GET", "/assets/admin/accounts", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    // Their own usage works for a member, of course.
    let (s, _) = send_cookie(&app, "GET", "/assets/usage", Some(&member), None).await;
    assert_eq!(s, StatusCode::OK);
    let _ = member_id;
    t.teardown().await;
}

#[tokio::test]
async fn the_accounts_page_has_counts_and_sizes_and_never_a_file_name() {
    let Some(t) = TestDb::new().await else { return };
    let (admin_id, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (a, _) = user(&t.pool, "alice@example.com").await;
    let secret = "alice-private-holiday-photo.jpg";
    let one = asset(&t.pool, a, secret).await;
    asset(&t.pool, a, "second.dng").await;
    asset(&t.pool, admin_id, "mine.jpg").await;
    sqlx::query("UPDATE assets SET size_bytes = 1000 WHERE id = $1")
        .bind(one)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE original_name = 'second.dng'")
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO albums (owner_id, title) VALUES ($1, 'Secret album title'), ($1, 'Another')")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO shares (owner_id, token, kind, album_id) SELECT $1, repeat('x', 30), 'album', id FROM albums WHERE owner_id = $1 LIMIT 1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO upload_log (owner_id, bytes) VALUES ($1, 1000), ($1, 1)")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    let app = app(&t.pool);

    let (s, v) = send_cookie(&app, "GET", "/assets/admin/accounts", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    let text = v.to_string();
    for private in [
        secret,
        "second.dng",
        "Secret album title",
        "original_name",
        "original_key",
        "alice-private",
    ] {
        assert!(!text.contains(private), "{private} leaked into the accounts page");
    }
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let alice = items.iter().find(|i| i["email"] == "alice@example.com").unwrap();
    assert_eq!(alice["role"], "member");
    assert_eq!(
        (
            alice["originals_bytes"].as_i64(),
            alice["trash_bytes"].as_i64(),
            alice["photo_count"].as_i64(),
            alice["trash_count"].as_i64()
        ),
        (Some(1000), Some(1), Some(1), Some(1))
    );
    assert_eq!(
        (
            alice["albums"].as_i64(),
            alice["shares"].as_i64(),
            alice["api_keys"].as_i64()
        ),
        (Some(2), Some(1), Some(1))
    );
    assert_eq!(
        (alice["uploads_24h_count"].as_i64(), alice["uploads_24h_bytes"].as_i64()),
        (Some(2), Some(1001))
    );
    assert!(alice["last_active"].is_string(), "{alice}");
    assert_eq!(
        items.iter().find(|i| i["email"] == "admin@example.com").unwrap()["role"],
        "admin"
    );
    // Totals add the accounts up.
    assert_eq!(v["totals"]["accounts"], 2);
    assert_eq!(v["totals"]["originals_bytes"], 1001);
    assert_eq!(v["totals"]["photo_count"], 2);
    assert_eq!(v["totals"]["uploads_24h_count"], 2);
    assert!(v["bucket_check"].is_null() && v["reconcile"].is_null());
    t.teardown().await;
}

#[tokio::test]
async fn the_bucket_check_is_queued_once_audited_and_its_result_is_served() {
    let Some(t) = TestDb::new().await else { return };
    let (admin_id, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let app = app(&t.pool);
    let (s, none) = send_cookie(&app, "GET", "/assets/admin/bucket-check", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(none["task"].is_null());

    let (s, first) = send_cookie(&app, "POST", "/assets/admin/bucket-check", Some(&admin), None).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(
        (first["task"]["state"].as_str(), first["already_running"].as_bool()),
        (Some("queued"), Some(false))
    );
    let (_, second) = send_cookie(&app, "POST", "/assets/admin/bucket-check", Some(&admin), None).await;
    assert_eq!(second["already_running"], true);
    assert_eq!(second["task"]["id"], first["task"]["id"]);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM admin_tasks")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    // One audit entry, by the admin.
    let rows: Vec<(Option<uuid::Uuid>, String)> = sqlx::query_as("SELECT actor_id, action FROM audit_log")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(rows, vec![(Some(admin_id), "storage.bucket_check".to_string())]);

    // The worker finishes it; the page then gets the result (with the accounts page carrying it too).
    let task = cyanotype_core::admin_tasks::claim(&t.pool, &["bucket_check"])
        .await
        .unwrap()
        .unwrap();
    cyanotype_core::admin_tasks::finish(&t.pool, task.id, Ok(json!({"originals_bytes": 123})))
        .await
        .unwrap();
    let (_, done) = send_cookie(&app, "GET", "/assets/admin/bucket-check", Some(&admin), None).await;
    assert_eq!(
        (
            done["task"]["state"].as_str(),
            done["task"]["result"]["originals_bytes"].as_i64()
        ),
        (Some("done"), Some(123))
    );
    let (_, accounts) = send_cookie(&app, "GET", "/assets/admin/accounts", Some(&admin), None).await;
    assert_eq!(accounts["bucket_check"]["state"], "done");
    t.teardown().await;
}
