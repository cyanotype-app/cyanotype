//! Named pets over a real Postgres (`docs/pets.md`, "Named pets"): the switch, making a pet from photos (one animal, several animals, none),
//! adding and removing photos, "not this pet", proposals with their tiers and the bulk answers, the list with its counts and covers, rename,
//! hide, merge and delete, the Hidden view, the trash, who owns what, the photo's own pet, the page's photos and timeline, the `{"pet"}`
//! scope, the `pet` search filter and the names read out of a query, the smart-album rule, and that no share link, public page or recovery
//! manifest carries a name. The job that fingerprints the animals is tested in the worker, the proposals' arithmetic in `cyanotype-core`.
use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use cyanotype_core::{
    named_pets::{LIKELY, MAYBE},
    pets::{self, EMBED_DIM, MODEL, NewDetection, Scan, VERSION},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::{routes_with, share_public::ShareState};
use crate::{
    AppState,
    testutil::{TestDb, app, app_with_features, app_with_pets, asset, dummy_storage, send, send_cookie, user},
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

/// A unit vector with cosine `sim` to `axis(0)`, otherwise along `axis(k)`.
fn print(sim: f32, k: usize) -> Vec<f32> {
    let mut v = vec![0.0; EMBED_DIM];
    v[k] = (1.0 - sim * sim).max(0.0).sqrt();
    v[0] = sim;
    v
}

fn animal(kind: &str, bbox: [f32; 4], embedding: Option<Vec<f32>>) -> NewDetection {
    NewDetection {
        kind: kind.into(),
        bbox,
        score: 0.8,
        embedding,
    }
}

/// What the worker stores for a photo it has looked at.
async fn scanned(pool: &PgPool, asset: Uuid, animals: Vec<NewDetection>) {
    let has = |k: &str| animals.iter().any(|d| d.kind == k);
    let scan = Scan {
        dog_share: if has("dog") { 0.9 } else { 0.01 },
        cat_share: if has("cat") { 0.9 } else { 0.01 },
        fingerprinted: true,
        found: animals,
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

async fn patch(app: &Router, key: &str, path: &str, body: Value) -> (StatusCode, Value) {
    send(app, "PATCH", path, key, Some(body)).await
}

fn item_ids(v: &Value) -> Vec<Uuid> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().parse().unwrap())
        .collect()
}

fn ids_of(v: &Value, key: &str) -> Vec<Uuid> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i[key].as_str().unwrap().parse().unwrap())
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

fn id_in(v: &Value) -> Uuid {
    v.as_str().unwrap().parse().unwrap()
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// The animal of a photo (`idx` in it).
async fn det(pool: &PgPool, asset: Uuid, idx: i16) -> Uuid {
    sqlx::query_scalar("SELECT id FROM pet_detections WHERE asset_id = $1 AND idx = $2")
        .bind(asset)
        .bind(idx)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn kind_of(pool: &PgPool, d: Uuid) -> (String, bool) {
    sqlx::query_as("SELECT kind, kind_by_owner FROM pet_detections WHERE id = $1")
        .bind(d)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn pet_of(pool: &PgPool, d: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT pet_id FROM pet_detections WHERE id = $1")
        .bind(d)
        .fetch_one(pool)
        .await
        .unwrap()
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

/// Alpha's library. Each photo has what its name says:
/// `d0`, `d1`, `d2`: one dog each (fingerprints on axis 0: the seeds), `likely`, `maybe`, `far`: one dog each at those similarities
/// to them, `pair`: two dogs (neither like the seeds), `cat1`: a cat (nothing like the seeds), `plain`: looked at, no pets, `fresh`: not looked at yet.
struct Scene {
    uid: Uuid,
    key: String,
    d: [Uuid; 3],
    likely: Uuid,
    maybe: Uuid,
    far: Uuid,
    pair: Uuid,
    cat: Uuid,
    plain: Uuid,
    fresh: Uuid,
}

async fn scene(pool: &PgPool) -> Scene {
    let (uid, key) = user(pool, "alpha@example.com").await;
    let mut d = [Uuid::nil(); 3];
    for (i, slot) in d.iter_mut().enumerate() {
        *slot = photo(pool, uid, &format!("d{i}.jpg"), &format!("2024-05-0{}", i + 1)).await;
        scanned(
            pool,
            *slot,
            vec![animal("dog", [0.1, 0.1, 0.5, 0.5], Some(print(1.0, 1)))],
        )
        .await;
    }
    let likely = photo(pool, uid, "likely.jpg", "2024-05-04").await;
    scanned(
        pool,
        likely,
        vec![animal("dog", [0.1, 0.1, 0.5, 0.5], Some(print(LIKELY + 0.05, 2)))],
    )
    .await;
    let maybe = photo(pool, uid, "maybe.jpg", "2024-05-05").await;
    scanned(
        pool,
        maybe,
        vec![animal(
            "dog",
            [0.1, 0.1, 0.5, 0.5],
            Some(print((LIKELY + MAYBE) / 2.0, 3)),
        )],
    )
    .await;
    let far = photo(pool, uid, "far.jpg", "2024-05-06").await;
    scanned(
        pool,
        far,
        vec![animal("dog", [0.1, 0.1, 0.5, 0.5], Some(print(MAYBE - 0.2, 4)))],
    )
    .await;
    let pair = photo(pool, uid, "pair.jpg", "2024-05-07").await;
    scanned(
        pool,
        pair,
        vec![
            animal("dog", [0.05, 0.1, 0.4, 0.5], Some(print(0.3, 5))),
            animal("dog", [0.55, 0.1, 0.4, 0.5], Some(print(0.0, 7))),
        ],
    )
    .await;
    let cat = photo(pool, uid, "cat1.jpg", "2024-05-08").await;
    scanned(
        pool,
        cat,
        vec![animal("cat", [0.2, 0.2, 0.4, 0.4], Some(print(0.0, 6)))],
    )
    .await;
    let plain = photo(pool, uid, "plain.jpg", "2024-05-09").await;
    scanned(pool, plain, vec![]).await;
    let fresh = photo(pool, uid, "fresh.jpg", "2024-05-10").await;
    Scene {
        uid,
        key,
        d,
        likely,
        maybe,
        far,
        pair,
        cat,
        plain,
        fresh,
    }
}

/// Make Biscuit (a dog) from the three seed photos; returns its id.
async fn biscuit(app: &Router, s: &Scene) -> Uuid {
    let (st, v) = post(
        app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Biscuit", "kind": "dog", "asset_ids": s.d}),
    )
    .await;
    assert_eq!((st, v["assigned"].clone()), (StatusCode::OK, json!(3)), "{v}");
    id_in(&v["pet"]["id"])
}

// ---- the switch -----------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn while_pets_is_off_nothing_about_named_pets_exists_and_its_parameters_do_nothing() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let on = app_with_pets(&t.pool);
    let id = biscuit(&on, &s).await;
    let off = app(&t.pool);
    let detection = det(&t.pool, s.d[0], 0).await;
    for (method, path, body) in [
        ("GET", "/assets/pets/named".to_string(), None),
        (
            "POST",
            "/assets/pets/named".into(),
            Some(json!({"name": "X", "kind": "dog"})),
        ),
        ("GET", format!("/assets/pets/named/{id}"), None),
        ("PATCH", format!("/assets/pets/named/{id}"), Some(json!({"name": "Y"}))),
        ("DELETE", format!("/assets/pets/named/{id}"), None),
        (
            "POST",
            format!("/assets/pets/named/{id}/merge"),
            Some(json!({"into": id})),
        ),
        ("GET", format!("/assets/pets/named/{id}/detections"), None),
        ("GET", format!("/assets/pets/named/{id}/photos"), None),
        (
            "POST",
            format!("/assets/pets/named/{id}/photos"),
            Some(json!({"asset_ids": [s.d[0]]})),
        ),
        (
            "POST",
            format!("/assets/pets/named/{id}/photos/remove"),
            Some(json!({"asset_ids": [s.d[0]]})),
        ),
        ("GET", format!("/assets/pets/named/{id}/timeline"), None),
        ("GET", format!("/assets/pets/named/{id}/suggestions"), None),
        (
            "POST",
            format!("/assets/pets/named/{id}/suggestions/answer"),
            Some(json!({"confirm": [detection]})),
        ),
        (
            "POST",
            format!("/assets/pets/detections/{detection}/assign"),
            Some(json!({"pet_id": id})),
        ),
        (
            "POST",
            format!("/assets/pets/detections/{detection}/reject"),
            Some(json!({"pet_id": id})),
        ),
    ] {
        let (st, v) = send(&off, method, &path, &s.key, body).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {path}: {v}");
    }
    // The filter and the names of a query do nothing; a scope and a rule are refused (never read as "no filter").
    let (st, v) = get(&off, &s.key, &format!("/assets/search?pet={id}")).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(item_ids(&v).len(), 10, "the filter is ignored: every photo");
    let (st, _) = post(
        &off,
        &s.key,
        "/assets/favorite",
        json!({"scope": {"pet": id}, "favorite": true}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = post(
        &off,
        &s.key,
        "/assets/trash",
        json!({"scope": {"search": {"pet": id.to_string()}}}),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "a bulk trash must not widen to the whole library"
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL").await,
        0
    );
    let (st, _) = post(
        &off,
        &s.key,
        "/assets/albums",
        json!({"title": "x", "rules": {"rules": [{"field": "pet", "ids": [id]}]}}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, v) = get(&off, &s.key, &format!("/assets/{}/pets", s.d[0])).await;
    assert_eq!(
        v["scanned"],
        json!(null),
        "the photo's own pets are a 404 while it is off"
    );
    t.teardown().await;
}

// ---- making a pet and adding photos -----------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_pet_is_made_from_photos_and_says_which_it_could_not_place() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "  Biscuit ", "kind": "Dog", "asset_ids": [s.d[0], s.d[1], s.pair, s.cat, s.plain, s.fresh]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["pet"]["name"], "Biscuit", "trimmed");
    assert_eq!(v["pet"]["kind"], "dog", "any case");
    assert_eq!(v["assigned"], 2, "the two photos with one dog each");
    // The photo with two dogs is not guessed: both are offered, with the photo to choose on.
    // So is a photo whose only animal is of the other kind (the detector may be wrong about a dog), with the kind the detector said.
    let amb = v["ambiguous"].as_array().unwrap();
    assert_eq!(amb.len(), 2, "{v}");
    let cat_photo = amb
        .iter()
        .find(|a| id_in(&a["asset_id"]) == s.cat)
        .expect("the cat's photo");
    assert_eq!(cat_photo["candidates"][0]["kind"], "cat");
    assert_eq!(cat_photo["candidates"].as_array().unwrap().len(), 1);
    let amb: Vec<Value> = amb
        .iter()
        .filter(|a| id_in(&a["asset_id"]) == s.pair)
        .cloned()
        .collect();
    assert_eq!(id_in(&amb[0]["asset_id"]), s.pair);
    assert!(amb[0]["preview_url"].as_str().unwrap().starts_with("http"));
    let cands = amb[0]["candidates"].as_array().unwrap();
    assert_eq!(cands.len(), 2);
    assert_eq!(id_in(&cands[0]["detection_id"]), det(&t.pool, s.pair, 0).await);
    assert_box(&cands[1]["box"], [0.55, 0.1, 0.4, 0.5]);
    // A photo with nothing, a photo not looked at yet: said so, by reason.
    let skipped: Vec<(Uuid, String)> = v["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (id_in(&x["asset_id"]), x["reason"].as_str().unwrap().to_string()))
        .collect();
    let mut want = vec![(s.plain, "no_animal".to_string()), (s.fresh, "not_scanned".to_string())];
    want.sort();
    let mut got = skipped;
    got.sort();
    assert_eq!(got, want);
    // Nothing was assigned in the ambiguous photo; the caller picks the second dog.
    let id = id_in(&v["pet"]["id"]);
    let second = det(&t.pool, s.pair, 1).await;
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}/photos"),
        json!({"detection_ids": [second]}),
    )
    .await;
    assert_eq!((st, v["assigned"].clone()), (StatusCode::OK, json!(1)), "{v}");
    assert_eq!(v["pet"]["detections"], 3);
    assert_eq!(v["pet"]["photos"], 3);
    assert_eq!(pet_of(&t.pool, det(&t.pool, s.pair, 0).await).await, None);
    assert_eq!(pet_of(&t.pool, second).await, Some(id));
    // The photo is in the pet already: adding it again by photo is skipped, however many animals it has.
    let (_, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}/photos"),
        json!({"asset_ids": [s.pair, s.d[0]]}),
    )
    .await;
    assert_eq!(v["assigned"], 0);
    assert!(v["ambiguous"].as_array().unwrap().is_empty());
    assert_eq!(v["skipped"].as_array().unwrap().len(), 2);
    assert!(
        v["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["reason"] == "already")
    );
    t.teardown().await;
}

#[tokio::test]
async fn making_a_pet_checks_what_it_is_given() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (bob, _) = user(&t.pool, "bravo@example.com").await;
    let theirs = photo(&t.pool, bob, "theirs.jpg", "2024-05-01").await;
    scanned(
        &t.pool,
        theirs,
        vec![animal("dog", [0.1, 0.1, 0.5, 0.5], Some(print(1.0, 1)))],
    )
    .await;
    let theirs_det = det(&t.pool, theirs, 0).await;
    let app = app_with_pets(&t.pool);
    for (body, why) in [
        (json!({"name": "", "kind": "dog"}), "a pet needs a name"),
        (json!({"name": "   ", "kind": "dog"}), "a blank name"),
        (json!({"name": "Bad\u{7}name", "kind": "dog"}), "a control character"),
        (json!({"name": "x".repeat(101), "kind": "dog"}), "a long name"),
        (json!({"name": "Biscuit", "kind": "horse"}), "a kind that is not one"),
        (json!({"name": "Biscuit", "kind": "any"}), "any is not a kind of pet"),
        (json!({"name": "Biscuit"}), "no kind"),
        (json!({"kind": "dog"}), "no name"),
    ] {
        let (st, v) = post(&app, &s.key, "/assets/pets/named", body.clone()).await;
        assert!(
            st == StatusCode::BAD_REQUEST || st == StatusCode::UNPROCESSABLE_ENTITY,
            "{why}: {st} {v}"
        );
    }
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pets").await, 0, "nothing was made");
    // Somebody else's photo or animal, or one that does not exist, is a 404 and the pet is not made at all.
    for body in [
        json!({"name": "Biscuit", "kind": "dog", "asset_ids": [theirs]}),
        json!({"name": "Biscuit", "kind": "dog", "asset_ids": [s.d[0], Uuid::new_v4()]}),
        json!({"name": "Biscuit", "kind": "dog", "detection_ids": [theirs_det]}),
    ] {
        let (st, v) = post(&app, &s.key, "/assets/pets/named", body.clone()).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{body}: {v}");
    }
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pets").await, 0, "all or nothing");
    // Too many at once.
    let many: Vec<Uuid> = (0..1001).map(|_| Uuid::new_v4()).collect();
    let (st, _) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "B", "kind": "dog", "asset_ids": many}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Without photos is fine: a pet to tag photos into later. Two pets can share a name.
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Biscuit", "kind": "dog"}),
    )
    .await;
    assert_eq!((st, v["assigned"].clone()), (StatusCode::OK, json!(0)), "{v}");
    let (st, _) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Biscuit", "kind": "cat"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    // Nobody is anonymous.
    let (st, _) = send(&app, "GET", "/assets/pets/named", "nope", None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

#[tokio::test]
async fn the_number_of_pets_is_bounded() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    sqlx::query("INSERT INTO pets (owner_id, name, kind) SELECT $1, 'Pet ' || g, 'dog' FROM generate_series(1, 500) g")
        .bind(s.uid)
        .execute(&t.pool)
        .await
        .unwrap();
    let app = app_with_pets(&t.pool);
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "One more", "kind": "dog"}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    t.teardown().await;
}

#[tokio::test]
async fn a_photo_goes_to_the_pet_by_its_free_animal_and_an_animal_of_another_pet_is_not_taken_by_accident() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let first = biscuit(&app, &s).await;
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Mochi", "kind": "dog"}),
    )
    .await;
    let second = id_in(&v["pet"]["id"]);
    // The photo with two dogs: one is Biscuit's; adding it to Mochi by photo offers the other alone and takes it.
    let first_dog = det(&t.pool, s.pair, 0).await;
    let (_, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{first}/photos"),
        json!({"detection_ids": [first_dog]}),
    )
    .await;
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{second}/photos"),
        json!({"asset_ids": [s.pair]}),
    )
    .await;
    assert_eq!((st, v["assigned"].clone()), (StatusCode::OK, json!(1)), "{v}");
    assert_eq!(pet_of(&t.pool, det(&t.pool, s.pair, 1).await).await, Some(second));
    assert_eq!(pet_of(&t.pool, first_dog).await, Some(first), "Biscuit's dog stayed");
    // A photo whose only animal is Biscuit's: Mochi is not given it by photo.
    let (_, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{second}/photos"),
        json!({"asset_ids": [s.d[0]]}),
    )
    .await;
    assert_eq!(v["assigned"], 0);
    assert_eq!(v["skipped"][0]["reason"], "in_other_pet");
    // By animal it is the owner's explicit word: it moves, and Biscuit is refused for it.
    let d0 = det(&t.pool, s.d[0], 0).await;
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{second}/photos"),
        json!({"detection_ids": [d0]}),
    )
    .await;
    assert_eq!((st, v["assigned"].clone()), (StatusCode::OK, json!(1)), "{v}");
    assert_eq!(pet_of(&t.pool, d0).await, Some(second));
    let refused: i64 = count(
        &t.pool,
        &format!("SELECT count(*) FROM pet_rejections WHERE detection_id = '{d0}' AND pet_id = '{first}'"),
    )
    .await;
    assert_eq!(refused, 1, "moved: not the first pet");
    // Moving it back forgets that.
    let (_, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{first}/photos"),
        json!({"detection_ids": [d0]}),
    )
    .await;
    assert_eq!(pet_of(&t.pool, d0).await, Some(first));
    assert_eq!(
        count(
            &t.pool,
            "SELECT count(*) FROM pet_rejections WHERE pet_id = (SELECT id FROM pets WHERE name = 'Biscuit')"
        )
        .await,
        0
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_rejections").await,
        1,
        "and the refusal for Mochi is the move back"
    );
    t.teardown().await;
}

// ---- the list and one pet ---------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_list_has_counts_pending_proposals_and_a_cover_and_cover_can_be_chosen() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Pepper", "kind": "cat", "asset_ids": [s.cat]}),
    )
    .await;
    let pepper = id_in(&v["pet"]["id"]);
    let (st, list) = get(&app, &s.key, "/assets/pets/named").await;
    assert_eq!(st, StatusCode::OK, "{list}");
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let b = &items[0];
    assert_eq!(
        (b["name"].clone(), b["kind"].clone()),
        (json!("Biscuit"), json!("dog")),
        "most photos first"
    );
    assert_eq!(
        (b["photos"].clone(), b["detections"].clone(), b["hidden"].clone()),
        (json!(3), json!(3), json!(false))
    );
    assert_eq!(b["pending"], 2, "the likely and the maybe");
    assert_eq!(items[1]["pending"], 0);
    let cover = &b["cover"];
    assert!(cover["preview_url"].as_str().unwrap().starts_with("http"));
    assert_box(&cover["box"], [0.1, 0.1, 0.5, 0.5]);
    assert!(s.d.contains(&id_in(&cover["asset_id"])));
    // One pet by id: the same object.
    let (st, one) = get(&app, &s.key, &format!("/assets/pets/named/{id}")).await;
    assert_eq!(st, StatusCode::OK);
    for key in ["id", "name", "kind", "hidden", "photos", "detections", "pending"] {
        assert_eq!(one[key], b[key], "{key}");
    }
    assert_eq!(one["cover"]["detection_id"], b["cover"]["detection_id"]);
    // The cover choices (paged) and choosing one.
    let (st, choices) = get(&app, &s.key, &format!("/assets/pets/named/{id}/detections?limit=2")).await;
    assert_eq!(st, StatusCode::OK, "{choices}");
    assert_eq!(choices["items"].as_array().unwrap().len(), 2);
    assert_eq!(choices["next_cursor"], "2");
    let (_, rest) = get(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}/detections?limit=2&cursor=2"),
    )
    .await;
    assert_eq!(rest["items"].as_array().unwrap().len(), 1);
    assert_eq!(rest["next_cursor"], json!(null));
    let want = id_in(&rest["items"][0]["detection_id"]);
    let (st, v) = patch(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}"),
        json!({"cover_detection_id": want}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(id_in(&v["cover"]["detection_id"]), want);
    // The cover must be one of the pet's animals, in a photo that can be seen: not a stranger's, not another pet's, not a proposal.
    let free = det(&t.pool, s.likely, 0).await;
    let other = det(&t.pool, s.cat, 0).await;
    for bad in [free, other, Uuid::new_v4()] {
        let (st, _) = patch(
            &app,
            &s.key,
            &format!("/assets/pets/named/{id}"),
            json!({"cover_detection_id": bad}),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(pepper, id_in(&items[1]["id"]));
    // A bad cover is a 400, an empty change too.
    let (st, _) = patch(&app, &s.key, &format!("/assets/pets/named/{id}"), json!({})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    t.teardown().await;
}

#[tokio::test]
async fn rename_hide_and_delete_are_the_owners_alone() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (_, bob_key) = user(&t.pool, "bravo@example.com").await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let path = format!("/assets/pets/named/{id}");
    let (st, v) = patch(&app, &s.key, &path, json!({"name": " Biscuit the Great "})).await;
    assert_eq!(
        (st, v["name"].clone()),
        (StatusCode::OK, json!("Biscuit the Great")),
        "{v}"
    );
    for bad in [
        json!({"name": ""}),
        json!({"name": "  "}),
        json!({"name": "a\nb"}),
        json!({"name": "x".repeat(101)}),
    ] {
        let (st, _) = patch(&app, &s.key, &path, bad.clone()).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (_, v) = patch(&app, &s.key, &path, json!({"hidden": true})).await;
    assert_eq!(v["hidden"], true);
    assert_eq!(v["pending"], 0, "a hidden pet has no pending badge");
    let (_, v) = patch(&app, &s.key, &path, json!({"hidden": false})).await;
    assert_eq!(v["hidden"], false);
    // Bravo sees nothing of it, whatever the verb.
    for (method, p, body) in [
        ("GET", path.clone(), None),
        ("PATCH", path.clone(), Some(json!({"name": "Mine"}))),
        ("DELETE", path.clone(), None),
        ("GET", format!("{path}/photos"), None),
        ("GET", format!("{path}/timeline"), None),
        ("GET", format!("{path}/suggestions"), None),
        ("GET", format!("{path}/detections"), None),
        ("POST", format!("{path}/photos"), Some(json!({"asset_ids": [s.d[0]]}))),
        (
            "POST",
            format!("{path}/photos/remove"),
            Some(json!({"asset_ids": [s.d[0]]})),
        ),
        (
            "POST",
            format!("{path}/suggestions/answer"),
            Some(json!({"reject": [det(&t.pool, s.likely, 0).await]})),
        ),
    ] {
        let (st, v) = send(&app, method, &p, &bob_key, body).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {p}: {v}");
    }
    let (_, theirs) = get(&app, &bob_key, "/assets/pets/named").await;
    assert!(theirs["items"].as_array().unwrap().is_empty());
    // Delete: the animals are free, the photos are still there, and the audit trail says how many, not who.
    let (st, _) = send(&app, "DELETE", &path, &s.key, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_detections WHERE pet_id IS NOT NULL").await,
        0
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 9);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NULL").await,
        10
    );
    let audit: Vec<(String, Value)> = sqlx::query_as("SELECT action, detail FROM audit_log WHERE action LIKE 'pets.%'")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(audit, vec![("pets.delete".to_string(), json!({"detections": 3}))]);
    let (st, _) = get(&app, &s.key, &path).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[tokio::test]
async fn pets_merge_when_they_are_of_one_kind_and_what_was_refused_for_the_first_is_refused_for_the_second() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let a = biscuit(&app, &s).await;
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Mochi", "kind": "dog", "asset_ids": [s.likely]}),
    )
    .await;
    let b = id_in(&v["pet"]["id"]);
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Pepper", "kind": "cat", "asset_ids": [s.cat]}),
    )
    .await;
    let cat = id_in(&v["pet"]["id"]);
    // Refuse the maybe-photo's dog for Mochi, and Mochi's own dog for Biscuit.
    let maybe = det(&t.pool, s.maybe, 0).await;
    let (st, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{b}/suggestions/answer"),
        json!({"reject": [maybe]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{b}/merge"),
        json!({"into": Uuid::new_v4()}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{v}");
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{b}/merge"),
        json!({"into": b}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{b}/merge"),
        json!({"into": cat}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a dog and a cat: {v}");
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pets").await, 3);
    // Mochi into Biscuit: its dog moves, it is gone, the refusal for Mochi became one for Biscuit, and Biscuit keeps its name.
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{b}/merge"),
        json!({"into": a}),
    )
    .await;
    assert_eq!((st, v["detections"].clone()), (StatusCode::OK, json!(1)), "{v}");
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pets WHERE name = 'Mochi'").await,
        0
    );
    let (_, one) = get(&app, &s.key, &format!("/assets/pets/named/{a}")).await;
    assert_eq!(
        (one["name"].clone(), one["detections"].clone()),
        (json!("Biscuit"), json!(4))
    );
    let refused: Vec<Uuid> = sqlx::query_scalar("SELECT pet_id FROM pet_rejections WHERE detection_id = $1")
        .bind(maybe)
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(refused, vec![a]);
    let audit: Vec<Value> = sqlx::query_scalar("SELECT detail FROM audit_log WHERE action = 'pets.merge'")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(audit, vec![json!({"detections": 1})], "a count, no names");
    t.teardown().await;
}

// ---- removing: "not this pet" ----------------------------------------------------------------------------------------------------

#[tokio::test]
async fn not_this_pet_frees_the_animal_and_it_is_never_proposed_for_that_pet_again() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let d1 = det(&t.pool, s.d[1], 0).await;
    sqlx::query("UPDATE pets SET cover_detection_id = $2 WHERE id = $1")
        .bind(id)
        .bind(d1)
        .execute(&t.pool)
        .await
        .unwrap();
    let path = format!("/assets/pets/named/{id}/photos/remove");
    let (st, v) = post(&app, &s.key, &path, json!({})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let (st, v) = post(&app, &s.key, &path, json!({"asset_ids": [s.d[1], s.likely]})).await;
    assert_eq!(
        (st, v["removed"].clone()),
        (StatusCode::OK, json!(1)),
        "the photo that was in the pet: {v}"
    );
    assert_eq!(pet_of(&t.pool, d1).await, None);
    let cover: Option<Uuid> = sqlx::query_scalar("SELECT cover_detection_id FROM pets WHERE id = $1")
        .bind(id)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(cover, None, "a cover that was removed is no longer the cover");
    // It is the same animal as the seeds (cosine 1): without the refusal it would be the best proposal.
    let (_, v) = get(&app, &s.key, &format!("/assets/pets/named/{id}/suggestions")).await;
    assert!(!ids_of(&v, "detection_id").contains(&d1), "{v}");
    // By animal, too, and a photo or an animal that is not the pet's changes nothing.
    let d2 = det(&t.pool, s.d[2], 0).await;
    let (_, v) = post(
        &app,
        &s.key,
        &path,
        json!({"detection_ids": [d2, det(&t.pool, s.cat, 0).await]}),
    )
    .await;
    assert_eq!(v["removed"], 1);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_rejections").await, 2);
    // Putting it back by hand forgets the refusal.
    let (_, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}/photos"),
        json!({"asset_ids": [s.d[1]]}),
    )
    .await;
    assert_eq!(pet_of(&t.pool, d1).await, Some(id));
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_rejections").await, 1);
    t.teardown().await;
}

// ---- proposals ---------------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn proposals_come_in_tiers_with_counts_and_leave_out_what_does_not_belong() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let path = format!("/assets/pets/named/{id}/suggestions");
    let (st, v) = get(&app, &s.key, &path).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let items = v["items"].as_array().unwrap();
    let got: Vec<(Uuid, &str)> = items
        .iter()
        .map(|i| (id_in(&i["asset_id"]), i["tier"].as_str().unwrap()))
        .collect();
    assert_eq!(got, vec![(s.likely, "likely"), (s.maybe, "maybe")], "{v}");
    assert_eq!(v["counts"], json!({"likely": 1, "maybe": 1}));
    let first = &items[0];
    assert!(first["similarity"].as_f64().unwrap() >= f64::from(LIKELY));
    assert_box(&first["box"], [0.1, 0.1, 0.5, 0.5]);
    assert!(first["preview_url"].as_str().unwrap().starts_with("http"));
    // A tier and a limit narrow the list, not the counts.
    let (_, v) = get(&app, &s.key, &format!("{path}?tier=maybe")).await;
    assert_eq!(
        (v["items"].as_array().unwrap().len(), v["counts"]["likely"].clone()),
        (1, json!(1))
    );
    let (_, v) = get(&app, &s.key, &format!("{path}?limit=1")).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let (st, _) = get(&app, &s.key, &format!("{path}?tier=sure")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Not in the list: photos in the trash, hidden ones (until the view is unlocked) and a stranger's dog that looks the same.
    let (bob, _) = user(&t.pool, "bravo@example.com").await;
    let theirs = photo(&t.pool, bob, "theirs.jpg", "2024-05-01").await;
    scanned(
        &t.pool,
        theirs,
        vec![animal("dog", [0.1, 0.1, 0.5, 0.5], Some(print(1.0, 1)))],
    )
    .await;
    set(&t.pool, s.maybe, "deleted_at = now()").await;
    set(&t.pool, s.likely, "hidden = true").await;
    let (_, v) = get(&app, &s.key, &path).await;
    assert!(v["items"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["counts"], json!({"likely": 0, "maybe": 0}));
    t.teardown().await;
}

#[tokio::test]
async fn the_hidden_view_holds_its_proposals_and_its_pets_back_until_it_is_unlocked() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    // The same owner through a browser session: its Hidden view is locked until it is unlocked.
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) SELECT $2, $1, now() + interval '1 day'")
        .bind(s.uid)
        .bind(cyanotype_service::auth::sha256(b"tok"))
        .execute(&t.pool)
        .await
        .unwrap();
    let cookie = &format!("{}=tok", cyanotype_service::auth::SESSION_COOKIE);
    let (st, v) = send_cookie(
        &app,
        "GET",
        &format!("/assets/pets/named/{id}/suggestions"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    set(&t.pool, s.likely, "hidden = true").await;
    let (_, locked) = send_cookie(
        &app,
        "GET",
        &format!("/assets/pets/named/{id}/suggestions"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(
        locked["items"].as_array().unwrap().len(),
        1,
        "the hidden photo is not offered"
    );
    unlock(&t.pool, cookie).await;
    let (_, open) = send_cookie(
        &app,
        "GET",
        &format!("/assets/pets/named/{id}/suggestions"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(open["items"].as_array().unwrap().len(), 2);
    // A pet that only hidden photos have is not in a locked session's list; the page, its photos and its timeline are a 404.
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Shy", "kind": "cat", "asset_ids": [s.cat]}),
    )
    .await;
    let shy = id_in(&v["pet"]["id"]);
    set(&t.pool, s.cat, "hidden = true").await;
    sqlx::query("UPDATE sessions SET hidden_unlocked_until = NULL WHERE token_hash = $1")
        .bind(cyanotype_service::auth::sha256(b"tok"))
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, list) = send_cookie(&app, "GET", "/assets/pets/named", Some(cookie), None).await;
    assert_eq!(ids_of(&list, "id"), vec![id], "{list}");
    for p in [
        format!("/assets/pets/named/{shy}"),
        format!("/assets/pets/named/{shy}/photos"),
        format!("/assets/pets/named/{shy}/timeline"),
    ] {
        let (st, _) = send_cookie(&app, "GET", &p, Some(cookie), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{p}");
    }
    unlock(&t.pool, cookie).await;
    let (_, list) = send_cookie(&app, "GET", "/assets/pets/named", Some(cookie), None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    // Adding a hidden photo needs the unlocked view too.
    sqlx::query("UPDATE sessions SET hidden_unlocked_until = NULL WHERE token_hash = $1")
        .bind(cyanotype_service::auth::sha256(b"tok"))
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, _) = send_cookie(
        &app,
        "POST",
        &format!("/assets/pets/named/{id}/photos"),
        Some(cookie),
        Some(json!({"asset_ids": [s.likely]})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[tokio::test]
async fn every_answer_to_the_bulk_proposals_counts_and_the_rest_is_left_alone() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Mochi", "kind": "dog"}),
    )
    .await;
    let mochi = id_in(&v["pet"]["id"]);
    let likely = det(&t.pool, s.likely, 0).await;
    let maybe = det(&t.pool, s.maybe, 0).await;
    let far = det(&t.pool, s.far, 0).await;
    let answer = format!("/assets/pets/named/{id}/suggestions/answer");
    for (body, why) in [
        (json!({}), "nothing to answer"),
        (json!({"confirm": [likely], "reject": [likely]}), "both at once"),
    ] {
        let (st, v) = post(&app, &s.key, &answer, body).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{why}: {v}");
    }
    // A stranger's and one that does not exist: a 404 and nothing changes.
    let (st, _) = post(&app, &s.key, &answer, json!({"confirm": [likely, Uuid::new_v4()]})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(pet_of(&t.pool, likely).await, None);
    // Mochi has the maybe-dog already: confirming it for Biscuit is skipped (an owner's word is not undone by a bulk answer).
    let (_, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{mochi}/photos"),
        json!({"detection_ids": [maybe]}),
    )
    .await;
    let (st, v) = post(
        &app,
        &s.key,
        &answer,
        json!({"confirm": [likely, maybe, likely], "reject": [far]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (v["confirmed"].clone(), v["rejected"].clone(), v["skipped"].clone()),
        (json!(1), json!(1), json!(1))
    );
    assert_eq!(pet_of(&t.pool, likely).await, Some(id));
    assert_eq!(pet_of(&t.pool, maybe).await, Some(mochi));
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_rejections").await, 1);
    let (_, one) = get(&app, &s.key, &format!("/assets/pets/named/{id}")).await;
    assert_eq!(one["detections"], 4, "the confirmed one counts at once");
    // Answering again: a confirmed one is skipped, a refused one is refused once.
    let (_, v) = post(&app, &s.key, &answer, json!({"confirm": [likely], "reject": [far]})).await;
    assert_eq!(
        (v["confirmed"].clone(), v["rejected"].clone(), v["skipped"].clone()),
        (json!(0), json!(0), json!(2))
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_rejections").await,
        1,
        "a refusal is written once"
    );
    t.teardown().await;
}

#[tokio::test]
async fn one_animal_can_be_assigned_or_refused_on_its_own() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Pepper", "kind": "cat"}),
    )
    .await;
    let cat = id_in(&v["pet"]["id"]);
    let likely = det(&t.pool, s.likely, 0).await;
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/detections/{likely}/assign"),
        json!({"pet_id": id}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (v["pet"]["detections"].clone(), v["pet"]["name"].clone()),
        (json!(4), json!("Biscuit"))
    );
    // The owner's word wins over the detector's kind: a dog put in a cat pet is a cat from then on (the other direction is the same).
    let far = det(&t.pool, s.far, 0).await;
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/detections/{far}/assign"),
        json!({"pet_id": cat}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        kind_of(&t.pool, far).await,
        ("cat".to_string(), true),
        "the owner's word, kept through a rescan"
    );
    assert_eq!(kind_of(&t.pool, likely).await, ("dog".to_string(), false));
    let (st, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/detections/{likely}/reject"),
        json!({"pet_id": id}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "it is in that pet: remove it instead");
    let (st, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/detections/{likely}/reject"),
        json!({"pet_id": cat}),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NO_CONTENT,
        "a refusal is for any animal; it changes no kind"
    );
    assert_eq!(kind_of(&t.pool, likely).await, ("dog".to_string(), false));
    let maybe = det(&t.pool, s.maybe, 0).await;
    let (st, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/detections/{maybe}/reject"),
        json!({"pet_id": id}),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/detections/{maybe}/reject"),
        json!({"pet_id": id}),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "twice is the same");
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_rejections").await, 2);
    for (p, body) in [
        (
            format!("/assets/pets/detections/{}/assign", Uuid::new_v4()),
            json!({"pet_id": id}),
        ),
        (
            format!("/assets/pets/detections/{maybe}/assign"),
            json!({"pet_id": Uuid::new_v4()}),
        ),
        (
            format!("/assets/pets/detections/{maybe}/reject"),
            json!({"pet_id": Uuid::new_v4()}),
        ),
    ] {
        let (st, _) = post(&app, &s.key, &p, body).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{p}");
    }
    // Somebody else's animal, and somebody else's pet.
    let (_, bob_key) = user(&t.pool, "bravo@example.com").await;
    let (st, _) = post(
        &app,
        &bob_key,
        &format!("/assets/pets/detections/{maybe}/assign"),
        json!({"pet_id": id}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    t.teardown().await;
}

// ---- in a photo, on the pet's page ----------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_photos_own_pets_name_the_pet_each_animal_was_put_in() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let first = det(&t.pool, s.pair, 0).await;
    post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}/photos"),
        json!({"detection_ids": [first]}),
    )
    .await;
    let (st, v) = get(&app, &s.key, &format!("/assets/{}/pets", s.pair)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items[0]["pet"], json!({"id": id, "name": "Biscuit"}));
    assert_eq!(items[1]["pet"], json!(null));
    // A hidden pet is not named.
    patch(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}"),
        json!({"hidden": true}),
    )
    .await;
    let (_, v) = get(&app, &s.key, &format!("/assets/{}/pets", s.pair)).await;
    assert_eq!(v["items"][0]["pet"], json!(null));
    t.teardown().await;
}

#[tokio::test]
async fn the_pets_page_is_a_timeline_of_its_photos_without_the_trash() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let (st, v) = get(&app, &s.key, &format!("/assets/pets/named/{id}/photos")).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(item_ids(&v), vec![s.d[2], s.d[1], s.d[0]], "newest first");
    let (_, v) = get(&app, &s.key, &format!("/assets/pets/named/{id}/photos?limit=2")).await;
    assert_eq!(item_ids(&v), vec![s.d[2], s.d[1]]);
    let cursor = v["next_cursor"].as_str().unwrap().to_string();
    let (_, v) = get(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}/photos?limit=2&cursor={cursor}"),
    )
    .await;
    assert_eq!(item_ids(&v), vec![s.d[0]]);
    let (_, tl) = get(&app, &s.key, &format!("/assets/pets/named/{id}/timeline")).await;
    assert_eq!(tl["total"], 3);
    assert_eq!(
        tl["days"],
        json!([{"day": "2024-05-03", "count": 1}, {"day": "2024-05-02", "count": 1}, {"day": "2024-05-01", "count": 1}])
    );
    set(&t.pool, s.d[1], "deleted_at = now()").await;
    set(&t.pool, s.d[0], "archived = true").await;
    let (_, v) = get(&app, &s.key, &format!("/assets/pets/named/{id}/photos")).await;
    assert_eq!(
        item_ids(&v),
        vec![s.d[2], s.d[0]],
        "archived ones are in, trashed ones are not"
    );
    let (_, one) = get(&app, &s.key, &format!("/assets/pets/named/{id}")).await;
    assert_eq!(one["photos"], 2);
    t.teardown().await;
}

// ---- scope, filter, words, rule ----------------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_pet_scope_names_the_pets_photos_for_every_bulk_action_and_never_widens() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (_, bob_key) = user(&t.pool, "bravo@example.com").await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/favorite",
        json!({"scope": {"pet": id}, "except": [s.d[0]], "favorite": true}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(2)), "{v}");
    assert_eq!(count(&t.pool, "SELECT count(*) FROM assets WHERE favorite").await, 2);
    // Archived photos are in the page, hidden ones are not.
    set(&t.pool, s.d[2], "archived = true").await;
    set(&t.pool, s.d[1], "hidden = true").await;
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/rating",
        json!({"scope": {"pet": id}, "except": [], "rating": 4}),
    )
    .await;
    assert_eq!(v["count"], 2, "{v}");
    // Bravo cannot name it, and neither can an unknown id (the same answer); it never reads as "everything".
    for (key, who) in [(&bob_key, id), (&s.key, Uuid::new_v4())] {
        let (st, _) = post(&app, key, "/assets/trash", json!({"scope": {"pet": who}})).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL").await,
        0
    );
    // Trash through the scope.
    let (st, v) = post(&app, &s.key, "/assets/trash", json!({"scope": {"pet": id}})).await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(2)), "{v}");
    // Only the hidden photo is left in the page: not for a locked session to see (a key is one).
    let (st, _) = get(&app, &s.key, &format!("/assets/pets/named/{id}/photos")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[tokio::test]
async fn save_as_album_works_through_the_scope() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let (st, v) = post(&app, &s.key, "/assets/albums", json!({"title": "Dog days"})).await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let album = id_in(&v["id"]);
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/albums/{album}/assets"),
        json!({"scope": {"pet": id}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, listed) = get(&app, &s.key, &format!("/assets/albums/{album}/assets")).await;
    assert_eq!(sorted(item_ids(&listed)), sorted(s.d.to_vec()));
    t.teardown().await;
}

#[tokio::test]
async fn the_pet_filter_finds_the_photos_where_all_the_pets_are_together() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (_, bob_key) = user(&t.pool, "bravo@example.com").await;
    let app = app_with_pets(&t.pool);
    let a = biscuit(&app, &s).await;
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Mochi", "kind": "dog", "detection_ids": [det(&t.pool, s.pair, 1).await, det(&t.pool, s.far, 0).await]}),
    )
    .await;
    let b = id_in(&v["pet"]["id"]);
    post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{a}/photos"),
        json!({"detection_ids": [det(&t.pool, s.pair, 0).await]}),
    )
    .await;
    let ids = |v: &Value| sorted(item_ids(v));
    let (st, one) = get(&app, &s.key, &format!("/assets/search?pet={a}")).await;
    assert_eq!(st, StatusCode::OK, "{one}");
    assert_eq!(ids(&one), sorted(vec![s.d[0], s.d[1], s.d[2], s.pair]));
    assert_eq!(one["total"], 4);
    let (_, both) = get(&app, &s.key, &format!("/assets/search?pet={a},{b}")).await;
    assert_eq!(ids(&both), vec![s.pair], "together");
    // With the other filters.
    let (_, v) = get(
        &app,
        &s.key,
        &format!("/assets/search?pet={a}&from=2024-05-02&to=2024-05-03"),
    )
    .await;
    assert_eq!(ids(&v), sorted(vec![s.d[1], s.d[2]]));
    let (_, v) = get(&app, &s.key, &format!("/assets/search?pet={a}&pets=cat")).await;
    assert!(item_ids(&v).is_empty());
    // An id that is not the owner's matches nothing, exactly like one that never existed.
    let (st, theirs) = get(&app, &bob_key, &format!("/assets/search?pet={a}")).await;
    let (st2, nothing) = get(&app, &bob_key, &format!("/assets/search?pet={}", Uuid::new_v4())).await;
    assert_eq!((st, st2), (StatusCode::OK, StatusCode::OK));
    assert_eq!(theirs, nothing);
    assert!(item_ids(&theirs).is_empty());
    // Mistakes.
    for bad in ["pet=nope", &format!("pet={a},nope")] {
        let (st, _) = get(&app, &s.key, &format!("/assets/search?{bad}")).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    }
    let eleven = (0..11)
        .map(|_| Uuid::new_v4().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let (st, _) = get(&app, &s.key, &format!("/assets/search?pet={eleven}")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // A photo being scanned again does not count until its new scan is in.
    sqlx::query("DELETE FROM pet_scans WHERE asset_id = $1")
        .bind(s.d[0])
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, v) = get(&app, &s.key, &format!("/assets/search?pet={a}")).await;
    assert_eq!(ids(&v), sorted(vec![s.d[1], s.d[2], s.pair]));
    // The same filter in "More like this".
    let (st, v) = get(&app, &s.key, &format!("/assets/{}/similar?pet={a}", s.d[1])).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    t.teardown().await;
}

#[tokio::test]
async fn names_in_a_query_become_the_filter_and_a_chip_that_skip_takes_off() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_features(&t.pool, true, true);
    let a = biscuit(&app, &s).await;
    let q = |text: &str| format!("/assets/search?q={}", text.replace(' ', "%20"));
    let (st, v) = get(&app, &s.key, &q("Biscuit")).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(sorted(item_ids(&v)), sorted(s.d.to_vec()));
    assert_eq!(v["ranked"], false, "nothing is left to rank");
    let chip = &v["parsed"]["chips"][0];
    assert_eq!(
        (chip["kind"].clone(), chip["label"].clone(), chip["matched"].clone()),
        (json!("pet"), json!("Biscuit"), json!("Biscuit"))
    );
    assert_eq!(chip["pet"], json!([{"id": a, "name": "Biscuit"}]));
    assert_eq!(v["parsed"]["text"], "");
    // Like people's: at the start or the end, or after an introducing word; the middle of a query is words.
    for text in ["biscuit 2024", "photos with biscuit", "2024 biscuit"] {
        let (_, v) = get(&app, &s.key, &q(text)).await;
        assert_eq!(
            v["parsed"]["chips"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["kind"] == "pet")
                .count(),
            1,
            "{text}: {v}"
        );
    }
    let (_, v) = get(&app, &s.key, &q("beach biscuit sunset")).await;
    assert!(
        v["parsed"]["chips"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["kind"] != "pet"),
        "{v}"
    );
    // With a second pet and a date: all of them, together.
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Mochi", "kind": "dog", "detection_ids": [det(&t.pool, s.pair, 1).await]}),
    )
    .await;
    let b = id_in(&v["pet"]["id"]);
    post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{a}/photos"),
        json!({"detection_ids": [det(&t.pool, s.pair, 0).await]}),
    )
    .await;
    let (_, v) = get(&app, &s.key, &q("Biscuit and Mochi may 2024")).await;
    assert_eq!(item_ids(&v), vec![s.pair], "{v}");
    let kinds: Vec<&str> = v["parsed"]["chips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["date", "pet"]);
    assert_eq!(v["parsed"]["chips"][1]["pet"].as_array().unwrap().len(), 2);
    // `skip=pet` is what the chip's x sends: the words are searched like any others.
    let (_, v) = get(&app, &s.key, &format!("{}&skip=pet&cutoff=false", q("Biscuit"))).await;
    assert_eq!(
        (v["ranked"].clone(), v["parsed"]["text"].clone()),
        (json!(true), json!("Biscuit"))
    );
    assert!(v["parsed"]["chips"].as_array().unwrap().is_empty());
    // `skip=pets` (the kinds) leaves the names alone, and a `pet` filter chosen in the filters wins over the words.
    let (_, v) = get(&app, &s.key, &format!("{}&skip=pets", q("Biscuit"))).await;
    assert_eq!(v["parsed"]["chips"][0]["kind"], "pet");
    let (_, v) = get(&app, &s.key, &format!("{}&pet={b}&cutoff=false", q("Biscuit"))).await;
    assert_eq!(v["parsed"]["text"], "Biscuit");
    // A hidden pet, a pet with no photo a search can show, and a name two pets share are never read.
    patch(
        &app,
        &s.key,
        &format!("/assets/pets/named/{a}"),
        json!({"hidden": true}),
    )
    .await;
    let (_, v) = get(&app, &s.key, &format!("{}&cutoff=false", q("Biscuit"))).await;
    assert!(v["parsed"]["chips"].as_array().unwrap().is_empty(), "{v}");
    patch(
        &app,
        &s.key,
        &format!("/assets/pets/named/{a}"),
        json!({"hidden": false}),
    )
    .await;
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Biscuit", "kind": "cat", "asset_ids": [s.cat]}),
    )
    .await;
    assert_eq!(v["assigned"], 1);
    let (_, v) = get(&app, &s.key, &format!("{}&cutoff=false", q("Biscuit"))).await;
    assert!(
        v["parsed"]["chips"].as_array().unwrap().is_empty(),
        "two pets are called Biscuit: nobody is guessed: {v}"
    );
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Ghost", "kind": "dog"}),
    )
    .await;
    assert_eq!(v["assigned"], 0);
    let (_, v) = get(&app, &s.key, &format!("{}&cutoff=false", q("Ghost"))).await;
    assert!(v["parsed"]["chips"].as_array().unwrap().is_empty());
    // A person with the same name is read as the person (people come first), and the pet's name is then only words.
    let person: Uuid = sqlx::query_scalar("INSERT INTO people (owner_id, name) VALUES ($1, 'Mochi') RETURNING id")
        .bind(s.uid)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO faces (asset_id, owner_id, model, version, idx, box_x, box_y, box_w, box_h, landmarks, score, person_id)
         SELECT $1, $2, 'm', 1, 0, 0.1, 0.1, 0.2, 0.2, array_fill(0.1::real, ARRAY[10]), 0.9, $3",
    )
    .bind(s.pair)
    .bind(s.uid)
    .bind(person)
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO face_scans (asset_id, model, version, faces) VALUES ($1, 'm', 1, 1)")
        .bind(s.pair)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, v) = get(&app, &s.key, &format!("{}&cutoff=false", q("Mochi"))).await;
    let kinds: Vec<&str> = v["parsed"]["chips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["people"], "{v}");
    let _ = b;
    t.teardown().await;
}

#[tokio::test]
async fn a_smart_album_can_follow_named_pets() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let (_, bob_key) = user(&t.pool, "bravo@example.com").await;
    let on = app_with_pets(&t.pool);
    let id = biscuit(&on, &s).await;
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
    let (st, v) = album(&on, &s.key, json!({"rules": [{"field": "pet", "ids": [id, id]}]})).await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    assert_eq!(
        v["rules"]["rules"][0],
        json!({"field": "pet", "ids": [id]}),
        "duplicates are dropped"
    );
    let smart: Uuid = id_in(&v["id"]);
    assert_eq!(members(&on, &s.key, smart).await, sorted(s.d.to_vec()));
    // Live: another photo of the pet joins.
    post(
        &on,
        &s.key,
        &format!("/assets/pets/named/{id}/photos"),
        json!({"asset_ids": [s.likely]}),
    )
    .await;
    assert_eq!(
        members(&on, &s.key, smart).await,
        sorted(vec![s.d[0], s.d[1], s.d[2], s.likely])
    );
    // Rules that are not valid; somebody else's pet and an unknown one are a 404, the same.
    for rule in [
        json!({"field": "pet", "ids": []}),
        json!({"field": "pet"}),
        json!({"field": "pet", "ids": ["nope"]}),
    ] {
        let (st, v) = album(&on, &s.key, json!({"rules": [rule.clone()]})).await;
        assert!(
            st == StatusCode::BAD_REQUEST || st == StatusCode::UNPROCESSABLE_ENTITY,
            "{rule}: {st} {v}"
        );
    }
    let too_many: Vec<Uuid> = (0..11).map(|_| id).chain((0..11).map(|_| Uuid::new_v4())).collect();
    let (st, _) = album(&on, &s.key, json!({"rules": [{"field": "pet", "ids": too_many}]})).await;
    assert!(st == StatusCode::BAD_REQUEST || st == StatusCode::NOT_FOUND);
    for who in [id, Uuid::new_v4()] {
        let (st, _) = album(&on, &bob_key, json!({"rules": [{"field": "pet", "ids": [who]}]})).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{who}");
        let (st, _) = post(
            &on,
            &bob_key,
            "/assets/albums/preview",
            json!({"rules": {"rules": [{"field": "pet", "ids": [who]}]}}),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "preview {who}");
    }
    let (st, v) = post(
        &on,
        &s.key,
        "/assets/albums/preview",
        json!({"rules": {"rules": [{"field": "pet", "ids": [id]}]}}),
    )
    .await;
    assert_eq!((st, v["count"].clone()), (StatusCode::OK, json!(4)), "{v}");
    // Saved while Pets was on, read while it is off: matches nothing. Deleting the pet leaves a rule that matches nothing too.
    let off = app(&t.pool);
    assert!(members(&off, &s.key, smart).await.is_empty());
    let (st, _) = send(&on, "DELETE", &format!("/assets/pets/named/{id}"), &s.key, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(members(&on, &s.key, smart).await.is_empty(), "never everything");
    t.teardown().await;
}

// ---- who sees what ------------------------------------------------------------------------------------------------------------------

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
async fn no_share_link_public_page_or_recovery_manifest_carries_a_pets_name_or_ids() {
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
    let id = biscuit(&app, &s).await;
    // A link to the pet's photos, a smart album that follows the pet, and an album made from the pet by scope ("Save as album").
    let (_, plain) = post(&app, &s.key, "/assets/albums", json!({"title": "Walks"})).await;
    post(
        &app,
        &s.key,
        &format!("/assets/albums/{}/assets", plain["id"].as_str().unwrap()),
        json!({"scope": {"pet": id}}),
    )
    .await;
    let (_, smart) = post(
        &app,
        &s.key,
        "/assets/albums",
        json!({"title": "Smart", "rules": {"rules": [{"field": "pet", "ids": [id]}]}}),
    )
    .await;
    let (st, share) = post(
        &app,
        &s.key,
        "/assets/shares",
        json!({"asset_ids": s.d, "allow_download": true}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{share}");
    let (st, smart_share) = post(&app, &s.key, "/assets/shares", json!({"album_id": smart["id"]})).await;
    assert_eq!(st, StatusCode::CREATED, "{smart_share}");
    let (st, plain_share) = post(&app, &s.key, "/assets/shares", json!({"album_id": plain["id"]})).await;
    assert_eq!(st, StatusCode::CREATED, "{plain_share}");
    let mut bodies = vec![share.to_string(), smart_share.to_string(), plain_share.to_string()];
    for sh in [&share, &smart_share, &plain_share] {
        let token = sh["token"].as_str().unwrap();
        for path in [
            format!("/assets/public/s/{token}"),
            format!("/assets/public/s/{token}/items"),
            format!("/assets/public/s/{token}/items/{}", s.d[0]),
        ] {
            let (st, body) = visit(&app, &path).await;
            assert!(st == StatusCode::OK || st == StatusCode::NOT_FOUND, "{path}: {st}");
            bodies.push(body);
        }
    }
    let (_, items) = visit(
        &app,
        &format!("/assets/public/s/{}/items", smart_share["token"].as_str().unwrap()),
    )
    .await;
    assert!(
        items.contains(&s.d[0].to_string()),
        "the smart link shows the photos the rule matches: {items}"
    );
    let detections: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM pet_detections")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    let mut needles = vec![
        "Biscuit".to_string(),
        id.to_string(),
        "pet_id".into(),
        "\"pet\"".into(),
        "detection".into(),
        "\"box\"".into(),
    ];
    needles.extend(detections.iter().map(Uuid::to_string));
    for body in &bodies {
        for needle in &needles {
            assert!(!body.contains(needle.as_str()), "{needle:?} leaked into: {body}");
        }
    }
    // The owner's album and share lists name no pet, and the recovery manifest leaves the pet rule (its ids) out, and the name never in.
    for path in [
        format!("/assets/albums/{}", plain["id"].as_str().unwrap()),
        "/assets/shares".to_string(),
    ] {
        let (_, v) = get(&app, &s.key, &path).await;
        for needle in ["Biscuit", "pet_id"] {
            assert!(!v.to_string().contains(needle), "{needle:?} in {path}: {v}");
        }
    }
    let (bytes, _) = cyanotype_core::manifest::build(&t.pool, s.uid).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for needle in &needles[..3] {
        assert!(!text.contains(needle.as_str()), "{needle:?} in the manifest");
    }
    assert!(!text.contains("\"pet\""), "the pet rule is not in the manifest: {text}");
    // The visitor cannot reach the owner's routes.
    for path in [
        "/assets/pets/named".to_string(),
        format!("/assets/pets/named/{id}"),
        format!("/assets/pets/named/{id}/suggestions"),
    ] {
        assert_eq!(visit(&app, &path).await.0, StatusCode::UNAUTHORIZED, "{path}");
    }
    t.teardown().await;
}

#[tokio::test]
async fn a_new_scan_keeps_the_pets_photos_through_the_routes_too() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    // The photo is edited and looked at again: while it waits, the pet's page does not list it (no scan), then it is back with its pet.
    pets::forget_and_requeue(&t.pool, s.d[0], VERSION, false).await.unwrap();
    let (_, v) = get(&app, &s.key, &format!("/assets/pets/named/{id}/photos")).await;
    assert_eq!(item_ids(&v), vec![s.d[2], s.d[1]]);
    scanned(
        &t.pool,
        s.d[0],
        vec![animal("dog", [0.4, 0.4, 0.3, 0.3], Some(print(0.97, 9)))],
    )
    .await;
    let (_, v) = get(&app, &s.key, &format!("/assets/pets/named/{id}/photos")).await;
    assert_eq!(item_ids(&v), vec![s.d[2], s.d[1], s.d[0]]);
    let (_, one) = get(&app, &s.key, &format!("/assets/pets/named/{id}")).await;
    assert_eq!(one["detections"], 3);
    t.teardown().await;
}

/// The list and the proposals of a pet at the size of a big library: 20,000 free animals and twenty pets. Fast enough to open a page
/// with: well under a second. `cargo test --release -p cyanotype-assets named_pets_tests::the_pets_list -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "a measurement: seeds 20,000 animals"]
async fn the_pets_list_and_proposals_of_a_big_library_stay_fast() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "alpha@example.com").await;
    sqlx::query(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash, taken_at)
         SELECT $1, sha256(g::text::bytea), 'k', 'p.jpg', 'image/jpeg', 1, '\\x01', now() - (g || ' minutes')::interval
         FROM generate_series(1, 20600) g",
    )
    .bind(uid)
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO pet_scans (asset_id, model, version, detections) SELECT id, $1, $2, 1 FROM assets")
        .bind(MODEL)
        .bind(VERSION)
        .execute(&t.pool)
        .await
        .unwrap();
    let dim = EMBED_DIM as i32;
    sqlx::query(&format!(
        "CREATE TABLE dirs AS SELECT p, (SELECT array_agg(random() - 0.5) FROM generate_series(1, {dim}) i WHERE p >= 0) AS d FROM generate_series(0, 19) p"
    ))
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query(&format!(
        "INSERT INTO pet_detections (asset_id, owner_id, kind, model, version, idx, box_x, box_y, box_w, box_h, score, embedding, embed_model)
         SELECT a.id, a.owner_id, 'dog', $1, $2, 0, 0.1, 0.1, 0.5, 0.5, 0.9,
                (SELECT array_agg(x ORDER BY i)::vector({dim}) FROM (
                    SELECT i, dd.d[i] * 3 + (random() - 0.5) AS x FROM generate_series(1, {dim}) i,
                         LATERAL (SELECT d FROM dirs WHERE p = a.rn % 20) dd) q)::halfvec, $3
         FROM (SELECT id, owner_id, row_number() OVER () AS rn FROM assets) a"
    ))
    .bind(MODEL)
    .bind(VERSION)
    .bind(pets::EMBED_MODEL)
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE pet_detections").execute(&t.pool).await.unwrap();
    for p in 0..20 {
        let pet: Uuid =
            sqlx::query_scalar("INSERT INTO pets (owner_id, name, kind) VALUES ($1, $2, 'dog') RETURNING id")
                .bind(uid)
                .bind(format!("Pet {p}"))
                .fetch_one(&t.pool)
                .await
                .unwrap();
        sqlx::query(
            "UPDATE pet_detections SET pet_id = $1 WHERE id IN (
                 SELECT d.id FROM pet_detections d JOIN (SELECT id, row_number() OVER () AS rn FROM assets) a ON a.id = d.asset_id
                 WHERE a.rn % 20 = $2 ORDER BY a.rn LIMIT 30)",
        )
        .bind(pet)
        .bind(p)
        .execute(&t.pool)
        .await
        .unwrap();
    }
    let app = app_with_pets(&t.pool);
    let started = std::time::Instant::now();
    let counts = cyanotype_core::named_pets::pending_counts(&t.pool, uid, None, false)
        .await
        .unwrap();
    println!(
        "  of which the pending counts: {} ms ({} pets)",
        started.elapsed().as_millis(),
        counts.len()
    );
    let started = std::time::Instant::now();
    let (st, v) = get(&app, &key, "/assets/pets/named").await;
    println!(
        "GET /assets/pets/named (20 pets, 20,000 free animals): {} ms",
        started.elapsed().as_millis()
    );
    assert_eq!(st, StatusCode::OK);
    assert!(
        started.elapsed().as_secs() < 10,
        "the list of 20 pets over 20,000 animals took {:?}",
        started.elapsed()
    );
    let id = id_in(&v["items"][0]["id"]);
    let started = std::time::Instant::now();
    let (st, v) = get(&app, &key, &format!("/assets/pets/named/{id}/suggestions")).await;
    println!(
        "GET .../suggestions: {} ms ({} proposals)",
        started.elapsed().as_millis(),
        v["items"].as_array().unwrap().len()
    );
    assert_eq!(st, StatusCode::OK);
    t.teardown().await;
}

// ---- a wrong dog or cat: the owner's word wins ------------------------------------------------------------------------------------

#[tokio::test]
async fn a_dog_the_detector_called_a_cat_goes_in_a_dog_pet_and_the_owners_word_counts_everywhere() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let cat = det(&t.pool, s.cat, 0).await;
    let kinds = |v: &Value| -> Vec<(String, i64)> {
        v["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| (k["kind"].as_str().unwrap().to_string(), k["photos"].as_i64().unwrap()))
            .collect()
    };
    let (_, v) = get(&app, &s.key, "/assets/pets").await;
    assert_eq!(kinds(&v), vec![("dog".into(), 7), ("cat".into(), 1)]);
    // By the photo: the photo has no dog, so the owner is asked, with the animal's kind to read.
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}/photos"),
        json!({"asset_ids": [s.cat]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["assigned"], 0);
    assert_eq!(v["ambiguous"][0]["candidates"][0]["kind"], "cat");
    assert_eq!(
        kind_of(&t.pool, cat).await,
        ("cat".to_string(), false),
        "asking changes nothing"
    );
    // By the animal: the owner's word. It is a dog now, and says so.
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}/photos"),
        json!({"detection_ids": [cat]}),
    )
    .await;
    assert_eq!((st, v["assigned"].clone()), (StatusCode::OK, json!(1)), "{v}");
    assert_eq!(kind_of(&t.pool, cat).await, ("dog".to_string(), true));
    assert_eq!(pet_of(&t.pool, cat).await, Some(id));
    // Everything that reads the kind follows: the summary, the view, the filter, the facet, the smart rule, the viewer, the scope.
    let (_, v) = get(&app, &s.key, "/assets/pets").await;
    assert_eq!(kinds(&v), vec![("dog".into(), 8), ("cat".into(), 0)]);
    let (_, v) = get(&app, &s.key, "/assets/pets/photos?kind=cat").await;
    assert!(item_ids(&v).is_empty(), "{v}");
    let (_, v) = get(&app, &s.key, "/assets/pets/photos?kind=dog").await;
    assert!(item_ids(&v).contains(&s.cat), "{v}");
    let (_, v) = get(&app, &s.key, "/assets/search?pets=dog&cutoff=false").await;
    assert!(item_ids(&v).contains(&s.cat));
    let (_, v) = get(&app, &s.key, "/assets/search?pets=cat&cutoff=false").await;
    assert!(item_ids(&v).is_empty(), "{v}");
    let (_, f) = get(&app, &s.key, "/assets/search/facets").await;
    assert_eq!(
        f["pets"],
        json!([{"value": "any", "count": 8}, {"value": "dog", "count": 8}])
    );
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/albums",
        json!({"title": "Dogs", "rules": {"rules": [{"field": "pets", "kind": "dog"}]}}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let album = v["id"].as_str().unwrap().to_string();
    let (_, v) = get(&app, &s.key, &format!("/assets/albums/{album}/assets")).await;
    assert!(item_ids(&v).contains(&s.cat), "{v}");
    let (_, v) = get(&app, &s.key, &format!("/assets/{}/pets", s.cat)).await;
    assert_eq!(
        (v["items"][0]["kind"].clone(), v["items"][0]["pet"]["name"].clone()),
        (json!("dog"), json!("Biscuit"))
    );
    let (st, _) = post(
        &app,
        &s.key,
        "/assets/favorite",
        json!({"scope": {"pets": "dog"}, "except": [], "favorite": true}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        count(
            &t.pool,
            "SELECT count(*) FROM assets WHERE favorite AND id = (SELECT asset_id FROM pet_detections WHERE kind_by_owner)"
        )
        .await,
        1,
        "the corrected photo is in the dog scope"
    );
    // A new scan that still says "cat" keeps the correction and the pet (what the worker does after a rotate).
    scanned(
        &t.pool,
        s.cat,
        vec![animal("cat", [0.3, 0.3, 0.4, 0.4], Some(print(0.2, 9)))],
    )
    .await;
    let again = det(&t.pool, s.cat, 0).await;
    assert_eq!(kind_of(&t.pool, again).await, ("dog".to_string(), true));
    assert_eq!(pet_of(&t.pool, again).await, Some(id));
    t.teardown().await;
}

async fn pet_id(app: &Router, key: &str, name: &str) -> Uuid {
    let (_, list) = get(app, key, "/assets/pets/named").await;
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .map(|p| id_in(&p["id"]))
        .expect("the pet")
}

#[tokio::test]
async fn every_way_of_putting_an_animal_in_a_pet_takes_the_pets_kind_both_ways() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let cat = det(&t.pool, s.cat, 0).await;
    let dog = det(&t.pool, s.likely, 0).await;
    let maybe = det(&t.pool, s.maybe, 0).await;
    let far = det(&t.pool, s.far, 0).await;
    // Making a pet: a dog pet from a "cat".
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Biscuit", "kind": "dog", "detection_ids": [cat]}),
    )
    .await;
    assert_eq!((st, v["assigned"].clone()), (StatusCode::OK, json!(1)), "{v}");
    assert_eq!(kind_of(&t.pool, cat).await, ("dog".to_string(), true));
    // A cat pet takes a dog by `assign`, and another by the answer to a proposal.
    let (_, v) = post(
        &app,
        &s.key,
        "/assets/pets/named",
        json!({"name": "Pepper", "kind": "cat"}),
    )
    .await;
    let pepper = id_in(&v["pet"]["id"]);
    let (st, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/detections/{dog}/assign"),
        json!({"pet_id": pepper}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(kind_of(&t.pool, dog).await, ("cat".to_string(), true));
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{pepper}/suggestions/answer"),
        json!({"confirm": [maybe], "reject": [far]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!((v["confirmed"].clone(), v["rejected"].clone()), (json!(1), json!(1)));
    assert_eq!(kind_of(&t.pool, maybe).await, ("cat".to_string(), true));
    assert_eq!(
        kind_of(&t.pool, far).await,
        ("dog".to_string(), false),
        "a refusal changes no kind"
    );
    // Pets of two kinds still do not merge.
    let biscuit = pet_id(&app, &s.key, "Biscuit").await;
    let (st, _) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{pepper}/merge"),
        json!({"into": biscuit}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Nobody else's animal: nothing changes for a stranger.
    let (_, bravo_key) = user(&t.pool, "bravo@example.com").await;
    let (st, _) = post(
        &app,
        &bravo_key,
        &format!("/assets/pets/detections/{far}/assign"),
        json!({"pet_id": pepper}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(kind_of(&t.pool, far).await, ("dog".to_string(), false));
    t.teardown().await;
}

#[tokio::test]
async fn a_pet_is_proposed_the_animals_of_either_kind_and_each_says_what_it_is_called() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    // A "cat" that looks like Biscuit is proposed, and says it is a cat; the cat that does not look like it, is not.
    let alike = photo(&t.pool, s.uid, "alike.jpg", "2024-05-11").await;
    scanned(
        &t.pool,
        alike,
        vec![animal("cat", [0.1, 0.1, 0.5, 0.5], Some(print(LIKELY + 0.1, 11)))],
    )
    .await;
    let (_, v) = get(&app, &s.key, &format!("/assets/pets/named/{id}/suggestions")).await;
    let items = v["items"].as_array().unwrap();
    let mine = items.iter().find(|i| id_in(&i["asset_id"]) == alike).expect("proposed");
    assert_eq!(mine["kind"], "cat");
    assert!(
        items.iter().all(|i| id_in(&i["asset_id"]) != s.cat),
        "the unlike cat is not proposed"
    );
    let (_, list) = get(&app, &s.key, "/assets/pets/named").await;
    assert_eq!(
        list["items"][0]["pending"], 3,
        "the pending count agrees: likely, maybe and the cat"
    );
    // Yes to it: it is a dog.
    let d = det(&t.pool, alike, 0).await;
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/pets/named/{id}/suggestions/answer"),
        json!({"confirm": [d]}),
    )
    .await;
    assert_eq!((st, v["confirmed"].clone()), (StatusCode::OK, json!(1)), "{v}");
    assert_eq!(kind_of(&t.pool, d).await, ("dog".to_string(), true));
    t.teardown().await;
}

#[tokio::test]
async fn the_owner_can_say_what_an_animal_is_without_a_pet() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool).await;
    let app = app_with_pets(&t.pool);
    let id = biscuit(&app, &s).await;
    let cat = det(&t.pool, s.cat, 0).await;
    let path = |d: Uuid| format!("/assets/pets/detections/{d}/kind");
    let (st, v) = post(&app, &s.key, &path(cat), json!({"kind": " Dog "})).await;
    assert_eq!((st, v["kind"].clone()), (StatusCode::OK, json!("dog")), "{v}");
    assert_eq!(kind_of(&t.pool, cat).await, ("dog".to_string(), true));
    let (_, v) = get(&app, &s.key, &format!("/assets/{}/pets", s.cat)).await;
    assert_eq!(v["items"][0]["kind"], "dog");
    let (_, v) = get(&app, &s.key, "/assets/search?pets=dog&cutoff=false").await;
    assert!(item_ids(&v).contains(&s.cat));
    // A bad kind, an animal that is not there, a stranger's.
    for (d, body, want) in [
        (cat, json!({"kind": "horse"}), StatusCode::BAD_REQUEST),
        (cat, json!({"kind": "any"}), StatusCode::BAD_REQUEST),
        (Uuid::new_v4(), json!({"kind": "cat"}), StatusCode::NOT_FOUND),
    ] {
        let (st, v) = post(&app, &s.key, &path(d), body.clone()).await;
        assert_eq!(st, want, "{body}: {v}");
    }
    let (_, bravo_key) = user(&t.pool, "bravo@example.com").await;
    let (st, _) = post(&app, &bravo_key, &path(cat), json!({"kind": "cat"})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(kind_of(&t.pool, cat).await, ("dog".to_string(), true));
    // An animal in a pet that the owner says is the other kind leaves it, and is refused for it (like "Not this pet").
    let seed = det(&t.pool, s.d[0], 0).await;
    let (st, _) = post(&app, &s.key, &path(seed), json!({"kind": "cat"})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(kind_of(&t.pool, seed).await, ("cat".to_string(), true));
    assert_eq!(pet_of(&t.pool, seed).await, None);
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM pet_rejections WHERE pet_id = '{id}' AND detection_id = '{seed}'")
        )
        .await,
        1
    );
    // Saying what it already is keeps the animal where it is.
    let other = det(&t.pool, s.d[1], 0).await;
    let (st, _) = post(&app, &s.key, &path(other), json!({"kind": "dog"})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(pet_of(&t.pool, other).await, Some(id));
    // Pets off: a 404 like the rest.
    let off = app_off(&t.pool);
    let (st, _) = post(&off, &s.key, &path(cat), json!({"kind": "cat"})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    t.teardown().await;
}

fn app_off(pool: &PgPool) -> Router {
    app(pool)
}
