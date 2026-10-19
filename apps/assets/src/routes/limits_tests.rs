//! Per-account limits through the API (docs/admin.md, "Limits"): the largest file, the storage quota (trash included), the
//! rolling daily cap, uploads racing each other, parts that add up to more than was declared, and the admin's editing.
//! The upload tests need an S3 API (`S3_ENDPOINT`...); they return early without one.
use axum::{Router, http::StatusCode};
use cyanotype_core::{limits::Limits, storage::Storage};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::testutil::{TestDb, app, app_with_s3, asset, send, send_cookie, session, user};

fn sha_of(n: u128) -> String {
    format!("{:064x}", n + 1_000_000)
}

async fn declare(app: &Router, key: &str, sha: &str, size: i64) -> (StatusCode, Value) {
    send(
        app,
        "POST",
        "/assets/check",
        key,
        Some(json!({"sha256": sha, "size_bytes": size, "filename": "IMG_1.jpg", "content_type": "image/jpeg", "transport": "proxy"})),
    )
    .await
}

async fn set_limits(pool: &sqlx::PgPool, uid: Uuid, quota: Option<i64>, daily: Option<i64>, max_file: Option<i64>) {
    cyanotype_core::limits::set(
        pool,
        uid,
        Limits {
            quota_bytes: quota,
            daily_upload_bytes: daily,
            max_file_bytes: max_file,
        },
    )
    .await
    .unwrap();
}

async fn with_s3(t: &TestDb) -> Option<(Router, Storage)> {
    app_with_s3(&t.pool)
}

#[tokio::test]
async fn the_largest_file_is_enforced_exactly() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    set_limits(&t.pool, uid, None, None, Some(100)).await;
    assert_eq!(declare(&app, &key, &sha_of(1), 100).await.0, StatusCode::OK);
    let (s, body) = declare(&app, &key, &sha_of(2), 101).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["code"], "file_too_large");
    assert_eq!(body["max_file_bytes"], 100);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("largest file your account may upload")
    );
    // Another account, no limit: the same file is fine.
    let (_, other) = user(&t.pool, "b@example.com").await;
    assert_eq!(declare(&app, &other, &sha_of(2), 101).await.0, StatusCode::OK);
    t.teardown().await;
}

#[tokio::test]
async fn the_quota_counts_stored_files_trash_and_unfinished_uploads_and_allows_exactly_the_limit() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let stored = asset(&t.pool, uid, "stored.jpg").await;
    sqlx::query("UPDATE assets SET size_bytes = 600 WHERE id = $1")
        .bind(stored)
        .execute(&t.pool)
        .await
        .unwrap();
    set_limits(&t.pool, uid, Some(1000), None, None).await;

    // 600 stored: 400 more fits exactly, and the declared 400 is then reserved.
    assert_eq!(declare(&app, &key, &sha_of(1), 400).await.0, StatusCode::OK);
    let (s, body) = declare(&app, &key, &sha_of(2), 1).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["code"], "quota_exceeded");
    assert_eq!(
        (
            body["quota_bytes"].as_i64(),
            body["stored_bytes"].as_i64(),
            body["reserved_bytes"].as_i64(),
            body["remaining_bytes"].as_i64()
        ),
        (Some(1000), Some(600), Some(400), Some(0))
    );
    assert!(body["error"].as_str().unwrap().contains("Not enough storage"));
    assert!(body["error"].as_str().unwrap().contains("trash"));

    // The same upload asked for again (a resume) is not counted twice.
    assert_eq!(declare(&app, &key, &sha_of(1), 400).await.0, StatusCode::OK);

    // Trash counts until it is purged: trashing the 600 frees nothing...
    send(&app, "POST", "/assets/trash", &key, Some(json!({"ids": [stored]}))).await;
    assert_eq!(
        declare(&app, &key, &sha_of(3), 1).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    // ...until the row is gone.
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(stored)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(declare(&app, &key, &sha_of(3), 600).await.0, StatusCode::OK);
    assert_eq!(
        declare(&app, &key, &sha_of(4), 1).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    t.teardown().await;
}

#[tokio::test]
async fn re_uploading_a_trashed_file_at_the_quota_adds_nothing_to_what_is_stored() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let id = asset(&t.pool, uid, "again.jpg").await;
    sqlx::query("UPDATE assets SET size_bytes = 1000, sha256 = decode($2, 'hex'), deleted_at = now() WHERE id = $1")
        .bind(id)
        .bind(sha_of(9))
        .execute(&t.pool)
        .await
        .unwrap();
    set_limits(&t.pool, uid, Some(1000), None, None).await;
    // Completing it revives the trashed file, so it is allowed even though a new 1000-byte file would not be.
    assert_eq!(declare(&app, &key, &sha_of(9), 1000).await.0, StatusCode::OK);
    assert_eq!(
        declare(&app, &key, &sha_of(10), 1000).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    t.teardown().await;
}

#[tokio::test]
async fn no_limit_means_unlimited_and_an_admin_starts_that_way() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    let (_, key) = user(&t.pool, "a@example.com").await;
    let (s, body) = declare(&app, &key, &sha_of(1), 50 * 1024 * 1024 * 1024).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (id, _) = session(&t.pool, "admin@example.com", "admin").await;
    assert!(cyanotype_core::limits::of(&t.pool, id).await.unwrap().is_unlimited());
    t.teardown().await;
}

#[tokio::test]
async fn the_daily_cap_is_a_rolling_day_with_a_reset_time() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    set_limits(&t.pool, uid, None, Some(1000), None).await;
    // 600 two hours ago, 300 one hour ago, and 5000 more than a day ago (which no longer counts).
    sqlx::query(
        "INSERT INTO upload_log (owner_id, at, bytes) VALUES
           ($1, now() - interval '2 hours', 600), ($1, now() - interval '1 hour', 300), ($1, now() - interval '25 hours', 5000)",
    )
    .bind(uid)
    .execute(&t.pool)
    .await
    .unwrap();
    // 900 used: exactly 100 more fits.
    assert_eq!(declare(&app, &key, &sha_of(1), 100).await.0, StatusCode::OK);
    // Now 1000 is used (the declared 100 counts at once): one byte more is refused until the oldest upload rolls out.
    let (s, body) = declare(&app, &key, &sha_of(2), 1).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["code"], "daily_cap_exceeded");
    assert_eq!(
        (body["daily_upload_bytes"].as_i64(), body["used_bytes"].as_i64()),
        (Some(1000), Some(1000))
    );
    // Freed by the 600 from two hours ago: 22 hours from now.
    let secs = body["retry_after_secs"].as_i64().unwrap();
    assert!((22 * 3600 - 60..=22 * 3600 + 60).contains(&secs), "{secs}");
    let retry = time::OffsetDateTime::parse(
        body["retry_at"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert!(((retry - time::OffsetDateTime::now_utc()).whole_seconds() - secs).abs() < 5);
    assert!(body["error"].as_str().unwrap().contains("Uploads can resume at"));
    // A file bigger than the whole cap can never fit: a 413, with no reset time.
    let (s, body) = declare(&app, &key, &sha_of(3), 1001).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["code"], "file_exceeds_daily_cap");
    // When the day has rolled over, uploading works again.
    sqlx::query("UPDATE upload_log SET at = at - interval '23 hours'")
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM uploads").execute(&t.pool).await.unwrap();
    assert_eq!(declare(&app, &key, &sha_of(4), 1000).await.0, StatusCode::OK);
    t.teardown().await;
}

#[tokio::test]
async fn the_429_carries_retry_after() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    set_limits(&t.pool, uid, None, Some(10), None).await;
    sqlx::query("INSERT INTO upload_log (owner_id, at, bytes) VALUES ($1, now() - interval '1 hour', 10)")
        .bind(uid)
        .execute(&t.pool)
        .await
        .unwrap();
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let req = Request::builder()
        .method("POST")
        .uri("/assets/check")
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"sha256": sha_of(1), "size_bytes": 5, "filename": "a.jpg"}).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    let secs: i64 = res.headers()["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((23 * 3600 - 60..=23 * 3600 + 60).contains(&secs), "{secs}");
    t.teardown().await;
}

#[tokio::test]
async fn concurrent_uploads_cannot_pass_the_quota_together() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    set_limits(&t.pool, uid, Some(1000), Some(1000), None).await;
    let mut tasks = Vec::new();
    for i in 0..12u128 {
        let (app, key) = (app.clone(), key.clone());
        tasks.push(tokio::spawn(async move { declare(&app, &key, &sha_of(i), 300).await }));
    }
    let (mut ok, mut quota, mut daily) = (0, 0, 0);
    for t in tasks {
        let (s, body) = t.await.unwrap();
        match (s, body["code"].as_str()) {
            (StatusCode::OK, _) => ok += 1,
            (StatusCode::PAYLOAD_TOO_LARGE, Some("quota_exceeded")) => quota += 1,
            (StatusCode::TOO_MANY_REQUESTS, Some("daily_cap_exceeded")) => daily += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    // Three of 300 fit in 1000; whichever limit the later ones hit, no more than three got through.
    assert_eq!(ok, 3, "ok {ok}, quota {quota}, daily {daily}");
    assert_eq!(ok + quota + daily, 12);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM uploads")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 3);
    t.teardown().await;
}

#[tokio::test]
async fn an_abandoned_upload_stops_reserving_quota_after_a_day() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    set_limits(&t.pool, uid, Some(1000), None, None).await;
    assert_eq!(declare(&app, &key, &sha_of(1), 1000).await.0, StatusCode::OK);
    assert_eq!(
        declare(&app, &key, &sha_of(2), 1).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    sqlx::query("UPDATE uploads SET created_at = now() - interval '25 hours'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(declare(&app, &key, &sha_of(2), 1000).await.0, StatusCode::OK);
    t.teardown().await;
}

#[tokio::test]
async fn parts_that_add_up_to_more_than_was_declared_never_become_a_file() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    set_limits(&t.pool, uid, Some(1000), None, None).await;
    // Declares 10 bytes (and so reserves 10), then sends 500 straight to storage as a client ignoring the part layout could.
    let body = vec![7u8; 500];
    let sha: String = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect()
    };
    let (s, check) = declare(&app, &key, &sha, 10).await;
    assert_eq!(s, StatusCode::OK, "{check}");
    let upload_id = check["upload_id"].as_str().unwrap().to_string();
    let (okey,): (String,) = sqlx::query_as("SELECT key FROM uploads WHERE upload_id = $1")
        .bind(&upload_id)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let etag = storage
        .upload_part(&okey, &upload_id, 1, body.clone().into())
        .await
        .unwrap();
    let (s, done) = send(
        &app,
        "POST",
        "/assets/complete",
        &key,
        Some(json!({"upload_id": upload_id, "parts": [{"number": 1, "etag": etag}]})),
    )
    .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{done}");
    assert_eq!(done["code"], "upload_larger_than_declared");
    assert_eq!(
        (done["declared_bytes"].as_i64(), done["received_bytes"].as_i64()),
        (Some(10), Some(500))
    );
    // Discarded: no file, no pending upload, no object, nothing counted.
    let (assets, pending): (i64, i64) =
        sqlx::query_as("SELECT (SELECT count(*) FROM assets), (SELECT count(*) FROM uploads)")
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert_eq!((assets, pending), (0, 0));
    assert!(storage.head_size(&okey).await.unwrap().is_none());
    let (stored,): (i64,) =
        sqlx::query_as("SELECT coalesce(sum(originals_bytes + trash_bytes), 0)::bigint FROM user_usage")
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert_eq!(stored, 0);
    t.teardown().await;
}

#[tokio::test]
async fn a_proxied_part_larger_than_the_declared_layout_is_refused() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    set_limits(&t.pool, uid, Some(1000), None, None).await;
    let (_, check) = declare(&app, &key, &sha_of(1), 10).await;
    let upload_id = check["upload_id"].as_str().unwrap();
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    for len in [11usize, 500_000] {
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/assets/uploads/{upload_id}/parts/1"))
            .header("authorization", format!("Bearer {key}"))
            .header("content-length", len.to_string())
            .body(Body::from(vec![1u8; len]))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert!(
            res.status() == StatusCode::BAD_REQUEST || res.status() == StatusCode::PAYLOAD_TOO_LARGE,
            "{len}: {}",
            res.status()
        );
    }
    // A second part of a one-part upload is refused too.
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/assets/uploads/{upload_id}/parts/2"))
        .header("authorization", format!("Bearer {key}"))
        .header("content-length", "10")
        .body(Body::from(vec![1u8; 10]))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    t.teardown().await;
}

// ---- the admin's side

#[tokio::test]
async fn admins_edit_limits_and_every_edit_is_audited() {
    let Some(t) = TestDb::new().await else { return };
    let (admin_id, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (_, member) = session(&t.pool, "member@example.com", "member").await;
    let (uid, _) = user(&t.pool, "sam@example.com").await;
    let app = app(&t.pool);
    let path = format!("/assets/admin/accounts/{uid}/limits");
    let body = json!({"quota_bytes": 5000, "daily_upload_bytes": 1000, "max_file_bytes": null});

    // Members and anonymous callers cannot, and nothing changes.
    assert_eq!(
        send_cookie(&app, "PUT", &path, Some(&member), Some(body.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send_cookie(&app, "PUT", &path, None, Some(body.clone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(cyanotype_core::limits::of(&t.pool, uid).await.unwrap().is_unlimited());

    let (s, v) = send_cookie(&app, "PUT", &path, Some(&admin), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        cyanotype_core::limits::of(&t.pool, uid).await.unwrap(),
        Limits {
            quota_bytes: Some(5000),
            daily_upload_bytes: Some(1000),
            max_file_bytes: None
        }
    );
    // Back to unlimited by leaving the fields out.
    assert_eq!(
        send_cookie(&app, "PUT", &path, Some(&admin), Some(json!({}))).await.0,
        StatusCode::OK
    );
    assert!(cyanotype_core::limits::of(&t.pool, uid).await.unwrap().is_unlimited());
    // Bad values and unknown accounts.
    for bad in [
        json!({"quota_bytes": 0}),
        json!({"daily_upload_bytes": -5}),
        json!({"max_file_bytes": 0}),
    ] {
        assert_eq!(
            send_cookie(&app, "PUT", &path, Some(&admin), Some(bad)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let unknown = format!("/assets/admin/accounts/{}/limits", Uuid::new_v4());
    assert_eq!(
        send_cookie(&app, "PUT", &unknown, Some(&admin), Some(json!({})))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // Two audited edits, with before and after, by the admin; the failures left nothing.
    let rows: Vec<(Option<Uuid>, String, Option<String>, Value)> = sqlx::query_as(
        "SELECT actor_id, action, target_id, detail FROM audit_log WHERE action = 'account.limits' ORDER BY id",
    )
    .fetch_all(&t.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0].0, rows[0].2.as_deref()),
        (Some(admin_id), Some(uid.to_string().as_str()))
    );
    assert_eq!(rows[0].3["before"]["quota_bytes"], Value::Null);
    assert_eq!(rows[0].3["after"]["quota_bytes"], 5000);
    assert_eq!(rows[0].3["email"], "sam@example.com");
    assert_eq!(rows[1].3["before"]["quota_bytes"], 5000);
    assert_eq!(rows[1].3["after"]["quota_bytes"], Value::Null);
    t.teardown().await;
}

#[tokio::test]
async fn the_server_default_for_new_accounts_is_admin_only_and_audited() {
    let Some(t) = TestDb::new().await else { return };
    let (_, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (_, member) = session(&t.pool, "member@example.com", "member").await;
    let app = app(&t.pool);
    assert_eq!(
        send_cookie(&app, "GET", "/assets/admin/default-limits", Some(&member), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send_cookie(
            &app,
            "PUT",
            "/assets/admin/default-limits",
            Some(&member),
            Some(json!({"quota_bytes": 1}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_, v) = send_cookie(&app, "GET", "/assets/admin/default-limits", Some(&admin), None).await;
    assert_eq!(
        v,
        json!({"quota_bytes": null, "daily_upload_bytes": null, "max_file_bytes": null})
    );
    let (s, _) = send_cookie(
        &app,
        "PUT",
        "/assets/admin/default-limits",
        Some(&admin),
        Some(json!({"quota_bytes": 10_000_000_000i64, "daily_upload_bytes": 1_000_000_000i64})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = send_cookie(&app, "GET", "/assets/admin/accounts", Some(&admin), None).await;
    assert_eq!(v["default_limits"]["quota_bytes"], 10_000_000_000i64);
    assert_eq!(
        send_cookie(
            &app,
            "PUT",
            "/assets/admin/default-limits",
            Some(&admin),
            Some(json!({"quota_bytes": 0}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM audit_log WHERE action = 'settings.default_limits'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    // Existing accounts did not change.
    let (_, other) = user(&t.pool, "old@example.com").await;
    let (_, u) = send(&app, "GET", "/assets/usage", &other, None).await;
    assert!(u["limits"]["quota_bytes"].is_null());
    t.teardown().await;
}

#[tokio::test]
async fn the_usage_page_reports_the_limits_and_the_rolling_day() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    set_limits(&t.pool, uid, Some(5000), Some(1000), Some(200)).await;
    sqlx::query("INSERT INTO upload_log (owner_id, at, bytes) VALUES ($1, now() - interval '3 hours', 400)")
        .bind(uid)
        .execute(&t.pool)
        .await
        .unwrap();
    let app = app(&t.pool);
    let (_, u) = send(&app, "GET", "/assets/usage", &key, None).await;
    assert_eq!(
        (
            u["limits"]["quota_bytes"].as_i64(),
            u["limits"]["daily_upload_bytes"].as_i64(),
            u["limits"]["max_file_bytes"].as_i64()
        ),
        (Some(5000), Some(1000), Some(200))
    );
    assert_eq!(u["daily"]["used_bytes"], 400);
    let rolls = time::OffsetDateTime::parse(
        u["daily"]["oldest_rolls_out_at"].as_str().unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let hours = (rolls - time::OffsetDateTime::now_utc()).whole_minutes() as f64 / 60.0;
    assert!((20.9..21.1).contains(&hours), "{hours}");
    // The accounts page shows each account's limits to the admin.
    let (_, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (_, v) = send_cookie(&app, "GET", "/assets/admin/accounts", Some(&admin), None).await;
    let a = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["email"] == "a@example.com")
        .unwrap();
    assert_eq!(
        (a["quota_bytes"].as_i64(), a["max_file_bytes"].as_i64()),
        (Some(5000), Some(200))
    );
    t.teardown().await;
}

/// A presigned part URL is signed for the part's exact length (`content-length` is among its signed headers), and a PUT of
/// that length works. Storage that verifies signatures against the real request (AWS S3, Ceph) then refuses any other
/// length; the SeaweedFS the tests run on does not check it, so that half cannot be shown here and `complete` (the
/// authoritative check, tested above) does not rely on it. Needs `curl`.
#[tokio::test]
async fn a_direct_part_url_is_signed_for_the_declared_length_and_that_length_uploads() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = with_s3(&t).await else {
        t.teardown().await;
        return;
    };
    if std::process::Command::new("curl").arg("--version").output().is_err() {
        t.teardown().await;
        return;
    }
    let (_, key) = user(&t.pool, "a@example.com").await;
    let (s, check) = send(
        &app,
        "POST",
        "/assets/check",
        &key,
        Some(json!({"sha256": sha_of(1), "size_bytes": 10, "filename": "a.jpg", "transport": "direct"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{check}");
    let url = check["parts"][0]["url"].as_str().unwrap().to_string();
    assert!(url.contains("X-Amz-SignedHeaders=content-length"), "{url}");
    let out = std::process::Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "-X",
            "PUT",
            "--data-binary",
            "0123456789",
        ])
        .arg(&url)
        .output()
        .unwrap();
    assert!(String::from_utf8(out.stdout).unwrap().starts_with('2'));
    t.teardown().await;
}
