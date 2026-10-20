//! Rotate and crop (`recipe` in `POST /assets/edits`, `GET /assets/{id}/download/edited`) over a real Postgres and S3 API
//! (the tests return early without them): the stored recipe and the edited size, relative turns, validation, bulk, owner
//! isolation, the render being queued again, the object `reindex` restores from, and the edited download.
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use cyanotype_core::{edits, keys, reindex};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use crate::testutil::{TestDb, app_with_s3, asset, send, user};

async fn post(app: &axum::Router, key: &str, body: Value) -> (StatusCode, Value) {
    send(app, "POST", "/assets/edits", key, Some(body)).await
}

/// A 4000 x 3000 photo.
async fn photo(pool: &PgPool, owner: Uuid, name: &str) -> Uuid {
    let id = asset(pool, owner, name).await;
    sqlx::query("UPDATE assets SET width = 4000, height = 3000 WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn row(pool: &PgPool, id: Uuid) -> (Option<Value>, i32, i32, Option<i32>, Option<i32>) {
    sqlx::query_as("SELECT recipe, width, height, width_orig, height_orig FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn get_raw(app: &axum::Router, path: &str, key: &str) -> (StatusCode, HeaderMap, Value) {
    let req = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let (st, headers) = (res.status(), res.headers().clone());
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (st, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_recipe_is_stored_changes_the_size_and_queues_a_render() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = photo(&t.pool, alice, "rc-one.jpg").await;
    // The upload's own render has finished.
    sqlx::query("UPDATE jobs SET state = 'done' WHERE asset_id = $1 AND kind = 'thumbnail'")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();

    let (st, r) = post(
        &app,
        &key,
        json!({"ids": [id], "recipe": {"rotate": 90, "straighten": -1.26, "crop": {"x": 0.0, "y": 0.0, "w": 0.5, "h": 1.0}}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["updated"], 1);

    // The row: the canonical recipe, the edited size in width/height, the file's size next to it.
    let (recipe, w, h, wo, ho) = row(&t.pool, id).await;
    assert_eq!(
        recipe.unwrap(),
        json!({"rotate": 90, "straighten": -1.3, "crop": {"x": 0.0, "y": 0.0, "w": 0.5, "h": 1.0}})
    );
    // 4000 x 3000 turned is 3000 x 4000; half the width is 1500 x 4000.
    assert_eq!((w, h, wo, ho), (1500, 4000, Some(4000), Some(3000)));

    // The render is queued again (a different job from the upload's, which was done), and the embedding follows it.
    let jobs: Vec<(String, i32, String)> = sqlx::query_as(
        "SELECT kind, version, state FROM jobs WHERE asset_id = $1 AND kind = 'thumbnail' ORDER BY version",
    )
    .bind(id)
    .fetch_all(&t.pool)
    .await
    .unwrap();
    assert_eq!(
        jobs,
        vec![
            ("thumbnail".to_string(), 1, "done".to_string()),
            ("thumbnail".to_string(), 2, "queued".to_string())
        ]
    );

    // The bucket has it first (and `reindex` restores from it).
    let sha: Vec<u8> = sqlx::query_scalar("SELECT sha256 FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let sha: [u8; 32] = sha.try_into().unwrap();
    let e = edits::parse(&storage.get_bytes(&keys::edits(alice, &sha)).await.unwrap()).unwrap();
    assert_eq!(e.recipe.unwrap().rotate, 90);

    // Details: the recipe, and the file's own size (not the cropped picture's).
    let (st, d) = send(&app, "GET", &format!("/assets/{id}/details"), &key, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(d["edits"]["recipe"]["rotate"], 90);
    assert_eq!(
        (d["file"]["width"].clone(), d["file"]["height"].clone()),
        (json!(4000), json!(3000))
    );
    assert_eq!(d["edits"]["edited_copy_ready"], false);

    // The photo's own record: the recipe, "still rendering" until the worker is done, and only then the whole picture
    // the editor works on.
    let (st, a) = send(&app, "GET", &format!("/assets/{id}"), &key, None).await;
    assert_eq!(st, StatusCode::OK, "{a}");
    assert_eq!(
        (a["rendering"].clone(), a["edit_base_url"].clone()),
        (json!(true), Value::Null)
    );
    assert_eq!(a["recipe"]["rotate"], 90);
    sqlx::query("UPDATE jobs SET state = 'done' WHERE asset_id = $1 AND kind = 'thumbnail'")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET thumbhash = '\\x01'::bytea WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, a) = send(&app, "GET", &format!("/assets/{id}"), &key, None).await;
    assert_eq!(a["rendering"], false);
    let base_url = a["edit_base_url"].as_str().unwrap();
    assert!(
        base_url.contains(&format!("derived/{alice}/{id}/uncropped.webp")),
        "{base_url}"
    );
    // A photo with no recipe has neither.
    let plain = photo(&t.pool, alice, "rc-plain.jpg").await;
    let (_, p) = send(&app, "GET", &format!("/assets/{plain}"), &key, None).await;
    assert_eq!(
        (p["recipe"].clone(), p["edit_base_url"].clone()),
        (Value::Null, Value::Null)
    );

    // The list says it is edited and draws the edited size.
    let (st, l) = send(&app, "GET", "/assets", &key, None).await;
    assert_eq!(st, StatusCode::OK, "{l}");
    let item = l["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!(id))
        .unwrap();
    assert_eq!(
        (item["edited"].clone(), item["width"].clone(), item["height"].clone()),
        (json!(true), json!(1500), json!(4000))
    );

    // The same recipe again changes nothing.
    let (_, r) = post(
        &app,
        &key,
        json!({"ids": [id], "recipe": {"rotate": 90, "straighten": -1.3, "crop": {"x": 0.0, "y": 0.0, "w": 0.5, "h": 1.0}}}),
    )
    .await;
    assert_eq!((r["updated"].clone(), r["unchanged"].clone()), (json!(0), json!(1)));

    // A crop edit alone keeps the other parts; the finished render is queued again each time.
    sqlx::query("UPDATE jobs SET state = 'done' WHERE asset_id = $1 AND kind = 'thumbnail'")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET edited_copy_at = now(), edited_copy_bytes = 5 WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, r) = post(&app, &key, json!({"ids": [id], "recipe": {"rotate": 90}})).await;
    assert_eq!(r["updated"], 1);
    let (recipe, w, h, ..) = row(&t.pool, id).await;
    assert_eq!((recipe.unwrap(), w, h), (json!({"rotate": 90}), 3000, 4000));
    let (state, copy): (String, Option<bool>) = sqlx::query_as(
        "SELECT (SELECT state FROM jobs WHERE asset_id = $1 AND kind = 'thumbnail' AND version = 2),
                (SELECT edited_copy_at IS NULL FROM assets WHERE id = $1)",
    )
    .bind(id)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(
        (state.as_str(), copy),
        ("queued", Some(true)),
        "a new recipe makes the edited copy stale"
    );

    // Revert: the file's own size comes back, the recipe is gone, the object is an empty tombstone, and the picture is
    // rendered once more.
    let (_, r) = post(&app, &key, json!({"ids": [id], "recipe": {"revert": true}})).await;
    assert_eq!(r["updated"], 1);
    assert_eq!(row(&t.pool, id).await, (None, 4000, 3000, None, None));
    let e = edits::parse(&storage.get_bytes(&keys::edits(alice, &sha)).await.unwrap()).unwrap();
    assert!(e.recipe.is_none() && e.is_empty());
    // "Nothing to do" sent as a recipe is a revert too.
    post(&app, &key, json!({"ids": [id], "recipe": {"rotate": 90}})).await;
    let (_, r) = post(
        &app,
        &key,
        json!({"ids": [id], "recipe": {"rotate": 0, "straighten": 0}}),
    )
    .await;
    assert_eq!(r["updated"], 1);
    assert_eq!(row(&t.pool, id).await.0, None);
    t.teardown().await;
}

#[tokio::test]
async fn a_turn_composes_with_the_recipe_and_takes_the_crop_with_it() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = photo(&t.pool, alice, "turn.jpg").await;

    // From nothing: a quarter turn right, then left again.
    post(&app, &key, json!({"ids": [id], "recipe": {"turn": 90}})).await;
    let (recipe, w, h, ..) = row(&t.pool, id).await;
    assert_eq!((recipe.unwrap(), w, h), (json!({"rotate": 90}), 3000, 4000));
    post(&app, &key, json!({"ids": [id], "recipe": {"turn": -90}})).await;
    assert_eq!(row(&t.pool, id).await.0, None, "turning back is no recipe at all");

    // A crop of the top left quarter of the frame follows a quarter turn to the top right.
    post(
        &app,
        &key,
        json!({"ids": [id], "recipe": {"crop": {"x": 0.0, "y": 0.0, "w": 0.25, "h": 0.5}}}),
    )
    .await;
    post(&app, &key, json!({"ids": [id], "recipe": {"turn": 90}})).await;
    let (recipe, w, h, ..) = row(&t.pool, id).await;
    assert_eq!(
        recipe.unwrap(),
        json!({"rotate": 90, "crop": {"x": 0.5, "y": 0.0, "w": 0.5, "h": 0.25}})
    );
    // 4000 x 3000 turned: 3000 x 4000; the crop is half of 3000 by a quarter of 4000.
    assert_eq!((w, h), (1500, 1000));
    // Four turns are the identity: crop back where it was.
    for _ in 0..3 {
        post(&app, &key, json!({"ids": [id], "recipe": {"turn": 90}})).await;
    }
    let (recipe, ..) = row(&t.pool, id).await;
    assert_eq!(
        recipe.unwrap(),
        json!({"crop": {"x": 0.0, "y": 0.0, "w": 0.25, "h": 0.5}})
    );
    t.teardown().await;
}

#[tokio::test]
async fn bad_recipes_are_refused_before_anything_is_written() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = photo(&t.pool, alice, "bad.jpg").await;
    for bad in [
        json!({"rotate": 45}),
        json!({"rotate": 91}),
        json!({"straighten": 50}),
        json!({"straighten": -45.5}),
        json!({"crop": {"x": 0.5, "y": 0.5, "w": 0.6, "h": 0.2}}),
        json!({"crop": {"x": 0, "y": 0, "w": 0.01, "h": 0.5}}),
        json!({"crop": {"x": -0.2, "y": 0, "w": 0.5, "h": 0.5}}),
        json!({"crop": {"x": 0, "y": 0, "w": 0.5}}),
        json!({"turn": 45}),
        json!({"revert": false}),
        json!({"rotat": 90}),
    ] {
        let (st, r) = post(&app, &key, json!({"ids": [id], "recipe": bad.clone()})).await;
        // 400 from the checks, 422 where the body does not even have the shape of a recipe.
        assert!(
            st == StatusCode::BAD_REQUEST || st == StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}: {st} {r}"
        );
    }
    let (st, _) = post(&app, &key, json!({"ids": [id], "recipe": "rotate"})).await;
    assert!(st.is_client_error());
    assert_eq!(row(&t.pool, id).await.0, None);
    t.teardown().await;
}

#[tokio::test]
async fn only_your_own_live_photos_are_edited_in_bulk() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, akey) = user(&t.pool, "a@example.com").await;
    let (bob, bkey) = user(&t.pool, "b@example.com").await;
    let mine = photo(&t.pool, alice, "mine.jpg").await;
    let mine2 = photo(&t.pool, alice, "mine2.jpg").await;
    let theirs = photo(&t.pool, bob, "theirs.jpg").await;
    let trashed = photo(&t.pool, alice, "trashed.jpg").await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&t.pool)
        .await
        .unwrap();

    let (st, r) = post(
        &app,
        &akey,
        json!({"ids": [mine, mine2, theirs, trashed], "recipe": {"turn": 90}}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(
        (r["updated"].clone(), r["missing"].clone()),
        (json!(2), json!(2)),
        "another account's photo and a trashed one are missing"
    );
    assert_eq!(row(&t.pool, theirs).await.0, None);
    assert_eq!(row(&t.pool, trashed).await.0, None);
    // Bob cannot reach Alice's either.
    let (_, r) = post(&app, &bkey, json!({"ids": [mine], "recipe": {"revert": true}})).await;
    assert_eq!(r["missing"], 1);
    assert!(row(&t.pool, mine).await.0.is_some());
    // No credentials, no edit.
    let (st, _) = crate::testutil::send_cookie(
        &app,
        "POST",
        "/assets/edits",
        None,
        Some(json!({"ids": [mine], "recipe": {"revert": true}})),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

#[tokio::test]
async fn reindex_restores_a_recipe_and_renders_again() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, storage)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let id = photo(&t.pool, alice, "ri.jpg").await;
    post(
        &app,
        &key,
        json!({"ids": [id], "recipe": {"rotate": 270, "crop": {"x": 0.1, "y": 0.1, "w": 0.5, "h": 0.5}}}),
    )
    .await;
    // A database restored from a dump that predates the edit.
    sqlx::query(
        "UPDATE assets SET recipe = NULL, width = 4000, height = 3000, width_orig = NULL, height_orig = NULL,
                edited_at = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(&t.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE jobs SET state = 'done' WHERE asset_id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    let opts = reindex::Options {
        apply: true,
        min_age: std::time::Duration::ZERO,
        ..Default::default()
    };
    let done = reindex::run(&t.pool, &storage, &opts).await.unwrap();
    assert_eq!(done.edits_restored, 1);
    let (recipe, w, h, wo, ho) = row(&t.pool, id).await;
    assert_eq!(recipe.unwrap()["rotate"], 270);
    // 3000 x 4000 frame, a half by a half.
    assert_eq!((w, h, wo, ho), (1500, 2000, Some(4000), Some(3000)));
    let state: String =
        sqlx::query_scalar("SELECT state FROM jobs WHERE asset_id = $1 AND kind = 'thumbnail' AND version = 2")
            .bind(id)
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert_eq!(state, "queued", "the restored recipe is rendered");
    t.teardown().await;
}

#[tokio::test]
async fn the_edited_download_asks_the_worker_then_redirects() {
    let Some(t) = TestDb::new().await else { return };
    let Some((app, _)) = app_with_s3(&t.pool) else {
        return t.teardown().await;
    };
    let (alice, key) = user(&t.pool, "a@example.com").await;
    let (_, bkey) = user(&t.pool, "b@example.com").await;
    let id = photo(&t.pool, alice, "IMG_0042.HEIC").await;
    let path = format!("/assets/{id}/download/edited");

    // Nothing to apply: a plain photo has no edited version.
    let (st, ..) = get_raw(&app, &path, &key).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    post(&app, &key, json!({"ids": [id], "recipe": {"turn": 90}})).await;
    // Not written yet: 202, and one job queued however often it is asked.
    for _ in 0..3 {
        let (st, _, v) = get_raw(&app, &path, &key).await;
        assert_eq!((st, v["pending"].clone()), (StatusCode::ACCEPTED, json!(true)));
    }
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE asset_id = $1 AND kind = 'edited_copy'")
        .bind(id)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(queued, 1);

    // The worker has written it: a redirect to a presigned URL that saves it as IMG_0042-edited.jpg.
    sqlx::query("UPDATE assets SET edited_copy_at = now(), edited_copy_bytes = 9 WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, h, _) = get_raw(&app, &path, &key).await;
    assert_eq!(st, StatusCode::TEMPORARY_REDIRECT);
    let loc = h[header::LOCATION].to_str().unwrap();
    assert!(loc.contains(&format!("derived/{alice}/{id}/edited.jpg")), "{loc}");
    assert!(loc.contains("IMG_0042-edited.jpg"), "{loc}");
    assert_eq!(h[header::CACHE_CONTROL], "no-store");

    // The original's download is the original, untouched by the recipe.
    let (st, h, _) = get_raw(&app, &format!("/assets/{id}/download"), &key).await;
    assert_eq!(st, StatusCode::TEMPORARY_REDIRECT);
    let loc = h[header::LOCATION].to_str().unwrap();
    assert!(loc.contains("IMG_0042.HEIC") && !loc.contains("edited"), "{loc}");

    // Someone else's photo, an unknown id, and a trashed photo are all 404.
    assert_eq!(get_raw(&app, &path, &bkey).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        get_raw(&app, &format!("/assets/{}/download/edited", Uuid::new_v4()), &key)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(id)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(get_raw(&app, &path, &key).await.0, StatusCode::NOT_FOUND);
    t.teardown().await;
}

#[test]
fn the_edited_file_name() {
    assert_eq!(keys::edited_file_name("IMG_0042.HEIC"), "IMG_0042-edited.jpg");
    assert_eq!(keys::edited_file_name("a.b.png"), "a.b-edited.jpg");
    assert_eq!(keys::edited_file_name("noext"), "noext-edited.jpg");
    assert_eq!(keys::edited_file_name(".hidden"), ".hidden-edited.jpg");
    assert_eq!(keys::edited_file_name(""), "photo-edited.jpg");
}
