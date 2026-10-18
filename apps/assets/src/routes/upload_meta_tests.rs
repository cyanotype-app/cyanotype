//! The real upload path (`/assets/check` -> part -> `/assets/complete`) stores self-describing metadata on the
//! original. Needs Postgres (`DATABASE_URL`) and an S3 API with an existing bucket (`S3_ENDPOINT`, `S3_BUCKET`,
//! `S3_ACCESS_KEY`, `S3_SECRET_KEY`, `S3_FORCE_PATH_STYLE`); skipped otherwise.
use axum::http::StatusCode;
use cyanotype_core::{config::S3Config, objmeta, storage::Storage};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    testutil::{TestDb, send, user},
};

#[tokio::test]
async fn both_upload_transports_store_name_owner_time_and_mtime_on_the_original() {
    let Some(t) = TestDb::new().await else { return };
    if std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty()).is_none() {
        t.teardown().await;
        return;
    }
    let storage = Storage::new(&S3Config::from_env().unwrap());
    let app = crate::routes::routes().with_state(AppState {
        db: t.pool.clone(),
        storage: storage.clone(),
        trash_retention_days: 30,
        purge_wake: Default::default(),
        text: std::sync::Arc::new(crate::testutil::FakeText::default()),
        search: crate::testutil::default_search(),
    });
    let (uid, key) = user(&t.pool, "meta@example.com").await;

    for (transport, name) in [
        ("direct", "Süd-Tirol \"Urlaub\" 🌅.JPG"),
        ("proxy", "proxied photo.heic"),
    ] {
        let body = format!("bytes of {name} {}", uuid::Uuid::new_v4()).into_bytes();
        let sha: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
        let (status, check) = send(
            &app,
            "POST",
            "/assets/check",
            &key,
            Some(json!({
                "sha256": sha, "size_bytes": body.len(), "filename": name, "content_type": "image/jpeg",
                "file_modified_at": "2019-08-01T07:00:01Z", "transport": transport,
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{check}");
        let upload_id = check["upload_id"].as_str().unwrap().to_string();
        let (okey,): (String,) = sqlx::query_as("SELECT key FROM uploads WHERE upload_id = $1")
            .bind(&upload_id)
            .fetch_one(&t.pool)
            .await
            .unwrap();
        // The client sends the part to S3 (direct) or to the service (proxy); the metadata was fixed at create.
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
        assert_eq!(status, StatusCode::CREATED, "{done}");

        let head = storage.head(&okey).await.unwrap().expect("stored");
        let m = objmeta::decode(&head.metadata);
        assert_eq!(m.name.as_deref(), Some(name), "{transport}");
        assert_eq!(m.owner, Some(uid));
        assert_eq!(
            m.file_modified_at,
            Some(time::macros::datetime!(2019-08-01 07:00:01 UTC))
        );
        assert!(
            m.created_at
                .is_some_and(|c| (time::OffsetDateTime::now_utc() - c).whole_minutes() < 5)
        );
        assert_eq!(head.content_type.as_deref(), Some("image/jpeg"));
        assert!(head.metadata.values().all(|v| v.is_ascii()));
    }
    t.teardown().await;
}
