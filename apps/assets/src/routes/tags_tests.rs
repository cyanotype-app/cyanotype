//! Tags over a real Postgres: the tag endpoints (make, list, rename, merge, delete, bulk apply), who owns what, the search
//! filter (all / any), text search matching tag words, pairs, and the audit trail. The hidden-photo rules are also in
//! `visibility_tests` (the every-endpoint test).
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::search_v2_tests::photo;
use crate::testutil::{TEST_MODEL, TestDb, app, asset, send, user};

async fn post(app: &axum::Router, key: &str, path: &str, body: Value) -> (StatusCode, Value) {
    send(app, "POST", path, key, Some(body)).await
}

async fn get(app: &axum::Router, key: &str, path: &str) -> (StatusCode, Value) {
    send(app, "GET", path, key, None).await
}

fn ids_of(v: &Value) -> Vec<Uuid> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().parse().unwrap())
        .collect()
}

/// `(name, count)` of the tag list, in the order served.
fn listed(v: &Value) -> Vec<(String, i64)> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t["count"].as_i64().unwrap()))
        .collect()
}

async fn tag_names(pool: &sqlx::PgPool, asset: Uuid) -> Vec<(String, bool)> {
    sqlx::query_as(
        "SELECT t.name, at.imported FROM asset_tags at JOIN tags t ON t.id = at.tag_id WHERE at.asset_id = $1
         ORDER BY lower(t.name)",
    )
    .bind(asset)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn tag_id(pool: &sqlx::PgPool, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM tags WHERE owner_id = $1 AND lower(name) = lower($2)")
        .bind(owner)
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn tags_are_made_once_whatever_the_case_and_names_are_checked() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (_bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);

    let (st, a) = post(&app, &ka, "/assets/tags", json!({"name": "  Summer   2024 "})).await;
    assert_eq!(
        (st, a["name"].as_str(), a["created"].as_bool()),
        (StatusCode::CREATED, Some("Summer 2024"), Some(true))
    );
    let (st, again) = post(&app, &ka, "/assets/tags", json!({"name": "summer 2024"})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        (again["id"].clone(), again["name"].as_str(), again["created"].as_bool()),
        (a["id"].clone(), Some("Summer 2024"), Some(false))
    );
    // A slash is a character like any other (no hierarchy yet); another user may use the same name.
    assert_eq!(
        post(&app, &ka, "/assets/tags", json!({"name": "trip/2024"})).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        post(&app, &kb, "/assets/tags", json!({"name": "Summer 2024"})).await.0,
        StatusCode::CREATED
    );
    for bad in ["", "   ", "a,b", "x\u{7}y", &"z".repeat(101)] {
        let (st, _) = post(&app, &ka, "/assets/tags", json!({"name": bad})).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM tags WHERE owner_id = $1")
        .bind(alice)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
    t.teardown().await;
}

#[tokio::test]
async fn bulk_apply_adds_and_removes_by_ids_and_scope_and_counts_what_changed() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    let (a, b, c) = (
        asset(&t.pool, alice, "a.jpg").await,
        asset(&t.pool, alice, "b.jpg").await,
        asset(&t.pool, alice, "c.jpg").await,
    );
    let theirs = asset(&t.pool, bob, "theirs.jpg").await;

    // Two tags on two photos, and a photo of Bob's and an unknown id that are left alone.
    let (st, r) = post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [a, b, theirs, Uuid::new_v4()], "add": ["Beach", "beach", "Sunset"]}),
    )
    .await;
    assert_eq!(
        (st, r["added"].as_i64(), r["removed"].as_i64()),
        (StatusCode::OK, Some(4), Some(0)),
        "{r}"
    );
    assert_eq!(
        tag_names(&t.pool, a).await,
        [("Beach".to_string(), false), ("Sunset".to_string(), false)]
    );
    assert!(tag_names(&t.pool, theirs).await.is_empty(), "not Alice's to tag");
    // Repeating changes nothing; adding to one more photo counts one.
    let (_, r) = post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [a, b], "add": ["Beach"]}),
    )
    .await;
    assert_eq!(r["added"], 0);
    let (_, r) = post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [a, b, c], "add": ["BEACH"]}),
    )
    .await;
    assert_eq!(r["added"], 1);
    // Remove: unknown names are ignored, other users' tags are not reachable by name.
    post(
        &app,
        &kb,
        "/assets/tags/apply",
        json!({"ids": [theirs], "add": ["Sunset"]}),
    )
    .await;
    let (_, r) = post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [a, b, c], "remove": ["sunset", "nope"]}),
    )
    .await;
    assert_eq!(r["removed"], 2);
    assert_eq!(
        tag_names(&t.pool, theirs).await,
        [("Sunset".to_string(), false)],
        "Bob's tag stays"
    );
    // A whole view, except one photo; add and remove in one request.
    let (_, r) = post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"scope": "library", "except": [c], "add": ["Trip"], "remove": ["Beach"]}),
    )
    .await;
    assert_eq!((r["added"].as_i64(), r["removed"].as_i64()), (Some(2), Some(2)), "{r}");
    assert_eq!(tag_names(&t.pool, c).await, [("Beach".to_string(), false)]);

    // Validation.
    for body in [
        json!({"ids": [a]}),
        json!({"ids": [a], "add": []}),
        json!({"ids": [a], "add": ["a,b"]}),
        json!({"ids": [a], "add": (0..21).map(|i| format!("t{i}")).collect::<Vec<_>>()}),
    ] {
        let (st, _) = post(&app, &ka, "/assets/tags/apply", body).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
    t.teardown().await;
}

#[tokio::test]
async fn an_imported_tag_becomes_the_owners_when_the_owner_adds_it_and_a_raw_id_means_its_primary() {
    use cyanotype_core::{pairing::pair_asset, tags};
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let jpg = asset(&t.pool, alice, "p.jpg").await;
    let raw = asset(&t.pool, alice, "p.dng").await;
    for id in [jpg, raw] {
        sqlx::query("UPDATE assets SET taken_at = '2024-05-01T12:00:00Z', width = 10, height = 10 WHERE id = $1")
            .bind(id)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    // The RAW was tagged on its own (one imported, one by the owner) before it was paired, the JPEG has the imported one.
    let mut tx = t.pool.begin().await.unwrap();
    let (imported, mine) = (
        tags::ensure(&mut tx, alice, "From file").await.unwrap(),
        tags::ensure(&mut tx, alice, "Mine").await.unwrap(),
    );
    tags::attach(&mut tx, raw, imported, true).await.unwrap();
    tags::attach(&mut tx, raw, mine, false).await.unwrap();
    tags::attach(&mut tx, jpg, imported, true).await.unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        pair_asset(&t.pool, raw).await.unwrap(),
        cyanotype_core::pairing::Paired::Linked { .. }
    ));
    assert!(
        tag_names(&t.pool, raw).await.is_empty(),
        "the RAW's tags moved to the visible photo"
    );
    assert_eq!(
        tag_names(&t.pool, jpg).await,
        [("From file".to_string(), true), ("Mine".to_string(), false)]
    );
    // The owner adds the imported tag by the RAW's id: it is the owner's now.
    let (_, r) = post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [raw], "add": ["from file"]}),
    )
    .await;
    assert_eq!(r["added"], 1);
    assert_eq!(
        tag_names(&t.pool, jpg).await,
        [("From file".to_string(), false), ("Mine".to_string(), false)]
    );
    // And the other way round never demotes an owner's tag to imported.
    let mut tx = t.pool.begin().await.unwrap();
    tags::attach(&mut tx, jpg, imported, true).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(tag_names(&t.pool, jpg).await[0], ("From file".to_string(), false));
    t.teardown().await;
}

#[tokio::test]
async fn the_tag_list_has_counts_for_visible_photos_sorts_and_finds_by_text() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    let photos: Vec<Uuid> = {
        let mut v = Vec::new();
        for n in 0..4 {
            v.push(asset(&t.pool, alice, &format!("{n}.jpg")).await);
        }
        v
    };
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": photos, "add": ["Common"]}),
    )
    .await;
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [photos[0], photos[1]], "add": ["Pair"]}),
    )
    .await;
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [photos[0]], "add": ["alone", "Under_score 100%"]}),
    )
    .await;
    post(&app, &ka, "/assets/tags", json!({"name": "Empty"})).await;
    let b = asset(&t.pool, bob, "b.jpg").await;
    post(&app, &kb, "/assets/tags/apply", json!({"ids": [b], "add": ["Bobs"]})).await;

    let (_, l) = get(&app, &ka, "/assets/tags").await;
    assert_eq!(
        listed(&l),
        [
            ("alone".into(), 1),
            ("Common".into(), 4),
            ("Empty".into(), 0),
            ("Pair".into(), 2),
            ("Under_score 100%".into(), 1)
        ],
        "alphabetical, ignoring case, only Alice's"
    );
    let (_, l) = get(&app, &ka, "/assets/tags?sort=count").await;
    assert_eq!(listed(&l)[0], ("Common".into(), 4));
    assert_eq!(listed(&l)[1], ("Pair".into(), 2));
    // Autocomplete: part of a name, any case; LIKE's own characters mean themselves.
    let (_, l) = get(&app, &ka, "/assets/tags?q=AI&limit=5").await;
    assert_eq!(listed(&l), [("Pair".into(), 2)]);
    let (_, l) = get(&app, &ka, "/assets/tags?q=_").await;
    assert_eq!(listed(&l), [("Under_score 100%".into(), 1)]);
    let (_, l) = get(&app, &ka, "/assets/tags?q=%25").await;
    assert_eq!(listed(&l), [("Under_score 100%".into(), 1)]);
    assert_eq!(
        get(&app, &ka, "/assets/tags?sort=size").await.0,
        StatusCode::BAD_REQUEST
    );
    // A trashed photo does not count (and its tag is still there).
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(photos[3])
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, l) = get(&app, &ka, "/assets/tags").await;
    assert!(listed(&l).contains(&("Common".into(), 3)));
    t.teardown().await;
}

#[tokio::test]
async fn rename_merge_and_delete_keep_to_the_owner_and_are_audited_with_counts_only() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (_bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    let (a, b, c) = (
        asset(&t.pool, alice, "a.jpg").await,
        asset(&t.pool, alice, "b.jpg").await,
        asset(&t.pool, alice, "c.jpg").await,
    );
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [a, b], "add": ["Holiday"]}),
    )
    .await;
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [b, c], "add": ["Vacation"]}),
    )
    .await;
    let (holiday, vacation) = (
        tag_id(&t.pool, alice, "holiday").await,
        tag_id(&t.pool, alice, "vacation").await,
    );

    // Rename: a case-only change is fine, another tag's name is a conflict that names it, nonsense is refused.
    let (st, r) = send(
        &app,
        "PATCH",
        &format!("/assets/tags/{holiday}"),
        &ka,
        Some(json!({"name": "HOLIDAY"})),
    )
    .await;
    assert_eq!((st, r["name"].as_str()), (StatusCode::OK, Some("HOLIDAY")));
    let (st, r) = send(
        &app,
        "PATCH",
        &format!("/assets/tags/{holiday}"),
        &ka,
        Some(json!({"name": "vacation"})),
    )
    .await;
    assert_eq!(
        (st, r["code"].as_str(), r["existing_id"].clone()),
        (StatusCode::CONFLICT, Some("tag_exists"), json!(vacation))
    );
    let (st, _) = send(
        &app,
        "PATCH",
        &format!("/assets/tags/{holiday}"),
        &ka,
        Some(json!({"name": "a,b"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = send(
        &app,
        "PATCH",
        &format!("/assets/tags/{holiday}"),
        &kb,
        Some(json!({"name": "Mine now"})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "another user's tag");

    // Merge: Holiday (a, b) into Vacation (b, c): all three have Vacation, Holiday is gone; Bob cannot, nor into his own.
    let (st, _) = post(
        &app,
        &kb,
        &format!("/assets/tags/{holiday}/merge"),
        json!({"into": vacation}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = post(
        &app,
        &ka,
        &format!("/assets/tags/{holiday}/merge"),
        json!({"into": holiday}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = post(
        &app,
        &ka,
        &format!("/assets/tags/{holiday}/merge"),
        json!({"into": Uuid::new_v4()}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, r) = post(
        &app,
        &ka,
        &format!("/assets/tags/{holiday}/merge"),
        json!({"into": vacation}),
    )
    .await;
    assert_eq!((st, r["photos"].as_i64()), (StatusCode::OK, Some(2)), "{r}");
    for id in [a, b, c] {
        assert_eq!(tag_names(&t.pool, id).await, [("Vacation".to_string(), false)]);
    }
    assert_eq!(
        get(&app, &ka, "/assets/search?tags=holiday&tag_mode=any").await.1["total"],
        0
    );

    // Delete: the tag leaves its photos, the photos stay.
    assert_eq!(
        send(&app, "DELETE", &format!("/assets/tags/{vacation}"), &kb, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "DELETE", &format!("/assets/tags/{vacation}"), &ka, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert!(tag_names(&t.pool, a).await.is_empty());
    let photos: i64 = sqlx::query_scalar("SELECT count(*) FROM assets WHERE owner_id = $1")
        .bind(alice)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(photos, 3);

    // Audit: counts and nothing else (no tag name, no photo).
    let rows: Vec<(String, Value)> =
        sqlx::query_as("SELECT action, detail FROM audit_log WHERE action LIKE 'tags.%' ORDER BY id")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert_eq!(rows[0], ("tags.merge".to_string(), json!({"photos": 2})));
    assert_eq!(rows[1], ("tags.delete".to_string(), json!({"photos": 3})));
    t.teardown().await;
}

#[tokio::test]
async fn the_tag_filter_matches_all_or_any_ignoring_case_and_combines_with_other_filters() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app(&t.pool);
    let mut p = Vec::new();
    for n in 0..4 {
        p.push(asset(&t.pool, alice, &format!("{n}.jpg")).await);
    }
    // p0: beach sunset; p1: beach; p2: sunset (favorite); p3: nothing.
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [p[0], p[1]], "add": ["Beach"]}),
    )
    .await;
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [p[0], p[2]], "add": ["Sunset"]}),
    )
    .await;
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [p[0]], "add": ["trip/2024"]}),
    )
    .await;
    sqlx::query("UPDATE assets SET favorite = true WHERE id = $1")
        .bind(p[2])
        .execute(&t.pool)
        .await
        .unwrap();
    let b = asset(&t.pool, bob, "b.jpg").await;
    post(&app, &kb, "/assets/tags/apply", json!({"ids": [b], "add": ["Beach"]})).await;

    let search = |q: &'static str| {
        let (app, ka) = (app.clone(), ka.clone());
        async move {
            let (st, v) = get(&app, &ka, &format!("/assets/search?{q}")).await;
            assert_eq!(st, StatusCode::OK, "{q}: {v}");
            v
        }
    };
    let sorted = |v: &Value| {
        let mut x = ids_of(v);
        x.sort();
        x
    };
    let want = |ids: &[Uuid]| {
        let mut x = ids.to_vec();
        x.sort();
        x
    };
    assert_eq!(sorted(&search("tags=beach").await), want(&[p[0], p[1]]));
    assert_eq!(
        sorted(&search("tags=BEACH,sunset").await),
        want(&[p[0]]),
        "all is the default"
    );
    assert_eq!(sorted(&search("tags=beach,sunset&tag_mode=all").await), want(&[p[0]]));
    assert_eq!(
        sorted(&search("tags=beach,sunset&tag_mode=any").await),
        want(&[p[0], p[1], p[2]])
    );
    assert_eq!(search("tags=beach,sunset&tag_mode=any").await["total"], 3);
    assert_eq!(
        sorted(&search("tags=trip/2024").await),
        want(&[p[0]]),
        "a slash is part of the name"
    );
    // Unknown tags: nothing in `all`, ignored in `any`; another user's tag names mean nothing to Alice's photos.
    assert!(ids_of(&search("tags=beach,nope").await).is_empty());
    assert_eq!(
        sorted(&search("tags=beach,nope&tag_mode=any").await),
        want(&[p[0], p[1]])
    );
    assert!(!ids_of(&search("tags=beach").await).contains(&b));
    // With other filters, and as a scope ("select all" over a tag filter).
    assert_eq!(sorted(&search("tags=sunset&favorite=true").await), want(&[p[2]]));
    let (_, r) = post(
        &app,
        &ka,
        "/assets/favorite",
        json!({"scope": {"search": {"tags": "beach,sunset", "tag_mode": "any"}}, "favorite": true}),
    )
    .await;
    assert_eq!(r["count"], 2, "p0 and p1 were not favorites yet; p2 was");
    // Bad input.
    for bad in ["tag_mode=some&tags=a", "tags=a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q,r,s,t,u"] {
        let (st, _) = get(&app, &ka, &format!("/assets/search?{bad}")).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    }
    // The photo itself lists its tags.
    let (_, one) = get(&app, &ka, &format!("/assets/{}", p[0])).await;
    let names: Vec<&str> = one["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Beach", "Sunset", "trip/2024"]);
    assert_eq!(one["tags"][0]["imported"], false);
    t.teardown().await;
}

#[tokio::test]
async fn text_search_matches_tag_words_in_front_of_the_models_ranking() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    // The model would put the dog first; the photo tagged "Birthday party" is found by the words of its tag.
    let dog = photo(&t.pool, alice, "dog.jpg", TEST_MODEL, &[("dog", 1.0)]).await;
    let tagged = photo(&t.pool, alice, "tagged.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    let _plain = photo(&t.pool, alice, "plain.jpg", TEST_MODEL, &[("car", 1.0)]).await;
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [tagged], "add": ["Birthday party"]}),
    )
    .await;

    let (st, r) = get(&app, &ka, "/assets/search?q=birthday&cutoff=false").await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(ids_of(&r)[0], tagged, "a tag word is as good a match as a caption word");
    // Every word must be in the caption or in a tag (not the same place): a caption and a tag together answer one query.
    sqlx::query("UPDATE assets SET caption = 'cake on the table' WHERE id = $1")
        .bind(tagged)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, r) = get(&app, &ka, "/assets/search?q=birthday%20cake&cutoff=false").await;
    assert_eq!(ids_of(&r)[0], tagged);
    // A word nobody wrote anywhere leads nothing.
    let (_, r) = get(&app, &ka, "/assets/search?q=dog&cutoff=false").await;
    assert_eq!(ids_of(&r)[0], dog);
    // Another user's tag words do not lead Alice's results.
    let (bob, kb) = user(&t.pool, "b@example.com").await;
    let b = photo(&t.pool, bob, "b.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    post(
        &app,
        &kb,
        "/assets/tags/apply",
        json!({"ids": [b], "add": ["zebra crossing"]}),
    )
    .await;
    let (_, r) = get(&app, &ka, "/assets/search?q=zebra&cutoff=false").await;
    assert!(!ids_of(&r).contains(&b));
    t.teardown().await;
}

/// The filters the new indexes serve: with sequential scans off (the planner then shows which index a plan *can* use), the rating
/// filter walks `assets_rating_idx` and a tag filter finds its tag by `tags_owner_name_idx` and the photos through the index of
/// `asset_tags` (the tag's own index once the table is big enough to matter: `docs/library.md` has the measured plans at 50,000 photos).
#[tokio::test]
async fn the_rating_and_tag_filters_have_their_indexes() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, _) = user(&t.pool, "a@example.com").await;
    sqlx::query(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, taken_at, rating, rating_source)
         SELECT $1, sha256((i::text)::bytea), 'k' || i, 'n' || i, 'image/jpeg', 1, now() - make_interval(secs => i),
                CASE WHEN i % 7 = 0 THEN 1 + i % 5 ELSE 0 END, CASE WHEN i % 7 = 0 THEN 'user' END
         FROM generate_series(1, 3000) i",
    )
    .bind(alice)
    .execute(&t.pool)
    .await
    .unwrap();
    let tag: Uuid = sqlx::query_scalar("INSERT INTO tags (owner_id, name) VALUES ($1, 'rare') RETURNING id")
        .bind(alice)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO asset_tags SELECT id, $1 FROM assets WHERE original_name IN ('n5', 'n50', 'n500')")
        .bind(tag)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("ANALYZE assets").execute(&t.pool).await.unwrap();
    sqlx::query("ANALYZE asset_tags").execute(&t.pool).await.unwrap();
    for (what, index, filter) in [
        ("rating", "assets_rating_idx", "a.rating >= 4".to_string()),
        (
            "tag",
            "tags_owner_name_idx",
            format!(
                "a.id IN (SELECT at.asset_id FROM asset_tags at JOIN tags t ON t.id = at.tag_id WHERE t.owner_id = '{alice}' AND lower(t.name) = 'rare')"
            ),
        ),
    ] {
        let mut tx = t.pool.begin().await.unwrap();
        sqlx::query("SET LOCAL enable_seqscan = off")
            .execute(&mut *tx)
            .await
            .unwrap();
        let plan: Vec<(String,)> = sqlx::query_as(&format!(
            "EXPLAIN SELECT a.id FROM assets a WHERE a.owner_id = '{alice}' AND a.deleted_at IS NULL AND a.primary_id IS NULL
               AND NOT a.hidden AND {filter}
             ORDER BY coalesce(a.taken_at, a.file_modified_at, a.created_at) DESC, a.id DESC LIMIT 61"
        ))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        let plan = plan.into_iter().map(|r| r.0).collect::<Vec<_>>().join("\n");
        assert!(plan.contains(index), "the {what} filter does not use {index}:\n{plan}");
        assert!(!plan.contains("Seq Scan"), "the {what} filter scans a table:\n{plan}");
    }
    t.teardown().await;
}

/// Tag words, captions and the text read off a photo (OCR) all lead a text search, in that order: the owner's own words (a caption,
/// a tag) first, then what the picture says; and the has-text filter combines with the tag filter.
#[tokio::test]
async fn tag_words_and_text_read_off_photos_lead_a_search_together() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let _other = photo(&t.pool, alice, "other.jpg", TEST_MODEL, &[("dog", 1.0)]).await;
    let sign = photo(&t.pool, alice, "sign.jpg", TEST_MODEL, &[("car", 1.0)]).await;
    let tagged = photo(&t.pool, alice, "tagged.jpg", TEST_MODEL, &[("cat", 1.0)]).await;
    sqlx::query("INSERT INTO asset_text (asset_id, model, version, text) VALUES ($1, 'm', 1, 'Welcome to the birthday banquet')")
        .bind(sign)
        .execute(&t.pool)
        .await
        .unwrap();
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [tagged], "add": ["Birthday party"]}),
    )
    .await;
    let (st, r) = get(&app, &ka, "/assets/search?q=birthday&cutoff=false").await;
    assert_eq!(st, StatusCode::OK, "{r}");
    let order = ids_of(&r);
    assert_eq!(
        &order[..2],
        [tagged, sign],
        "the tag first, then the text on the picture"
    );
    // The has-text filter and the tag filter narrow each other.
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [sign], "add": ["Signs"]}),
    )
    .await;
    let (_, r) = get(&app, &ka, "/assets/search?tags=signs&has_text=true").await;
    assert_eq!(ids_of(&r), [sign]);
    let (_, r) = get(&app, &ka, "/assets/search?tags=birthday%20party&has_text=true").await;
    assert!(ids_of(&r).is_empty());
    t.teardown().await;
}

/// An API key never has the Hidden view: it does not list a tag only hidden photos have, cannot tag a hidden photo by id and finds
/// nothing hidden through a tag filter.
#[tokio::test]
async fn an_api_key_never_reaches_a_hidden_photo_through_tags() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let app = app(&t.pool);
    let (shown, hidden) = (
        asset(&t.pool, alice, "s.jpg").await,
        asset(&t.pool, alice, "h.jpg").await,
    );
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [shown], "add": ["open"]}),
    )
    .await;
    post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [hidden], "add": ["secret", "open"]}),
    )
    .await;
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(hidden)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, l) = get(&app, &ka, "/assets/tags").await;
    assert_eq!(
        listed(&l),
        [("open".to_string(), 1)],
        "no secret, and open counts the visible photo only"
    );
    let (_, r) = post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"ids": [hidden], "add": ["more"]}),
    )
    .await;
    assert_eq!(r["added"], 0);
    let (st, _) = post(
        &app,
        &ka,
        "/assets/tags/apply",
        json!({"scope": "hidden", "add": ["more"]}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (_, r) = get(&app, &ka, "/assets/search?tags=secret").await;
    assert!(ids_of(&r).is_empty());
    let (_, r) = get(&app, &ka, "/assets/search?tags=open").await;
    assert_eq!(ids_of(&r), [shown]);
    t.teardown().await;
}
