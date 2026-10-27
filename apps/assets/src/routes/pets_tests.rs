//! Pets over a real Postgres: the switch (off, everything but the status is a 404 and the parameters are ignored), the summary with counts
//! and covers, the Pets view and its timeline, a photo's boxes, the `pets` search filter and facet, the words of a query, the `{"pets"}`
//! scope, the smart-album rule, the Hidden view and the trash, who owns what, and that no share link carries any of it. The job that
//! finds the animals is tested in the worker (`apps/worker/src/pets.rs`), the check in `cyanotype-clip`.
use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use cyanotype_core::pets::{self, MODEL, NewDetection, Scan, VERSION};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::{routes_with, share_public::ShareState};
use crate::{
    AppState,
    testutil::{
        TestDb, app, app_with_features, app_with_pets, asset, dummy_storage, send, send_cookie, session, set_embedding,
        user,
    },
};

/// A live photo with a preview, taken on `day` (`YYYY-MM-DD`, noon UTC).
async fn photo(pool: &PgPool, owner: Uuid, name: &str, day: &str) -> Uuid {
    let id = asset(pool, owner, name).await;
    sqlx::query("UPDATE assets SET thumbhash = '\\x01', taken_at = ($2 || 'T12:00:00Z')::timestamptz WHERE id = $1")
        .bind(id)
        .bind(day)
        .execute(pool)
        .await
        .unwrap();
    id
}

fn found(kind: &str, bbox: [f32; 4], score: f32) -> NewDetection {
    NewDetection {
        kind: kind.into(),
        bbox,
        score,
    }
}

/// What the worker stores for a photo it has looked at: the animals (`kind`, box, score) it found.
async fn scanned(pool: &PgPool, asset: Uuid, animals: &[NewDetection]) {
    let has = |k: &str| animals.iter().any(|d| d.kind == k);
    let scan = Scan {
        dog_share: if has("dog") { 0.9 } else { 0.01 },
        cat_share: if has("cat") { 0.9 } else { 0.01 },
        found: animals.to_vec(),
    };
    assert!(pets::store(pool, asset, MODEL, VERSION, &scan).await.unwrap());
}

async fn set(pool: &PgPool, id: Uuid, sql: &str) {
    sqlx::query(&format!("UPDATE assets SET {sql} WHERE id = $1"))
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

async fn get(app: &Router, key: &str, path: &str) -> (StatusCode, Value) {
    send(app, "GET", path, key, None).await
}

async fn post(app: &Router, key: &str, path: &str, body: Value) -> (StatusCode, Value) {
    send(app, "POST", path, key, Some(body)).await
}

fn item_ids(v: &Value) -> Vec<Uuid> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().parse().unwrap())
        .collect()
}

/// A box as served (f32 numbers) against the expected one.
fn assert_box(v: &Value, want: [f32; 4]) {
    let got: Vec<f32> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap() as f32)
        .collect();
    assert!(
        got.len() == 4 && got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6),
        "{v} != {want:?}"
    );
}

fn sorted(mut v: Vec<Uuid>) -> Vec<Uuid> {
    v.sort();
    v
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// What `POST /auth/hidden/unlock` does to the session row.
async fn unlock(pool: &PgPool, cookie: &str) {
    let token = cookie.split_once('=').unwrap().1;
    sqlx::query("UPDATE sessions SET hidden_unlocked_until = now() + interval '5 minutes' WHERE token_hash = $1")
        .bind(cyanotype_service::auth::sha256(token.as_bytes()))
        .execute(pool)
        .await
        .unwrap();
}

/// Alpha's library (a different library for Bravo is made where a test needs it):
/// `dog` (2024-05-01, one dog), `cat` (2024-05-02), `both` (2024-05-03, a dog and a cat), `nothing` (2024-05-04, looked at, no pets),
/// `unscanned` (2024-05-05, not looked at yet).
struct Scene {
    uid: Uuid,
    key: String,
    dog: Uuid,
    cat: Uuid,
    both: Uuid,
    nothing: Uuid,
    unscanned: Uuid,
}

async fn scene(pool: &PgPool) -> Scene {
    let (uid, key) = user(pool, "alpha@example.com").await;
    let dog = photo(pool, uid, "dog.jpg", "2024-05-01").await;
    let cat = photo(pool, uid, "cat.jpg", "2024-05-02").await;
    let both = photo(pool, uid, "both.jpg", "2024-05-03").await;
    let nothing = photo(pool, uid, "nothing.jpg", "2024-05-04").await;
    let unscanned = photo(pool, uid, "unscanned.jpg", "2024-05-05").await;
    scanned(pool, dog, &[found("dog", [0.1, 0.2, 0.3, 0.4], 0.8)]).await;
    scanned(pool, cat, &[found("cat", [0.2, 0.2, 0.4, 0.4], 0.6)]).await;
    scanned(
        pool,
        both,
        &[
            found("dog", [0.0, 0.1, 0.5, 0.8], 0.9),
            found("cat", [0.6, 0.5, 0.2, 0.3], 0.4),
        ],
    )
    .await;
    scanned(pool, nothing, &[]).await;
    Scene {
        uid,
        key,
        dog,
        cat,
        both,
        nothing,
        unscanned,
    }
}

// ---- the switch -----------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn while_pets_is_off_nothing_about_it_exists_and_its_parameters_do_nothing() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let off = app(&t.pool);
    let (st, v) = get(&off, &s.key, "/assets/pets/status").await;
    assert_eq!((st, v["enabled"].clone()), (StatusCode::OK, json!(false)));
    for path in [
        "/assets/pets".to_string(),
        "/assets/pets/photos".to_string(),
        "/assets/pets/timeline".to_string(),
        format!("/assets/{}/pets", s.dog),
    ] {
        assert_eq!(get(&off, &s.key, &path).await.0, StatusCode::NOT_FOUND, "{path}");
    }
    // The search filter is ignored (every photo is listed) and the words of a query stay words; the facets know nothing of it.
    let (_, all) = get(&off, &s.key, "/assets/search").await;
    let (st, v) = get(&off, &s.key, "/assets/search?pets=dog").await;
    assert_eq!((st, item_ids(&v).len()), (StatusCode::OK, item_ids(&all).len()));
    let (_, v) = get(&off, &s.key, "/assets/search?q=dog").await;
    assert!(
        v["parsed"]["chips"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["kind"] != "pets"),
        "{v}"
    );
    assert_eq!(v["parsed"]["text"], "dog");
    let (_, f) = get(&off, &s.key, "/assets/search/facets").await;
    assert!(f.get("pets").is_none(), "{f}");
    // A scope that names pets is refused, as is a rule, and nothing is trashed on the way.
    for scope in [json!({"pets": "dog"}), json!({"search": {"pets": "dog"}})] {
        let (st, _) = post(&off, &s.key, "/assets/trash", json!({"scope": scope, "except": []})).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{scope}");
    }
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL").await,
        0
    );
    let rule = json!({"rules": [{"field": "pets", "kind": "dog"}]});
    let (st, _) = post(&off, &s.key, "/assets/albums", json!({"title": "Dogs", "rules": rule})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = post(&off, &s.key, "/assets/albums/preview", json!({"rules": rule})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // The processing report says nothing about it either.
    let (_, p) = get(&off, &s.key, "/assets/processing").await;
    assert!(p.get("pets").is_none(), "{p}");
    // On, the status says so.
    let on = app_with_pets(&t.pool);
    let (_, v) = get(&on, &s.key, "/assets/pets/status").await;
    assert_eq!(v["enabled"], true);
    t.teardown().await;
}

// ---- the summary and the view ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_summary_counts_photos_per_kind_with_a_cover_and_how_far_the_scan_has_got() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let (st, v) = get(&app, &s.key, "/assets/pets").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // Three photos with a pet, two with a dog (one of them the dog and cat photo), two with a cat.
    assert_eq!(v["any"], 3);
    let kinds = v["kinds"].as_array().unwrap();
    assert_eq!(
        kinds
            .iter()
            .map(|k| (k["kind"].as_str().unwrap(), k["photos"].as_i64().unwrap()))
            .collect::<Vec<_>>(),
        vec![("dog", 2), ("cat", 2)]
    );
    // The scan has looked at four of the five photos.
    assert_eq!((v["scanned"].clone(), v["total"].clone()), (json!(4), json!(5)));
    // The cover is the clearest animal of the kind: the big, sure dog of the photo with both.
    let dog = &kinds[0]["cover"];
    assert_eq!(dog["asset_id"], s.both.to_string());
    assert_box(&dog["box"], [0.0, 0.1, 0.5, 0.8]);
    assert!(dog["preview_url"].as_str().unwrap().contains(&s.both.to_string()));
    assert_eq!(
        kinds[1]["cover"]["asset_id"],
        s.cat.to_string(),
        "the cat photo's cat is the clearer one"
    );
    // A kind nobody has is listed, with no cover.
    sqlx::query("DELETE FROM pet_detections WHERE kind = 'cat'")
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, v) = get(&app, &s.key, "/assets/pets").await;
    assert_eq!(
        (
            v["any"].clone(),
            v["kinds"][1]["photos"].clone(),
            v["kinds"][1]["cover"].clone()
        ),
        (json!(2), json!(0), Value::Null)
    );
    t.teardown().await;
}

#[tokio::test]
async fn the_pets_view_lists_a_kind_newest_first_with_paging_and_a_timeline_like_the_librarys() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let (st, v) = get(&app, &s.key, "/assets/pets/photos").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(item_ids(&v), vec![s.both, s.cat, s.dog], "any pet, newest first");
    assert!(
        v["items"][0]["thumb_url"].is_string(),
        "(the same items as the library's list)"
    );
    for (kind, want) in [
        ("dog", vec![s.both, s.dog]),
        ("cat", vec![s.both, s.cat]),
        ("any", vec![s.both, s.cat, s.dog]),
    ] {
        let (_, v) = get(&app, &s.key, &format!("/assets/pets/photos?kind={kind}")).await;
        assert_eq!(item_ids(&v), want, "{kind}");
        let (_, days) = get(&app, &s.key, &format!("/assets/pets/timeline?kind={kind}")).await;
        assert_eq!(days["total"], want.len() as i64);
        assert_eq!(days["days"].as_array().unwrap().len(), want.len());
    }
    assert_eq!(
        get(&app, &s.key, "/assets/pets/timeline?kind=dog").await.1["days"][0],
        json!({"day": "2024-05-03", "count": 1})
    );
    // Paged by the library's keyset cursor.
    let (_, p1) = get(&app, &s.key, "/assets/pets/photos?limit=2").await;
    assert_eq!(item_ids(&p1), vec![s.both, s.cat]);
    let cursor = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = get(&app, &s.key, &format!("/assets/pets/photos?limit=2&cursor={cursor}")).await;
    assert_eq!((item_ids(&p2), p2["next_cursor"].clone()), (vec![s.dog], Value::Null));
    // A kind that is not one, a cursor that is not one.
    for path in [
        "/assets/pets/photos?kind=bird",
        "/assets/pets/photos?kind=dogs",
        "/assets/pets/timeline?kind=all",
        "/assets/pets/photos?cursor=nonsense",
    ] {
        assert_eq!(get(&app, &s.key, path).await.0, StatusCode::BAD_REQUEST, "{path}");
    }
    t.teardown().await;
}

#[tokio::test]
async fn photos_in_the_trash_hidden_ones_and_other_peoples_are_not_in_the_view_unless_the_view_is_unlocked() {
    let Some(t) = TestDb::new().await else { return };
    let app = app_with_pets(&t.pool);
    let (uid, cookie) = session(&t.pool, "alpha@example.com", "member").await;
    let (bravo, bravo_key) = user(&t.pool, "bravo@example.com").await;
    let shown = photo(&t.pool, uid, "shown.jpg", "2024-01-05").await;
    let archived = photo(&t.pool, uid, "archived.jpg", "2024-01-04").await;
    let hidden = photo(&t.pool, uid, "hidden.jpg", "2024-01-03").await;
    let trashed = photo(&t.pool, uid, "trashed.jpg", "2024-01-02").await;
    let raw_half = photo(&t.pool, uid, "raw.cr3", "2024-01-01").await;
    let theirs = photo(&t.pool, bravo, "bravo.jpg", "2024-01-06").await;
    for p in [shown, archived, hidden, trashed, raw_half, theirs] {
        scanned(&t.pool, p, &[found("dog", [0.1, 0.1, 0.5, 0.5], 0.7)]).await;
    }
    set(&t.pool, archived, "archived = true").await;
    set(&t.pool, hidden, "hidden = true").await;
    set(&t.pool, trashed, "deleted_at = now()").await;
    set(
        &t.pool,
        raw_half,
        "primary_id = (SELECT id FROM assets WHERE original_name = 'shown.jpg')",
    )
    .await;
    let g = |path: &str| {
        let (app, cookie, path) = (app.clone(), cookie.clone(), path.to_string());
        async move { send_cookie(&app, "GET", &path, Some(&cookie), None).await }
    };
    // Locked: live photos, archived ones too (the view keeps them, flagged); not the hidden, the trashed, the RAW half, or Bravo's.
    let (_, v) = g("/assets/pets/photos").await;
    assert_eq!(item_ids(&v), vec![shown, archived]);
    assert_eq!(v["items"][1]["archived"], true);
    assert_eq!(g("/assets/pets").await.1["any"], 2);
    assert_eq!(g("/assets/pets/timeline").await.1["total"], 2);
    assert_eq!(g(&format!("/assets/{hidden}/pets")).await.0, StatusCode::NOT_FOUND);
    assert_eq!(g(&format!("/assets/{trashed}/pets")).await.0, StatusCode::NOT_FOUND);
    assert_eq!(g(&format!("/assets/{theirs}/pets")).await.0, StatusCode::NOT_FOUND);
    // Unlocked: the hidden one too.
    unlock(&t.pool, &cookie).await;
    let (_, v) = g("/assets/pets/photos").await;
    assert_eq!(item_ids(&v), vec![shown, archived, hidden]);
    assert_eq!(g("/assets/pets").await.1["any"], 3);
    assert_eq!(g(&format!("/assets/{hidden}/pets")).await.0, StatusCode::OK);
    // Bravo sees his own photo only.
    let (_, v) = get(&app, &bravo_key, "/assets/pets/photos").await;
    assert_eq!(item_ids(&v), vec![theirs]);
    assert_eq!(get(&app, &bravo_key, "/assets/pets").await.1["any"], 1);
    t.teardown().await;
}

#[tokio::test]
async fn a_photos_pets_are_its_boxes_strongest_first_and_say_whether_it_was_looked_at() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (_, bravo_key) = user(&t.pool, "bravo@example.com").await;
    let app = app_with_pets(&t.pool);
    let (st, v) = get(&app, &s.key, &format!("/assets/{}/pets", s.both)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["scanned"], true);
    let items = v["items"].as_array().unwrap();
    assert_eq!(
        items.iter().map(|i| i["kind"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["dog", "cat"]
    );
    assert_box(&items[0]["box"], [0.0, 0.1, 0.5, 0.8]);
    assert!((items[1]["score"].as_f64().unwrap() - 0.4).abs() < 1e-6);
    assert!(items[0]["id"].as_str().unwrap().parse::<Uuid>().is_ok());
    // Looked at, nothing there; not looked at yet.
    let (_, v) = get(&app, &s.key, &format!("/assets/{}/pets", s.nothing)).await;
    assert_eq!((v["items"].clone(), v["scanned"].clone()), (json!([]), json!(true)));
    let (_, v) = get(&app, &s.key, &format!("/assets/{}/pets", s.unscanned)).await;
    assert_eq!((v["items"].clone(), v["scanned"].clone()), (json!([]), json!(false)));
    // Someone else's photo and one that does not exist are the same 404.
    for id in [s.both, Uuid::new_v4()] {
        assert_eq!(
            get(&app, &bravo_key, &format!("/assets/{id}/pets")).await.0,
            StatusCode::NOT_FOUND
        );
    }
    // The animals of an older model answer until the photo is looked at by the current one, and are replaced by it.
    let old = Scan {
        dog_share: 0.9,
        cat_share: 0.0,
        found: vec![found("dog", [0.3, 0.3, 0.3, 0.3], 0.5)],
    };
    assert!(
        pets::store(&t.pool, s.unscanned, "an-older-model", 1, &old)
            .await
            .unwrap()
    );
    let (_, v) = get(&app, &s.key, &format!("/assets/{}/pets", s.unscanned)).await;
    assert_eq!(
        (v["items"].as_array().unwrap().len(), v["scanned"].clone()),
        (1, json!(true))
    );
    scanned(&t.pool, s.unscanned, &[found("cat", [0.1, 0.1, 0.1, 0.1], 0.5)]).await;
    let (_, v) = get(&app, &s.key, &format!("/assets/{}/pets", s.unscanned)).await;
    assert_eq!(v["items"][0]["kind"], "cat");
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM pet_detections WHERE asset_id = '{}'", s.unscanned)
        )
        .await,
        1,
        "the new scan took the older model's rows with it"
    );
    t.teardown().await;
}

// ---- search ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_pets_filter_lists_photos_by_kind_and_combines_with_the_other_filters() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (_, bravo_key) = user(&t.pool, "bravo@example.com").await;
    let app = app_with_pets(&t.pool);
    let ids = |v: &Value| sorted(item_ids(v));
    for (value, want) in [
        ("any", vec![s.dog, s.cat, s.both]),
        ("dog", vec![s.dog, s.both]),
        ("cat", vec![s.cat, s.both]),
        ("DOG", vec![s.dog, s.both]),
    ] {
        let (st, v) = get(&app, &s.key, &format!("/assets/search?pets={value}")).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(ids(&v), sorted(want), "{value}");
        assert_eq!(v["ranked"], false);
        assert_eq!(v["total"], ids(&v).len() as i64);
    }
    assert_eq!(
        get(&app, &s.key, "/assets/search?pets=bird").await.0,
        StatusCode::BAD_REQUEST
    );
    // Together with a date range, a favorite, a place, a rating.
    let (_, v) = get(&app, &s.key, "/assets/search?pets=dog&from=2024-05-02&to=2024-05-31").await;
    assert_eq!(item_ids(&v), vec![s.both]);
    set(&t.pool, s.dog, "favorite = true, rating = 4, place_city = 'Lisbon'").await;
    for q in ["favorite=true", "rating=4", "place=Lisbon"] {
        let (_, v) = get(&app, &s.key, &format!("/assets/search?pets=dog&{q}")).await;
        assert_eq!(item_ids(&v), vec![s.dog], "{q}");
        let (_, v) = get(&app, &s.key, &format!("/assets/search?pets=cat&{q}")).await;
        assert!(item_ids(&v).is_empty(), "{q}");
    }
    // With text: ranked among the photos with a dog.
    set_embedding(&t.pool, s.dog, &[("beach", 1.0)]).await;
    set_embedding(&t.pool, s.cat, &[("beach", 1.0)]).await;
    set_embedding(&t.pool, s.both, &[("car", 1.0)]).await;
    set_embedding(&t.pool, s.nothing, &[("beach", 1.0)]).await;
    let (st, v) = get(&app, &s.key, "/assets/search?q=beach&pets=dog&cutoff=false").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["ranked"], true);
    assert_eq!(
        item_ids(&v),
        vec![s.dog, s.both],
        "the beach photo with a dog first; the cat and the empty beach are not dogs"
    );
    // Not the photos of the owner who is not asking, nor a trashed one, nor a hidden one.
    set(&t.pool, s.both, "deleted_at = now()").await;
    let (_, v) = get(&app, &s.key, "/assets/search?pets=dog").await;
    assert_eq!(item_ids(&v), vec![s.dog]);
    set(&t.pool, s.dog, "hidden = true").await;
    let (_, v) = get(&app, &s.key, "/assets/search?pets=any").await;
    assert_eq!(item_ids(&v), vec![s.cat], "a hidden photo is in no search");
    let (_, v) = get(&app, &bravo_key, "/assets/search?pets=any").await;
    assert!(item_ids(&v).is_empty());
    t.teardown().await;
}

#[tokio::test]
async fn the_pets_filter_and_the_people_filter_work_together() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_features(&t.pool, true, true);
    let person: Uuid = sqlx::query_scalar("INSERT INTO people (owner_id, name) VALUES ($1, 'Person A') RETURNING id")
        .bind(s.uid)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    // Person A is in the photos with the cat and with nothing in them.
    for p in [s.cat, s.nothing] {
        let nf = cyanotype_core::faces::NewFace {
            bbox: [0.1, 0.1, 0.1, 0.1],
            landmarks: [0.1; 10],
            score: 0.9,
            embedding: None,
        };
        assert!(
            cyanotype_core::faces::store(
                &t.pool,
                p,
                cyanotype_core::faces::MODEL,
                cyanotype_core::faces::VERSION,
                &[nf]
            )
            .await
            .unwrap()
        );
        sqlx::query("UPDATE faces SET person_id = $2 WHERE asset_id = $1")
            .bind(p)
            .bind(person)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    let (st, v) = get(&app, &s.key, &format!("/assets/search?people={person}&pets=any")).await;
    assert_eq!((st, item_ids(&v)), (StatusCode::OK, vec![s.cat]));
    let (_, v) = get(&app, &s.key, &format!("/assets/search?people={person}&pets=dog")).await;
    assert!(item_ids(&v).is_empty());
    // And in words: a name and a pet.
    let (_, v) = get(&app, &s.key, "/assets/search?q=Person%20A%20and%20cats").await;
    assert_eq!(item_ids(&v), vec![s.cat], "{v}");
    let kinds: Vec<&str> = v["parsed"]["chips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["people", "pets"]);
    t.teardown().await;
}

#[tokio::test]
async fn the_words_dog_and_cat_become_the_filter_and_a_chip_that_skip_takes_off() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let q = |text: &str| format!("/assets/search?q={}", text.replace(' ', "%20"));
    let (st, v) = get(&app, &s.key, &q("dogs")).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(sorted(item_ids(&v)), sorted(vec![s.dog, s.both]));
    assert_eq!(v["ranked"], false, "nothing is left to rank: a filter-only search");
    let chip = &v["parsed"]["chips"][0];
    assert_eq!(
        (
            chip["kind"].clone(),
            chip["label"].clone(),
            chip["pets"].clone(),
            chip["matched"].clone()
        ),
        (json!("pets"), json!("Dogs"), json!("dog"), json!("dogs"))
    );
    assert_eq!(v["parsed"]["text"], "");
    let (_, v) = get(&app, &s.key, &q("kitten")).await;
    assert_eq!(sorted(item_ids(&v)), sorted(vec![s.cat, s.both]));
    assert_eq!(v["parsed"]["chips"][0]["label"], "Cats");
    for text in ["my pets", "cats and dogs", "dog and cat", "photos with a dog or a cat"] {
        let (_, v) = get(&app, &s.key, &q(text)).await;
        let chips = v["parsed"]["chips"].as_array().unwrap();
        assert_eq!(chips[0]["pets"], "any", "{text}: {v}");
        assert_eq!(v["parsed"]["text"], "", "{text}");
        assert_eq!(sorted(item_ids(&v)), sorted(vec![s.dog, s.cat, s.both]), "{text}");
    }
    // With a date, and with the rest of the words searched among the photos with a dog.
    let (_, v) = get(&app, &s.key, &q("dog may 2024")).await;
    assert_eq!(sorted(item_ids(&v)), sorted(vec![s.dog, s.both]));
    let kinds: Vec<&str> = v["parsed"]["chips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["date", "pets"]);
    set_embedding(&t.pool, s.dog, &[("beach", 1.0)]).await;
    set_embedding(&t.pool, s.both, &[("car", 1.0)]).await;
    set_embedding(&t.pool, s.cat, &[("beach", 1.0)]).await;
    let (_, v) = get(&app, &s.key, &format!("{}&cutoff=false", q("dog beach"))).await;
    assert_eq!(
        (v["ranked"].clone(), v["parsed"]["text"].clone()),
        (json!(true), json!("beach"))
    );
    assert_eq!(item_ids(&v), vec![s.dog, s.both]);
    // A word that is part of something else stays a word.
    for text in ["hot dog", "dog food", "cat tree"] {
        let (_, v) = get(&app, &s.key, &q(text)).await;
        assert!(
            v["parsed"]["chips"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["kind"] != "pets"),
            "{text}: {v}"
        );
        assert_eq!(v["parsed"]["text"], text);
    }
    // `skip=pets` is what the chip's x sends: the words are searched like any others, over every photo.
    let (_, v) = get(&app, &s.key, &format!("{}&skip=pets&cutoff=false", q("dog"))).await;
    assert_eq!(
        (v["ranked"].clone(), v["parsed"]["text"].clone()),
        (json!(true), json!("dog"))
    );
    assert!(v["parsed"]["chips"].as_array().unwrap().is_empty());
    assert!(item_ids(&v).len() > 2, "{v}");
    // A filter chosen in the parameters wins over the words (which stay in the query).
    let (_, v) = get(&app, &s.key, &format!("{}&pets=cat&cutoff=false", q("dog"))).await;
    assert_eq!(v["parsed"]["text"], "dog");
    assert_eq!(sorted(item_ids(&v)), sorted(vec![s.cat, s.both]));
    t.teardown().await;
}

#[tokio::test]
async fn the_facet_counts_the_photos_with_each_kind() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (_, bravo_key) = user(&t.pool, "bravo@example.com").await;
    let app = app_with_pets(&t.pool);
    let (st, f) = get(&app, &s.key, "/assets/search/facets").await;
    assert_eq!(st, StatusCode::OK, "{f}");
    assert_eq!(
        f["pets"],
        json!([{"value": "any", "count": 3}, {"value": "dog", "count": 2}, {"value": "cat", "count": 2}])
    );
    // Trashed and hidden photos are not counted; a kind nobody has is left out; Bravo has none.
    set(&t.pool, s.cat, "deleted_at = now()").await;
    set(&t.pool, s.dog, "hidden = true").await;
    let (_, f) = get(&app, &s.key, "/assets/search/facets").await;
    assert_eq!(
        f["pets"],
        json!([{"value": "any", "count": 1}, {"value": "dog", "count": 1}, {"value": "cat", "count": 1}])
    );
    set(&t.pool, s.both, "deleted_at = now()").await;
    let (_, f) = get(&app, &s.key, "/assets/search/facets").await;
    assert_eq!(f["pets"], json!([]));
    assert_eq!(
        get(&app, &bravo_key, "/assets/search/facets").await.1["pets"],
        json!([])
    );
    t.teardown().await;
}

#[tokio::test]
async fn more_like_this_can_be_limited_to_the_photos_with_a_pet() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let on = app_with_pets(&t.pool);
    for p in [s.dog, s.cat, s.both, s.nothing, s.unscanned] {
        set_embedding(&t.pool, p, &[("beach", 1.0)]).await;
    }
    let (st, v) = get(&on, &s.key, &format!("/assets/{}/similar?pets=cat&cutoff=false", s.dog)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(sorted(item_ids(&v)), sorted(vec![s.cat, s.both]));
    // Off, the parameter does nothing.
    let (_, v) = get(
        &app(&t.pool),
        &s.key,
        &format!("/assets/{}/similar?pets=cat&cutoff=false", s.dog),
    )
    .await;
    assert!(item_ids(&v).contains(&s.nothing), "{v}");
    t.teardown().await;
}

// ---- scopes, smart albums, shares -----------------------------------------------------------------------------------------

#[tokio::test]
async fn the_pets_scope_names_the_photos_of_the_view_for_every_bulk_action_and_never_widens() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (_, bravo_key) = user(&t.pool, "bravo@example.com").await;
    let app = app_with_pets(&t.pool);
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/favorite",
        json!({"scope": {"pets": "dog"}, "except": [s.both], "favorite": true}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(1)), "{v}");
    assert_eq!(count(&t.pool, "SELECT count(*) FROM assets WHERE favorite").await, 1);
    assert!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM assets WHERE favorite AND id = '{}'", s.dog)
        )
        .await
            == 1
    );
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/rating",
        json!({"scope": {"pets": "any"}, "except": [], "rating": 3}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(3)), "{v}");
    // The same photos as the view, so an archived one is in and a hidden one is out; Bravo's scope is Bravo's.
    set(&t.pool, s.cat, "archived = true").await;
    set(&t.pool, s.both, "hidden = true").await;
    let (st, _) = post(
        &app,
        &s.key,
        "/assets/tags/apply",
        json!({"scope": {"pets": "any"}, "except": [], "add": ["Pets"]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM asset_tags").await,
        2,
        "the dog and the archived cat; not the hidden photo"
    );
    let (st, _) = post(
        &app,
        &bravo_key,
        "/assets/tags/apply",
        json!({"scope": {"pets": "any"}, "except": [], "add": ["Pets"]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM asset_tags").await,
        2,
        "Bravo has no pets"
    );
    // Trash and Undo.
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/trash",
        json!({"scope": {"pets": "cat"}, "except": []}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(1)), "{v}");
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/restore",
        json!({"scope": {"trash_batch": v["batch"]}, "except": []}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(1)), "{v}");
    // A kind that is not one is a 400 and nothing happens; the search scope takes the filter too.
    let (st, _) = post(
        &app,
        &s.key,
        "/assets/trash",
        json!({"scope": {"pets": "birds"}, "except": []}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/archive",
        json!({"scope": {"search": {"pets": "dog"}}, "except": [], "archived": true}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(1)), "{v}");
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL").await,
        0
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_smart_album_can_follow_the_pets_in_the_photos() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (_, bravo_key) = user(&t.pool, "bravo@example.com").await;
    let on = app_with_pets(&t.pool);
    let album = |app: &Router, key: &str, rules: Value| {
        let (app, key) = (app.clone(), key.to_string());
        async move { post(&app, &key, "/assets/albums", json!({"title": "Smart", "rules": rules})).await }
    };
    let members = |app: &Router, key: &str, id: Uuid| {
        let (app, key) = (app.clone(), key.to_string());
        async move {
            let (st, v) = send(&app, "GET", &format!("/assets/albums/{id}/assets"), &key, None).await;
            assert_eq!(st, StatusCode::OK, "{v}");
            sorted(item_ids(&v))
        }
    };
    let (st, v) = album(&on, &s.key, json!({"rules": [{"field": "pets", "kind": "dog"}]})).await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(v["rules"]["rules"][0], json!({"field": "pets", "kind": "dog"}));
    let dogs: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(members(&on, &s.key, dogs).await, sorted(vec![s.dog, s.both]));
    // The kind is normalised; "any" and a second rule joined by all or any.
    let (_, v) = album(&on, &s.key, json!({"rules": [{"field": "pets", "kind": " Any "}]})).await;
    assert_eq!(v["rules"]["rules"][0]["kind"], "any");
    let any: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(members(&on, &s.key, any).await, sorted(vec![s.dog, s.cat, s.both]));
    set(&t.pool, s.cat, "favorite = true").await;
    let (_, v) = album(
        &on,
        &s.key,
        json!({"match": "all", "rules": [{"field": "pets", "kind": "cat"}, {"field": "favorite", "value": true}]}),
    )
    .await;
    let fav_cats: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(members(&on, &s.key, fav_cats).await, vec![s.cat]);
    let (_, v) = album(
        &on,
        &s.key,
        json!({"match": "any", "rules": [{"field": "pets", "kind": "cat"}, {"field": "favorite", "value": true}]}),
    )
    .await;
    let either: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(members(&on, &s.key, either).await, sorted(vec![s.cat, s.both]));
    // Live: a photo that is looked at later joins; one that is trashed leaves; Bravo's album has only Bravo's photos.
    scanned(&t.pool, s.unscanned, &[found("dog", [0.1, 0.1, 0.4, 0.4], 0.5)]).await;
    set(&t.pool, s.dog, "deleted_at = now()").await;
    assert_eq!(members(&on, &s.key, dogs).await, sorted(vec![s.both, s.unscanned]));
    let (_, v) = album(&on, &bravo_key, json!({"rules": [{"field": "pets", "kind": "any"}]})).await;
    let theirs: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert!(members(&on, &bravo_key, theirs).await.is_empty());
    // The preview of rules and what the editor lists.
    let (st, v) = post(
        &on,
        &s.key,
        "/assets/albums/preview",
        json!({"rules": {"rules": [{"field": "pets", "kind": "cat"}]}}),
    )
    .await;
    assert_eq!((st, v["count"].clone()), (StatusCode::OK, json!(2)), "{v}");
    // Rules that are not valid.
    for rule in [
        json!({"field": "pets", "kind": "bird"}),
        json!({"field": "pets", "kind": ""}),
        json!({"field": "pets"}),
        json!({"field": "pets", "kind": "dog", "extra": 1}),
    ] {
        let (st, v) = album(&on, &s.key, json!({"rules": [rule.clone()]})).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{rule}: {v}");
    }
    // Saved while Pets was on, read while it is off: matches no photo (never "no rule", which would be every photo).
    let off = app(&t.pool);
    assert!(members(&off, &s.key, dogs).await.is_empty());
    // (One rule of "any of: a cat, a favorite" matches nothing and the other still matches its photo.)
    assert_eq!(members(&off, &s.key, either).await, vec![s.cat]);
    t.teardown().await;
}

async fn visit(app: &Router, path: &str) -> (StatusCode, String) {
    let mut req = Request::builder().method("GET").uri(path).body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("203.0.113.1:1234".parse::<std::net::SocketAddr>().unwrap()));
    let res = app.clone().oneshot(req).await.unwrap();
    let st = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (st, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn a_share_link_carries_nothing_of_the_pets_in_the_photos_it_shows() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app =
        routes_with(ShareState::new(false, vec![]).with_limits((10_000, 10_000), (10_000, 10_000), (10_000, 10_000)))
            .with_state(AppState {
                db: t.pool.clone(),
                storage: dummy_storage(),
                trash_retention_days: 30,
                purge_wake: Default::default(),
                text: std::sync::Arc::new(crate::testutil::FakeText::default()),
                search: crate::testutil::default_search(),
                faces_enabled: false,
                pets_enabled: true,
            });
    let detection: Uuid = sqlx::query_scalar("SELECT id FROM pet_detections WHERE asset_id = $1 LIMIT 1")
        .bind(s.dog)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    // A link to photos, and a link to a smart album whose rule is "a dog".
    let (st, share) = post(
        &app,
        &s.key,
        "/assets/shares",
        json!({"asset_ids": [s.dog, s.both], "allow_download": true}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{share}");
    let (_, album) = post(
        &app,
        &s.key,
        "/assets/albums",
        json!({"title": "Dogs", "rules": {"rules": [{"field": "pets", "kind": "dog"}]}}),
    )
    .await;
    let (st, smart_share) = post(&app, &s.key, "/assets/shares", json!({"album_id": album["id"]})).await;
    assert_eq!(st, StatusCode::CREATED, "{smart_share}");
    let mut bodies = vec![share.to_string(), smart_share.to_string()];
    for sh in [&share, &smart_share] {
        let token = sh["token"].as_str().unwrap();
        for path in [
            format!("/assets/public/s/{token}"),
            format!("/assets/public/s/{token}/items"),
        ] {
            let (st, body) = visit(&app, &path).await;
            assert_eq!(st, StatusCode::OK, "{path}: {body}");
            bodies.push(body);
        }
    }
    // The smart link shows the photos the rule matches now: the two with a dog.
    let token = smart_share["token"].as_str().unwrap();
    let (_, items) = visit(&app, &format!("/assets/public/s/{token}/items")).await;
    assert_eq!(
        items.matches(&s.dog.to_string()).count().min(1) + items.matches(&s.both.to_string()).count().min(1),
        2
    );
    assert!(!items.contains(&s.cat.to_string()));
    for body in bodies {
        for needle in [
            "pets",
            "dog_share",
            "cat_share",
            "\"box\"",
            "\"score\"",
            &detection.to_string(),
        ] {
            assert!(!body.contains(needle), "{needle:?} leaked into: {body}");
        }
    }
    // The visitor cannot reach the owner's routes.
    assert_eq!(visit(&app, "/assets/pets").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        visit(&app, &format!("/assets/{}/pets", s.dog)).await.0,
        StatusCode::UNAUTHORIZED
    );
    t.teardown().await;
}

// ---- progress -------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_processing_report_tells_how_far_looking_for_pets_has_got_apart_from_the_other_jobs() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (bravo, bravo_key) = user(&t.pool, "bravo@example.com").await;
    let theirs = photo(&t.pool, bravo, "bravo.jpg", "2024-05-01").await;
    // A queued job, a running one and a failed one, and one of Bravo's.
    for (a, state) in [
        (s.unscanned, "queued"),
        (s.nothing, "running"),
        (s.cat, "failed"),
        (theirs, "queued"),
    ] {
        sqlx::query("INSERT INTO jobs (kind, asset_id, version, state) VALUES ('pets', $1, $2, $3)")
            .bind(a)
            .bind(VERSION)
            .bind(state)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    let app = app_with_pets(&t.pool);
    let (st, p) = get(&app, &s.key, "/assets/processing").await;
    assert_eq!(st, StatusCode::OK, "{p}");
    assert_eq!(
        p["pets"],
        json!({"scanned": 4, "total": 5, "found": 4, "queued": 1, "running": 1, "failed": 1, "model": MODEL, "percent": 80})
    );
    // Finding pets is not the photos' own processing: only a failure of it is listed with the others.
    let kinds: Vec<&str> = p["by_kind"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["pets", "thumbnail"], "{p}");
    assert_eq!(p["by_kind"][0]["failed"], 1);
    assert_eq!(p["by_kind"][0]["queued"], 0);
    let (_, p) = get(&app, &bravo_key, "/assets/processing").await;
    assert_eq!(p["pets"]["scanned"], 0);
    assert_eq!(p["pets"]["queued"], 1);
    t.teardown().await;
}

// ---- a big library -------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_pets_queries_of_a_big_library_stay_fast_and_on_their_index() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, key) = user(&t.pool, "alpha@example.com").await;
    let (bravo, _) = user(&t.pool, "bravo@example.com").await;
    // 30,000 photos of Alpha (one in eight with a dog or a cat, a third of those with a second animal in the picture) and 10,000 of Bravo's.
    for (owner, n) in [(alice, 30_000), (bravo, 10_000)] {
        sqlx::query(
            "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, taken_at, thumbhash)
             SELECT $1, sha256(($1::text || i::text)::bytea), 'k' || i, 'n' || i, 'image/jpeg', 1, now() - make_interval(secs => i * 60), '\\x01'
             FROM generate_series(1, $2) i",
        )
        .bind(owner)
        .bind(n)
        .execute(&t.pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO pet_scans (asset_id, model, version, detections) SELECT id, $1, $2, 0 FROM assets")
        .bind(MODEL)
        .bind(VERSION)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO pet_detections (asset_id, owner_id, kind, model, version, idx, box_x, box_y, box_w, box_h, score)
         SELECT id, owner_id, CASE WHEN row_number() OVER (ORDER BY id) % 2 = 0 THEN 'dog' ELSE 'cat' END, $1, $2, 0, 0.1, 0.1, 0.5, 0.5, 0.7
         FROM assets WHERE hashtext(id::text) % 8 = 0",
    )
    .bind(MODEL)
    .bind(VERSION)
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE").execute(&t.pool).await.unwrap();
    let with_pets = count(
        &t.pool,
        &format!("SELECT count(DISTINCT asset_id) FROM pet_detections WHERE owner_id = '{alice}'"),
    )
    .await;
    assert!(with_pets > 3000, "{with_pets}");

    let app = app_with_pets(&t.pool);
    for path in [
        "/assets/pets",
        "/assets/pets/photos?kind=dog&limit=60",
        "/assets/pets/photos?limit=60",
        "/assets/pets/timeline?kind=cat",
        "/assets/search?pets=dog",
        "/assets/search?pets=any&from=2024-01-01&limit=60",
        "/assets/search/facets",
    ] {
        let started = std::time::Instant::now();
        let (st, v) = get(&app, &key, path).await;
        let ms = started.elapsed().as_millis();
        assert_eq!(st, StatusCode::OK, "{path}: {v}");
        println!("{path}: {ms} ms");
        assert!(ms < 3000, "{path} took {ms} ms");
    }
    // The photos of one owner, newest first, a page at a time: the first page does not read the library's whole list.
    let plan: Vec<(String,)> = sqlx::query_as(&format!(
        "EXPLAIN SELECT x.id FROM assets x WHERE x.owner_id = '{alice}' AND {} AND {}
         ORDER BY coalesce(x.taken_at, x.file_modified_at, x.created_at) DESC, x.id DESC LIMIT 60",
        super::people::seen("x", "false"),
        super::pets::with_pets("x", "'dog'::text"),
    ))
    .fetch_all(&t.pool)
    .await
    .unwrap();
    let plan = plan.into_iter().map(|r| r.0).collect::<Vec<_>>().join("\n");
    println!("{plan}");
    assert!(
        plan.contains("assets_timeline_idx") || plan.contains("pet_detections_owner_kind_idx"),
        "{plan}"
    );
    t.teardown().await;
}
