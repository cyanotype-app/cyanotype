//! The admin health card and retry of failed jobs, and what a disabled account can no longer do in this service.
//! The backup test needs an S3 API (`S3_ENDPOINT`...); the rest need only Postgres.
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::testutil::{TestDb, app, app_with_s3, asset, send, send_cookie, session, user};

async fn job(pool: &sqlx::PgPool, asset: Uuid, kind: &str, state: &str, err: Option<&str>) {
    sqlx::query("DELETE FROM jobs WHERE asset_id = $1 AND kind = $2")
        .bind(asset)
        .bind(kind)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO jobs (kind, asset_id, state, attempts, last_error, finished_at)
         VALUES ($1, $2, $3, 5, $4, CASE WHEN $3 = 'failed' THEN now() END)",
    )
    .bind(kind)
    .bind(asset)
    .bind(state)
    .bind(err)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_health_endpoints_are_for_admin_sessions_only() {
    let Some(t) = TestDb::new().await else { return };
    let (_, member) = session(&t.pool, "member@example.com", "member").await;
    let (admin_id, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (_, key) = user(&t.pool, "keyed@example.com").await;
    let app = app(&t.pool);
    for (m, p) in [("GET", "/assets/admin/health"), ("POST", "/assets/admin/jobs/retry")] {
        assert_eq!(
            send_cookie(&app, m, p, None, None).await.0,
            StatusCode::UNAUTHORIZED,
            "{m} {p}"
        );
        assert_eq!(
            send_cookie(&app, m, p, Some(&member), None).await.0,
            StatusCode::FORBIDDEN,
            "{m} {p}"
        );
        assert_eq!(
            send(&app, m, p, &key, None).await.0,
            StatusCode::FORBIDDEN,
            "{m} {p} with a key"
        );
    }
    assert_eq!(
        send_cookie(&app, "GET", "/assets/admin/health", Some(&admin), None)
            .await
            .0,
        StatusCode::OK
    );
    let _ = admin_id;
    t.teardown().await;
}

#[tokio::test]
async fn the_health_card_reports_the_queue_failures_workers_and_sizes_without_names() {
    let Some(t) = TestDb::new().await else { return };
    let (_, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (alice, _) = user(&t.pool, "alice@example.com").await;
    let secret = "alice-private-photo.jpg";
    let a1 = asset(&t.pool, alice, secret).await;
    let a2 = asset(&t.pool, alice, "second.jpg").await;
    sqlx::query("DELETE FROM jobs").execute(&t.pool).await.unwrap();
    job(&t.pool, a1, "thumbnail", "failed", Some("decode refused: too big")).await;
    job(&t.pool, a1, "metadata", "queued", None).await;
    job(&t.pool, a2, "thumbnail", "queued", None).await;
    job(&t.pool, a2, "metadata", "running", None).await;
    sqlx::query("INSERT INTO server_state (key, value) VALUES ('worker:w1', $1), ('worker:old', $1)")
        .bind(json!({"version": "sha-abc1234", "rss_bytes": 300_000_000u64, "budget_used_bytes": 1000, "budget_total_bytes": 640_000_000u64}))
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE server_state SET updated_at = now() - interval '1 hour' WHERE key = 'worker:old'")
        .execute(&t.pool)
        .await
        .unwrap();
    let app = app(&t.pool);

    let (s, h) = send_cookie(&app, "GET", "/assets/admin/health", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK, "{h}");
    let q = |kind: &str| {
        h["queue"]
            .as_array()
            .unwrap()
            .iter()
            .find(|q| q["kind"] == kind)
            .unwrap()
            .clone()
    };
    assert_eq!(
        (q("thumbnail")["queued"].as_i64(), q("thumbnail")["failed"].as_i64()),
        (Some(1), Some(1))
    );
    assert_eq!(
        (q("metadata")["queued"].as_i64(), q("metadata")["running"].as_i64()),
        (Some(1), Some(1))
    );
    let failed = h["failed_jobs"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(
        (failed[0]["kind"].as_str(), failed[0]["last_error"].as_str()),
        (Some("thumbnail"), Some("decode refused: too big"))
    );
    // Workers: the live one reports its memory, the stale one is marked as not reporting.
    let workers = h["workers"].as_array().unwrap();
    let w1 = workers.iter().find(|w| w["host"] == "w1").unwrap();
    assert_eq!(
        (
            w1["reporting"].as_bool(),
            w1["rss_bytes"].as_u64(),
            w1["budget_total_bytes"].as_u64()
        ),
        (Some(true), Some(300_000_000), Some(640_000_000))
    );
    assert_eq!(workers.iter().find(|w| w["host"] == "old").unwrap()["reporting"], false);
    assert!(h["version"]["assets"].is_string());
    assert_eq!(h["version"]["workers"][0]["version"], "sha-abc1234");
    // Sizes.
    assert!(h["storage"]["database_bytes"].as_i64().unwrap() > 0);
    assert_eq!(h["storage"]["stored_bytes"], 2);
    assert!(h["storage"]["bucket_check"].is_null());
    // No file names, no owners, no job ids of anyone's file beyond the queue's own.
    let text = h.to_string();
    for private in [secret, "second.jpg", "alice", "asset_id", "original_name"] {
        assert!(!text.contains(private), "{private} leaked into the health card");
    }
    // A finished bucket check shows up in the sizes.
    sqlx::query(
        "INSERT INTO admin_tasks (kind, state, result, finished_at) VALUES ('bucket_check', 'done', $1, now())",
    )
    .bind(json!({"current_bytes": 123, "noncurrent_bytes": 45, "objects": 6}))
    .execute(&t.pool)
    .await
    .unwrap();
    let (_, h) = send_cookie(&app, "GET", "/assets/admin/health", Some(&admin), None).await;
    assert_eq!(
        (
            h["storage"]["bucket_check"]["current_bytes"].as_i64(),
            h["storage"]["bucket_check"]["noncurrent_bytes"].as_i64()
        ),
        (Some(123), Some(45))
    );
    t.teardown().await;
}

#[tokio::test]
async fn the_backup_card_finds_the_newest_dump_in_the_backup_prefix() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        t.teardown().await;
        return;
    };
    let (_, admin) = session(&t.pool, "admin@example.com", "admin").await;
    // Dumps with far-future names, so they are the newest whatever else the shared test bucket holds.
    let older = "backups/postgres/cyanotype-29980101T031700Z.dump";
    let newer = "backups/postgres/cyanotype-29990102T031700Z.dump";
    storage
        .put_bytes(older, vec![0; 100], "application/octet-stream")
        .await
        .unwrap();
    storage
        .put_bytes(newer, vec![0; 2048], "application/octet-stream")
        .await
        .unwrap();
    storage
        .put_bytes("backups/postgres/not-a-dump.txt", vec![0; 5], "text/plain")
        .await
        .unwrap();

    let (_, h) = send_cookie(&app, "GET", "/assets/admin/health", Some(&admin), None).await;
    let b = &h["backup"];
    assert_eq!(b["prefix"], "backups/postgres");
    assert_eq!(
        (b["last_at"].as_str(), b["last_size_bytes"].as_i64()),
        (Some("2999-01-02T03:17:00Z"), Some(2048)),
        "{b}"
    );
    assert!(b["count"].as_i64().unwrap() >= 2);
    storage
        .delete_all_versions(&[older.into(), newer.into(), "backups/postgres/not-a-dump.txt".into()])
        .await
        .unwrap();
    t.teardown().await;
}

#[tokio::test]
async fn a_bucket_that_cannot_be_listed_is_reported_not_fatal() {
    let Some(t) = TestDb::new().await else { return };
    let (_, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let app = app(&t.pool); // storage points nowhere
    let (s, h) = send_cookie(&app, "GET", "/assets/admin/health", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h["backup"]["error"].is_string(), "{h}");
    assert!(h["queue"].is_array());
    t.teardown().await;
}

#[tokio::test]
async fn retrying_failed_jobs_requeues_them_for_every_account_by_kind_and_is_audited() {
    let Some(t) = TestDb::new().await else { return };
    let (admin_id, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (alice, _) = user(&t.pool, "alice@example.com").await;
    let (bob, _) = user(&t.pool, "bob@example.com").await;
    let (a, b) = (asset(&t.pool, alice, "a.jpg").await, asset(&t.pool, bob, "b.jpg").await);
    sqlx::query("DELETE FROM jobs").execute(&t.pool).await.unwrap();
    job(&t.pool, a, "thumbnail", "failed", Some("boom")).await;
    job(&t.pool, a, "metadata", "failed", Some("boom")).await;
    job(&t.pool, b, "thumbnail", "failed", Some("boom")).await;
    job(&t.pool, b, "metadata", "running", None).await;
    let app = app(&t.pool);
    let state = |asset: Uuid, kind: &'static str| {
        let pool = t.pool.clone();
        async move {
            sqlx::query_as::<_, (String, i32)>("SELECT state, attempts FROM jobs WHERE asset_id = $1 AND kind = $2")
                .bind(asset)
                .bind(kind)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    // One kind: only that kind's failures, both accounts.
    let (s, v) = send_cookie(
        &app,
        "POST",
        "/assets/admin/jobs/retry",
        Some(&admin),
        Some(json!({"kind": "thumbnail"})),
    )
    .await;
    assert_eq!((s, v["requeued"].as_i64()), (StatusCode::OK, Some(2)));
    assert_eq!(state(a, "thumbnail").await, ("queued".into(), 0));
    assert_eq!(state(b, "thumbnail").await, ("queued".into(), 0));
    assert_eq!(state(a, "metadata").await.0, "failed", "another kind stays failed");
    assert_eq!(state(b, "metadata").await.0, "running", "running jobs are left alone");

    // Everything else, with no body at all; nothing is left the second time.
    let (_, v) = send_cookie(&app, "POST", "/assets/admin/jobs/retry", Some(&admin), None).await;
    assert_eq!(v["requeued"], 1);
    assert_eq!(state(a, "metadata").await, ("queued".into(), 0));
    let (_, v) = send_cookie(&app, "POST", "/assets/admin/jobs/retry", Some(&admin), None).await;
    assert_eq!(v["requeued"], 0);

    // A file being purged is not brought back to life.
    job(&t.pool, a, "thumbnail", "failed", Some("boom")).await;
    sqlx::query("UPDATE assets SET purging_at = now(), deleted_at = now() WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, v) = send_cookie(&app, "POST", "/assets/admin/jobs/retry", Some(&admin), None).await;
    assert_eq!(v["requeued"], 0);

    let rows: Vec<(Option<Uuid>, Value)> =
        sqlx::query_as("SELECT actor_id, detail FROM audit_log WHERE action = 'jobs.retry' ORDER BY id")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().all(|r| r.0 == Some(admin_id)));
    assert_eq!(
        (rows[0].1["kind"].as_str(), rows[0].1["requeued"].as_i64()),
        (Some("thumbnail"), Some(2))
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_disabled_account_is_refused_with_its_session_and_its_keys_until_it_is_enabled() {
    let Some(t) = TestDb::new().await else { return };
    let (_, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (uid, cookie) = session(&t.pool, "sam@example.com", "member").await;
    let key = format!("cyk_{}", Uuid::new_v4().simple());
    sqlx::query("INSERT INTO api_keys (user_id, name, key_hash) VALUES ($1, 'k', $2)")
        .bind(uid)
        .bind(cyanotype_service::auth::sha256(key.as_bytes()))
        .execute(&t.pool)
        .await
        .unwrap();
    let app = app(&t.pool);
    assert_eq!(
        send_cookie(&app, "GET", "/assets/usage", Some(&cookie), None).await.0,
        StatusCode::OK
    );
    assert_eq!(send(&app, "GET", "/assets/usage", &key, None).await.0, StatusCode::OK);

    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
        .bind(uid)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        send_cookie(&app, "GET", "/assets/usage", Some(&cookie), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app, "GET", "/assets/usage", &key, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    // Uploads too (the check endpoint authenticates the same way), and the key is not stamped as used.
    assert_eq!(
        send(
            &app,
            "POST",
            "/assets/check",
            &key,
            Some(json!({"sha256": "00".repeat(32), "size_bytes": 1, "filename": "a.jpg"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let used_after: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT last_used_at FROM api_keys WHERE user_id = $1")
            .bind(uid)
            .fetch_one(&t.pool)
            .await
            .unwrap();
    sqlx::query("UPDATE users SET disabled_at = NULL WHERE id = $1")
        .bind(uid)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        send(&app, "GET", "/assets/usage", &key, None).await.0,
        StatusCode::OK,
        "the key is back"
    );
    assert_eq!(
        send_cookie(&app, "GET", "/assets/usage", Some(&cookie), None).await.0,
        StatusCode::OK
    );
    assert!(used_after.is_some(), "stamped by the first, allowed call only");

    // A disabled admin loses the admin endpoints as well.
    sqlx::query("UPDATE users SET disabled_at = now() WHERE email = 'admin@example.com'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        send_cookie(&app, "GET", "/assets/admin/health", Some(&admin), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    t.teardown().await;
}

#[tokio::test]
async fn the_accounts_page_marks_disabled_accounts_and_pending_deletions() {
    let Some(t) = TestDb::new().await else { return };
    let (_, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (a, _) = user(&t.pool, "alice@example.com").await;
    let (b, _) = user(&t.pool, "bob@example.com").await;
    user(&t.pool, "carol@example.com").await;
    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = ANY($1)")
        .bind(vec![a, b])
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO admin_tasks (kind, state, params) VALUES ('delete_account', 'queued', $1)")
        .bind(json!({"user_id": b, "email": "bob@example.com"}))
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO admin_tasks (kind, state, params, error) VALUES ('delete_account', 'failed', $1, 'jobs still running')")
        .bind(json!({"user_id": a, "email": "alice@example.com"}))
        .execute(&t.pool)
        .await
        .unwrap();
    let app = app(&t.pool);
    let (_, v) = send_cookie(&app, "GET", "/assets/admin/accounts", Some(&admin), None).await;
    let by = |email: &str| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["email"] == email)
            .unwrap()
            .clone()
    };
    assert!(by("alice@example.com")["disabled_at"].is_string());
    assert_eq!(
        (
            by("alice@example.com")["deletion"].as_str(),
            by("alice@example.com")["deletion_error"].as_str()
        ),
        (Some("failed"), Some("jobs still running"))
    );
    assert_eq!(by("bob@example.com")["deletion"], "queued");
    assert!(by("carol@example.com")["disabled_at"].is_null() && by("carol@example.com")["deletion"].is_null());
    t.teardown().await;
}
