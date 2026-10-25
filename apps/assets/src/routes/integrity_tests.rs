//! The integrity check's admin routes: who may call them, the queueing of a run, the findings list and the actions on a finding
//! (restore a version, restore a photo, delete an object, trash, acknowledge): each audited once and idempotent. The actions
//! that touch objects need an S3 API (`S3_ENDPOINT`...); the rest need only Postgres.
use axum::http::StatusCode;
use cyanotype_core::keys;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::testutil::{TestDb, app, app_with_s3, asset, send, send_cookie, session, user};

async fn finding(pool: &PgPool, kind: &str, owner: Uuid, asset: Option<Uuid>, key: &str, detail: Value) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO integrity_findings (kind, owner_id, asset_id, key, detail) VALUES ($1,$2,$3,$4,$5) RETURNING id",
    )
    .bind(kind)
    .bind(owner)
    .bind(asset)
    .bind(key)
    .bind(detail)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn audits(pool: &PgPool, action: &str) -> Vec<(Option<Uuid>, Option<String>, Value)> {
    sqlx::query_as("SELECT actor_id, target_id, detail FROM audit_log WHERE action = $1 ORDER BY id")
        .bind(action)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_integrity_endpoints_are_for_admin_sessions_only() {
    let Some(t) = TestDb::new().await else { return };
    let (_, member) = session(&t.pool, "member@example.com", "member").await;
    let (_, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (alpha, key) = user(&t.pool, "keyed@example.com").await;
    let id = finding(
        &t.pool,
        "untracked_object",
        alpha,
        None,
        "originals/x",
        json!({"reason": "no_row"}),
    )
    .await;
    let app = app(&t.pool);
    let act = format!("/assets/admin/integrity/findings/{id}");
    let body = Some(json!({"action": "acknowledge"}));
    for (m, p, b) in [
        ("GET", "/assets/admin/integrity", None),
        ("POST", "/assets/admin/integrity/run", None),
        ("GET", "/assets/admin/integrity/findings", None),
        ("POST", act.as_str(), body.clone()),
    ] {
        assert_eq!(
            send_cookie(&app, m, p, None, b.clone()).await.0,
            StatusCode::UNAUTHORIZED,
            "{m} {p}"
        );
        assert_eq!(
            send_cookie(&app, m, p, Some(&member), b.clone()).await.0,
            StatusCode::FORBIDDEN,
            "{m} {p}"
        );
        assert_eq!(
            send(&app, m, p, &key, b.clone()).await.0,
            StatusCode::FORBIDDEN,
            "{m} {p} with a key"
        );
    }
    // Nothing happened to the finding or the queue.
    let (state,): (String,) = sqlx::query_as("SELECT state FROM integrity_findings WHERE id = $1")
        .bind(id)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(state, "open");
    let (tasks,): (i64,) = sqlx::query_as("SELECT count(*) FROM admin_tasks")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(tasks, 0);
    assert_eq!(
        send_cookie(&app, "GET", "/assets/admin/integrity", Some(&admin), None)
            .await
            .0,
        StatusCode::OK
    );
    t.teardown().await;
}

#[tokio::test]
async fn run_check_now_queues_one_audited_task_and_the_summary_shows_it() {
    let Some(t) = TestDb::new().await else { return };
    let (admin_id, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let app = app(&t.pool);

    let (s, v) = send_cookie(&app, "GET", "/assets/admin/integrity", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["last_run"].is_null() && v["task"].is_null(), "{v}");
    assert_eq!(v["open"]["missing_original"], 0);
    assert_eq!(v["coverage"]["assets"], 0);

    let (s, v) = send_cookie(&app, "POST", "/assets/admin/integrity/run", Some(&admin), None).await;
    assert_eq!(
        (s, v["already_running"].as_bool()),
        (StatusCode::ACCEPTED, Some(false)),
        "{v}"
    );
    let (s, v) = send_cookie(&app, "POST", "/assets/admin/integrity/run", Some(&admin), None).await;
    assert_eq!(
        (s, v["already_running"].as_bool()),
        (StatusCode::ACCEPTED, Some(true)),
        "{v}"
    );
    assert_eq!(audits(&t.pool, "integrity.run").await.len(), 1, "audited once");
    let task = cyanotype_core::admin_tasks::claim(&t.pool, &["integrity_check"])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (task.requested_by, task.params["trigger"].as_str()),
        (Some(admin_id), Some("manual"))
    );
    let (_, v) = send_cookie(&app, "GET", "/assets/admin/integrity", Some(&admin), None).await;
    assert_eq!(v["task"]["state"], "running");
    t.teardown().await;
}

#[tokio::test]
async fn findings_are_listed_in_pages_by_state_and_kind() {
    let Some(t) = TestDb::new().await else { return };
    let (_, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (alpha, _) = user(&t.pool, "alpha@example.com").await;
    let a = asset(&t.pool, alpha, "one.jpg").await;
    for n in 0..30 {
        finding(
            &t.pool,
            "untracked_object",
            alpha,
            None,
            &format!("originals/{n}"),
            json!({"reason": "no_row"}),
        )
        .await;
    }
    finding(&t.pool, "missing_original", alpha, Some(a), "originals/gone", json!({})).await;
    let done = finding(&t.pool, "untracked_object", alpha, None, "originals/done", json!({})).await;
    sqlx::query("UPDATE integrity_findings SET state = 'resolved', resolution = 'no longer found' WHERE id = $1")
        .bind(done)
        .execute(&t.pool)
        .await
        .unwrap();
    let app = app(&t.pool);
    let get = |q: &str| {
        let (app, admin, q) = (app.clone(), admin.clone(), q.to_string());
        async move {
            send_cookie(
                &app,
                "GET",
                &format!("/assets/admin/integrity/findings{q}"),
                Some(&admin),
                None,
            )
            .await
        }
    };

    let (s, p1) = get("?kind=untracked_object").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(p1["items"].as_array().unwrap().len(), 25);
    let before = p1["next_before"].as_i64().unwrap();
    assert_eq!(p1["items"][0]["owner_email"], "alpha@example.com");
    let (_, p2) = get(&format!("?kind=untracked_object&before={before}")).await;
    assert_eq!(p2["items"].as_array().unwrap().len(), 5);
    assert!(p2["next_before"].is_null());
    let (_, missing) = get("?kind=missing_original").await;
    assert_eq!(missing["items"].as_array().unwrap().len(), 1);
    let (_, resolved) = get("?state=resolved").await;
    assert_eq!(resolved["items"][0]["resolution"], "no longer found");
    assert_eq!(get("?state=nope").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get("?kind=nope").await.0, StatusCode::BAD_REQUEST);
    let (_, sum) = send_cookie(&app, "GET", "/assets/admin/integrity", Some(&admin), None).await;
    assert_eq!(
        (
            sum["open"]["untracked_object"].as_i64(),
            sum["open"]["missing_original"].as_i64()
        ),
        (Some(30), Some(1))
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_trash_or_mark_lost_action_is_audited_once_and_idempotent() {
    let Some(t) = TestDb::new().await else { return };
    let (admin_id, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (alpha, _) = user(&t.pool, "alpha@example.com").await;
    let a = asset(&t.pool, alpha, "gone.jpg").await;
    let b = asset(&t.pool, alpha, "lost.jpg").await;
    let f1 = finding(&t.pool, "missing_original", alpha, Some(a), "originals/a", json!({})).await;
    let f2 = finding(&t.pool, "missing_original", alpha, Some(b), "originals/b", json!({})).await;
    let app = app(&t.pool);
    let post = |id: i64, body: Value| {
        let (app, admin) = (app.clone(), admin.clone());
        async move {
            send_cookie(
                &app,
                "POST",
                &format!("/assets/admin/integrity/findings/{id}"),
                Some(&admin),
                Some(body),
            )
            .await
        }
    };

    let (s, v) = post(f1, json!({"action": "trash"})).await;
    assert_eq!(
        (s, v["changed"].as_bool(), v["finding"]["state"].as_str()),
        (StatusCode::OK, Some(true), Some("resolved")),
        "{v}"
    );
    let (s, v) = post(f1, json!({"action": "trash"})).await;
    assert_eq!((s, v["changed"].as_bool()), (StatusCode::OK, Some(false)));
    let trashed: bool = sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM assets WHERE id = $1")
        .bind(a)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(trashed);
    let entries = audits(&t.pool, "integrity.trash").await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].0, entries[0].1.clone()),
        (Some(admin_id), Some(f1.to_string()))
    );
    assert_eq!(entries[0].2["kind"], "missing_original");

    let (s, v) = post(f2, json!({"action": "acknowledge"})).await;
    assert_eq!(
        (s, v["finding"]["state"].as_str()),
        (StatusCode::OK, Some("acknowledged")),
        "{v}"
    );
    assert_eq!(post(f2, json!({"action": "acknowledge"})).await.1["changed"], false);
    assert_eq!(audits(&t.pool, "integrity.acknowledge").await.len(), 1);

    // Refusals: a different action on a resolved finding, an action of the wrong kind, an unknown action or finding.
    assert_eq!(post(f1, json!({"action": "acknowledge"})).await.0, StatusCode::CONFLICT);
    assert_eq!(
        post(f2, json!({"action": "delete_object", "confirm": "delete"}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(post(f2, json!({"action": "explode"})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        post(999_999, json!({"action": "acknowledge"})).await.0,
        StatusCode::NOT_FOUND
    );
    t.teardown().await;
}

#[tokio::test]
async fn restoring_a_version_a_photo_or_deleting_an_object_works_through_the_api_and_is_audited() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        t.teardown().await;
        return;
    };
    let (admin_id, admin) = session(&t.pool, "admin@example.com", "admin").await;
    let (_, member) = session(&t.pool, "member@example.com", "member").await;
    let (alpha, _) = user(&t.pool, "alpha@example.com").await;
    let post = |id: i64, body: Value| {
        let (app, admin) = (app.clone(), admin.clone());
        async move {
            send_cookie(
                &app,
                "POST",
                &format!("/assets/admin/integrity/findings/{id}"),
                Some(&admin),
                Some(body),
            )
            .await
        }
    };
    let put = |body: &'static [u8]| {
        let storage = storage.clone();
        async move {
            let sha: [u8; 32] = Sha256::digest(body).into();
            let key = keys::original(alpha, &sha, "jpg");
            storage.put_bytes(&key, body.to_vec(), "image/jpeg").await.unwrap();
            (key, sha)
        }
    };

    // Restore a version: the right bytes, then other bytes of the same size over them.
    let (key, sha) = put(b"alpha seven").await;
    let row: Uuid = sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
         VALUES ($1,$2,$3,'a.jpg','image/jpeg',11) RETURNING id",
    )
    .bind(alpha)
    .bind(&sha[..])
    .bind(&key)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    storage
        .put_bytes(&key, b"alpha SEVEN".to_vec(), "image/jpeg")
        .await
        .unwrap();
    let f = finding(
        &t.pool,
        "checksum_mismatch",
        alpha,
        Some(row),
        &key,
        json!({"reason": "hash"}),
    )
    .await;
    // A member cannot.
    let (s, _) = send_cookie(
        &app,
        "POST",
        &format!("/assets/admin/integrity/findings/{f}"),
        Some(&member),
        Some(json!({"action": "restore_version"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(
        storage.hash_object(&key, None, 11).await.unwrap().unwrap().sha256,
        Sha256::digest(b"alpha SEVEN").as_slice()
    );
    let (s, v) = post(f, json!({"action": "restore_version"})).await;
    assert_eq!((s, v["changed"].as_bool()), (StatusCode::OK, Some(true)), "{v}");
    assert_eq!(storage.hash_object(&key, None, 11).await.unwrap().unwrap().sha256, sha);
    assert_eq!(post(f, json!({"action": "restore_version"})).await.1["changed"], false);
    assert_eq!(audits(&t.pool, "integrity.restore_version").await.len(), 1);

    // Restore as a photo.
    let (okey, _) = put(b"bravo eight").await;
    let o = finding(
        &t.pool,
        "untracked_object",
        alpha,
        None,
        &okey,
        json!({"reason": "no_row"}),
    )
    .await;
    let (s, v) = post(o, json!({"action": "restore_photo"})).await;
    assert_eq!((s, v["changed"].as_bool()), (StatusCode::OK, Some(true)), "{v}");
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM assets WHERE original_key = $1")
        .bind(&okey)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(post(o, json!({"action": "restore_photo"})).await.1["changed"], false);
    assert_eq!(audits(&t.pool, "integrity.restore_photo").await.len(), 1);

    // Delete an object: it must be asked for in words.
    let (dkey, _) = put(b"charlie nine").await;
    let d = finding(
        &t.pool,
        "untracked_object",
        alpha,
        None,
        &dkey,
        json!({"reason": "no_row"}),
    )
    .await;
    assert_eq!(
        post(d, json!({"action": "delete_object"})).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(d, json!({"action": "delete_object", "confirm": "yes"})).await.0,
        StatusCode::BAD_REQUEST
    );
    assert!(storage.head_size(&dkey).await.unwrap().is_some(), "still there");
    let (s, v) = post(d, json!({"action": "delete_object", "confirm": "delete"})).await;
    assert_eq!((s, v["changed"].as_bool()), (StatusCode::OK, Some(true)), "{v}");
    assert!(storage.head_size(&dkey).await.unwrap().is_none());
    assert!(storage.list_versions(&dkey).await.unwrap().is_empty());
    assert_eq!(
        post(d, json!({"action": "delete_object", "confirm": "delete"})).await.1["changed"],
        false
    );
    let entries = audits(&t.pool, "integrity.delete_object").await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].0, entries[0].2["key"].as_str()),
        (Some(admin_id), Some(dkey.as_str()))
    );

    // Cleanup of what these tests wrote to the shared bucket.
    storage.delete_all_versions(&[key, okey]).await.unwrap();
    t.teardown().await;
}
