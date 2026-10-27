//! Hidden photos, the archive, "On this day", "Recently added" and the library switches over a real Postgres (skipped without
//! `DATABASE_URL`).
//!
//! The first test is the one that matters most: it makes a photo hidden that is everywhere it could leak from (the timeline,
//! favorites, search with its ranked and its timeline plans, captions, facets, places, the map, an album and its share links,
//! "More like this", a burst, memories) and then calls every endpoint that lists photos, searches, clusters, shares or
//! describes them, asserting that neither the photo's id nor anything only it carries (its caption, its place, its camera)
//! is in any response, whichever of the two search plans answers.
use axum::{Router, http::StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{routes_with, search::EXACT_MAX_OVERRIDE, search_v2_tests::photo, share_public::ShareState};
use crate::{
    AppState,
    testutil::{TEST_MODEL, TestDb, asset, dummy_storage, send, send_cookie, session, user},
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
                pets_enabled: false,
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

/// What `POST /auth/hidden/unlock` does to the session row (the auth service's own tests cover the endpoint).
async fn unlock(pool: &PgPool, cookie: &str, minutes: i32) {
    let token = cookie.split_once('=').unwrap().1;
    sqlx::query("UPDATE sessions SET hidden_unlocked_until = now() + make_interval(mins => $2) WHERE token_hash = $1")
        .bind(cyanotype_service::auth::sha256(token.as_bytes()))
        .bind(minutes)
        .execute(pool)
        .await
        .unwrap();
}

/// Another login session of the same user.
async fn second_session(pool: &PgPool, uid: Uuid) -> String {
    let token = Uuid::new_v4().simple().to_string();
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1,$2, now() + interval '1 day')")
        .bind(cyanotype_service::auth::sha256(token.as_bytes()))
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
    format!("{}={token}", cyanotype_service::auth::SESSION_COOKIE)
}

async fn get(app: &Router, cookie: &str, path: &str) -> (StatusCode, Value) {
    send_cookie(app, "GET", path, Some(cookie), None).await
}

async fn post(app: &Router, cookie: &str, path: &str, body: Value) -> (StatusCode, Value) {
    send_cookie(app, "POST", path, Some(cookie), Some(body)).await
}

fn item_ids(v: &Value) -> Vec<Uuid> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().parse().unwrap())
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

/// A live, embedded photo taken at `taken`.
async fn photo_at(pool: &PgPool, owner: Uuid, name: &str, taken: &str) -> Uuid {
    let id = photo(pool, owner, name, TEST_MODEL, &[("cat", 1.0)]).await;
    set(pool, id, &format!("taken_at = '{taken}'::timestamptz")).await;
    id
}

struct Library {
    app: Router,
    uid: Uuid,
    cookie: String,
    /// The photo that gets hidden, and the ones that stay.
    hidden: Uuid,
    v1: Uuid,
    v2: Uuid,
    album: Uuid,
    album_token: String,
    selection_token: String,
    /// A smart album (favorites) that matches the hidden photo too, and a link to it.
    smart: Uuid,
    smart_token: String,
    /// The near-duplicate group of v1, v2 and the hidden photo.
    dup: Uuid,
}

/// A library in which `h.jpg` is as exposed as a photo can be, then hidden through the API.
async fn library(t: &TestDb) -> Library {
    let (uid, cookie) = session(&t.pool, "alice@example.com", "member").await;
    let app = app()(&t.pool);
    let m = TEST_MODEL;
    let v1 = photo(&t.pool, uid, "v1.jpg", m, &[("cat", 1.0), ("dog", 0.3)]).await;
    let v2 = photo(&t.pool, uid, "v2.jpg", m, &[("cat", 0.9), ("dog", 0.5)]).await;
    let h = photo(&t.pool, uid, "h.jpg", m, &[("cat", 1.0)]).await;
    set(
        &t.pool,
        v1,
        "taken_at = '2023-10-19T10:00:00Z', latitude = 38.72, longitude = -9.14, place_city = 'Lisbon', place_country = 'Portugal', favorite = true, rating = 4, rating_source = 'user'",
    )
    .await;
    set(&t.pool, v2, "taken_at = '2023-10-18T10:00:00Z', burst_id = id").await;
    set(
        &t.pool,
        h,
        "taken_at = '2023-10-19T11:00:00Z', latitude = 38.71, longitude = -9.13, place_city = 'Hiddenville', place_country = 'Portugal',
         camera_make = 'Hidden', camera_model = 'Cam', lens_model = 'Hidden Lens', favorite = true,
         caption = 'secret cat', rating = 5, rating_source = 'user', burst_id = (SELECT burst_id FROM assets WHERE id = (SELECT id FROM assets WHERE original_name = 'v2.jpg'))",
    )
    .await;
    // A tag only the hidden photo has, and one the visible photo has.
    for (photo, tag) in [(h, "secret-tag"), (v1, "Lisbon trip")] {
        let tag_id: Uuid = sqlx::query_scalar("INSERT INTO tags (owner_id, name) VALUES ($1, $2) RETURNING id")
            .bind(uid)
            .bind(tag)
            .fetch_one(&t.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO asset_tags (asset_id, tag_id) VALUES ($1, $2)")
            .bind(photo)
            .bind(tag_id)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    // A near-duplicate group of the two visible photos and the hidden one.
    let dup: Uuid = sqlx::query_scalar("INSERT INTO duplicate_groups (owner_id) VALUES ($1) RETURNING id")
        .bind(uid)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO duplicate_members (group_id, asset_id) SELECT $1, unnest($2::uuid[])")
        .bind(dup)
        .bind(vec![v1, v2, h])
        .execute(&t.pool)
        .await
        .unwrap();
    // The hidden photo is v2's burst-mate and the album's cover.
    set(&t.pool, h, &format!("burst_id = '{v2}'")).await;

    let (st, a) = post(
        &app,
        &cookie,
        "/assets/albums",
        json!({"title": "Trip", "asset_ids": [v1, v2, h]}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{a}");
    let album: Uuid = a["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE albums SET cover_asset_id = $2 WHERE id = $1")
        .bind(album)
        .bind(h)
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, album_share) = post(&app, &cookie, "/assets/shares", json!({"album_id": album})).await;
    assert_eq!(st, StatusCode::CREATED, "{album_share}");
    let (st, selection_share) = post(&app, &cookie, "/assets/shares", json!({"asset_ids": [h, v1]})).await;
    assert_eq!(st, StatusCode::CREATED, "{selection_share}");

    let (st, sm) = post(
        &app,
        &cookie,
        "/assets/albums",
        json!({"title": "Faves", "rules": {"rules": [{"field": "favorite", "value": true}]}}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{sm}");
    let smart: Uuid = sm["id"].as_str().unwrap().parse().unwrap();
    let (st, smart_share) = post(&app, &cookie, "/assets/shares", json!({"album_id": smart})).await;
    assert_eq!(st, StatusCode::CREATED, "{smart_share}");

    // Hide it (the way the app does).
    let (st, r) = post(&app, &cookie, "/assets/hide", json!({"ids": [h], "hidden": true})).await;
    assert_eq!((st, r["count"].as_i64()), (StatusCode::OK, Some(1)), "{r}");
    Library {
        app,
        uid,
        cookie,
        hidden: h,
        v1,
        v2,
        album,
        album_token: album_share["token"].as_str().unwrap().into(),
        selection_token: selection_share["token"].as_str().unwrap().into(),
        smart,
        smart_token: smart_share["token"].as_str().unwrap().into(),
        dup,
    }
}

/// Nothing of the hidden photo in a response, whatever the endpoint made of it.
fn assert_clean(what: &str, l: &Library, body: &Value) {
    let text = body.to_string();
    for needle in [
        l.hidden.to_string().as_str(),
        "h.jpg",
        "secret cat",
        "Hiddenville",
        "Hidden Cam",
        "Hidden Lens",
        "secret-tag",
    ] {
        assert!(!text.contains(needle), "{what} leaks {needle:?}: {text}");
    }
}

#[tokio::test]
async fn a_hidden_photo_is_in_no_list_search_map_album_share_or_similar_result() {
    let Some(t) = TestDb::new().await else { return };
    let l = library(&t).await;
    let (app, c) = (&l.app, l.cookie.as_str());
    let album = l.album;

    let paths: Vec<String> = [
        "/assets",
        "/assets?limit=1",
        "/assets?favorite=true",
        "/assets?archived=true",
        "/assets?sort=added",
        "/assets?bbox=-180,-90,180,90",
        "/assets?favorite=true&sort=added",
        "/assets/timeline",
        "/assets/timeline?favorite=true",
        "/assets/timeline?sort=added",
        "/assets/timeline?archived=true",
        "/assets/search",
        "/assets/search?cutoff=false",
        "/assets/search?q=cat&cutoff=false",
        "/assets/search?q=cat&weaker=true",
        "/assets/search?q=dog",
        "/assets/search?q=secret",
        "/assets/search?q=cat&favorite=true",
        "/assets/search?favorite=true",
        "/assets/search?rating=5",
        "/assets/duplicates",
        "/assets/duplicates?limit=1",
        "/assets/tags",
        "/assets/tags?q=secret",
        "/assets/tags?sort=count",
        "/assets/search?tags=secret-tag",
        "/assets/search?tags=secret-tag,lisbon%20trip&tag_mode=any",
        "/assets/search?tags=lisbon%20trip",
        "/assets/search?q=secret&cutoff=false",
        "/assets/search?rating=1",
        "/assets/search?q=cat&rating=1&cutoff=false",
        "/assets/search?place=Hiddenville",
        "/assets/search?camera=Hidden%20Cam",
        "/assets/search?lens=Hidden%20Lens",
        "/assets/search?has_location=true",
        "/assets/search?type=jpeg",
        "/assets/search?from=2023-10-19&to=2023-10-19",
        "/assets/search?include_archived=false",
        "/assets/search/facets",
        "/assets/map?zoom=0",
        "/assets/map?zoom=22",
        "/assets/map?zoom=22&favorite=true",
        "/assets/places?q=Hidden",
        "/assets/places",
        "/assets/albums",
        "/assets/memories?date=2026-10-19",
        "/assets/memories?date=2026-10-19&all=true",
        "/assets/shares",
        "/assets/trash",
    ]
    .iter()
    .map(|p| p.to_string())
    .chain([
        format!("/assets/search?album={album}"),
        format!("/assets/search?q=cat&album={album}"),
        format!("/assets/{}/similar?cutoff=false", l.v1),
        format!("/assets/{}/similar?cutoff=false", l.v2),
        format!("/assets/{}/similar?cutoff=false&favorite=true", l.v1),
        format!("/assets/albums/{album}"),
        format!("/assets/albums/{album}/assets"),
        format!("/assets/albums/{}", l.smart),
        format!("/assets/albums/{}/assets", l.smart),
        format!("/assets/search?album={}", l.smart),
        format!("/assets/public/s/{}", l.smart_token),
        format!("/assets/public/s/{}/items", l.smart_token),
        format!("/assets/{}", l.v1),
        format!("/assets/{}/details", l.v1),
        format!("/assets/{}/details", l.v2),
        format!("/assets/public/s/{}", l.album_token),
        format!("/assets/public/s/{}/items", l.album_token),
        format!("/assets/public/s/{}", l.selection_token),
        format!("/assets/public/s/{}/items", l.selection_token),
    ])
    .collect();

    // Both search plans: the exact distance scan and the HNSW index.
    for exact_max in [None, Some(0)] {
        EXACT_MAX_OVERRIDE.with(|x| x.set(exact_max));
        for p in &paths {
            let (st, body) = get(app, c, p).await;
            assert!(st.is_success(), "{p} (exact_max {exact_max:?}): {st} {body}");
            assert_clean(&format!("{p} (exact_max {exact_max:?})"), &l, &body);
        }
    }
    EXACT_MAX_OVERRIDE.with(|x| x.set(None));

    // The ones that must not even answer for the hidden photo itself.
    for p in [
        format!("/assets/{}", l.hidden),
        format!("/assets/{}/details", l.hidden),
        format!("/assets/{}/download", l.hidden),
        format!("/assets/{}/similar", l.hidden),
        format!("/assets/public/s/{}/items/{}", l.album_token, l.hidden),
        format!("/assets/public/s/{}/items/{}", l.smart_token, l.hidden),
        format!("/assets/public/s/{}/items/{}", l.selection_token, l.hidden),
        format!("/assets/public/s/{}/items/{}/download", l.selection_token, l.hidden),
    ] {
        let (st, _) = get(app, c, &p).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{p}");
    }

    // What the lists do show: the other photos, counted without the hidden one.
    let (_, list) = get(app, c, "/assets").await;
    assert_eq!(item_ids(&list).len(), 2);
    let (_, tl) = get(app, c, "/assets/timeline").await;
    assert_eq!(tl["total"], 2);
    let (_, f) = get(app, c, "/assets?favorite=true").await;
    assert_eq!(item_ids(&f), [l.v1]);
    let (_, facets) = get(app, c, "/assets/search/facets").await;
    assert_eq!(
        (facets["total"].as_i64(), facets["favorites"].as_i64()),
        (Some(2), Some(1))
    );
    // The tag only the hidden photo has is not listed to a locked session; the ordinary one counts its visible photo.
    let (_, tag_list) = get(app, c, "/assets/tags").await;
    let names: Vec<(&str, i64)> = tag_list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["name"].as_str().unwrap(), x["count"].as_i64().unwrap()))
        .collect();
    assert_eq!(names, [("Lisbon trip", 1)], "{tag_list}");
    let (_, by_tag) = get(app, c, "/assets/search?tags=secret-tag").await;
    assert!(item_ids(&by_tag).is_empty());
    // The hidden photo's five stars are in no rating filter or facet.
    assert_eq!(
        facets["ratings"],
        json!([{"value": "4", "count": 1}, {"value": "3", "count": 1}, {"value": "2", "count": 1}, {"value": "1", "count": 1}])
    );
    let (_, five) = get(app, c, "/assets/search?rating=5").await;
    assert!(item_ids(&five).is_empty());
    let (_, map) = get(app, c, "/assets/map?zoom=22").await;
    assert_eq!(map["total"], 1);

    // The album keeps the member, says so to its owner, and never uses it as the cover.
    let (_, albums) = get(app, c, "/assets/albums").await;
    let a = albums["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == json!(l.album))
        .unwrap();
    // The smart album matches the visible favorite only, never the hidden one, and has nothing hidden to report.
    let smart = albums["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == json!(l.smart))
        .unwrap();
    assert_eq!(
        (smart["photo_count"].as_i64(), smart["hidden_count"].as_i64()),
        (Some(1), Some(0))
    );
    assert_eq!(smart["cover"]["id"], json!(l.v1));
    assert_eq!(
        (a["photo_count"].as_i64(), a["hidden_count"].as_i64()),
        (Some(2), Some(1))
    );
    assert_ne!(a["cover"]["id"], json!(l.hidden.to_string()));
    let in_album: i64 = sqlx::query_scalar("SELECT count(*) FROM album_assets WHERE album_id = $1")
        .bind(l.album)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(in_album, 3, "hiding keeps the album membership");

    // The links count and list only what is visible.
    let (_, info) = get(app, c, &format!("/assets/public/s/{}", l.selection_token)).await;
    assert_eq!(info["item_count"], 1, "{info}");
    let (_, shares) = get(app, c, "/assets/shares").await;
    let counts: Vec<i64> = shares["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["item_count"].as_i64().unwrap())
        .collect();
    assert_eq!(counts.iter().sum::<i64>(), 2 + 1 + 1, "{shares}");
    t.teardown().await;
}

#[tokio::test]
async fn a_hidden_photo_is_rated_only_from_an_unlocked_session() {
    let Some(t) = TestDb::new().await else { return };
    let l = library(&t).await;
    let stars = || async {
        sqlx::query_scalar::<_, i16>("SELECT rating FROM assets WHERE id = $1")
            .bind(l.hidden)
            .fetch_one(&t.pool)
            .await
            .unwrap()
    };
    // Locked: by id the request quietly does nothing, by scope the Hidden view is refused.
    let (st, r) = post(
        &l.app,
        &l.cookie,
        "/assets/rating",
        json!({"ids": [l.hidden], "rating": 1}),
    )
    .await;
    assert_eq!((st, r["count"].as_i64()), (StatusCode::OK, Some(0)), "{r}");
    let (st, _) = post(
        &l.app,
        &l.cookie,
        "/assets/rating",
        json!({"scope": "hidden", "rating": 1}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(stars().await, 5);
    unlock(&t.pool, &l.cookie, 10).await;
    let (_, r) = post(
        &l.app,
        &l.cookie,
        "/assets/rating",
        json!({"ids": [l.hidden], "rating": 2}),
    )
    .await;
    assert_eq!(r["count"], 1);
    assert_eq!(stars().await, 2);
    let (_, r) = post(
        &l.app,
        &l.cookie,
        "/assets/rating",
        json!({"scope": "hidden", "rating": 3}),
    )
    .await;
    assert_eq!(r["count"], 1);
    assert_eq!(stars().await, 3);
    t.teardown().await;
}

#[tokio::test]
async fn a_hidden_photo_is_tagged_only_from_an_unlocked_session_and_its_tags_are_listed_only_then() {
    let Some(t) = TestDb::new().await else { return };
    let l = library(&t).await;
    let tags_of = |name: &'static str| {
        let pool = t.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM asset_tags at JOIN tags t ON t.id = at.tag_id WHERE t.name = $1 AND at.asset_id IN (SELECT id FROM assets WHERE hidden)",
            )
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    // Locked: adding by id quietly does nothing, by scope the Hidden view is refused; removing changes nothing either.
    let (st, r) = post(
        &l.app,
        &l.cookie,
        "/assets/tags/apply",
        json!({"ids": [l.hidden], "add": ["new-one"]}),
    )
    .await;
    assert_eq!((st, r["added"].as_i64()), (StatusCode::OK, Some(0)), "{r}");
    let (st, _) = post(
        &l.app,
        &l.cookie,
        "/assets/tags/apply",
        json!({"scope": "hidden", "add": ["new-one"]}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (_, r) = post(
        &l.app,
        &l.cookie,
        "/assets/tags/apply",
        json!({"ids": [l.hidden], "remove": ["secret-tag"]}),
    )
    .await;
    assert_eq!(r["removed"], 0);
    assert_eq!(tags_of("secret-tag").await, 1);
    // Unlocked: the tag is listed with its hidden photo counted, and the hidden photo can be tagged and untagged.
    unlock(&t.pool, &l.cookie, 10).await;
    let (_, list) = get(&l.app, &l.cookie, "/assets/tags").await;
    assert!(list.to_string().contains("secret-tag"), "{list}");
    let (_, r) = post(
        &l.app,
        &l.cookie,
        "/assets/tags/apply",
        json!({"ids": [l.hidden], "add": ["new-one"]}),
    )
    .await;
    assert_eq!(r["added"], 1);
    assert_eq!(tags_of("new-one").await, 1);
    let (_, r) = post(
        &l.app,
        &l.cookie,
        "/assets/tags/apply",
        json!({"scope": "hidden", "remove": ["new-one", "secret-tag"]}),
    )
    .await;
    assert_eq!(r["removed"], 2);
    t.teardown().await;
}

#[tokio::test]
async fn a_hidden_photo_is_in_a_duplicate_group_only_for_an_unlocked_session_and_is_never_merged_or_trashed_by_a_locked_one()
 {
    let Some(t) = TestDb::new().await else { return };
    let l = library(&t).await;
    let members = |v: &Value| -> Vec<Uuid> {
        v["groups"][0]["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().parse().unwrap())
            .collect()
    };
    // Locked: two members, never the hidden one; the total counts the group once.
    let (_, v) = get(&l.app, &l.cookie, "/assets/duplicates").await;
    assert_eq!(v["total"], 1);
    assert_eq!(sorted_ids(members(&v)), sorted_ids(vec![l.v1, l.v2]));
    // Unlocked: all three.
    unlock(&t.pool, &l.cookie, 10).await;
    let (_, v) = get(&l.app, &l.cookie, "/assets/duplicates").await;
    assert_eq!(members(&v).len(), 3);
    // A session that is locked again (another one of the same user) keeps v1: only v2 goes to the trash, the hidden photo is untouched.
    let other = second_session(&t.pool, l.uid).await;
    let (st, r) = post(
        &l.app,
        &other,
        &format!("/assets/duplicates/{}/keep", l.dup),
        json!({"keep": l.v1}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["trashed"], json!([l.v2]));
    let (deleted, hidden): (bool, bool) =
        sqlx::query_as("SELECT deleted_at IS NOT NULL, hidden FROM assets WHERE id = $1")
            .bind(l.hidden)
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert!(
        !deleted && hidden,
        "a locked session neither trashes nor unhides a hidden photo"
    );
    // The hidden photo's caption, tag and place were not merged onto v1 (nor anything of v1 onto it).
    let tags: i64 = sqlx::query_scalar("SELECT count(*) FROM asset_tags WHERE asset_id = $1 AND tag_id IN (SELECT id FROM tags WHERE name = 'secret-tag')")
        .bind(l.v1)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(tags, 0);
    let caption: Option<String> = sqlx::query_scalar("SELECT caption FROM assets WHERE id = $1")
        .bind(l.v1)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(caption, None, "the hidden photo's caption stays with it");
    // A group a locked session cannot see at all (fewer than two visible members) is a 404-free empty list, and keeping it is refused.
    let (_, v) = get(&l.app, &other, "/assets/duplicates").await;
    assert_eq!(v["total"], 0);
    let (st, _) = post(
        &l.app,
        &other,
        &format!("/assets/duplicates/{}/keep", l.dup),
        json!({"keep": l.hidden}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    t.teardown().await;
}

fn sorted_ids(mut v: Vec<Uuid>) -> Vec<Uuid> {
    v.sort();
    v
}

#[tokio::test]
async fn a_hidden_photo_cannot_be_added_to_a_new_share_link_or_chosen_by_scope() {
    let Some(t) = TestDb::new().await else { return };
    let l = library(&t).await;
    let (st, v) = post(&l.app, &l.cookie, "/assets/shares", json!({"asset_ids": [l.hidden]})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    // Even unlocked, a link cannot be made from the Hidden view.
    unlock(&t.pool, &l.cookie, 10).await;
    let (st, _) = post(&l.app, &l.cookie, "/assets/shares", json!({"scope": "hidden"})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // A mixed selection keeps only the visible photo.
    let (st, v) = post(
        &l.app,
        &l.cookie,
        "/assets/shares",
        json!({"asset_ids": [l.hidden, l.v1]}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["item_count"], 1);
    let (st, v) = post(
        &l.app,
        &l.cookie,
        "/assets/shares",
        json!({"scope": "library", "except": [l.v2]}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["item_count"], 1, "select-all never reaches a hidden photo");
    t.teardown().await;
}

#[tokio::test]
async fn the_hidden_view_needs_an_unlock_that_lives_on_the_session_and_runs_out() {
    let Some(t) = TestDb::new().await else { return };
    let l = library(&t).await;
    let (app, c) = (&l.app, l.cookie.as_str());
    let (uid, key) = (l.uid, {
        // An API key of the same user: never unlocked.
        let k = format!("cyk_{}", Uuid::new_v4().simple());
        sqlx::query("INSERT INTO api_keys (user_id, name, key_hash) VALUES ($1,'t',$2)")
            .bind(l.uid)
            .bind(cyanotype_service::auth::sha256(k.as_bytes()))
            .execute(&t.pool)
            .await
            .unwrap();
        k
    });

    // Locked: every hidden-only endpoint refuses, with a code the app can act on.
    for p in ["/assets?hidden=true", "/assets/timeline?hidden=true"] {
        let (st, body) = get(app, c, p).await;
        assert_eq!(
            (st, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("hidden_locked")),
            "{p}"
        );
    }
    let (st, body) = post(app, c, "/assets/hide", json!({"ids": [l.hidden], "hidden": false})).await;
    assert_eq!(
        (st, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("hidden_locked"))
    );
    let (st, _) = post(app, c, "/assets/hide", json!({"scope": "hidden", "hidden": false})).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = post(app, c, "/assets/trash", json!({"scope": "hidden"})).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = send(app, "GET", "/assets?hidden=true", &key, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "an API key can never open it");

    // Unlocked (ten minutes): the list, the day counts, the photo itself, its details and similar results.
    unlock(&t.pool, c, 10).await;
    let (st, v) = get(app, c, "/assets?hidden=true").await;
    assert_eq!((st, item_ids(&v)), (StatusCode::OK, vec![l.hidden]));
    assert_eq!(v["items"][0]["hidden"], true);
    let (_, tl) = get(app, c, "/assets/timeline?hidden=true").await;
    assert_eq!(tl["total"], 1);
    for p in [
        format!("/assets/{}", l.hidden),
        format!("/assets/{}/details", l.hidden),
        format!("/assets/{}/similar?cutoff=false", l.hidden),
    ] {
        let (st, body) = get(app, c, &p).await;
        assert_eq!(st, StatusCode::OK, "{p}: {body}");
    }
    // Still not in the normal views while unlocked: unlocking opens a view, it does not merge it in.
    let (_, list) = get(app, c, "/assets").await;
    assert!(!item_ids(&list).contains(&l.hidden));
    let (_, s) = get(app, c, "/assets/search?q=cat&cutoff=false").await;
    assert!(!item_ids(&s).contains(&l.hidden));
    // The unlock is this session's: another session of the same user, and the key, are still locked.
    let other = second_session(&t.pool, uid).await;
    let (st, _) = get(app, &other, "/assets?hidden=true").await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = get(app, &other, &format!("/assets/{}", l.hidden)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send(app, "GET", &format!("/assets/{}", l.hidden), &key, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // Another user, unlocked or not, has no way to it.
    let (_, bob) = session(&t.pool, "bob@example.com", "member").await;
    unlock(&t.pool, &bob, 10).await;
    let (_, theirs) = get(app, &bob, "/assets?hidden=true").await;
    assert!(item_ids(&theirs).is_empty());
    let (st, _) = get(app, &bob, &format!("/assets/{}", l.hidden)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // It runs out: ten minutes is not "until sign-out".
    sqlx::query("UPDATE sessions SET hidden_unlocked_until = now() - interval '1 second'")
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, _) = get(app, c, "/assets?hidden=true").await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = get(app, c, &format!("/assets/{}", l.hidden)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Unhiding works from the open view, and the photo is back everywhere.
    unlock(&t.pool, c, 10).await;
    let (st, r) = post(app, c, "/assets/hide", json!({"scope": "hidden", "hidden": false})).await;
    assert_eq!((st, r["count"].as_i64()), (StatusCode::OK, Some(1)), "{r}");
    let (_, list) = get(app, c, "/assets").await;
    assert!(item_ids(&list).contains(&l.hidden));
    let (_, empty) = get(app, c, "/assets?hidden=true").await;
    assert!(item_ids(&empty).is_empty());
    t.teardown().await;
}

#[tokio::test]
async fn hiding_and_unhiding_are_audited_with_counts_only() {
    let Some(t) = TestDb::new().await else { return };
    let l = library(&t).await;
    unlock(&t.pool, &l.cookie, 10).await;
    post(
        &l.app,
        &l.cookie,
        "/assets/hide",
        json!({"ids": [l.v1, l.v2], "hidden": true}),
    )
    .await;
    post(
        &l.app,
        &l.cookie,
        "/assets/hide",
        json!({"scope": "hidden", "hidden": false}),
    )
    .await;
    // Repeating changes nothing and writes nothing.
    post(
        &l.app,
        &l.cookie,
        "/assets/hide",
        json!({"scope": "hidden", "hidden": false}),
    )
    .await;
    let rows: Vec<(String, Option<String>, Option<String>, Value)> =
        sqlx::query_as("SELECT action, target_type, target_id, detail FROM audit_log ORDER BY at, id")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    let actions: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(actions, ["photos.hide", "photos.hide", "photos.unhide"]);
    for (_, target_type, target_id, detail) in &rows {
        assert!(target_type.is_none() && target_id.is_none(), "no photo is named");
        assert_eq!(detail.as_object().unwrap().keys().collect::<Vec<_>>(), ["count"]);
    }
    assert_eq!(rows[1].3["count"], 2);
    t.teardown().await;
}

#[tokio::test]
async fn hiding_a_raw_jpeg_pair_hides_both_and_a_raw_hidden_alone_hides_the_pair() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, c) = session(&t.pool, "a@example.com", "member").await;
    let app = app()(&t.pool);
    let jpg = asset(&t.pool, uid, "IMG_1.jpg").await;
    let raw = asset(&t.pool, uid, "IMG_1.cr3").await;
    sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
        .bind(raw)
        .bind(jpg)
        .execute(&t.pool)
        .await
        .unwrap();
    // The id of the RAW means its primary.
    let (_, r) = post(&app, &c, "/assets/hide", json!({"ids": [raw], "hidden": true})).await;
    assert_eq!(r["count"], 1);
    let flags: Vec<(String, bool)> = sqlx::query_as("SELECT original_name, hidden FROM assets ORDER BY original_name")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(flags, [("IMG_1.cr3".into(), true), ("IMG_1.jpg".into(), true)]);
    let (_, list) = get(&app, &c, "/assets").await;
    assert!(item_ids(&list).is_empty());

    // Pairing carries it: a RAW that was hidden on its own, joined to a visible JPEG, hides the pair; and a RAW joining a
    // hidden photo takes its state (so separating them keeps it).
    let jpg2 = asset(&t.pool, uid, "IMG_2.jpg").await;
    let raw2 = asset(&t.pool, uid, "IMG_2.cr3").await;
    for id in [jpg2, raw2] {
        set(&t.pool, id, "taken_at = '2024-05-01T12:00:00Z'").await;
    }
    set(&t.pool, raw2, "hidden = true").await;
    cyanotype_core::pairing::pair_asset(&t.pool, jpg2).await.unwrap();
    let flags: Vec<(String, bool)> = sqlx::query_as(
        "SELECT original_name, hidden FROM assets WHERE original_name LIKE 'IMG_2%' ORDER BY original_name",
    )
    .fetch_all(&t.pool)
    .await
    .unwrap();
    assert_eq!(flags, [("IMG_2.cr3".into(), true), ("IMG_2.jpg".into(), true)]);
    t.teardown().await;
}

#[tokio::test]
async fn trashed_hidden_photos_show_in_the_trash_only_when_unlocked() {
    let Some(t) = TestDb::new().await else { return };
    let l = library(&t).await;
    unlock(&t.pool, &l.cookie, 10).await;
    let (_, r) = post(&l.app, &l.cookie, "/assets/trash", json!({"ids": [l.hidden, l.v1]})).await;
    assert_eq!(r["count"], 2);
    let (_, locked_view) = get(&l.app, &second_session(&t.pool, l.uid).await, "/assets/trash").await;
    assert_eq!(
        (names(&locked_view), locked_view["total"].as_i64()),
        (vec!["v1.jpg".to_string()], Some(1))
    );
    let (_, open) = get(&l.app, &l.cookie, "/assets/trash").await;
    assert_eq!(open["total"], 2);
    // A locked session's "restore everything in the trash" cannot bring it back.
    let locked = second_session(&t.pool, l.uid).await;
    let (_, r) = post(&l.app, &locked, "/assets/restore", json!({"scope": "trash"})).await;
    assert_eq!(r["count"], 1);
    let hidden_deleted: bool = sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM assets WHERE id = $1")
        .bind(l.hidden)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(hidden_deleted);
    t.teardown().await;
}

#[tokio::test]
async fn select_all_in_the_library_never_touches_hidden_or_archived_photos() {
    let Some(t) = TestDb::new().await else { return };
    let l = library(&t).await;
    // v2 archived, h hidden, v1 visible.
    let (_, r) = post(
        &l.app,
        &l.cookie,
        "/assets/archive",
        json!({"ids": [l.v2], "archived": true}),
    )
    .await;
    assert_eq!(r["count"], 1);
    let (_, r) = post(
        &l.app,
        &l.cookie,
        "/assets/trash",
        json!({"scope": "library", "except": []}),
    )
    .await;
    assert_eq!(r["count"], 1, "only the one photo the timeline shows");
    let trashed: Vec<String> = sqlx::query_scalar("SELECT original_name FROM assets WHERE deleted_at IS NOT NULL")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(trashed, ["v1.jpg"]);
    t.teardown().await;
}

// ---------------------------------------------------------------------------------------------------------------------------
// Archive

#[tokio::test]
async fn archived_photos_leave_the_timeline_the_map_and_memories_but_stay_in_albums_search_and_favorites() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, c) = session(&t.pool, "a@example.com", "member").await;
    let app = app()(&t.pool);
    let a = photo_at(&t.pool, uid, "a.jpg", "2023-10-19T10:00:00Z").await;
    let b = photo_at(&t.pool, uid, "b.jpg", "2023-10-19T11:00:00Z").await;
    set(&t.pool, a, "latitude = 10, longitude = 10, favorite = true").await;
    set(&t.pool, b, "latitude = 10, longitude = 10").await;
    let (_, album) = post(&app, &c, "/assets/albums", json!({"title": "X", "asset_ids": [a, b]})).await;
    let album = album["id"].as_str().unwrap().to_string();

    let (st, r) = post(&app, &c, "/assets/archive", json!({"ids": [a], "archived": true})).await;
    assert_eq!((st, r["count"].as_i64()), (StatusCode::OK, Some(1)));
    assert!(r.get("batch").is_none(), "an id-based archive is undone by ids");

    let (_, list) = get(&app, &c, "/assets").await;
    assert_eq!(item_ids(&list), [b]);
    let (_, tl) = get(&app, &c, "/assets/timeline").await;
    assert_eq!(tl["total"], 1);
    let (_, map) = get(&app, &c, "/assets/map?zoom=22").await;
    assert_eq!(map["total"], 1);
    let (_, mem) = get(&app, &c, "/assets/memories?date=2026-10-19&all=true").await;
    assert_eq!(item_ids(&mem["years"][0]), [b]);
    // The Archive view lists it, counts it and flags it.
    let (_, arch) = get(&app, &c, "/assets?archived=true").await;
    assert_eq!(item_ids(&arch), [a]);
    assert_eq!(arch["items"][0]["archived"], true);
    let (_, atl) = get(&app, &c, "/assets/timeline?archived=true").await;
    assert_eq!(atl["total"], 1);
    // Still in the album, in search (flagged) and in favorites.
    let (_, in_album) = get(&app, &c, &format!("/assets/albums/{album}/assets")).await;
    assert_eq!(item_ids(&in_album).len(), 2);
    let (_, albums) = get(&app, &c, "/assets/albums").await;
    assert_eq!(albums["items"][0]["photo_count"], 2);
    let (_, s) = get(&app, &c, "/assets/search?cutoff=false").await;
    assert_eq!(item_ids(&s).len(), 2);
    let flagged: Vec<(Uuid, bool)> = s["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["id"].as_str().unwrap().parse().unwrap(),
                i["archived"].as_bool().unwrap(),
            )
        })
        .collect();
    assert!(
        flagged.contains(&(a, true)) && flagged.contains(&(b, false)),
        "{flagged:?}"
    );
    let (_, s) = get(&app, &c, "/assets/search?q=cat&cutoff=false").await;
    assert_eq!(item_ids(&s).len(), 2);
    let (_, s) = get(&app, &c, "/assets/search?include_archived=false&cutoff=false").await;
    assert_eq!(item_ids(&s), [b]);
    let (_, s) = get(&app, &c, "/assets/search?q=cat&include_archived=false&cutoff=false").await;
    assert_eq!(item_ids(&s), [b]);
    let (_, fav) = get(&app, &c, "/assets?favorite=true").await;
    assert_eq!(item_ids(&fav), [a]);
    let (_, ftl) = get(&app, &c, "/assets/timeline?favorite=true").await;
    assert_eq!(ftl["total"], 1);

    // Restore from the Archive.
    let (_, r) = post(&app, &c, "/assets/archive", json!({"ids": [a], "archived": false})).await;
    assert_eq!(r["count"], 1);
    let (_, list) = get(&app, &c, "/assets").await;
    assert_eq!(item_ids(&list).len(), 2);
    // Idempotent.
    let (_, r) = post(&app, &c, "/assets/archive", json!({"ids": [a], "archived": false})).await;
    assert_eq!(r["count"], 0);
    t.teardown().await;
}

#[tokio::test]
async fn a_bulk_archive_by_scope_is_undone_by_its_batch_and_only_that_batch() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, c) = session(&t.pool, "a@example.com", "member").await;
    let app = app()(&t.pool);
    let old = photo_at(&t.pool, uid, "old.jpg", "2020-01-01T00:00:00Z").await;
    for i in 0..3 {
        photo_at(&t.pool, uid, &format!("p{i}.jpg"), "2021-01-01T00:00:00Z").await;
    }
    post(&app, &c, "/assets/archive", json!({"ids": [old], "archived": true})).await;
    let (_, r) = post(
        &app,
        &c,
        "/assets/archive",
        json!({"scope": "library", "except": [], "archived": true}),
    )
    .await;
    assert_eq!(r["count"], 3);
    let batch = r["batch"]
        .as_str()
        .expect("a scope-based archive names its batch")
        .to_string();
    let (_, r) = post(
        &app,
        &c,
        "/assets/archive",
        json!({"scope": {"archive_batch": batch}, "archived": false}),
    )
    .await;
    assert_eq!(r["count"], 3, "{r}");
    let (_, list) = get(&app, &c, "/assets").await;
    assert_eq!(item_ids(&list).len(), 3);
    let (_, arch) = get(&app, &c, "/assets?archived=true").await;
    assert_eq!(item_ids(&arch), [old], "the photo archived earlier stays archived");
    t.teardown().await;
}

// ---------------------------------------------------------------------------------------------------------------------------
// Recently added

#[tokio::test]
async fn recently_added_orders_by_upload_time_and_counts_days_of_upload() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, c) = session(&t.pool, "a@example.com", "member").await;
    let app = app()(&t.pool);
    // Captured long ago, uploaded recently (and the reverse).
    let a = photo_at(&t.pool, uid, "a.jpg", "2010-01-01T00:00:00Z").await;
    let b = photo_at(&t.pool, uid, "b.jpg", "2024-01-01T00:00:00Z").await;
    let d = photo_at(&t.pool, uid, "c.jpg", "2015-01-01T00:00:00Z").await;
    set(&t.pool, a, "created_at = '2026-10-18T12:00:00Z'").await;
    set(&t.pool, b, "created_at = '2026-10-10T12:00:00Z'").await;
    set(&t.pool, d, "created_at = '2026-10-18T13:00:00Z'").await;

    let (_, taken) = get(&app, &c, "/assets").await;
    assert_eq!(item_ids(&taken), [b, d, a], "by capture time");
    let (_, added) = get(&app, &c, "/assets?sort=added").await;
    assert_eq!(item_ids(&added), [d, a, b], "by upload time, newest first");
    assert_eq!(added["items"][0]["sort_time"], "2026-10-18T13:00:00Z");
    let (_, days) = get(&app, &c, "/assets/timeline?sort=added").await;
    assert_eq!(
        days["days"],
        json!([{"day": "2026-10-18", "count": 2}, {"day": "2026-10-10", "count": 1}])
    );
    // Paging continues without gaps in upload order.
    let (_, p1) = get(&app, &c, "/assets?sort=added&limit=2").await;
    let cursor = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = get(&app, &c, &format!("/assets?sort=added&limit=2&cursor={cursor}")).await;
    assert_eq!((item_ids(&p1), item_ids(&p2)), (vec![d, a], vec![b]));
    // It leaves out what the timeline leaves out.
    post(&app, &c, "/assets/archive", json!({"ids": [a], "archived": true})).await;
    post(&app, &c, "/assets/hide", json!({"ids": [d], "hidden": true})).await;
    let (_, added) = get(&app, &c, "/assets?sort=added").await;
    assert_eq!(item_ids(&added), [b]);
    let (st, _) = get(&app, &c, "/assets?sort=sideways").await;
    assert!(st.is_client_error());
    t.teardown().await;
}

#[tokio::test]
async fn the_sort_order_and_the_memories_strip_are_remembered_per_user() {
    let Some(t) = TestDb::new().await else { return };
    let (_, c) = session(&t.pool, "a@example.com", "member").await;
    let (_, other) = session(&t.pool, "b@example.com", "member").await;
    let app = app()(&t.pool);
    let (_, p) = get(&app, &c, "/assets/prefs").await;
    assert_eq!(
        p,
        json!({"memories_enabled": true, "library_sort": "taken", "stacks_collapsed": true})
    );
    let (st, p) = send_cookie(
        &app,
        "PUT",
        "/assets/prefs",
        Some(&c),
        Some(json!({"library_sort": "added"})),
    )
    .await;
    assert_eq!(
        (st, &p["library_sort"], &p["memories_enabled"]),
        (StatusCode::OK, &json!("added"), &json!(true))
    );
    let (_, p) = send_cookie(
        &app,
        "PUT",
        "/assets/prefs",
        Some(&c),
        Some(json!({"memories_enabled": false})),
    )
    .await;
    assert_eq!(
        p,
        json!({"memories_enabled": false, "library_sort": "added", "stacks_collapsed": true})
    );
    let (_, again) = get(&app, &c, "/assets/prefs").await;
    assert_eq!(again, p);
    let (_, theirs) = get(&app, &other, "/assets/prefs").await;
    assert_eq!(
        theirs,
        json!({"memories_enabled": true, "library_sort": "taken", "stacks_collapsed": true})
    );
    let (st, _) = send_cookie(
        &app,
        "PUT",
        "/assets/prefs",
        Some(&c),
        Some(json!({"library_sort": "sideways"})),
    )
    .await;
    assert!(st.is_client_error());
    t.teardown().await;
}

// ---------------------------------------------------------------------------------------------------------------------------
// On this day

fn years(v: &Value) -> Vec<(i64, i64, Vec<String>)> {
    v["years"]
        .as_array()
        .unwrap()
        .iter()
        .map(|y| (y["years_ago"].as_i64().unwrap(), y["count"].as_i64().unwrap(), names(y)))
        .collect()
}

#[tokio::test]
async fn on_this_day_groups_by_year_and_uses_the_photos_own_local_date() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, c) = session(&t.pool, "a@example.com", "member").await;
    let app = app()(&t.pool);
    // 19 October in earlier years.
    photo_at(&t.pool, uid, "y1.jpg", "2025-10-19T12:00:00Z").await;
    photo_at(&t.pool, uid, "y3a.jpg", "2023-10-19T09:00:00Z").await;
    photo_at(&t.pool, uid, "y3b.jpg", "2023-10-19T15:00:00Z").await;
    // Not this day, and not an earlier year.
    photo_at(&t.pool, uid, "other_day.jpg", "2023-10-18T12:00:00Z").await;
    photo_at(&t.pool, uid, "this_year.jpg", "2026-10-19T08:00:00Z").await;
    // Local dates: 06:30 UTC on the 20th is the 19th at 23:30 in UTC-7 (a memory); 23:30 UTC on the 19th is the 20th at 01:30
    // in UTC+2 (not one); with no zone it is the UTC date.
    let west = photo_at(&t.pool, uid, "west.jpg", "2022-10-20T06:30:00Z").await;
    set(
        &t.pool,
        west,
        r#"exif = '{"DateTimeOriginal": "2022:10:19 23:30:00", "OffsetTimeOriginal": "-07:00"}'::jsonb"#,
    )
    .await;
    let east = photo_at(&t.pool, uid, "east.jpg", "2021-10-19T23:30:00Z").await;
    set(
        &t.pool,
        east,
        r#"exif = '{"DateTimeOriginal": "2021:10:20 01:30:00", "OffsetTimeOriginal": "+02:00"}'::jsonb"#,
    )
    .await;
    photo_at(&t.pool, uid, "nozone.jpg", "2020-10-19T23:59:00Z").await;
    // A photo with no capture time has no date of its own, whatever its file says.
    let nodate = asset(&t.pool, uid, "nodate.jpg").await;
    set(&t.pool, nodate, "file_modified_at = '2019-10-19T12:00:00Z'").await;

    let (st, m) = get(&app, &c, "/assets/memories?date=2026-10-19").await;
    assert_eq!(st, StatusCode::OK, "{m}");
    assert_eq!(m["day"], "10-19");
    assert_eq!(m["enabled"], true);
    assert_eq!(
        years(&m),
        [
            (1, 1, vec!["y1.jpg".into()]),
            (3, 2, vec!["y3a.jpg".into(), "y3b.jpg".into()]),
            (4, 1, vec!["west.jpg".into()]),
            (6, 1, vec!["nozone.jpg".into()]),
        ]
    );
    assert_eq!(m["years"][0]["year"], 2025);
    // The photo's UTC offset shows up in the (UTC) timeline day but not in the memory.
    t.teardown().await;
}

#[tokio::test]
async fn on_this_day_honours_edits_and_skips_hidden_archived_trashed_dismissed_and_burst_mates() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, c) = session(&t.pool, "a@example.com", "member").await;
    let app = app()(&t.pool);
    let ok = photo_at(&t.pool, uid, "ok.jpg", "2024-03-05T10:00:00Z").await;
    let hidden = photo_at(&t.pool, uid, "hidden.jpg", "2024-03-05T10:01:00Z").await;
    let archived = photo_at(&t.pool, uid, "archived.jpg", "2024-03-05T10:02:00Z").await;
    let trashed = photo_at(&t.pool, uid, "trashed.jpg", "2024-03-05T10:03:00Z").await;
    let dismissed = photo_at(&t.pool, uid, "dismissed.jpg", "2024-03-05T10:04:00Z").await;
    set(&t.pool, hidden, "hidden = true").await;
    set(&t.pool, archived, "archived = true").await;
    set(&t.pool, trashed, "deleted_at = now()").await;
    // A burst of three counts once, and its favorite is the one shown.
    let b1 = photo_at(&t.pool, uid, "b1.jpg", "2024-03-05T12:00:00Z").await;
    let b2 = photo_at(&t.pool, uid, "b2.jpg", "2024-03-05T12:00:01Z").await;
    let b3 = photo_at(&t.pool, uid, "b3.jpg", "2024-03-05T12:00:02Z").await;
    for b in [b1, b2, b3] {
        set(&t.pool, b, &format!("burst_id = '{b1}'")).await;
    }
    set(&t.pool, b2, "favorite = true").await;
    // A RAW + JPEG pair is one photo.
    let jpg = photo_at(&t.pool, uid, "pair.jpg", "2024-03-05T14:00:00Z").await;
    let raw = photo_at(&t.pool, uid, "pair.cr3", "2024-03-05T14:00:00Z").await;
    set(&t.pool, raw, &format!("primary_id = '{jpg}'")).await;
    // An edited date moves a photo to (or away from) the day, in the zone the edit chose.
    let moved_in = photo_at(&t.pool, uid, "moved_in.jpg", "2024-07-01T12:00:00Z").await;
    set(
        &t.pool,
        moved_in,
        "taken_at = '2020-03-05T22:30:00Z', date_edited = true, taken_offset_edit = -18000",
    )
    .await;
    let moved_out = photo_at(&t.pool, uid, "moved_out.jpg", "2019-03-05T12:00:00Z").await;
    set(
        &t.pool,
        moved_out,
        "taken_at = '2019-03-05T23:30:00Z', date_edited = true, taken_offset_edit = 7200",
    )
    .await;

    let (_, m) = get(&app, &c, "/assets/memories?date=2026-03-05&all=true").await;
    let y2 = &m["years"]
        .as_array()
        .unwrap()
        .iter()
        .find(|y| y["year"] == 2024)
        .unwrap()
        .clone();
    assert_eq!(
        y2["count"], 4,
        "ok, the burst once, the pair once, and nothing hidden, archived, trashed or dismissed"
    );
    let shown: Vec<Uuid> = item_ids(y2);
    assert!(
        shown.contains(&ok) && shown.contains(&b2) && shown.contains(&jpg),
        "{shown:?}"
    );
    assert!(!shown.contains(&b1) && !shown.contains(&b3) && !shown.contains(&raw));
    assert!(![hidden, archived, trashed].iter().any(|x| shown.contains(x)));
    // Edited: 22:30 UTC read at UTC-5 is 17:30 on the 5th; 23:30 UTC read at UTC+2 is 01:30 on the 6th.
    assert_eq!(
        years(&m).iter().filter(|y| y.2 == ["moved_in.jpg"]).count(),
        1,
        "{:?}",
        years(&m)
    );
    assert!(!years(&m).iter().any(|y| y.2.contains(&"moved_out.jpg".to_string())));

    // "Not this one": the photo goes, and so does the rest of its burst; taking it back restores them.
    let (_, r) = post(&app, &c, "/assets/memories/dismiss", json!({"ids": [dismissed, b1]})).await;
    assert_eq!(r["count"], 4, "the photo, and the three of the burst");
    let (_, m) = get(&app, &c, "/assets/memories?date=2026-03-05&all=true").await;
    let y2 = m["years"]
        .as_array()
        .unwrap()
        .iter()
        .find(|y| y["year"] == 2024)
        .unwrap()
        .clone();
    assert_eq!(y2["count"], 2, "what is left: the photo and the pair");
    let (_, r) = post(
        &app,
        &c,
        "/assets/memories/dismiss",
        json!({"ids": [b3], "dismissed": false}),
    )
    .await;
    assert_eq!(r["count"], 3);
    let (_, m) = get(&app, &c, "/assets/memories?date=2026-03-05&all=true").await;
    assert_eq!(
        m["years"]
            .as_array()
            .unwrap()
            .iter()
            .find(|y| y["year"] == 2024)
            .unwrap()["count"],
        3
    );
    // Somebody else's id is nothing.
    let (other, _) = user(&t.pool, "b@example.com").await;
    let theirs = photo_at(&t.pool, other, "theirs.jpg", "2024-03-05T10:00:00Z").await;
    let (_, r) = post(&app, &c, "/assets/memories/dismiss", json!({"ids": [theirs]})).await;
    assert_eq!(r["count"], 0);
    t.teardown().await;
}

#[tokio::test]
async fn on_this_day_can_be_switched_off_and_shows_nothing_when_there_is_nothing() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, c) = session(&t.pool, "a@example.com", "member").await;
    let app = app()(&t.pool);
    // Empty library, and a library with nothing on this day: no years at all (the app then draws no strip).
    let (_, m) = get(&app, &c, "/assets/memories?date=2026-10-19").await;
    assert_eq!(
        (m["years"].as_array().unwrap().len(), m["enabled"].clone()),
        (0, json!(true))
    );
    photo_at(&t.pool, uid, "a.jpg", "2024-06-01T00:00:00Z").await;
    let (_, m) = get(&app, &c, "/assets/memories?date=2026-10-19").await;
    assert!(m["years"].as_array().unwrap().is_empty());
    // On its day it has one; switched off, the strip is empty but the page (all) still works.
    let (_, m) = get(&app, &c, "/assets/memories?date=2026-06-01").await;
    assert_eq!(years(&m), [(2, 1, vec!["a.jpg".to_string()])]);
    send_cookie(
        &app,
        "PUT",
        "/assets/prefs",
        Some(&c),
        Some(json!({"memories_enabled": false})),
    )
    .await;
    let (_, m) = get(&app, &c, "/assets/memories?date=2026-06-01").await;
    assert_eq!(
        (m["years"].as_array().unwrap().len(), m["enabled"].clone()),
        (0, json!(false))
    );
    let (_, page) = get(&app, &c, "/assets/memories?date=2026-06-01&all=true").await;
    assert_eq!(years(&page).len(), 1);
    // Bad dates are refused; 29 February only looks at leap years.
    let (st, _) = get(&app, &c, "/assets/memories?date=2026-02-30").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    photo_at(&t.pool, uid, "leap.jpg", "2024-02-29T10:00:00Z").await;
    photo_at(&t.pool, uid, "feb28.jpg", "2023-02-28T10:00:00Z").await;
    let (_, m) = get(&app, &c, "/assets/memories?date=2028-02-29&all=true").await;
    assert_eq!(years(&m), [(4, 1, vec!["leap.jpg".to_string()])]);
    t.teardown().await;
}

#[tokio::test]
async fn memories_belong_to_their_owner() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = session(&t.pool, "a@example.com", "member").await;
    let (_, bob) = session(&t.pool, "b@example.com", "member").await;
    let app = app()(&t.pool);
    photo_at(&t.pool, alice, "a.jpg", "2024-06-01T00:00:00Z").await;
    let (_, m) = get(&app, &bob, "/assets/memories?date=2026-06-01&all=true").await;
    assert!(m["years"].as_array().unwrap().is_empty());
    let (st, _) = send_cookie(&app, "GET", "/assets/memories", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

#[tokio::test]
async fn a_hidden_photo_is_rotated_cropped_and_downloaded_edited_only_from_an_unlocked_session() {
    let Some(t) = TestDb::new().await else { return };
    let l = library(&t).await;
    sqlx::query("UPDATE assets SET width = 400, height = 300 WHERE id = $1")
        .bind(l.hidden)
        .execute(&t.pool)
        .await
        .unwrap();
    let recipe = || async {
        sqlx::query_scalar::<_, Option<Value>>("SELECT recipe FROM assets WHERE id = $1")
            .bind(l.hidden)
            .fetch_one(&t.pool)
            .await
            .unwrap()
    };
    // Locked: by id the request quietly finds nothing, by scope the Hidden view is refused, and nothing is written.
    let (st, r) = post(
        &l.app,
        &l.cookie,
        "/assets/edits",
        json!({"ids": [l.hidden], "recipe": {"turn": 90}}),
    )
    .await;
    assert_eq!(
        (st, r["updated"].as_i64(), r["missing"].as_i64()),
        (StatusCode::OK, Some(0), Some(1)),
        "{r}"
    );
    let (st, _) = post(
        &l.app,
        &l.cookie,
        "/assets/edits",
        json!({"scope": "hidden", "recipe": {"turn": 90}}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(recipe().await, None);

    // The edited download and the details of a hidden photo: 404 while locked, whatever it has.
    sqlx::query(
        "UPDATE assets SET recipe = '{\"rotate\":90}', edited_copy_at = now(), edited_copy_bytes = 1 WHERE id = $1",
    )
    .bind(l.hidden)
    .execute(&t.pool)
    .await
    .unwrap();
    for p in [
        format!("/assets/{}/download/edited", l.hidden),
        format!("/assets/{}/details", l.hidden),
    ] {
        let (st, _) = get(&l.app, &l.cookie, &p).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{p}");
    }
    // The recipe flag is not in any list a locked session can read, and the visible photos are unaffected.
    let (_, list) = get(&l.app, &l.cookie, "/assets").await;
    assert!(
        item_ids(&list).iter().all(|id| *id != l.hidden),
        "the hidden photo is not listed"
    );
    sqlx::query("UPDATE assets SET recipe = NULL, edited_copy_at = NULL, edited_copy_bytes = NULL WHERE id = $1")
        .bind(l.hidden)
        .execute(&t.pool)
        .await
        .unwrap();

    // Unlocked: it can be rotated, and its edited copy is asked for and served.
    unlock(&t.pool, &l.cookie, 10).await;
    if let Some((app, _)) = crate::testutil::app_with_s3(&t.pool) {
        let (st, r) = post(
            &app,
            &l.cookie,
            "/assets/edits",
            json!({"ids": [l.hidden], "recipe": {"turn": 90}}),
        )
        .await;
        assert_eq!((st, r["updated"].as_i64()), (StatusCode::OK, Some(1)), "{r}");
        assert_eq!(recipe().await, Some(json!({"rotate": 90})));
        let (st, v) = get(&app, &l.cookie, &format!("/assets/{}/download/edited", l.hidden)).await;
        assert_eq!((st, v["pending"].clone()), (StatusCode::ACCEPTED, json!(true)));
        // Locking again hides it again.
        sqlx::query("UPDATE sessions SET hidden_unlocked_until = now() - interval '1 second'")
            .execute(&t.pool)
            .await
            .unwrap();
        let (st, _) = get(&app, &l.cookie, &format!("/assets/{}/download/edited", l.hidden)).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    t.teardown().await;
}
