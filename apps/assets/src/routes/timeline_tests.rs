//! Timeline day counts, the `before` cursor, favorites and downloads against a real Postgres (skipped without
//! `DATABASE_URL`, like the other DB tests).
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use cyanotype_core::pairing::{Paired, pair_asset};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use crate::testutil::{TestDb, app, ids, send, user};

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

async fn days(app: &Router, key: &str, query: &str) -> Value {
    let (st, body) = send(app, "GET", &format!("/assets/timeline{query}"), key, None).await;
    assert_eq!(st, StatusCode::OK);
    body
}

fn day_pairs(v: &Value) -> Vec<(String, i64)> {
    v["days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["day"].as_str().unwrap().to_string(), d["count"].as_i64().unwrap()))
        .collect()
}

fn names(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["original_name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn day_counts_skip_trash_and_hidden_raws_and_other_owners() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (other, other_key) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);

    photo(&t.pool, uid, "a1.jpg", "2026-10-14T09:00:00Z").await;
    photo(&t.pool, uid, "a2.jpg", "2026-10-14T23:59:59Z").await;
    photo(&t.pool, uid, "a3.jpg", "2026-10-15T00:00:00Z").await; // next UTC day
    let gone = photo(&t.pool, uid, "a4.jpg", "2026-10-14T10:00:00Z").await;
    photo(&t.pool, uid, "a5.jpg", "2024-02-29T12:00:00Z").await;
    photo(&t.pool, other, "b1.jpg", "2026-10-14T09:00:00Z").await;
    // A pair counts once: the RAW is hidden behind its JPEG.
    photo(&t.pool, uid, "IMG_9.JPG", "2025-03-01T08:00:00Z").await;
    let raw = photo(&t.pool, uid, "IMG_9.CR3", "2025-03-01T08:00:00Z").await;
    assert!(matches!(pair_asset(&t.pool, raw).await.unwrap(), Paired::Linked { .. }));
    // Trashed photos do not count.
    let (st, _) = send(&app, "POST", "/assets/trash", &key, Some(ids(&[gone]))).await;
    assert_eq!(st, StatusCode::OK);
    // Falls back to file_modified_at, then created_at, exactly like the list.
    sqlx::query(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, file_modified_at)
         VALUES ($1, sha256('m'::bytea), 'k-m', 'm.jpg', 'image/jpeg', 1, '2015-03-04T05:06:07Z')",
    )
    .bind(uid)
    .execute(&t.pool)
    .await
    .unwrap();

    let v = days(&app, &key, "").await;
    assert_eq!(
        day_pairs(&v),
        [
            ("2026-10-15".into(), 1),
            ("2026-10-14".into(), 2),
            ("2025-03-01".into(), 1),
            ("2024-02-29".into(), 1),
            ("2015-03-04".into(), 1),
        ]
    );
    assert_eq!(v["total"], 6);
    // The counts are what the list returns.
    let (_, all) = send(&app, "GET", "/assets?limit=500", &key, None).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 6);
    let (_, count) = send(&app, "GET", "/assets/count", &key, None).await;
    assert_eq!(count["count"], 6);

    // The other owner sees only their own photo.
    let o = days(&app, &other_key, "").await;
    assert_eq!(day_pairs(&o), [("2026-10-14".into(), 1)]);

    // Needs a login.
    let (st, _) = send(&app, "GET", "/assets/timeline", "cyk_wrong", None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // No photos: empty, not an error.
    let (_, empty_key) = user(&t.pool, "c@example.com").await;
    let e = days(&app, &empty_key, "").await;
    assert_eq!((e["total"].as_i64(), e["days"].as_array().unwrap().len()), (Some(0), 0));
    t.teardown().await;
}

#[tokio::test]
async fn day_counts_can_be_limited_to_favorites() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let a = photo(&t.pool, uid, "a.jpg", "2026-01-02T00:00:00Z").await;
    photo(&t.pool, uid, "b.jpg", "2026-01-02T05:00:00Z").await;
    let c = photo(&t.pool, uid, "c.jpg", "2025-12-31T05:00:00Z").await;
    let (st, r) = send(
        &app,
        "POST",
        "/assets/favorite",
        &key,
        Some(json!({"ids": [a, c], "favorite": true})),
    )
    .await;
    assert_eq!((st, r["count"].as_i64()), (StatusCode::OK, Some(2)));
    let v = days(&app, &key, "?favorite=true").await;
    assert_eq!(day_pairs(&v), [("2026-01-02".into(), 1), ("2025-12-31".into(), 1)]);
    assert_eq!(v["total"], 2);
    assert_eq!(days(&app, &key, "").await["total"], 3);
    t.teardown().await;
}

#[tokio::test]
async fn day_counts_are_an_index_only_scan_of_the_timeline_index() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, _) = user(&t.pool, "a@example.com").await;
    for i in 0..200 {
        photo(
            &t.pool,
            uid,
            &format!("p{i}.jpg"),
            &format!("2026-01-{:02}T00:00:00Z", 1 + i % 28),
        )
        .await;
    }
    sqlx::query("ANALYZE assets").execute(&t.pool).await.unwrap();
    sqlx::query("UPDATE assets SET thumbhash = NULL")
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("VACUUM assets").execute(&t.pool).await.unwrap(); // visibility map: index-only scans need it
    for (favorite, index) in [(false, "assets_timeline_idx"), (true, "assets_favorites_idx")] {
        let mut tx = t.pool.begin().await.unwrap();
        sqlx::query("SET LOCAL enable_seqscan = off")
            .execute(&mut *tx)
            .await
            .unwrap();
        let plan: Vec<(String,)> = sqlx::query_as(&format!(
            "EXPLAIN {}",
            super::timeline::days_sql(favorite).replace("$1", &format!("'{uid}'"))
        ))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        let plan = plan.into_iter().map(|r| r.0).collect::<Vec<_>>().join("\n");
        assert!(plan.contains(index), "{index} not used:\n{plan}");
        assert!(!plan.contains("Seq Scan"), "sequential scan:\n{plan}");
    }
    t.teardown().await;
}

#[tokio::test]
async fn before_starts_a_page_at_any_date_and_paging_continues_without_gaps() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    // Ten photos, two per day, over five days, plus one in another year.
    for d in 1..=5 {
        for h in [9, 18] {
            photo(
                &t.pool,
                uid,
                &format!("d{d}h{h}.jpg"),
                &format!("2026-03-0{d}T{h:02}:00:00Z"),
            )
            .await;
        }
    }
    photo(&t.pool, uid, "old.jpg", "2021-07-01T00:00:00Z").await;

    // "Start at 3 March": everything strictly before the end of that day, newest first.
    let (_, page) = send(&app, "GET", "/assets?before=2026-03-04T00:00:00Z&limit=3", &key, None).await;
    assert_eq!(names(&page), ["d3h18.jpg", "d3h9.jpg", "d2h18.jpg"]);
    // The cursor of that page continues exactly after it.
    let cursor = page["next_cursor"].as_str().unwrap();
    let (_, next) = send(&app, "GET", &format!("/assets?cursor={cursor}&limit=50"), &key, None).await;
    assert_eq!(names(&next), ["d2h9.jpg", "d1h18.jpg", "d1h9.jpg", "old.jpg"]);
    assert!(next["next_cursor"].is_null());
    // The page that starts a day is where the day counts say it does: 2 newer days of 2 photos each.
    let (_, tl) = send(&app, "GET", "/assets/timeline", &key, None).await;
    let before_3rd: i64 = day_pairs(&tl)
        .iter()
        .filter(|(d, _)| d.as_str() > "2026-03-03")
        .map(|d| d.1)
        .sum();
    assert_eq!(before_3rd, 4);
    let (_, head) = send(&app, "GET", "/assets?limit=50", &key, None).await;
    assert_eq!(names(&head)[4], "d3h18.jpg");

    // Favorites filter combines with it; other owners' photos never leak in.
    let (other, _) = user(&t.pool, "b@example.com").await;
    photo(&t.pool, other, "theirs.jpg", "2026-03-02T00:00:00Z").await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM assets WHERE original_name = 'd2h9.jpg'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    send(
        &app,
        "POST",
        "/assets/favorite",
        &key,
        Some(json!({"ids": [id], "favorite": true})),
    )
    .await;
    let (_, f) = send(
        &app,
        "GET",
        "/assets?favorite=true&before=2026-03-04T00:00:00Z",
        &key,
        None,
    )
    .await;
    assert_eq!(names(&f), ["d2h9.jpg"]);
    assert_eq!(f["items"][0]["favorite"], true);
    let (st, _) = send(&app, "GET", "/assets?before=garbage", &key, None).await;
    assert!(st.is_client_error());
    t.teardown().await;
}

#[tokio::test]
async fn favorites_bulk_idempotent_and_owner_scoped() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (other, other_key) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    let a = photo(&t.pool, uid, "a.jpg", "2026-01-03T00:00:00Z").await;
    let b = photo(&t.pool, uid, "b.jpg", "2026-01-02T00:00:00Z").await;
    let c = photo(&t.pool, uid, "c.jpg", "2026-01-01T00:00:00Z").await;
    let theirs = photo(&t.pool, other, "theirs.jpg", "2026-01-01T00:00:00Z").await;

    let fav = |ids: Vec<Uuid>, on: bool| json!({"ids": ids, "favorite": on});
    // Another owner's id is ignored, not an error, and unchanged.
    let (st, r) = send(
        &app,
        "POST",
        "/assets/favorite",
        &key,
        Some(fav(vec![a, b, theirs, Uuid::new_v4()], true)),
    )
    .await;
    assert_eq!((st, r["count"].as_i64()), (StatusCode::OK, Some(2)));
    let flag = |id: Uuid| {
        let pool = t.pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>("SELECT favorite FROM assets WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        (flag(a).await, flag(b).await, flag(c).await, flag(theirs).await),
        (true, true, false, false)
    );
    // Repeating changes nothing.
    let (_, r) = send(&app, "POST", "/assets/favorite", &key, Some(fav(vec![a, b], true))).await;
    assert_eq!(r["count"], 0);
    // The list shows the flag and filters by it.
    let (_, all) = send(&app, "GET", "/assets", &key, None).await;
    assert_eq!(
        all["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["favorite"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        [true, true, false]
    );
    let (_, only) = send(&app, "GET", "/assets?favorite=true", &key, None).await;
    assert_eq!(names(&only), ["a.jpg", "b.jpg"]);
    // Other owners' favorites are separate.
    let (_, mine) = send(&app, "GET", "/assets?favorite=true", &other_key, None).await;
    assert!(mine["items"].as_array().unwrap().is_empty());
    // The single-photo view carries it too.
    let (_, one) = send(&app, "GET", &format!("/assets/{a}"), &key, None).await;
    assert_eq!(one["favorite"], true);
    // Unfavorite.
    let (_, r) = send(&app, "POST", "/assets/favorite", &key, Some(fav(vec![a, c], false))).await;
    assert_eq!(r["count"], 1);
    assert!(!flag(a).await);
    // Trashed photos are not touched, and keep their flag across trash and restore.
    send(&app, "POST", "/assets/trash", &key, Some(ids(&[b]))).await;
    let (_, r) = send(&app, "POST", "/assets/favorite", &key, Some(fav(vec![b], false))).await;
    assert_eq!(r["count"], 0);
    assert!(flag(b).await);
    let (_, trash) = send(&app, "GET", "/assets/trash", &key, None).await;
    assert_eq!(trash["items"][0]["favorite"], true);
    send(&app, "POST", "/assets/restore", &key, Some(ids(&[b]))).await;
    let (_, only) = send(&app, "GET", "/assets?favorite=true", &key, None).await;
    assert_eq!(names(&only), ["b.jpg"]);
    // Album listings carry it.
    let (_, album) = send(
        &app,
        "POST",
        "/assets/albums",
        &key,
        Some(json!({"title": "x", "asset_ids": [b]})),
    )
    .await;
    let (_, in_album) = send(
        &app,
        "GET",
        &format!("/assets/albums/{}/assets", album["id"].as_str().unwrap()),
        &key,
        None,
    )
    .await;
    assert_eq!(in_album["items"][0]["favorite"], true);
    // Bulk cap.
    let many: Vec<Uuid> = (0..1001).map(|_| Uuid::new_v4()).collect();
    let (st, _) = send(&app, "POST", "/assets/favorite", &key, Some(fav(many, true))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send(&app, "POST", "/assets/favorite", "cyk_wrong", Some(fav(vec![a], true))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

#[tokio::test]
async fn a_pair_is_favorited_through_its_primary() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let jpg = photo(&t.pool, uid, "IMG_1.JPG", "2026-05-01T12:00:00Z").await;
    let raw = photo(&t.pool, uid, "IMG_1.CR3", "2026-05-01T12:00:00Z").await;
    assert!(matches!(pair_asset(&t.pool, raw).await.unwrap(), Paired::Linked { .. }));
    // The id of the hidden RAW means its primary; the pair counts once.
    let (_, r) = send(
        &app,
        "POST",
        "/assets/favorite",
        &key,
        Some(json!({"ids": [raw, jpg], "favorite": true})),
    )
    .await;
    assert_eq!(r["count"], 1);
    let flags: Vec<(Uuid, bool)> = sqlx::query_as("SELECT id, favorite FROM assets ORDER BY original_name")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(flags, [(raw, false), (jpg, true)], "only the primary carries the flag");
    let (_, only) = send(&app, "GET", "/assets?favorite=true", &key, None).await;
    assert_eq!(names(&only), ["IMG_1.JPG"]);
    assert_eq!(only["items"][0]["paired"][0]["id"], raw.to_string());
    let v = days(&app, &key, "?favorite=true").await;
    assert_eq!(v["total"], 1);
    // And unfavorite through the RAW id.
    let (_, r) = send(
        &app,
        "POST",
        "/assets/favorite",
        &key,
        Some(json!({"ids": [raw], "favorite": false})),
    )
    .await;
    assert_eq!(r["count"], 1);

    // A RAW favorited on its own before it was paired hands the flag to its primary.
    let jpg2 = photo(&t.pool, uid, "IMG_2.JPG", "2026-05-02T12:00:00Z").await;
    let raw2 = photo(&t.pool, uid, "IMG_2.CR3", "2026-05-02T12:00:00Z").await;
    send(
        &app,
        "POST",
        "/assets/favorite",
        &key,
        Some(json!({"ids": [raw2], "favorite": true})),
    )
    .await;
    assert!(matches!(
        pair_asset(&t.pool, jpg2).await.unwrap(),
        Paired::Linked { .. }
    ));
    let f: bool = sqlx::query_scalar("SELECT favorite FROM assets WHERE id = $1")
        .bind(jpg2)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(f);
    t.teardown().await;
}

/// `GET path` without following redirects: `(status, location, cache-control)`.
async fn redirect(app: &Router, path: &str, key: &str) -> (StatusCode, Option<String>, Option<String>) {
    let req = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let h = |n| res.headers().get(n).map(|v| v.to_str().unwrap().to_string());
    (res.status(), h(header::LOCATION), h(header::CACHE_CONTROL))
}

fn query_param(url: &str, name: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

#[tokio::test]
async fn download_redirects_to_a_short_lived_attachment_url() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (_, other_key) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    let jpg = photo(&t.pool, uid, "IMG_7.JPG", "2026-05-01T12:00:00Z").await;
    let raw = photo(&t.pool, uid, "IMG_7.CR3", "2026-05-01T12:00:00Z").await;
    assert!(matches!(pair_asset(&t.pool, raw).await.unwrap(), Paired::Linked { .. }));
    let plain = photo(&t.pool, uid, "Zürich \"x\".png", "2026-05-02T12:00:00Z").await;

    let (st, loc, cache) = redirect(&app, &format!("/assets/{jpg}/download"), &key).await;
    assert_eq!(st, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(cache.as_deref(), Some("no-store"));
    let loc = loc.unwrap();
    // The signed object is the photo's own original; the bytes come from S3, not from this service.
    assert!(loc.starts_with("http://127.0.0.1:1/b/originals/"), "{loc}");
    assert!(loc.contains("IMG_7.JPG"), "{loc}");
    assert_eq!(
        query_param(&loc, "response-content-disposition").as_deref(),
        Some("attachment%3B%20filename%3D%22IMG_7.JPG%22")
    );
    // Short TTL (60 s) and signed: the disposition is part of the signature, so it cannot be dropped or changed.
    assert_eq!(query_param(&loc, "X-Amz-Expires").as_deref(), Some("60"));
    assert!(query_param(&loc, "X-Amz-Signature").is_some());
    assert!(query_param(&loc, "X-Amz-SignedHeaders").is_some());

    // The RAW of the pair, picked by its id from the photo's `paired` list.
    let (st, loc, _) = redirect(&app, &format!("/assets/{jpg}/download?file={raw}"), &key).await;
    assert_eq!(st, StatusCode::TEMPORARY_REDIRECT);
    let loc = loc.unwrap();
    assert!(loc.contains("IMG_7.CR3"), "{loc}");
    assert_eq!(
        query_param(&loc, "response-content-disposition").as_deref(),
        Some("attachment%3B%20filename%3D%22IMG_7.CR3%22")
    );
    // The photo itself can also be named explicitly.
    let (st, ..) = redirect(&app, &format!("/assets/{jpg}/download?file={jpg}"), &key).await;
    assert_eq!(st, StatusCode::TEMPORARY_REDIRECT);

    // A file that is not part of this photo, the hidden RAW as a photo of its own, junk.
    let (st, ..) = redirect(&app, &format!("/assets/{plain}/download?file={raw}"), &key).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, ..) = redirect(&app, &format!("/assets/{raw}/download"), &key).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "the hidden RAW is downloaded through its photo"
    );
    let (st, ..) = redirect(&app, &format!("/assets/{}/download", Uuid::new_v4()), &key).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, ..) = redirect(&app, &format!("/assets/{jpg}/download?file=nope"), &key).await;
    assert!(st.is_client_error());

    // Names that need escaping never reach the header raw.
    let (_, loc, _) = redirect(&app, &format!("/assets/{plain}/download"), &key).await;
    let d = query_param(&loc.unwrap(), "response-content-disposition").unwrap();
    // (the header's own percent signs are encoded once more as a query value)
    assert!(d.contains("filename%3D%22Z_rich%20_x_.png%22"), "ASCII fallback: {d}");
    assert!(
        d.contains("filename%2A%3DUTF-8%27%27Z%25C3%25BCrich%2520%2522x%2522.png"),
        "{d}"
    );

    // Another user's photo, and no login.
    let (st, loc, _) = redirect(&app, &format!("/assets/{jpg}/download"), &other_key).await;
    assert_eq!((st, loc), (StatusCode::NOT_FOUND, None));
    let (st, ..) = redirect(&app, &format!("/assets/{jpg}/download"), "cyk_wrong").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Trashed photos (and their RAW) cannot be downloaded; restoring brings it back.
    send(&app, "POST", "/assets/trash", &key, Some(ids(&[jpg]))).await;
    let (st, ..) = redirect(&app, &format!("/assets/{jpg}/download"), &key).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, ..) = redirect(&app, &format!("/assets/{jpg}/download?file={raw}"), &key).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    send(&app, "POST", "/assets/restore", &key, Some(ids(&[jpg]))).await;
    let (st, ..) = redirect(&app, &format!("/assets/{jpg}/download"), &key).await;
    assert_eq!(st, StatusCode::TEMPORARY_REDIRECT);
    t.teardown().await;
}
