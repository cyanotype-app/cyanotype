//! Smart albums over a real Postgres: rules of every kind, match all / any, live membership (nothing materialised), the hidden-photo
//! and owner rules, editing, the things a smart album refuses, share links that resolve the rules at view time, bulk scopes, the
//! preview, and the indexes. The rules' own validation is unit tested in `smart.rs`.
use axum::{Router, http::StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{routes_with, share_public::ShareState};
use crate::{
    AppState,
    testutil::{TestDb, asset, dummy_storage, send, send_cookie, session, user},
};

fn app() -> impl Fn(&PgPool) -> Router {
    |pool| {
        routes_with(ShareState::new(false, vec![]).with_limits((10_000, 10_000), (10_000, 10_000), (10_000, 10_000)))
            .with_state(AppState {
                db: pool.clone(),
                storage: dummy_storage(),
                trash_retention_days: 30,
                purge_wake: Default::default(),
                text: std::sync::Arc::new(crate::testutil::FakeText::default()),
                search: crate::testutil::default_search(),
                faces_enabled: false,
            })
    }
}

async fn set(pool: &PgPool, id: Uuid, sql: &str) {
    sqlx::query(&format!("UPDATE assets SET {sql} WHERE id = $1"))
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

async fn post(app: &Router, key: &str, path: &str, body: Value) -> (StatusCode, Value) {
    send(app, "POST", path, key, Some(body)).await
}

async fn smart(app: &Router, key: &str, title: &str, rules: Value) -> Uuid {
    let (st, a) = post(app, key, "/assets/albums", json!({"title": title, "rules": rules})).await;
    assert_eq!(st, StatusCode::CREATED, "{a}");
    assert_eq!(a["smart"], true);
    a["id"].as_str().unwrap().parse().unwrap()
}

async fn members(app: &Router, key: &str, album: Uuid) -> Vec<Uuid> {
    let (st, v) = send(app, "GET", &format!("/assets/albums/{album}/assets"), key, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().parse().unwrap())
        .collect()
}

fn sorted(mut v: Vec<Uuid>) -> Vec<Uuid> {
    v.sort();
    v
}

struct Lib {
    /// a: Lisbon, Canon, 2023, jpeg, favorite, 5 stars, caption "birthday cake", tags beach+sunset.
    /// b: Lisbon, Sony, 2024, raw name, 3 stars, tag beach.
    /// c: Porto, Canon, 2024, no location, favorite, archived.
    /// d: nothing at all (2022).
    ids: [Uuid; 4],
}

async fn lib(t: &TestDb, owner: Uuid, key: &str, app: &Router) -> Lib {
    let a = asset(&t.pool, owner, "a.jpg").await;
    let b = asset(&t.pool, owner, "b.cr3").await;
    let c = asset(&t.pool, owner, "c.jpg").await;
    let d = asset(&t.pool, owner, "d.jpg").await;
    set(&t.pool, a, "taken_at = '2023-06-01T10:00:00Z', latitude = 38.7, longitude = -9.1, place_city = 'Lisbon', place_country = 'Portugal', camera_make = 'Canon', camera_model = 'R5', lens_model = 'RF 24', favorite = true, rating = 5, rating_source = 'user', caption = 'Birthday cake on the table'").await;
    set(&t.pool, b, "taken_at = '2024-03-01T10:00:00Z', latitude = 38.7, longitude = -9.1, place_city = 'Lisbon', place_country = 'Portugal', camera_make = 'Sony', camera_model = 'A7', rating = 3, rating_source = 'user'").await;
    set(&t.pool, c, "taken_at = '2024-04-01T10:00:00Z', place_city = 'Porto', camera_make = 'Canon', camera_model = 'R5', favorite = true, archived = true").await;
    set(&t.pool, d, "taken_at = '2022-01-01T10:00:00Z'").await;
    post(
        app,
        key,
        "/assets/tags/apply",
        json!({"ids": [a], "add": ["Beach", "Sunset"]}),
    )
    .await;
    post(app, key, "/assets/tags/apply", json!({"ids": [b], "add": ["Beach"]})).await;
    Lib { ids: [a, b, c, d] }
}

#[tokio::test]
async fn every_rule_maps_onto_the_search_filters_and_match_all_or_any_joins_them() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app()(&t.pool);
    let l = lib(&t, alice, &ka, &app).await;
    let [a, b, c, d] = l.ids;
    let cases: Vec<(&str, Value, Vec<Uuid>)> = vec![
        (
            "date",
            json!({"field": "date", "from": "2024-01-01", "to": "2024-12-31"}),
            vec![b, c],
        ),
        (
            "date from only",
            json!({"field": "date", "from": "2024-04-01"}),
            vec![c],
        ),
        ("place", json!({"field": "place", "value": "lisbon"}), vec![a, b]),
        ("camera", json!({"field": "camera", "value": "Canon R5"}), vec![a, c]),
        ("lens", json!({"field": "lens", "value": "RF 24"}), vec![a]),
        ("type raw", json!({"field": "type", "value": "raw"}), vec![b]),
        ("favorite", json!({"field": "favorite", "value": true}), vec![a, c]),
        ("rating", json!({"field": "rating", "min": 4}), vec![a]),
        (
            "tags all",
            json!({"field": "tags", "names": ["beach", "SUNSET"], "mode": "all"}),
            vec![a],
        ),
        (
            "tags any",
            json!({"field": "tags", "names": ["sunset", "nope"], "mode": "any"}),
            vec![a],
        ),
        (
            "has location",
            json!({"field": "has_location", "value": true}),
            vec![a, b],
        ),
        (
            "no location",
            json!({"field": "has_location", "value": false}),
            vec![c, d],
        ),
        ("caption", json!({"field": "caption", "text": "BIRTHDAY"}), vec![a]),
    ];
    for (name, rule, want) in &cases {
        let album = smart(&app, &ka, name, json!({"rules": [rule]})).await;
        assert_eq!(sorted(members(&app, &ka, album).await), sorted(want.clone()), "{name}");
        let (_, got) = send(&app, "GET", &format!("/assets/albums/{album}"), &ka, None).await;
        assert_eq!(got["photo_count"].as_i64(), Some(want.len() as i64), "{name}: {got}");
    }
    // The same rule and the same search filter list the same photos.
    let (_, search) = send(&app, "GET", "/assets/search?camera=Canon%20R5", &ka, None).await;
    let from_search: Vec<Uuid> = search["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().parse().unwrap())
        .collect();
    let album = smart(
        &app,
        &ka,
        "same",
        json!({"rules": [{"field": "camera", "value": "Canon R5"}]}),
    )
    .await;
    assert_eq!(sorted(members(&app, &ka, album).await), sorted(from_search));

    // Match all / any over two rules; the archived switch is for the whole album.
    let two = json!([{"field": "place", "value": "Lisbon"}, {"field": "favorite", "value": true}]);
    let all = smart(&app, &ka, "all", json!({"match": "all", "rules": two})).await;
    let any = smart(&app, &ka, "any", json!({"match": "any", "rules": two})).await;
    assert_eq!(members(&app, &ka, all).await, [a]);
    assert_eq!(sorted(members(&app, &ka, any).await), sorted(vec![a, b, c]));
    let any_no_archive = smart(
        &app,
        &ka,
        "any2",
        json!({"match": "any", "archived": "exclude", "rules": two}),
    )
    .await;
    assert_eq!(
        sorted(members(&app, &ka, any_no_archive).await),
        sorted(vec![a, b]),
        "c is archived"
    );
    t.teardown().await;
}

#[tokio::test]
async fn membership_is_live_newest_first_with_a_newest_cover_and_never_holds_hidden_trashed_or_foreign_photos() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app()(&t.pool);
    let l = lib(&t, alice, &ka, &app).await;
    let [a, b, c, d] = l.ids;
    let album = smart(&app, &ka, "Canon or Sony", json!({"match": "any", "rules": [{"field": "camera", "value": "Canon R5"}, {"field": "camera", "value": "Sony A7"}]})).await;
    // Newest first; the cover is the newest; counts and dates are the matches'.
    assert_eq!(members(&app, &ka, album).await, [c, b, a]);
    let (_, got) = send(&app, "GET", &format!("/assets/albums/{album}"), &ka, None).await;
    assert_eq!(
        (got["photo_count"].as_i64(), got["hidden_count"].as_i64()),
        (Some(3), Some(0))
    );
    assert_eq!(got["cover"]["id"], json!(c));
    assert_eq!(got["first_at"], "2023-06-01T10:00:00Z");
    assert_eq!(got["last_at"], "2024-04-01T10:00:00Z");
    // Nothing is stored: no member rows.
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM album_assets WHERE album_id = $1")
        .bind(album)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);

    // A photo starts to match, one stops, one is trashed, one is hidden, a stranger has the same camera.
    set(&t.pool, d, "camera_make = 'Sony', camera_model = 'A7'").await;
    set(&t.pool, b, "camera_make = 'Nikon', camera_model = 'Z6'").await;
    set(&t.pool, a, "deleted_at = now()").await;
    let theirs = asset(&t.pool, bob, "theirs.jpg").await;
    set(&t.pool, theirs, "camera_make = 'Canon', camera_model = 'R5'").await;
    assert_eq!(members(&app, &ka, album).await, [c, d]);
    set(&t.pool, c, "hidden = true").await;
    assert_eq!(
        members(&app, &ka, album).await,
        [d],
        "a hidden photo is never in a smart album"
    );
    let (_, got) = send(&app, "GET", &format!("/assets/albums/{album}"), &ka, None).await;
    assert_eq!(got["photo_count"], 1);
    assert_eq!(got["cover"]["id"], json!(d), "nor is it the cover");
    // Pages by the search's cursor.
    for n in 0..3 {
        let x = asset(&t.pool, alice, &format!("x{n}.jpg")).await;
        set(&t.pool, x, "camera_make = 'Sony', camera_model = 'A7'").await;
    }
    let (_, p1) = send(
        &app,
        "GET",
        &format!("/assets/albums/{album}/assets?limit=2"),
        &ka,
        None,
    )
    .await;
    let cursor = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = send(
        &app,
        "GET",
        &format!("/assets/albums/{album}/assets?limit=2&cursor={cursor}"),
        &ka,
        None,
    )
    .await;
    assert_eq!(
        p1["items"].as_array().unwrap().len() + p2["items"].as_array().unwrap().len(),
        4
    );
    // Bob cannot see or touch Alice's album; his own photos do not leak into hers.
    for (m, path) in [
        ("GET", format!("/assets/albums/{album}")),
        ("GET", format!("/assets/albums/{album}/assets")),
        ("DELETE", format!("/assets/albums/{album}")),
    ] {
        assert_eq!(
            send(&app, m, &path, &kb, None).await.0,
            StatusCode::NOT_FOUND,
            "{m} {path}"
        );
    }
    let (_, list) = send(&app, "GET", "/assets/albums", &kb, None).await;
    assert!(list["items"].as_array().unwrap().is_empty());
    t.teardown().await;
}

#[tokio::test]
async fn a_smart_album_is_editable_and_refuses_what_only_an_ordinary_album_can_do() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app()(&t.pool);
    let l = lib(&t, alice, &ka, &app).await;
    let [a, b, _, _] = l.ids;
    let album = smart(
        &app,
        &ka,
        "Lisbon",
        json!({"rules": [{"field": "place", "value": "Lisbon"}]}),
    )
    .await;
    let path = format!("/assets/albums/{album}");
    // Edit the rules and the title.
    let (st, v) = send(&app, "PATCH", &path, &ka, Some(json!({"title": "Favorites", "rules": {"rules": [{"field": "favorite", "value": true}], "archived": "exclude"}}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (v["title"].as_str(), v["rules"]["archived"].as_str()),
        (Some("Favorites"), Some("exclude"))
    );
    assert_eq!(members(&app, &ka, album).await, [a]);
    // A text query is no rule, and says why; bad rules are plain 400s; the old rules stay.
    let (st, v) = send(
        &app,
        "PATCH",
        &path,
        &ka,
        Some(json!({"rules": {"rules": [{"field": "query", "value": "dog on the beach"}]}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("ranking is not stable"), "{v}");
    let (st, _) = send(&app, "PATCH", &path, &ka, Some(json!({"rules": {"rules": []}}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(members(&app, &ka, album).await, [a]);
    // Refused: photos by hand, a cover, rules with photos, rules on an ordinary album.
    for (m, p, body) in [
        ("POST", format!("{path}/assets"), json!({"ids": [b]})),
        ("POST", format!("{path}/assets/remove"), json!({"ids": [a]})),
        ("PATCH", path.clone(), json!({"cover_asset_id": a})),
    ] {
        let (st, v) = send(&app, m, &p, &ka, Some(body)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{m} {p}: {v}");
    }
    let (st, _) = post(
        &app,
        &ka,
        "/assets/albums",
        json!({"title": "x", "asset_ids": [a], "rules": {"rules": [{"field": "favorite", "value": true}]}}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, plain) = post(&app, &ka, "/assets/albums", json!({"title": "Plain"})).await;
    assert_eq!((st, plain["smart"].clone()), (StatusCode::CREATED, json!(false)));
    let (st, _) = send(
        &app,
        "PATCH",
        &format!("/assets/albums/{}", plain["id"].as_str().unwrap()),
        &ka,
        Some(json!({"rules": {"rules": [{"field": "favorite", "value": true}]}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "an ordinary album cannot become smart");
    // A photo's own albums list only ordinary albums.
    let (_, one) = send(&app, "GET", &format!("/assets/{a}"), &ka, None).await;
    assert!(one["albums"].as_array().unwrap().is_empty());
    // Delete removes the album and nothing else.
    assert_eq!(send(&app, "DELETE", &path, &ka, None).await.0, StatusCode::NO_CONTENT);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE owner_id = $1")
        .bind(alice)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 4);
    t.teardown().await;
}

#[tokio::test]
async fn bulk_scopes_over_a_smart_album_act_on_what_it_matches_now() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app()(&t.pool);
    let l = lib(&t, alice, &ka, &app).await;
    let [a, b, _, d] = l.ids;
    let album = smart(
        &app,
        &ka,
        "Lisbon",
        json!({"rules": [{"field": "place", "value": "Lisbon"}]}),
    )
    .await;
    let (st, r) = post(
        &app,
        &ka,
        "/assets/rating",
        json!({"scope": {"album": album}, "except": [b], "rating": 2}),
    )
    .await;
    assert_eq!((st, r["count"].as_i64()), (StatusCode::OK, Some(1)), "{r}");
    let stars: Vec<i16> = sqlx::query_scalar("SELECT rating FROM assets WHERE id = ANY($1) ORDER BY original_name")
        .bind(vec![a, b, d])
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(stars, [2, 3, 0], "only a was in scope");
    let (_, r) = post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"scope": {"album": album}, "except": [], "add": ["lisbon trip"]}),
    )
    .await;
    assert_eq!(r["added"], 2);
    t.teardown().await;
}

#[tokio::test]
async fn the_preview_counts_and_shows_photos_without_saving_anything() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app()(&t.pool);
    let l = lib(&t, alice, &ka, &app).await;
    let (st, v) = post(
        &app,
        &ka,
        "/assets/albums/preview",
        json!({"rules": {"rules": [{"field": "place", "value": "Lisbon"}]}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["count"], 2);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    assert_eq!(v["items"][0]["id"], json!(l.ids[1]), "newest first");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM albums")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let (st, v) = post(
        &app,
        &ka,
        "/assets/albums/preview",
        json!({"rules": {"rules": [{"field": "q", "value": "dog"}]}}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("text search"));
    t.teardown().await;
}

#[tokio::test]
async fn a_share_link_to_a_smart_album_resolves_the_rules_when_it_is_viewed_and_never_shows_hidden_photos() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, cookie) = session(&t.pool, "a@example.com", "member").await;
    let app = app()(&t.pool);
    let a = asset(&t.pool, alice, "a.jpg").await;
    let b = asset(&t.pool, alice, "b.jpg").await;
    let c = asset(&t.pool, alice, "c.jpg").await;
    for (id, t0) in [(a, "2024-01-01"), (b, "2024-01-02"), (c, "2024-01-03")] {
        set(&t.pool, id, &format!("taken_at = '{t0}T10:00:00Z'")).await;
    }
    set(&t.pool, a, "favorite = true").await;
    set(&t.pool, b, "favorite = true").await;
    let (st, album) = send_cookie(
        &app,
        "POST",
        "/assets/albums",
        Some(&cookie),
        Some(json!({"title": "Faves", "rules": {"rules": [{"field": "favorite", "value": true}]}})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{album}");
    let album_id = album["id"].as_str().unwrap();
    let (st, share) = send_cookie(
        &app,
        "POST",
        "/assets/shares",
        Some(&cookie),
        Some(json!({"album_id": album_id})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{share}");
    assert_eq!(share["item_count"], 2);
    let token = share["token"].as_str().unwrap().to_string();
    let ids = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect()
    };
    let items = || async {
        let (st, v) = send_cookie(&app, "GET", &format!("/assets/public/s/{token}/items"), None, None).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        ids(&v)
    };
    assert_eq!(items().await, [b.to_string(), a.to_string()]);
    let (_, info) = send_cookie(&app, "GET", &format!("/assets/public/s/{token}"), None, None).await;
    assert_eq!(info["item_count"], 2);
    // c becomes a favorite, a is hidden, b is un-favorited: the link follows with nothing stored.
    set(&t.pool, c, "favorite = true").await;
    set(&t.pool, a, "hidden = true").await;
    set(&t.pool, b, "favorite = false").await;
    assert_eq!(items().await, [c.to_string()]);
    let (st, _) = send_cookie(&app, "GET", &format!("/assets/public/s/{token}/items/{a}"), None, None).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "a hidden photo is not reachable by id either"
    );
    let (_, info) = send_cookie(&app, "GET", &format!("/assets/public/s/{token}"), None, None).await;
    assert_eq!(info["item_count"], 1);
    let (_, list) = send_cookie(&app, "GET", "/assets/shares", Some(&cookie), None).await;
    assert_eq!(list["items"][0]["item_count"], 1);
    // The owner edits the rules: the same link shows the new ones.
    send_cookie(
        &app,
        "PATCH",
        &format!("/assets/albums/{album_id}"),
        Some(&cookie),
        Some(json!({"rules": {"rules": [{"field": "date", "from": "2024-01-02"}]}})),
    )
    .await;
    assert_eq!(items().await, [c.to_string(), b.to_string()]);
    t.teardown().await;
}

/// The two queries a smart album runs for every view, on a library the size of a real one: the page of photos walks the timeline
/// index and the count is one index-supported scan, with the rule predicates inside one group.
#[tokio::test]
async fn smart_album_queries_stay_on_the_timeline_index() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = user(&t.pool, "a@example.com").await;
    sqlx::query(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, taken_at, camera_make, camera_model, favorite, rating)
         SELECT $1, sha256((i::text)::bytea), 'k' || i, 'n' || i, 'image/jpeg', 1, now() - make_interval(secs => i * 60),
                CASE WHEN i % 3 = 0 THEN 'Canon' ELSE 'Sony' END, 'M', i % 11 = 0, CASE WHEN i % 7 = 0 THEN 4 ELSE 0 END
         FROM generate_series(1, 6000) i",
    )
    .bind(alice)
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE assets").execute(&t.pool).await.unwrap();
    // OR of a favorite and a rating: the plan may not sort the whole library for a page of 61.
    let plan: Vec<(String,)> = sqlx::query_as(&format!(
        "EXPLAIN SELECT a.id FROM assets a WHERE a.owner_id = '{alice}' AND a.deleted_at IS NULL AND a.primary_id IS NULL AND NOT a.hidden
           AND ((TRUE AND a.favorite) OR (TRUE AND a.rating >= 4))
         ORDER BY coalesce(a.taken_at, a.file_modified_at, a.created_at) DESC, a.id DESC LIMIT 61"
    ))
    .fetch_all(&t.pool)
    .await
    .unwrap();
    let plan = plan.into_iter().map(|r| r.0).collect::<Vec<_>>().join("\n");
    assert!(plan.contains("assets_timeline_idx"), "{plan}");
    assert!(!plan.contains("Sort Method") && !plan.contains("\n  Sort"), "{plan}");
    t.teardown().await;
}
