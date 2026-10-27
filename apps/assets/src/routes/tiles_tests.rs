//! The basemap range relay against a real S3 API (`S3_ENDPOINT`, ...) and Postgres (`DATABASE_URL`); skipped without
//! both, like the upload tests.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use cyanotype_core::{config::S3Config, storage::Storage};
use tower::ServiceExt;

use crate::{
    AppState,
    testutil::{TestDb, send, user},
};

async fn get(
    app: &Router,
    path: &str,
    key: &str,
    range: Option<&str>,
    if_match: Option<&str>,
) -> axum::response::Response {
    let mut b = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {key}"));
    if let Some(r) = range {
        b = b.header(header::RANGE, r);
    }
    if let Some(m) = if_match {
        b = b.header(header::IF_MATCH, m);
    }
    app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap()
}

async fn bytes(res: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(res.into_body(), 8 << 20).await.unwrap().to_vec()
}

#[tokio::test]
async fn ranges_of_the_installed_archives_are_relayed_and_nothing_else_is() {
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
        faces_enabled: false,
        pets_enabled: false,
    });
    let (_, key) = user(&t.pool, "tiles@example.com").await;
    // A stand-in archive: 5000 recognisable bytes, bigger than the range cap is not needed to test the cap itself
    // (unit-tested), but a range that runs past the end must be clipped by the store.
    let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    storage
        .put_bytes("basemap/world.pmtiles", data.clone(), "application/octet-stream")
        .await
        .unwrap();
    let _ = storage
        .delete_all_versions(&["basemap/detail.pmtiles".to_string()])
        .await;

    let (st, v) = send(&app, "GET", "/assets/tiles", &key, None).await;
    assert_eq!(
        (st, v["world"].as_bool(), v["detail"].as_bool()),
        (StatusCode::OK, Some(true), Some(false))
    );

    // The PMTiles header read, a tile in the middle, an open-ended range, and the tail.
    let res = get(&app, "/assets/tiles/world.pmtiles", &key, Some("bytes=0-126"), None).await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 0-126/5000");
    assert_eq!(res.headers()[header::ACCEPT_RANGES], "bytes");
    assert!(
        res.headers()[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("max-age")
    );
    let etag = res.headers()[header::ETAG].to_str().unwrap().to_string();
    assert_eq!(bytes(res).await, data[..127]);

    let res = get(&app, "/assets/tiles/world.pmtiles", &key, Some("bytes=1000-1999"), None).await;
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 1000-1999/5000");
    assert_eq!(bytes(res).await, data[1000..2000]);
    let res = get(&app, "/assets/tiles/world.pmtiles", &key, Some("bytes=4900-"), None).await;
    assert_eq!(bytes(res).await, data[4900..]);
    let res = get(&app, "/assets/tiles/world.pmtiles", &key, Some("bytes=-100"), None).await;
    assert_eq!(bytes(res).await, data[4900..]);
    let res = get(&app, "/assets/tiles/world.pmtiles", &key, Some("bytes=4990-9999"), None).await;
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 4990-4999/5000");

    // Past the end; a replaced archive (If-Match with a stale ETag); no or several ranges.
    let res = get(&app, "/assets/tiles/world.pmtiles", &key, Some("bytes=6000-6100"), None).await;
    assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    let res = get(
        &app,
        "/assets/tiles/world.pmtiles",
        &key,
        Some("bytes=0-9"),
        Some(&etag),
    )
    .await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    let res = get(
        &app,
        "/assets/tiles/world.pmtiles",
        &key,
        Some("bytes=0-9"),
        Some("\"stale\""),
    )
    .await;
    assert_eq!(res.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        get(&app, "/assets/tiles/world.pmtiles", &key, None, None)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&app, "/assets/tiles/world.pmtiles", &key, Some("bytes=0-1,5-6"), None)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    // Not installed, not a served name (the route cannot read other bucket objects), and not logged in.
    assert_eq!(
        get(&app, "/assets/tiles/detail.pmtiles", &key, Some("bytes=0-9"), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for name in ["other.pmtiles", "..%2Foriginals", "world.pmtiles.bak"] {
        let res = get(&app, &format!("/assets/tiles/{name}"), &key, Some("bytes=0-9"), None).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{name}");
    }
    assert_eq!(
        get(&app, "/assets/tiles/world.pmtiles", "cyk_nope", Some("bytes=0-9"), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    storage
        .delete_all_versions(&["basemap/world.pmtiles".to_string()])
        .await
        .unwrap();
    t.teardown().await;
}
