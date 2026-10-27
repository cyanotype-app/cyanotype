//! People over a real Postgres: the switch (off, everything but the status is a 404), the list with counts and covers, the person's
//! photos, renaming, hiding, covers, merging, deleting, taking faces out ("not this person"), assigning, suggestions, a photo's faces,
//! who owns what, the Hidden view, and that no share link carries any of it. The grouping itself is tested in
//! `cyanotype-core/tests/people.rs`.
use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use cyanotype_core::faces::{self, DIM, MODEL, NewFace, VERSION};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use super::{routes_with, share_public::ShareState};
use crate::{
    AppState,
    testutil::{TestDb, app, app_with_people, asset, dummy_storage, send, send_cookie, session, user},
};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
    }
}

fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter_mut().for_each(|x| *x /= n);
    v
}

/// A made-up identity, and a shot of it (about 0.85 alike another shot of the same identity, about 0 alike another identity).
fn identity(seed: u64) -> Vec<f32> {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    unit((0..DIM).map(|_| r.next()).collect())
}

fn shot(who: &[f32], seed: u64) -> Vec<f32> {
    let mut r = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
    let noise = unit((0..DIM).map(|_| r.next()).collect());
    unit(who.iter().zip(&noise).map(|(a, n)| a + 0.4 * n).collect())
}

/// A live photo with a preview, named so that it is unique.
async fn photo(pool: &PgPool, owner: Uuid, name: &str) -> Uuid {
    let id = asset(pool, owner, name).await;
    sqlx::query(
        "UPDATE assets SET thumbhash = '\\x01', taken_at = now() - make_interval(secs => length($2)) WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Store one face with a fingerprint in `asset`; returns its id.
async fn face(pool: &PgPool, asset: Uuid, e: Option<Vec<f32>>, bbox: [f32; 4]) -> Uuid {
    let f = NewFace {
        bbox,
        landmarks: [0.1; 10],
        score: 0.9,
        embedding: e,
    };
    assert!(faces::store(pool, asset, MODEL, VERSION, &[f]).await.unwrap());
    sqlx::query_scalar("SELECT id FROM faces WHERE asset_id = $1 AND model = $2")
        .bind(asset)
        .bind(MODEL)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn person(pool: &PgPool, owner: Uuid, name: Option<&str>) -> Uuid {
    sqlx::query_scalar("INSERT INTO people (owner_id, name) VALUES ($1, $2) RETURNING id")
        .bind(owner)
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn put(pool: &PgPool, face: Uuid, person: Uuid) {
    sqlx::query("UPDATE faces SET person_id = $2, assigned_by = 'auto' WHERE id = $1")
        .bind(face)
        .bind(person)
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

/// `(photos, faces)` of the listed people, in the order served, by name ("" for none).
fn listed(v: &Value) -> Vec<(String, i64, i64)> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap_or("").to_string(),
                p["photos"].as_i64().unwrap(),
                p["faces"].as_i64().unwrap(),
            )
        })
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
        got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6),
        "{v} != {want:?}"
    );
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

// ---- the switch -----------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn while_the_feature_is_off_nothing_about_people_exists_but_its_status() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let a = photo(&t.pool, uid, "a.jpg").await;
    let f = face(&t.pool, a, Some(identity(1)), [0.1, 0.1, 0.2, 0.2]).await;
    let p = person(&t.pool, uid, Some("Person A")).await;
    put(&t.pool, f, p).await;
    let off = app(&t.pool);
    assert_eq!(
        get(&off, &key, "/assets/people/status").await,
        (StatusCode::OK, json!({"enabled": false}))
    );
    for path in [
        "/assets/people".to_string(),
        format!("/assets/people/{p}"),
        format!("/assets/people/{p}/photos"),
        format!("/assets/people/{p}/suggestions"),
        format!("/assets/{a}/faces"),
    ] {
        let (st, body) = get(&off, &key, &path).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{path}: {body}");
    }
    for (method, path, body) in [
        ("PATCH", format!("/assets/people/{p}"), json!({"name": "x"})),
        ("DELETE", format!("/assets/people/{p}"), Value::Null),
        ("POST", format!("/assets/people/{p}/merge"), json!({"into": p})),
        (
            "POST",
            format!("/assets/people/{p}/faces/remove"),
            json!({"face_ids": [f]}),
        ),
        ("POST", format!("/assets/faces/{f}/assign"), json!({"new": true})),
        ("POST", format!("/assets/faces/{f}/reject"), json!({"person_id": p})),
        ("POST", format!("/assets/faces/{f}/not-a-face"), Value::Null),
        ("DELETE", format!("/assets/faces/{f}/not-a-face"), Value::Null),
    ] {
        let body = (!body.is_null()).then_some(body);
        assert_eq!(
            send(&off, method, &path, &key, body).await.0,
            StatusCode::NOT_FOUND,
            "{method} {path}"
        );
    }
    // Nothing changed behind the 404s, and the processing report does not mention people either.
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM people WHERE name = 'Person A'").await,
        1
    );
    // On, the same account sees it; a visitor without credentials sees nothing either way.
    let on = app_with_people(&t.pool);
    assert_eq!(
        get(&on, &key, "/assets/people/status").await,
        (StatusCode::OK, json!({"enabled": true}))
    );
    assert_eq!(
        send_cookie(&on, "GET", "/assets/people", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send_cookie(&off, "GET", "/assets/people/status", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    t.teardown().await;
}

// ---- list, one person, rename, hide, cover --------------------------------------------------------------------------------------

#[tokio::test]
async fn people_are_listed_with_counts_and_a_cover_and_belong_to_their_owner() {
    let Some(t) = TestDb::new().await else { return };
    let (alice, ka) = user(&t.pool, "a@example.com").await;
    let (bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let (p_big, p_small, _p_empty) = (
        person(&t.pool, alice, Some("Person A")).await,
        person(&t.pool, alice, None).await,
        person(&t.pool, alice, Some("Person E")).await,
    );
    let mut faces_ = Vec::new();
    for i in 0..3 {
        let a = photo(&t.pool, alice, &format!("big{i}.jpg")).await;
        // The third face is the largest: the default cover.
        let size = if i == 2 { 0.4 } else { 0.1 };
        let f = face(&t.pool, a, Some(identity(1)), [0.1, 0.1, size, size]).await;
        put(&t.pool, f, p_big).await;
        faces_.push((a, f));
    }
    let a = photo(&t.pool, alice, "small.jpg").await;
    let f = face(&t.pool, a, Some(identity(2)), [0.3, 0.3, 0.2, 0.2]).await;
    put(&t.pool, f, p_small).await;
    // A photo in the trash and a second face of the same photo are not photos; a RAW half has no faces of its own.
    let trashed = photo(&t.pool, alice, "trashed.jpg").await;
    let ft = face(&t.pool, trashed, Some(identity(1)), [0.1, 0.1, 0.2, 0.2]).await;
    put(&t.pool, ft, p_big).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&t.pool)
        .await
        .unwrap();
    // Bob's person is not Alice's.
    let theirs = person(&t.pool, bob, Some("Person B")).await;

    let (st, v) = get(&app, &ka, "/assets/people").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        listed(&v),
        vec![("Person A".into(), 3, 3), ("".into(), 1, 1), ("Person E".into(), 0, 0)]
    );
    let big = &v["items"][0];
    assert_eq!(big["id"], p_big.to_string());
    assert_eq!(big["hidden"], false);
    assert_eq!(big["cover"]["face_id"], faces_[2].1.to_string());
    assert_eq!(big["cover"]["asset_id"], faces_[2].0.to_string());
    assert_box(&big["cover"]["box"], [0.1, 0.1, 0.4, 0.4]);
    assert!(
        big["cover"]["preview_url"]
            .as_str()
            .unwrap()
            .contains(&format!("{}/preview.webp", faces_[2].0))
    );
    assert!(v["items"][2]["cover"].is_null(), "a person without photos has no cover");

    let (st, one) = get(&app, &ka, &format!("/assets/people/{p_big}")).await;
    assert_eq!(
        (st, one["name"].as_str(), one["photos"].as_i64()),
        (StatusCode::OK, Some("Person A"), Some(3))
    );
    // Someone else's person is a 404, whatever the verb.
    assert_eq!(
        get(&app, &ka, &format!("/assets/people/{theirs}")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &kb, &format!("/assets/people/{p_big}")).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, bobs) = get(&app, &kb, "/assets/people").await;
    assert_eq!(listed(&bobs), vec![("Person B".into(), 0, 0)]);
    for (method, path, body) in [
        ("PATCH", format!("/assets/people/{p_big}"), Some(json!({"name": "x"}))),
        ("DELETE", format!("/assets/people/{p_big}"), None),
        (
            "POST",
            format!("/assets/people/{p_big}/merge"),
            Some(json!({"into": theirs})),
        ),
        (
            "POST",
            format!("/assets/people/{theirs}/merge"),
            Some(json!({"into": p_big})),
        ),
        ("GET", format!("/assets/people/{p_big}/photos"), None),
        ("GET", format!("/assets/people/{p_big}/suggestions"), None),
        (
            "POST",
            format!("/assets/people/{p_big}/faces/remove"),
            Some(json!({"face_ids": [faces_[0].1]})),
        ),
    ] {
        let st = send(&app, method, &path, &kb, body).await.0;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {path} as another user");
    }
    // Merging Alice's person into Bob's is as if it did not exist.
    assert_eq!(
        post(
            &app,
            &ka,
            &format!("/assets/people/{p_big}/merge"),
            json!({"into": theirs})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM faces WHERE person_id IS NOT NULL").await,
        5
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_person_is_renamed_hidden_and_given_a_cover_with_the_inputs_checked() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (_, other) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let p = person(&t.pool, uid, None).await;
    let a = photo(&t.pool, uid, "a.jpg").await;
    let f = face(&t.pool, a, Some(identity(1)), [0.1, 0.1, 0.2, 0.2]).await;
    put(&t.pool, f, p).await;
    let path = format!("/assets/people/{p}");
    let patch = |body: Value| {
        let (app, key, path) = (app.clone(), key.clone(), path.clone());
        async move { send(&app, "PATCH", &path, &key, Some(body)).await }
    };

    let (st, v) = patch(json!({"name": "  Person A  "})).await;
    assert_eq!((st, v["name"].as_str()), (StatusCode::OK, Some("Person A")));
    let (_, v) = patch(json!({"hidden": true})).await;
    assert_eq!(
        (v["hidden"].as_bool(), v["name"].as_str()),
        (Some(true), Some("Person A")),
        "a field left out is left alone"
    );
    // null and blank both clear the name.
    assert!(patch(json!({"name": null})).await.1["name"].is_null());
    patch(json!({"name": "Person A"})).await;
    assert!(patch(json!({"name": "   "})).await.1["name"].is_null());
    for bad in [json!({"name": "x".repeat(101)}), json!({"name": "a\u{7}b"}), json!({})] {
        assert_eq!(patch(bad.clone()).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    // The cover must be one of this person's faces.
    let b = photo(&t.pool, uid, "b.jpg").await;
    let f2 = face(&t.pool, b, Some(identity(1)), [0.3, 0.3, 0.1, 0.1]).await;
    assert_eq!(
        patch(json!({"cover_face_id": f2})).await.0,
        StatusCode::BAD_REQUEST,
        "not in the person yet"
    );
    put(&t.pool, f2, p).await;
    let (st, v) = patch(json!({"cover_face_id": f2})).await;
    assert_eq!(
        (st, v["cover"]["face_id"].as_str()),
        (StatusCode::OK, Some(f2.to_string().as_str()))
    );
    let theirs_a = photo(&t.pool, user(&t.pool, "c@example.com").await.0, "c.jpg").await;
    let theirs_f = face(&t.pool, theirs_a, Some(identity(3)), [0.1, 0.1, 0.1, 0.1]).await;
    assert_eq!(
        patch(json!({"cover_face_id": theirs_f})).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&app, "PATCH", &path, &other, Some(json!({"name": "mine now"})))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let stored: Option<String> = sqlx::query_scalar("SELECT name FROM people WHERE id = $1")
        .bind(p)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(stored, None);
    t.teardown().await;
}

// ---- a person's photos ----------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_persons_photos_are_paged_newest_first_and_leave_out_the_trash_and_other_peoples() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app_with_people(&t.pool);
    let (p, q) = (
        person(&t.pool, uid, Some("Person A")).await,
        person(&t.pool, uid, None).await,
    );
    let mut mine = Vec::new();
    for i in 0..5 {
        let a = photo(&t.pool, uid, &format!("{}.jpg", "x".repeat(i + 1))).await;
        let f = face(&t.pool, a, Some(identity(1)), [0.1, 0.1, 0.2, 0.2]).await;
        put(&t.pool, f, p).await;
        mine.push(a);
    }
    let other = photo(&t.pool, uid, "other.jpg").await;
    let fo = face(&t.pool, other, Some(identity(2)), [0.1, 0.1, 0.2, 0.2]).await;
    put(&t.pool, fo, q).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(mine[0])
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, v) = get(&app, &key, &format!("/assets/people/{p}/photos?limit=2")).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let mut seen = item_ids(&v);
    assert_eq!(seen.len(), 2);
    assert!(
        v["items"][0]["thumb_url"].as_str().is_some(),
        "same item shape as the library"
    );
    let mut cursor = v["next_cursor"].as_str().map(str::to_string);
    while let Some(c) = cursor {
        let (_, v) = get(&app, &key, &format!("/assets/people/{p}/photos?limit=2&cursor={c}")).await;
        seen.extend(item_ids(&v));
        cursor = v["next_cursor"].as_str().map(str::to_string);
    }
    // Newest first: the names are longer for the older photos (taken_at is now minus the name's length), so shortest name first.
    assert_eq!(seen, vec![mine[1], mine[2], mine[3], mine[4]]);
    assert_eq!(
        get(&app, &key, &format!("/assets/people/{p}/photos?cursor=zz")).await.0,
        StatusCode::BAD_REQUEST
    );
    t.teardown().await;
}

// ---- the Hidden view ------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn faces_in_hidden_photos_are_not_counted_listed_or_shown_to_a_session_that_has_not_unlocked_the_view() {
    let Some(t) = TestDb::new().await else { return };
    let app = app_with_people(&t.pool);
    let (uid, cookie) = session(&t.pool, "a@example.com", "member").await;
    let (p_mixed, p_secret) = (
        person(&t.pool, uid, Some("Person A")).await,
        person(&t.pool, uid, Some("Person S")).await,
    );
    let shown = photo(&t.pool, uid, "shown.jpg").await;
    let secret = photo(&t.pool, uid, "secret.jpg").await;
    let secret2 = photo(&t.pool, uid, "secret2.jpg").await;
    let f_shown = face(&t.pool, shown, Some(identity(1)), [0.1, 0.1, 0.2, 0.2]).await;
    let f_secret = face(&t.pool, secret, Some(identity(1)), [0.5, 0.5, 0.3, 0.3]).await;
    let f_secret2 = face(&t.pool, secret2, Some(identity(2)), [0.1, 0.1, 0.2, 0.2]).await;
    put(&t.pool, f_shown, p_mixed).await;
    put(&t.pool, f_secret, p_mixed).await;
    put(&t.pool, f_secret2, p_secret).await;
    for a in [secret, secret2] {
        sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
            .bind(a)
            .execute(&t.pool)
            .await
            .unwrap();
    }
    let g = |path: String| {
        let (app, cookie) = (app.clone(), cookie.clone());
        async move { send_cookie(&app, "GET", &path, Some(&cookie), None).await }
    };
    // Locked: the mixed person counts the visible photo only, the secret person is not there at all.
    let (_, v) = g("/assets/people".into()).await;
    assert_eq!(listed(&v), vec![("Person A".into(), 1, 1)]);
    assert!(!v.to_string().contains(&secret.to_string()) && !v.to_string().contains("Person S"));
    assert_eq!(
        v["items"][0]["cover"]["asset_id"],
        shown.to_string(),
        "a hidden photo is never the cover of a locked view"
    );
    assert_eq!(g(format!("/assets/people/{p_secret}")).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        g(format!("/assets/people/{p_secret}/photos")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(g(format!("/assets/{secret}/faces")).await.0, StatusCode::NOT_FOUND);
    let (_, ph) = g(format!("/assets/people/{p_mixed}/photos")).await;
    assert_eq!(item_ids(&ph), vec![shown]);
    // Not assignable, rejectable or removable from a locked session either.
    for (path, body) in [
        (format!("/assets/faces/{f_secret}/assign"), json!({"new": true})),
        (
            format!("/assets/faces/{f_secret}/reject"),
            json!({"person_id": p_mixed}),
        ),
    ] {
        assert_eq!(
            send_cookie(&app, "POST", &path, Some(&cookie), Some(body)).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let (_, r) = send_cookie(
        &app,
        "POST",
        &format!("/assets/people/{p_mixed}/faces/remove"),
        Some(&cookie),
        Some(json!({"asset_ids": [secret]})),
    )
    .await;
    assert_eq!(r["removed"], 0);
    // Unlocked: everything, covers included.
    unlock(&t.pool, &cookie).await;
    let (_, v) = g("/assets/people".into()).await;
    assert_eq!(listed(&v), vec![("Person A".into(), 2, 2), ("Person S".into(), 1, 1)]);
    let (_, ph) = g(format!("/assets/people/{p_mixed}/photos")).await;
    assert_eq!(item_ids(&ph).len(), 2);
    let (st, fs) = g(format!("/assets/{secret}/faces")).await;
    assert_eq!((st, fs["items"].as_array().unwrap().len()), (StatusCode::OK, 1));
    t.teardown().await;
}

// ---- taking faces out, assigning, suggestions ----------------------------------------------------------------------------------

#[tokio::test]
async fn removing_a_persons_faces_unassigns_them_writes_rejections_and_the_worker_never_puts_them_back() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (other, ko) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let ann = identity(1);
    let p = person(&t.pool, uid, Some("Person A")).await;
    let mut fs = Vec::new();
    for i in 0..4 {
        let a = photo(&t.pool, uid, &format!("p{i}.jpg")).await;
        let f = face(&t.pool, a, Some(shot(&ann, 10 + i)), [0.1, 0.1, 0.2, 0.2]).await;
        put(&t.pool, f, p).await;
        fs.push((a, f));
    }
    sqlx::query("UPDATE people SET cover_face_id = $2 WHERE id = $1")
        .bind(p)
        .bind(fs[0].1)
        .execute(&t.pool)
        .await
        .unwrap();
    let path = format!("/assets/people/{p}/faces/remove");
    for bad in [json!({}), json!({"asset_ids": [], "face_ids": []})] {
        assert_eq!(post(&app, &key, &path, bad).await.0, StatusCode::BAD_REQUEST);
    }
    // By photo, and by face; a stranger's face id and a face that is not this person's change nothing.
    let their_a = photo(&t.pool, other, "t.jpg").await;
    let their_f = face(&t.pool, their_a, Some(identity(5)), [0.1, 0.1, 0.2, 0.2]).await;
    let (st, v) = post(
        &app,
        &key,
        &path,
        json!({"asset_ids": [fs[0].0, fs[1].0, their_a], "face_ids": [fs[2].1, their_f]}),
    )
    .await;
    assert_eq!((st, v["removed"].as_i64()), (StatusCode::OK, Some(3)));
    let rejected: i64 = count(
        &t.pool,
        &format!("SELECT count(*) FROM face_rejections WHERE person_id = '{p}'"),
    )
    .await;
    assert_eq!(rejected, 3);
    let cover: Option<Uuid> = sqlx::query_scalar("SELECT cover_face_id FROM people WHERE id = $1")
        .bind(p)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(cover, None, "the cover went with its face");
    let (_, one) = get(&app, &key, &format!("/assets/people/{p}")).await;
    assert_eq!((one["photos"].as_i64(), one["faces"].as_i64()), (Some(1), Some(1)));
    // Again: nothing more to remove.
    assert_eq!(
        post(&app, &key, &path, json!({"face_ids": [fs[2].1]})).await.1["removed"],
        0
    );
    // The worker's grouping leaves them alone, though they look exactly like the person.
    cyanotype_core::people::run(&t.pool, uid).await.unwrap();
    let back = count(&t.pool, &format!("SELECT count(*) FROM faces WHERE person_id = '{p}'")).await;
    assert_eq!(back, 1, "never assigned to a person they were rejected from");
    // They look alike, so they were grouped on their own, into a new person.
    assert_eq!(count(&t.pool, "SELECT count(*) FROM people").await, 2);
    // Another account cannot remove from this person; the other's face stayed theirs.
    assert_eq!(
        post(&app, &ko, &path, json!({"face_ids": [fs[3].1]})).await.0,
        StatusCode::NOT_FOUND
    );
    t.teardown().await;
}

#[tokio::test]
async fn assigning_a_face_to_a_person_or_a_new_one_moves_it_and_remembers_where_it_was_not() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (other, ko) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let (p, q) = (
        person(&t.pool, uid, Some("Person A")).await,
        person(&t.pool, uid, None).await,
    );
    let a = photo(&t.pool, uid, "a.jpg").await;
    let f = face(&t.pool, a, Some(identity(1)), [0.1, 0.1, 0.2, 0.2]).await;
    put(&t.pool, f, q).await;
    let path = format!("/assets/faces/{f}/assign");
    for bad in [
        json!({}),
        json!({"person_id": p, "new": true}),
        json!({"person_id": p, "name": "x"}),
        json!({"new": true, "name": "a\u{7}"}),
    ] {
        assert_eq!(
            post(&app, &key, &path, bad.clone()).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    let (st, v) = post(&app, &key, &path, json!({"person_id": p})).await;
    assert_eq!(
        (st, v["person"]["id"].as_str(), v["person"]["photos"].as_i64()),
        (StatusCode::OK, Some(p.to_string().as_str()), Some(1))
    );
    let (by, now): (String, Option<Uuid>) = sqlx::query_as("SELECT assigned_by, person_id FROM faces WHERE id = $1")
        .bind(f)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!((by.as_str(), now), ("user", Some(p)));
    // Moved: not q, and q (unnamed, with no face left) is gone.
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM face_rejections WHERE face_id = '{f}' AND person_id = '{q}'")
        )
        .await,
        0,
        "the rejection went with the person"
    );
    assert_eq!(
        count(&t.pool, &format!("SELECT count(*) FROM people WHERE id = '{q}'")).await,
        0
    );
    // To a new person, with a name, and back to the first (which removes the rejection of it).
    let (_, v) = post(&app, &key, &path, json!({"new": true, "name": " Person N "})).await;
    let n: Uuid = v["person"]["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(v["person"]["name"], "Person N");
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM face_rejections WHERE face_id = '{f}' AND person_id = '{p}'")
        )
        .await,
        1
    );
    post(&app, &key, &path, json!({"person_id": p})).await;
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM face_rejections").await,
        1,
        "not N now; no longer not-A"
    );
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM face_rejections WHERE person_id = '{n}'")
        )
        .await,
        1
    );
    // Someone else's person, face or an unknown id: 404, and nothing was made.
    let theirs = person(&t.pool, other, Some("Person B")).await;
    assert_eq!(
        post(&app, &key, &path, json!({"person_id": theirs})).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(&app, &ko, &path, json!({"new": true})).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(
            &app,
            &key,
            &format!("/assets/faces/{}/assign", Uuid::new_v4()),
            json!({"new": true})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM people WHERE owner_id = '{other}'")
        )
        .await,
        1
    );
    // A face that was scanned before a rotate waits unread: it cannot be assigned until its photo is scanned again.
    sqlx::query("DELETE FROM face_scans WHERE asset_id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        post(&app, &key, &path, json!({"new": true})).await.0,
        StatusCode::NOT_FOUND
    );
    t.teardown().await;
}

#[tokio::test]
async fn suggestions_list_faces_that_look_like_the_person_and_a_refused_one_does_not_come_back() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (_, ko) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let ann = identity(1);
    let p = person(&t.pool, uid, Some("Person A")).await;
    for i in 0..3 {
        let a = photo(&t.pool, uid, &format!("k{i}.jpg")).await;
        let f = face(&t.pool, a, Some(shot(&ann, i)), [0.1, 0.1, 0.2, 0.2]).await;
        put(&t.pool, f, p).await;
    }
    let near_a = photo(&t.pool, uid, "near.jpg").await;
    let near = face(&t.pool, near_a, Some(shot(&ann, 50)), [0.2, 0.3, 0.1, 0.1]).await;
    let far_a = photo(&t.pool, uid, "far.jpg").await;
    face(&t.pool, far_a, Some(identity(9)), [0.2, 0.3, 0.1, 0.1]).await;
    let none_a = photo(&t.pool, uid, "none.jpg").await;
    face(&t.pool, none_a, None, [0.2, 0.3, 0.05, 0.05]).await;
    let path = format!("/assets/people/{p}/suggestions");
    let (st, v) = get(&app, &key, &path).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let s = &v["items"][0];
    assert_eq!(
        (s["face_id"].as_str(), s["asset_id"].as_str()),
        (Some(near.to_string().as_str()), Some(near_a.to_string().as_str()))
    );
    assert_box(&s["box"], [0.2, 0.3, 0.1, 0.1]);
    assert!(s["preview_url"].as_str().unwrap().contains("preview.webp") && s["similarity"].as_f64().unwrap() > 0.6);
    assert_eq!(get(&app, &ko, &path).await.0, StatusCode::NOT_FOUND);
    // "No": it is not suggested again, and the worker would not take it either.
    let reject = format!("/assets/faces/{near}/reject");
    assert_eq!(
        post(&app, &key, &reject, json!({"person_id": p})).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post(&app, &key, &reject, json!({"person_id": p})).await.0,
        StatusCode::NO_CONTENT,
        "idempotent"
    );
    assert!(get(&app, &key, &path).await.1["items"].as_array().unwrap().is_empty());
    assert_eq!(
        post(&app, &ko, &reject, json!({"person_id": p})).await.0,
        StatusCode::NOT_FOUND
    );
    // "Yes" on a fresh one: it joins and leaves the list.
    let more_a = photo(&t.pool, uid, "more.jpg").await;
    let more = face(&t.pool, more_a, Some(shot(&ann, 51)), [0.2, 0.3, 0.1, 0.1]).await;
    assert_eq!(get(&app, &key, &path).await.1["items"].as_array().unwrap().len(), 1);
    post(
        &app,
        &key,
        &format!("/assets/faces/{more}/assign"),
        json!({"person_id": p}),
    )
    .await;
    assert!(get(&app, &key, &path).await.1["items"].as_array().unwrap().is_empty());
    // A face that is in the person cannot be "rejected" from them: take it out instead.
    assert_eq!(
        post(
            &app,
            &key,
            &format!("/assets/faces/{more}/reject"),
            json!({"person_id": p})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    t.teardown().await;
}

// ---- merge and delete -----------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn merging_moves_every_face_keeps_a_name_and_the_audit_trail_has_no_names() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app = app_with_people(&t.pool);
    let (named, unnamed, other) = (
        person(&t.pool, uid, Some("Person Secret")).await,
        person(&t.pool, uid, None).await,
        person(&t.pool, uid, Some("Person O")).await,
    );
    let mut fs = Vec::new();
    for (i, who) in [named, named, unnamed, other].into_iter().enumerate() {
        let a = photo(&t.pool, uid, &format!("m{i}.jpg")).await;
        let f = face(&t.pool, a, Some(identity(1)), [0.1, 0.1, 0.2, 0.2]).await;
        put(&t.pool, f, who).await;
        fs.push(f);
    }
    // A face rejected from the first person and one that is the second's by now.
    let loose_a = photo(&t.pool, uid, "loose.jpg").await;
    let loose = face(&t.pool, loose_a, Some(identity(2)), [0.1, 0.1, 0.2, 0.2]).await;
    sqlx::query("INSERT INTO face_rejections (face_id, person_id) VALUES ($1, $2), ($3, $2), ($3, $4)")
        .bind(loose)
        .bind(named)
        .bind(fs[2])
        .bind(unnamed)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        post(
            &app,
            &key,
            &format!("/assets/people/{named}/merge"),
            json!({"into": named})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(
            &app,
            &key,
            &format!("/assets/people/{named}/merge"),
            json!({"into": Uuid::new_v4()})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // The unnamed one takes the name of the named one it is merged with; here the named merges into the unnamed.
    let (st, v) = post(
        &app,
        &key,
        &format!("/assets/people/{named}/merge"),
        json!({"into": unnamed}),
    )
    .await;
    assert_eq!(
        (st, v["into"].as_str(), v["faces"].as_i64()),
        (StatusCode::OK, Some(unnamed.to_string().as_str()), Some(2))
    );
    let (_, one) = get(&app, &key, &format!("/assets/people/{unnamed}")).await;
    assert_eq!(
        (one["name"].as_str(), one["photos"].as_i64()),
        (Some("Person Secret"), Some(3))
    );
    assert_eq!(
        get(&app, &key, &format!("/assets/people/{named}")).await.0,
        StatusCode::NOT_FOUND
    );
    // "Not the first" became "not the second" for the loose face; the face now in the second person lost its own rejection.
    let rej: Vec<(Uuid, Uuid)> = sqlx::query_as("SELECT face_id, person_id FROM face_rejections")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(rej, vec![(loose, unnamed)]);
    let audit: Vec<(String, Value)> =
        sqlx::query_as("SELECT action, detail FROM audit_log WHERE action LIKE 'people.%'")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].0, "people.merge");
    assert!(!audit[0].1.to_string().contains("Person"), "{}", audit[0].1);

    // Deleting a person frees its faces and drops what was decided about it.
    let (st, _) = send(&app, "DELETE", &format!("/assets/people/{unnamed}"), &key, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM faces WHERE person_id IS NOT NULL").await,
        1
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_rejections").await, 0);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM faces").await,
        5,
        "faces are not deleted with their person"
    );
    assert_eq!(
        send(&app, "DELETE", &format!("/assets/people/{unnamed}"), &key, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    t.teardown().await;
}

// ---- a photo's faces ------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_photos_faces_name_their_people_and_leave_out_hidden_people_and_other_peoples_photos() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (other, ko) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let a = photo(&t.pool, uid, "a.jpg").await;
    let fs: Vec<NewFace> = (0..3)
        .map(|i| NewFace {
            bbox: [0.1 * i as f32, 0.2, 0.1, 0.1],
            landmarks: [0.1; 10],
            score: 0.9,
            embedding: Some(identity(i + 1)),
        })
        .collect();
    faces::store(&t.pool, a, MODEL, VERSION, &fs).await.unwrap();
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM faces WHERE asset_id = $1 ORDER BY idx")
        .bind(a)
        .fetch_all(&t.pool)
        .await
        .unwrap();
    let (named, shy) = (
        person(&t.pool, uid, Some("Person A")).await,
        person(&t.pool, uid, Some("Person H")).await,
    );
    put(&t.pool, ids[0], named).await;
    put(&t.pool, ids[1], shy).await;
    sqlx::query("UPDATE people SET hidden = true WHERE id = $1")
        .bind(shy)
        .execute(&t.pool)
        .await
        .unwrap();
    let (st, v) = get(&app, &key, &format!("/assets/{a}/faces")).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "the hidden person's face is left out");
    assert_eq!(items[0]["id"], ids[0].to_string());
    assert_eq!(items[0]["person"], json!({"id": named, "name": "Person A"}));
    assert_box(&items[0]["box"], [0.0, 0.2, 0.1, 0.1]);
    assert_eq!(items[1]["id"], ids[2].to_string());
    assert!(items[1]["person"].is_null());
    assert!(!v.to_string().contains("Person H"));
    assert_eq!(
        get(&app, &ko, &format!("/assets/{a}/faces")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &key, &format!("/assets/{}/faces", Uuid::new_v4())).await.0,
        StatusCode::NOT_FOUND
    );
    // A trashed photo and a photo of nobody's: no faces.
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        get(&app, &key, &format!("/assets/{a}/faces")).await.0,
        StatusCode::NOT_FOUND
    );
    let theirs = photo(&t.pool, other, "t.jpg").await;
    face(&t.pool, theirs, Some(identity(7)), [0.1, 0.1, 0.1, 0.1]).await;
    assert_eq!(
        get(&app, &key, &format!("/assets/{theirs}/faces")).await.0,
        StatusCode::NOT_FOUND
    );
    t.teardown().await;
}

// ---- not a face -----------------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_detection_marked_not_a_face_leaves_its_person_the_counts_the_chips_and_the_suggestions_and_can_be_brought_back()
 {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (other, ko) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let alpha = identity(31);
    let p = person(&t.pool, uid, Some("Person A")).await;
    let unnamed = person(&t.pool, uid, None).await;
    let mut mine = Vec::new();
    for i in 0..3 {
        let a = photo(&t.pool, uid, &format!("p{i}.jpg")).await;
        let f = face(&t.pool, a, Some(shot(&alpha, 10 + i)), [0.1, 0.1, 0.2, 0.2]).await;
        put(&t.pool, f, p).await;
        mine.push((a, f));
    }
    // A lone face in an unnamed person, chosen as its cover.
    let lone_a = photo(&t.pool, uid, "lone.jpg").await;
    let lone = face(&t.pool, lone_a, Some(identity(40)), [0.2, 0.2, 0.2, 0.2]).await;
    put(&t.pool, lone, unnamed).await;
    sqlx::query("UPDATE people SET cover_face_id = $2 WHERE id = $1")
        .bind(unnamed)
        .bind(lone)
        .execute(&t.pool)
        .await
        .unwrap();
    // An unclaimed face that looks like Person A, to be suggested until it is marked.
    let near_a = photo(&t.pool, uid, "near.jpg").await;
    let near = face(&t.pool, near_a, Some(shot(&alpha, 99)), [0.3, 0.3, 0.2, 0.2]).await;
    let theirs_a = photo(&t.pool, other, "theirs.jpg").await;
    let theirs = face(&t.pool, theirs_a, Some(identity(41)), [0.1, 0.1, 0.2, 0.2]).await;

    let counts = |v: &Value| -> Vec<(String, i64, i64)> { listed(v) };
    let (_, v) = get(&app, &key, "/assets/people").await;
    assert_eq!(counts(&v), vec![("Person A".into(), 3, 3), ("".into(), 1, 1)]);
    let (_, s) = get(&app, &key, &format!("/assets/people/{p}/suggestions")).await;
    assert_eq!(item_ids_of(&s, "face_id"), vec![near]);
    assert_eq!(pending_of(&v, p), 1, "the badge counts the suggestion");

    // Marking a face of Person A: it leaves the person without a rejection, and every count follows.
    let (st, v) = post(
        &app,
        &key,
        &format!("/assets/faces/{}/not-a-face", mine[0].1),
        Value::Null,
    )
    .await;
    assert_eq!((st, v["person_id"].clone()), (StatusCode::OK, json!(p)));
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_rejections").await, 0);
    let (_, v) = get(&app, &key, "/assets/people").await;
    assert_eq!(counts(&v), vec![("Person A".into(), 2, 2), ("".into(), 1, 1)]);
    let (_, ph) = get(&app, &key, &format!("/assets/people/{p}/photos")).await;
    assert_eq!(ph["items"].as_array().unwrap().len(), 2);
    let (_, faces_of) = get(&app, &key, &format!("/assets/{}/faces", mine[0].0)).await;
    assert_eq!(faces_of["items"], json!([]), "not a chip, not a region");
    assert_eq!(item_ids_of(&faces_of, "id"), Vec::<Uuid>::new());
    assert_eq!(faces_of["dismissed"].as_array().unwrap().len(), 1);
    assert_eq!(faces_of["dismissed"][0]["id"], mine[0].1.to_string());
    assert_box(&faces_of["dismissed"][0]["box"], [0.1, 0.1, 0.2, 0.2]);

    // An unclaimed face: marked, no longer suggested, and the owner's People list has nothing new.
    let (st, v) = post(&app, &key, &format!("/assets/faces/{near}/not-a-face"), Value::Null).await;
    assert_eq!((st, v["person_id"].clone()), (StatusCode::OK, Value::Null));
    let (_, s) = get(&app, &key, &format!("/assets/people/{p}/suggestions")).await;
    assert!(item_ids_of(&s, "face_id").is_empty());
    assert_eq!(
        pending_of(&get(&app, &key, "/assets/people").await.1, p),
        0,
        "and a marked detection is not pending"
    );

    // The cover of an unnamed person, marked: the unnamed person goes with its last face.
    post(&app, &key, &format!("/assets/faces/{lone}/not-a-face"), Value::Null).await;
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM people WHERE name IS NULL").await,
        0
    );

    // Marking again is harmless; an assigned-by-the-worker run never gives a marked face a person.
    assert_eq!(
        post(&app, &key, &format!("/assets/faces/{near}/not-a-face"), Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    cyanotype_core::people::run(&t.pool, uid).await.unwrap();
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM faces WHERE id = '{near}' AND person_id IS NULL AND not_face")
        )
        .await,
        1
    );

    // Someone else's face, and a face that does not exist, are not found; nothing of theirs changed.
    for f in [theirs, Uuid::new_v4()] {
        assert_eq!(
            post(&app, &key, &format!("/assets/faces/{f}/not-a-face"), Value::Null)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(&app, "DELETE", &format!("/assets/faces/{f}/not-a-face"), &key, None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        post(&app, &ko, &format!("/assets/faces/{near}/not-a-face"), Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM faces WHERE id = '{theirs}' AND NOT not_face")
        )
        .await,
        1
    );

    // Undo: brought back, it is an ordinary unassigned face (suggested again), and can be put in its person with assign.
    assert_eq!(
        send(
            &app,
            "DELETE",
            &format!("/assets/faces/{}/not-a-face", mine[0].1),
            &key,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, faces_of) = get(&app, &key, &format!("/assets/{}/faces", mine[0].0)).await;
    assert_eq!(item_ids_of(&faces_of, "id"), vec![mine[0].1]);
    assert!(faces_of["items"][0]["person"].is_null());
    assert_eq!(faces_of["dismissed"], json!([]));
    let (st, _) = post(
        &app,
        &key,
        &format!("/assets/faces/{}/assign", mine[0].1),
        json!({"person_id": p}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, v) = get(&app, &key, "/assets/people").await;
    assert_eq!(counts(&v), vec![("Person A".into(), 3, 3)]);

    // Assigning a marked face is a decision too: the mark goes (no constraint error).
    let (st, _) = post(
        &app,
        &key,
        &format!("/assets/faces/{near}/assign"),
        json!({"person_id": p}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM faces WHERE not_face AND id <> '{lone}'")
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &t.pool,
            &format!("SELECT count(*) FROM faces WHERE id = '{near}' AND person_id = '{p}'")
        )
        .await,
        1
    );
    t.teardown().await;
}

/// The `key` of each item, as uuids.
fn item_ids_of(v: &Value, key: &str) -> Vec<Uuid> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i[key].as_str().unwrap().parse().unwrap())
        .collect()
}

// ---- never shared ---------------------------------------------------------------------------------------------------------------

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
async fn a_share_link_carries_nothing_of_the_people_in_the_photos_it_shows() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let app =
        routes_with(ShareState::new(false, vec![]).with_limits((10_000, 10_000), (10_000, 10_000), (10_000, 10_000)))
            .with_state(AppState {
                db: t.pool.clone(),
                storage: dummy_storage(),
                trash_retention_days: 30,
                purge_wake: Default::default(),
                text: std::sync::Arc::new(crate::testutil::FakeText::default()),
                search: crate::testutil::default_search(),
                faces_enabled: true,
                pets_enabled: false,
            });
    let a = photo(&t.pool, uid, "a.jpg").await;
    let f = face(&t.pool, a, Some(identity(1)), [0.1, 0.1, 0.2, 0.2]).await;
    let p = person(&t.pool, uid, Some("Person Visible")).await;
    put(&t.pool, f, p).await;
    let (st, share) = post(
        &app,
        &key,
        "/assets/shares",
        json!({"asset_ids": [a], "allow_download": true}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{share}");
    let token = share["token"].as_str().unwrap();
    let mut bodies = vec![share.to_string()];
    for path in [
        format!("/assets/public/s/{token}"),
        format!("/assets/public/s/{token}/items"),
        format!("/assets/public/s/{token}/items/{a}"),
    ] {
        let (st, body) = visit(&app, &path).await;
        assert_eq!(st, StatusCode::OK, "{path}: {body}");
        bodies.push(body);
    }
    for body in bodies {
        for needle in ["Person Visible", "people", "face", &p.to_string(), &f.to_string()] {
            assert!(!body.contains(needle), "{needle:?} leaked into: {body}");
        }
    }
    // The visitor cannot reach the owner's routes.
    assert_eq!(visit(&app, "/assets/people").await.0, StatusCode::UNAUTHORIZED);
    t.teardown().await;
}

// ---- People 3: the person's timeline, the {person} scope, saving as an album, the search filter, smart albums --------------------

/// The whole app (share links included), with People `faces`.
fn full_app(pool: &PgPool, faces: bool) -> Router {
    routes_with(ShareState::new(false, vec![]).with_limits((10_000, 10_000), (10_000, 10_000), (10_000, 10_000)))
        .with_state(AppState {
            db: pool.clone(),
            storage: dummy_storage(),
            trash_retention_days: 30,
            purge_wake: Default::default(),
            text: std::sync::Arc::new(crate::testutil::FakeText::default()),
            search: crate::testutil::default_search(),
            faces_enabled: faces,
            pets_enabled: false,
        })
}

/// A photo taken on `day` (`YYYY-MM-DD`, noon UTC) with one face of `who` in `person_id`.
async fn shot_of(
    pool: &PgPool,
    owner: Uuid,
    name: &str,
    day: &str,
    person_id: Uuid,
    who: &[f32],
    seed: u64,
) -> (Uuid, Uuid) {
    let a = photo(pool, owner, name).await;
    sqlx::query("UPDATE assets SET taken_at = ($2 || ' 12:00:00+00')::timestamptz WHERE id = $1")
        .bind(a)
        .bind(day)
        .execute(pool)
        .await
        .unwrap();
    let f = face(pool, a, Some(shot(who, seed)), [0.1, 0.1, 0.2, 0.2]).await;
    put(pool, f, person_id).await;
    (a, f)
}

/// Person A in three photos (2022-05-01, 2023-06-01, 2023-06-02), Person B in the last two of them and in one more (2023-07-01).
struct Scene {
    owner: Uuid,
    key: String,
    a: Uuid,
    b: Uuid,
    a_photos: Vec<Uuid>,
    b_photos: Vec<Uuid>,
}

async fn scene(pool: &PgPool, email: &str) -> Scene {
    let (owner, key) = user(pool, email).await;
    let a = person(pool, owner, Some("Person A")).await;
    let b = person(pool, owner, Some("Person B")).await;
    let (ia, ib) = (identity(11), identity(12));
    let (a1, _) = shot_of(pool, owner, "a1.jpg", "2022-05-01", a, &ia, 1).await;
    let (a2, _) = shot_of(pool, owner, "a2.jpg", "2023-06-01", a, &ia, 2).await;
    let (a3, _) = shot_of(pool, owner, "a3.jpg", "2023-06-02", a, &ia, 3).await;
    // Person B is in a2 and a3 too (a second face in the same photos) and has a photo of their own.
    let mut b_photos = Vec::new();
    for (asset, seed) in [(a2, 4), (a3, 5)] {
        sqlx::query("DELETE FROM face_scans WHERE asset_id = $1")
            .bind(asset)
            .execute(pool)
            .await
            .unwrap();
        let both = [
            NewFace {
                bbox: [0.1, 0.1, 0.2, 0.2],
                landmarks: [0.1; 10],
                score: 0.9,
                embedding: Some(shot(&ia, seed)),
            },
            NewFace {
                bbox: [0.5, 0.5, 0.2, 0.2],
                landmarks: [0.1; 10],
                score: 0.9,
                embedding: Some(shot(&ib, seed)),
            },
        ];
        assert!(faces::store(pool, asset, MODEL, VERSION, &both).await.unwrap());
        sqlx::query("UPDATE faces SET person_id = CASE WHEN box_x < 0.3 THEN $2 ELSE $3 END WHERE asset_id = $1")
            .bind(asset)
            .bind(a)
            .bind(b)
            .execute(pool)
            .await
            .unwrap();
        b_photos.push(asset);
    }
    let (b3, _) = shot_of(pool, owner, "b3.jpg", "2023-07-01", b, &ib, 6).await;
    b_photos.push(b3);
    Scene {
        owner,
        key,
        a,
        b,
        a_photos: vec![a1, a2, a3],
        b_photos,
    }
}

fn sorted(mut v: Vec<Uuid>) -> Vec<Uuid> {
    v.sort_unstable();
    v
}

#[tokio::test]
async fn a_person_has_a_timeline_that_counts_what_the_photos_list_returns() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool, "a@example.com").await;
    let app = app_with_people(&t.pool);
    let (st, days) = get(&app, &s.key, &format!("/assets/people/{}/timeline", s.a)).await;
    assert_eq!(st, StatusCode::OK, "{days}");
    assert_eq!(
        days,
        json!({"total": 3, "days": [
            {"day": "2023-06-02", "count": 1}, {"day": "2023-06-01", "count": 1}, {"day": "2022-05-01", "count": 1}]})
    );
    // The list pages the same photos, from any day (`before`) or by cursor, newest first.
    let (_, all) = get(&app, &s.key, &format!("/assets/people/{}/photos", s.a)).await;
    assert_eq!(item_ids(&all), vec![s.a_photos[2], s.a_photos[1], s.a_photos[0]]);
    let (_, older) = get(
        &app,
        &s.key,
        &format!("/assets/people/{}/photos?before=2023-06-02T12:00:00Z", s.a),
    )
    .await;
    assert_eq!(item_ids(&older), vec![s.a_photos[1], s.a_photos[0]]);
    // Trashed photos, and hidden ones for a locked session, are in neither.
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(s.a_photos[0])
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(s.a_photos[2])
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, days) = get(&app, &s.key, &format!("/assets/people/{}/timeline", s.a)).await;
    assert_eq!(days["total"], 1);
    // Archived photos stay (the person's page is not the timeline).
    sqlx::query("UPDATE assets SET archived = true WHERE id = $1")
        .bind(s.a_photos[1])
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, days) = get(&app, &s.key, &format!("/assets/people/{}/timeline", s.a)).await;
    assert_eq!(days, json!({"total": 1, "days": [{"day": "2023-06-01", "count": 1}]}));
    // Someone else's person, or none, is a 404.
    let (_, other) = user(&t.pool, "b@example.com").await;
    for who in [Uuid::new_v4().to_string(), s.a.to_string()] {
        assert_eq!(
            get(&app, &other, &format!("/assets/people/{who}/timeline")).await.0,
            StatusCode::NOT_FOUND
        );
    }
    t.teardown().await;
}

#[tokio::test]
async fn the_person_scope_works_for_the_bulk_actions_and_belongs_to_its_owner() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool, "a@example.com").await;
    let (_, other) = user(&t.pool, "b@example.com").await;
    let app = full_app(&t.pool, true);
    // Favorite everything of Person A except one photo, without listing ids.
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/favorite",
        json!({"scope": {"person": s.a}, "except": [s.a_photos[0]], "favorite": true}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(2)), "{v}");
    let favs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM assets WHERE favorite")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(sorted(favs), sorted(vec![s.a_photos[1], s.a_photos[2]]));
    // Rate, tag, archive and hide take the same scope.
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/rating",
        json!({"scope": {"person": s.a}, "except": [], "rating": 4}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(3)), "{v}");
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/tags/apply",
        json!({"scope": {"person": s.a}, "except": [], "add": ["Trip"]}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM asset_tags").await,
        3,
        "the tag went on all of Person A's photos"
    );
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/archive",
        json!({"scope": {"person": s.a}, "except": [], "archived": true}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(3)), "{v}");
    // Archived photos are still the person's (the page keeps them, flagged); hiding takes them out of a locked session's page.
    let (_, days) = get(&app, &s.key, &format!("/assets/people/{}/timeline", s.a)).await;
    assert_eq!(days["total"], 3);
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/hide",
        json!({"scope": {"person": s.a}, "except": [], "hidden": true}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(3)), "{v}");
    // (A person only hidden photos have is not for a locked session to see at all.)
    let (st, _) = get(&app, &s.key, &format!("/assets/people/{}/timeline", s.a)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    sqlx::query("UPDATE assets SET hidden = false, archived = false")
        .execute(&t.pool)
        .await
        .unwrap();
    // Trash, and Undo through the batch.
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/trash",
        json!({"scope": {"person": s.b}, "except": [s.b_photos[0]]}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(2)), "{v}");
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/restore",
        json!({"scope": {"trash_batch": v["batch"]}, "except": []}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(2)), "{v}");

    // Another user's person, and one that never existed, are the same 404, on every verb that takes a scope.
    for who in [Uuid::new_v4(), s.a] {
        for (path, body) in [
            ("/assets/favorite", json!({"favorite": true})),
            ("/assets/rating", json!({"rating": 3})),
            ("/assets/archive", json!({"archived": true})),
            ("/assets/hide", json!({"hidden": true})),
            ("/assets/trash", json!({})),
        ] {
            let mut b = body.clone();
            b["scope"] = json!({"person": who});
            b["except"] = json!([]);
            let (st, v) = post(&app, &other, path, b).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{path} with {who}: {v}");
        }
    }
    assert_eq!(
        count(
            &t.pool,
            "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL OR archived OR hidden"
        )
        .await,
        0
    );
    t.teardown().await;
}

#[tokio::test]
async fn saving_a_person_as_an_album_adds_the_photos_from_the_scope_and_the_filtered_subset() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool, "a@example.com").await;
    let app = full_app(&t.pool, true);
    // The album dialog's two steps: create it empty, then add by scope (no ids from the browser).
    let (st, album) = post(&app, &s.key, "/assets/albums", json!({"title": "Holiday"})).await;
    assert_eq!(st, StatusCode::CREATED, "{album}");
    let id: Uuid = album["id"].as_str().unwrap().parse().unwrap();
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/albums/{id}/assets"),
        json!({"scope": {"person": s.a}, "except": []}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(3)), "{v}");
    // A snapshot: an ordinary album that no longer follows the person.
    let (_, listed) = get(&app, &s.key, &format!("/assets/albums/{id}/assets")).await;
    assert_eq!(sorted(item_ids(&listed)), sorted(s.a_photos.clone()));
    sqlx::query("DELETE FROM people WHERE id = $1")
        .bind(s.a)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, listed) = get(&app, &s.key, &format!("/assets/albums/{id}/assets")).await;
    assert_eq!(item_ids(&listed).len(), 3);

    // The filtered subset: Person B in 2023, through the search scope.
    let (_, album) = post(&app, &s.key, "/assets/albums", json!({"title": "B in 2023"})).await;
    let id: Uuid = album["id"].as_str().unwrap().parse().unwrap();
    let (st, v) = post(
        &app,
        &s.key,
        &format!("/assets/albums/{id}/assets"),
        json!({"scope": {"search": {"people": s.b.to_string(), "from": "2023-01-01", "to": "2023-12-31"}}, "except": []}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(3)), "{v}");
    t.teardown().await;
}

#[tokio::test]
async fn an_album_saved_from_a_person_and_shared_leaks_nothing_of_the_people() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool, "a@example.com").await;
    let app = full_app(&t.pool, true);
    let (_, album) = post(&app, &s.key, "/assets/albums", json!({"title": "Holiday"})).await;
    let id: Uuid = album["id"].as_str().unwrap().parse().unwrap();
    let (st, _) = post(
        &app,
        &s.key,
        &format!("/assets/albums/{id}/assets"),
        json!({"scope": {"person": s.a}, "except": []}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, share) = post(
        &app,
        &s.key,
        "/assets/shares",
        json!({"album_id": id, "allow_download": true}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{share}");
    let token = share["token"].as_str().unwrap();
    let a1 = s.a_photos[0];
    let mut bodies = vec![share.to_string()];
    for path in [
        format!("/assets/public/s/{token}"),
        format!("/assets/public/s/{token}/items"),
        format!("/assets/public/s/{token}/items/{a1}"),
    ] {
        let (st, body) = visit(&app, &path).await;
        assert_eq!(st, StatusCode::OK, "{path}: {body}");
        bodies.push(body);
    }
    let face_ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM faces")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    let mut needles = vec![
        "Person A".to_string(),
        "Person B".into(),
        "people".into(),
        "face".into(),
        "person".into(),
    ];
    needles.extend([s.a.to_string(), s.b.to_string()]);
    needles.extend(face_ids.iter().map(Uuid::to_string));
    for body in &bodies {
        for needle in &needles {
            assert!(!body.contains(needle.as_str()), "{needle:?} leaked into: {body}");
        }
    }
    // The owner's album and share lists do not name people either, and neither does the recovery manifest.
    for path in [format!("/assets/albums/{id}"), "/assets/shares".to_string()] {
        let (_, v) = get(&app, &s.key, &path).await;
        for needle in &needles {
            assert!(!v.to_string().contains(needle.as_str()), "{needle:?} in {path}: {v}");
        }
    }
    let (bytes, _) = cyanotype_core::manifest::build(&t.pool, s.owner).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for needle in &needles {
        assert!(!text.contains(needle.as_str()), "{needle:?} in the manifest");
    }
    t.teardown().await;
}

#[tokio::test]
async fn the_people_filter_finds_photos_where_all_of_them_are_together() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool, "a@example.com").await;
    let (bob, kb) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let ids = |v: &Value| sorted(item_ids(v));
    let (st, one) = get(&app, &s.key, &format!("/assets/search?people={}", s.a)).await;
    assert_eq!(st, StatusCode::OK, "{one}");
    assert_eq!(ids(&one), sorted(s.a_photos.clone()));
    assert_eq!(one["total"], 3);
    // Together: only the two photos both are in.
    let (_, both) = get(&app, &s.key, &format!("/assets/search?people={},{}", s.a, s.b)).await;
    assert_eq!(ids(&both), sorted(vec![s.a_photos[1], s.a_photos[2]]));
    // Combined with the other filters.
    let (_, y) = get(
        &app,
        &s.key,
        &format!("/assets/search?people={}&from=2023-01-01&to=2023-12-31", s.a),
    )
    .await;
    assert_eq!(ids(&y), sorted(vec![s.a_photos[1], s.a_photos[2]]));
    let (_, f) = get(&app, &s.key, &format!("/assets/search?people={}&favorite=true", s.a)).await;
    assert!(item_ids(&f).is_empty());
    // Not the trash, not hidden photos.
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(s.a_photos[1])
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(s.a_photos[2])
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, both) = get(&app, &s.key, &format!("/assets/search?people={},{}", s.a, s.b)).await;
    assert!(item_ids(&both).is_empty());
    // Another user's person id behaves like one that never existed: nothing matches, and the answers are the same.
    let theirs = person(&t.pool, bob, Some("Person C")).await;
    let ghost = Uuid::new_v4();
    for who in [theirs, ghost, s.a] {
        let (st, v) = get(&app, &kb, &format!("/assets/search?people={who}")).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert!(item_ids(&v).is_empty(), "{who}: {v}");
    }
    let (_, v1) = get(&app, &kb, &format!("/assets/search?people={ghost}")).await;
    let (_, v2) = get(&app, &kb, &format!("/assets/search?people={}", s.a)).await;
    assert_eq!(v1, v2);
    // Malformed and too many.
    for bad in [
        "not-an-id".to_string(),
        (0..11)
            .map(|_| Uuid::new_v4().to_string())
            .collect::<Vec<_>>()
            .join(","),
    ] {
        assert_eq!(
            get(&app, &s.key, &format!("/assets/search?people={bad}")).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    // The same id twice is one person; an empty value is no filter.
    let (_, dup) = get(&app, &s.key, &format!("/assets/search?people={0},{0}", s.b)).await;
    assert_eq!(
        item_ids(&dup).len(),
        1,
        "Person B has one photo left that is not in the trash or hidden"
    );
    let (_, none) = get(&app, &s.key, "/assets/search?people=").await;
    assert!(none["total"].as_i64().unwrap() > 0);
    t.teardown().await;
}

#[tokio::test]
async fn a_query_that_names_people_becomes_the_filter_and_a_chip() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool, "a@example.com").await;
    let (bob, kb) = user(&t.pool, "b@example.com").await;
    person(&t.pool, bob, Some("Person C")).await;
    let app = app_with_people(&t.pool);
    let q = |text: &str| format!("/assets/search?q={}", text.replace(' ', "%20"));
    let (st, v) = get(&app, &s.key, &q("Person A and Person B")).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(sorted(item_ids(&v)), sorted(vec![s.a_photos[1], s.a_photos[2]]));
    assert_eq!(v["ranked"], false, "nothing is left to rank: a filter-only search");
    let chip = &v["parsed"]["chips"][0];
    assert_eq!(chip["kind"], "people");
    assert_eq!(chip["label"], "Person A and Person B");
    assert_eq!(
        chip["people"],
        json!([{"id": s.a, "name": "Person A"}, {"id": s.b, "name": "Person B"}])
    );
    // With a date: "Person A 2023".
    let (_, v) = get(&app, &s.key, &q("Person A 2023")).await;
    assert_eq!(sorted(item_ids(&v)), sorted(vec![s.a_photos[1], s.a_photos[2]]));
    let kinds: Vec<&str> = v["parsed"]["chips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["date", "people"]);
    // Names that are not people's: not hidden ones, not unnamed ones, not someone else's, not a person without a photo to find.
    let no_chip = |v: &Value| {
        v["parsed"]["chips"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["kind"] != "people")
    };
    sqlx::query("UPDATE people SET hidden = true WHERE id = $1")
        .bind(s.b)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, v) = get(&app, &s.key, &q("Person B 2023")).await;
    assert!(no_chip(&v), "{v}");
    sqlx::query("UPDATE people SET hidden = false, name = NULL WHERE id = $1")
        .bind(s.b)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, v) = get(&app, &s.key, &q("Person B 2023")).await;
    assert!(no_chip(&v), "{v}");
    let (_, v) = get(&app, &kb, &q("Person A 2023")).await;
    assert!(no_chip(&v), "{v}");
    let (_, v) = get(&app, &kb, &q("Person C 2023")).await;
    assert!(no_chip(&v), "no photos, no match: {v}");
    t.teardown().await;
}

#[tokio::test]
async fn smart_albums_can_follow_a_person_or_several_together() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool, "a@example.com").await;
    let (_, other) = user(&t.pool, "b@example.com").await;
    let app = full_app(&t.pool, true);
    let make = |rules: Value| post(&app, &s.key, "/assets/albums", json!({"title": "Live", "rules": rules}));
    let (st, album) = make(json!({"rules": [{"field": "people", "ids": [s.a]}]})).await;
    assert_eq!(st, StatusCode::CREATED, "{album}");
    assert_eq!(album["photo_count"], 3);
    let id: Uuid = album["id"].as_str().unwrap().parse().unwrap();
    let (_, listed) = get(&app, &s.key, &format!("/assets/albums/{id}/assets")).await;
    assert_eq!(sorted(item_ids(&listed)), sorted(s.a_photos.clone()));
    // Live: a new photo of Person A appears with nothing stored.
    let (new, _) = shot_of(&t.pool, s.owner, "a4.jpg", "2024-01-01", s.a, &identity(11), 9).await;
    let (_, listed) = get(&app, &s.key, &format!("/assets/albums/{id}/assets")).await;
    assert!(item_ids(&listed).contains(&new));
    // Together, combined with another rule, and as a bulk scope.
    let (st, together) =
        make(json!({"rules": [{"field": "people", "ids": [s.a, s.b]}, {"field": "date", "from": "2023-06-02"}]})).await;
    assert_eq!(st, StatusCode::CREATED, "{together}");
    let tid: Uuid = together["id"].as_str().unwrap().parse().unwrap();
    let (_, listed) = get(&app, &s.key, &format!("/assets/albums/{tid}/assets")).await;
    assert_eq!(item_ids(&listed), vec![s.a_photos[2]]);
    let (st, v) = post(
        &app,
        &s.key,
        "/assets/favorite",
        json!({"scope": {"album": tid}, "except": [], "favorite": true}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(1)), "{v}");
    // The rules are checked: another owner's person and one that never existed are the same 404; none is a 400.
    let ghost = Uuid::new_v4();
    for who in [ghost, s.a] {
        let (st, v) = post(
            &app,
            &other,
            "/assets/albums",
            json!({"title": "x", "rules": {"rules": [{"field": "people", "ids": [who]}]}}),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{who}: {v}");
        let (st, _) = post(
            &app,
            &other,
            "/assets/albums/preview",
            json!({"rules": {"rules": [{"field": "people", "ids": [who]}]}}),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "preview {who}");
    }
    let (st, _) = post(
        &app,
        &s.key,
        "/assets/albums",
        json!({"title": "x", "rules": {"rules": [{"field": "people", "ids": []}]}}),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // The public link resolves the rules and never names the person; the manifest leaves the rules out.
    let (st, share) = post(&app, &s.key, "/assets/shares", json!({"album_id": id})).await;
    assert_eq!(st, StatusCode::CREATED, "{share}");
    let token = share["token"].as_str().unwrap();
    for path in [
        format!("/assets/public/s/{token}"),
        format!("/assets/public/s/{token}/items"),
    ] {
        let (st, body) = visit(&app, &path).await;
        assert_eq!(st, StatusCode::OK, "{path}");
        for needle in [s.a.to_string(), s.b.to_string(), "Person A".into(), "people".into()] {
            assert!(!body.contains(&needle), "{needle:?} in {body}");
        }
    }
    let (_, items) = visit(&app, &format!("/assets/public/s/{token}/items")).await;
    assert!(
        items.contains(&new.to_string()),
        "the link shows what the rules match now"
    );
    let (bytes, _) = cyanotype_core::manifest::build(&t.pool, s.owner).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains(&s.a.to_string()) && !text.contains("\"people\""),
        "{text}"
    );
    // Deleting the person leaves a rule that matches nothing (not "everything").
    sqlx::query("DELETE FROM people WHERE id = $1")
        .bind(s.a)
        .execute(&t.pool)
        .await
        .unwrap();
    let (_, listed) = get(&app, &s.key, &format!("/assets/albums/{id}/assets")).await;
    assert!(item_ids(&listed).is_empty());
    t.teardown().await;
}

#[tokio::test]
async fn with_the_feature_off_the_people_parameters_are_ignored_or_refused_and_nothing_is_parsed() {
    let Some(t) = TestDb::new().await else { return };
    let s = scene(&t.pool, "a@example.com").await;
    let on = full_app(&t.pool, true);
    let off = full_app(&t.pool, false);
    // A smart album made while it was on.
    let (_, album) = post(
        &on,
        &s.key,
        "/assets/albums",
        json!({"title": "Live", "rules": {"rules": [{"field": "people", "ids": [s.a]}]}}),
    )
    .await;
    let id: Uuid = album["id"].as_str().unwrap().parse().unwrap();
    // The person's timeline: a 404.
    assert_eq!(
        get(&off, &s.key, &format!("/assets/people/{}/timeline", s.a)).await.0,
        StatusCode::NOT_FOUND
    );
    // The search parameter is ignored (the search lists the library, as for an unknown parameter).
    let (st, v) = get(&off, &s.key, &format!("/assets/search?people={}", s.b)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(item_ids(&v).len(), 4, "every photo: the filter is not applied");
    // The parser does not match names.
    let (_, v) = get(&off, &s.key, "/assets/search?q=Person%20A%20and%20Person%20B%202023").await;
    assert!(
        v["parsed"]["chips"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["kind"] != "people"),
        "{v}"
    );
    // A bulk action never reads the person (or a people filter) as "no filter": it is refused.
    for scope in [json!({"person": s.a}), json!({"search": {"people": s.a.to_string()}})] {
        let (st, v) = post(&off, &s.key, "/assets/trash", json!({"scope": scope, "except": []})).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{v}");
    }
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM assets WHERE deleted_at IS NOT NULL").await,
        0
    );
    // A saved people rule matches nothing while it is off, and cannot be saved.
    let (_, listed) = get(&off, &s.key, &format!("/assets/albums/{id}/assets")).await;
    assert!(item_ids(&listed).is_empty());
    let (st, _) = post(
        &off,
        &s.key,
        "/assets/albums",
        json!({"title": "x", "rules": {"rules": [{"field": "people", "ids": [s.a]}]}}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // The smart album as a scope matches nothing either, and its share link shows nothing.
    let (st, v) = post(
        &off,
        &s.key,
        "/assets/favorite",
        json!({"scope": {"album": id}, "except": [], "favorite": true}),
    )
    .await;
    assert_eq!((st, v["count"].as_i64()), (StatusCode::OK, Some(0)), "{v}");
    let (_, share) = post(&on, &s.key, "/assets/shares", json!({"album_id": id})).await;
    let token = share["token"].as_str().unwrap();
    let (_, body) = visit(&off, &format!("/assets/public/s/{token}/items")).await;
    assert!(!body.contains(&s.a_photos[0].to_string()), "{body}");
    t.teardown().await;
}

// ---- follow-ups: pending badge, bulk changes, cover choices ----------------------------------------------------------------------

/// A person with three faces of `who`, and `near` unassigned faces that look like them (shots 50..); returns (person, near faces).
async fn badge_scene(
    pool: &PgPool,
    owner: Uuid,
    tag: &str,
    name: Option<&str>,
    who: &[f32],
    near: u64,
) -> (Uuid, Vec<Uuid>) {
    let p = person(pool, owner, name).await;
    for i in 0..3 {
        let a = photo(pool, owner, &format!("{tag}-k{i}.jpg")).await;
        let f = face(pool, a, Some(shot(who, i)), [0.1, 0.1, 0.2, 0.2]).await;
        put(pool, f, p).await;
    }
    let mut out = Vec::new();
    for i in 0..near {
        let a = photo(pool, owner, &format!("{tag}-n{i}.jpg")).await;
        out.push(face(pool, a, Some(shot(who, 50 + i)), [0.2, 0.3, 0.1, 0.1]).await);
    }
    (p, out)
}

/// The `pending` of one listed person, -1 when they are not listed.
fn pending_of(v: &Value, id: Uuid) -> i64 {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == json!(id))
        .map(|p| p["pending"].as_i64().unwrap())
        .unwrap_or(-1)
}

#[tokio::test]
async fn the_people_list_counts_pending_suggestions_in_one_query_and_only_for_the_owner() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (bid, kb) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let alpha = identity(1);
    // Alpha has three suggestions, Bravo (another face) one; an unnamed and a hidden person show none.
    let (a, near_a) = badge_scene(&t.pool, uid, "a", Some("Person A"), &alpha, 3).await;
    let (b, _) = badge_scene(&t.pool, uid, "b", Some("Person B"), &identity(2), 1).await;
    let (unnamed, _) = badge_scene(&t.pool, uid, "u", None, &identity(3), 2).await;
    let (hidden, _) = badge_scene(&t.pool, uid, "h", Some("Person H"), &identity(4), 2).await;
    sqlx::query("UPDATE people SET hidden = true WHERE id = $1")
        .bind(hidden)
        .execute(&t.pool)
        .await
        .unwrap();
    // A far face, a face without a fingerprint and a trashed photo's face are not suggestions.
    let far = photo(&t.pool, uid, "far.jpg").await;
    face(&t.pool, far, Some(identity(9)), [0.2, 0.3, 0.1, 0.1]).await;
    let none = photo(&t.pool, uid, "none.jpg").await;
    face(&t.pool, none, None, [0.2, 0.3, 0.1, 0.1]).await;
    let trashed = photo(&t.pool, uid, "trashed.jpg").await;
    face(&t.pool, trashed, Some(shot(&alpha, 90)), [0.2, 0.3, 0.1, 0.1]).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&t.pool)
        .await
        .unwrap();
    // Another owner: a person who looks the same as Alpha, with faces and near faces of their own. Nothing crosses over.
    let (theirs, _) = badge_scene(&t.pool, bid, "t", Some("Person T"), &alpha, 2).await;

    let (st, v) = get(&app, &key, "/assets/people").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!([a, b, unnamed, hidden].map(|p| pending_of(&v, p)), [3, 1, 0, 0], "{v}");
    assert_eq!(pending_of(&v, theirs), -1, "not listed to someone else");
    let (_, v) = get(&app, &kb, "/assets/people").await;
    assert_eq!(pending_of(&v, theirs), 2, "{v}");
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    // One person's own answer is the same number, and matches what the suggestions list holds.
    let (_, one) = get(&app, &key, &format!("/assets/people/{a}")).await;
    assert_eq!(one["pending"], 3);
    let (_, sug) = get(&app, &key, &format!("/assets/people/{a}/suggestions")).await;
    assert_eq!(sug["items"].as_array().unwrap().len(), 3);
    // "No" to one and "Yes" to another: both leave the count.
    let reject = format!("/assets/faces/{}/reject", near_a[0]);
    post(&app, &key, &reject, json!({"person_id": a})).await;
    assert_eq!(pending_of(&get(&app, &key, "/assets/people").await.1, a), 2);
    let assign = format!("/assets/faces/{}/assign", near_a[1]);
    post(&app, &key, &assign, json!({"person_id": a})).await;
    let (_, v) = get(&app, &key, "/assets/people").await;
    assert_eq!((pending_of(&v, a), pending_of(&v, b)), (1, 1));
    assert_eq!(pending_of(&get(&app, &kb, "/assets/people").await.1, theirs), 2);
    // Faces in hidden photos wait for the Hidden view, as the suggestions do.
    let (uid2, cookie) = session(&t.pool, "c@example.com", "member").await;
    let (p2, near2) = badge_scene(&t.pool, uid2, "c", Some("Person C"), &identity(5), 2).await;
    sqlx::query("UPDATE assets SET hidden = true WHERE id = (SELECT asset_id FROM faces WHERE id = $1)")
        .bind(near2[0])
        .execute(&t.pool)
        .await
        .unwrap();
    let pending = || async {
        let (_, v) = send_cookie(&app, "GET", "/assets/people", Some(&cookie), None).await;
        pending_of(&v, p2)
    };
    assert_eq!(pending().await, 1);
    unlock(&t.pool, &cookie).await;
    assert_eq!(pending().await, 2);
    t.teardown().await;
}

#[tokio::test]
async fn bulk_changes_hide_show_delete_and_merge_people_all_or_nothing_for_the_owner() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, key) = user(&t.pool, "a@example.com").await;
    let (bid, other) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let (a, b, c) = (
        person(&t.pool, uid, Some("Person A")).await,
        person(&t.pool, uid, None).await,
        person(&t.pool, uid, Some("Person C")).await,
    );
    for (i, who) in [a, a, b, c].into_iter().enumerate() {
        let ph = photo(&t.pool, uid, &format!("bulk{i}.jpg")).await;
        let f = face(&t.pool, ph, Some(identity(1)), [0.1, 0.1, 0.2, 0.2]).await;
        put(&t.pool, f, who).await;
    }
    let theirs = person(&t.pool, bid, Some("Person T")).await;
    let hidden_n = || count(&t.pool, "SELECT count(*) FROM people WHERE hidden");
    let url = "/assets/people/bulk";
    // Hide, then show, two at once (a repeated id counts once).
    let (st, v) = post(&app, &key, url, json!({"action": "hide", "ids": [a, b]})).await;
    assert_eq!((st, v["done"].as_i64()), (StatusCode::OK, Some(2)), "{v}");
    assert_eq!(hidden_n().await, 2);
    let (st, v) = post(&app, &key, url, json!({"action": "show", "ids": [a, b, a]})).await;
    assert_eq!((st, v["done"].as_i64()), (StatusCode::OK, Some(2)), "{v}");
    assert_eq!(hidden_n().await, 0);
    // Someone else's person among the ids, or one that is not there: a 404 and nothing at all changes.
    for bad in [theirs, Uuid::new_v4()] {
        for action in ["hide", "delete"] {
            let (st, _) = post(&app, &key, url, json!({"action": action, "ids": [a, bad]})).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{action}");
        }
    }
    assert_eq!(hidden_n().await, 0);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM people").await, 4);
    let (st, _) = post(&app, &other, url, json!({"action": "hide", "ids": [a]})).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = post(
        &app,
        &other,
        url,
        json!({"action": "merge", "ids": [a, theirs], "into": theirs}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM faces WHERE person_id IS NOT NULL").await,
        4
    );
    // Checked inputs (an action that does not exist is refused by the body's shape).
    let (st, _) = post(&app, &key, url, json!({"action": "explode", "ids": [a]})).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    for bad in [
        json!({"action": "hide", "ids": []}),
        json!({"action": "hide", "ids": [a], "into": a}),
        json!({"action": "hide", "ids": [a], "name": "Person X"}),
        json!({"action": "merge", "ids": [a]}),
        json!({"action": "merge", "ids": [a, b]}),
        json!({"action": "merge", "ids": [a, b], "into": c}),
        json!({"action": "merge", "ids": [a, b], "into": a, "name": "x".repeat(101)}),
    ] {
        assert_eq!(
            post(&app, &key, url, bad.clone()).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    assert_eq!(count(&t.pool, "SELECT count(*) FROM people").await, 4);
    // Merge: the unnamed one and Person C into Person A, who keeps the name; every face follows.
    let (st, v) = post(&app, &key, url, json!({"action": "merge", "ids": [a, b, c], "into": a})).await;
    assert_eq!(
        (st, v["done"].as_i64(), v["faces"].as_i64()),
        (StatusCode::OK, Some(2), Some(2)),
        "{v}"
    );
    assert_eq!(v["into"], json!(a));
    assert_eq!(count(&t.pool, "SELECT count(*) FROM people").await, 2);
    let (_, one) = get(&app, &key, &format!("/assets/people/{a}")).await;
    assert_eq!(
        (one["name"].as_str(), one["faces"].as_i64()),
        (Some("Person A"), Some(4))
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM audit_log WHERE action = 'people.merge'").await,
        2
    );
    // Merge into an unnamed person: a merged name is taken over; an explicit name wins over it.
    let (x, y, z) = (
        person(&t.pool, uid, None).await,
        person(&t.pool, uid, Some("Person Y")).await,
        person(&t.pool, uid, Some("Person Z")).await,
    );
    for (i, who) in [x, y, z].into_iter().enumerate() {
        let ph = photo(&t.pool, uid, &format!("bulkm{i}.jpg")).await;
        let f = face(&t.pool, ph, Some(identity(2)), [0.1, 0.1, 0.2, 0.2]).await;
        put(&t.pool, f, who).await;
    }
    let (st, v) = post(&app, &key, url, json!({"action": "merge", "ids": [x, y, z], "into": x})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, one) = get(&app, &key, &format!("/assets/people/{x}")).await;
    let name = one["name"].as_str().unwrap();
    assert!(name == "Person Y" || name == "Person Z", "{name}");
    assert_eq!(one["faces"], 3);
    let body = json!({"action": "merge", "ids": [a, x], "into": a, "name": "  Person Q "});
    assert_eq!(post(&app, &key, url, body).await.0, StatusCode::OK);
    let (_, one) = get(&app, &key, &format!("/assets/people/{a}")).await;
    assert_eq!(
        (one["name"].as_str(), one["faces"].as_i64()),
        (Some("Person Q"), Some(7))
    );
    // Delete: the grouping goes; the photos and faces stay (unassigned) and the other owner's person stays.
    let (st, v) = post(&app, &key, url, json!({"action": "delete", "ids": [a]})).await;
    assert_eq!((st, v["done"].as_i64()), (StatusCode::OK, Some(1)), "{v}");
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM faces WHERE person_id IS NOT NULL").await,
        0
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 7);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM assets").await, 7);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM people").await, 1);
    assert_eq!(
        count(
            &t.pool,
            "SELECT count(*) FROM audit_log WHERE detail::text LIKE '%Person%'"
        )
        .await,
        0,
        "no names in the audit trail"
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_person_lists_the_faces_a_cover_can_be_chosen_from_a_page_at_a_time() {
    let Some(t) = TestDb::new().await else { return };
    let (uid, cookie) = session(&t.pool, "a@example.com", "member").await;
    let (_, other) = user(&t.pool, "b@example.com").await;
    let app = app_with_people(&t.pool);
    let p = person(&t.pool, uid, Some("Person A")).await;
    let mut fs = Vec::new();
    for i in 0..5 {
        let ph = photo(&t.pool, uid, &format!("cv{i}.jpg")).await;
        let side = 0.1 + 0.02 * i as f32;
        let f = face(&t.pool, ph, Some(identity(1)), [0.1, 0.1, side, side]).await;
        put(&t.pool, f, p).await;
        fs.push((f, ph));
    }
    // A hidden photo's face waits for the Hidden view; a photo without a preview is never listed.
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(fs[0].1)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET thumbhash = NULL WHERE id = $1")
        .bind(fs[1].1)
        .execute(&t.pool)
        .await
        .unwrap();
    let path = format!("/assets/people/{p}/faces");
    let page = |q: &str| {
        let (app, cookie, url) = (app.clone(), cookie.clone(), format!("{path}{q}"));
        async move { send_cookie(&app, "GET", &url, Some(&cookie), None).await }
    };
    let ids = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["face_id"].as_str().unwrap().to_string())
            .collect()
    };
    let (st, v) = page("?limit=2").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // Largest faces first; two are listable on this page and one on the next.
    assert_eq!(ids(&v), vec![fs[4].0.to_string(), fs[3].0.to_string()]);
    assert_eq!(v["next_cursor"], "2");
    let first = &v["items"][0];
    assert_eq!(first["asset_id"], json!(fs[4].1));
    assert_box(&first["box"], [0.1, 0.1, 0.18, 0.18]);
    assert!(first["preview_url"].as_str().unwrap().contains("preview.webp"));
    let (_, v) = page("?limit=2&cursor=2").await;
    assert_eq!(ids(&v), vec![fs[2].0.to_string()]);
    assert!(v["next_cursor"].is_null());
    assert_eq!(page("?cursor=-3").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(page("?cursor=abc").await.0, StatusCode::BAD_REQUEST);
    unlock(&t.pool, &cookie).await;
    assert_eq!(
        ids(&page("").await.1).len(),
        4,
        "the hidden photo's face too, with the view unlocked"
    );
    // A listed face can be made the cover; someone else cannot read the list.
    let body = Some(json!({"cover_face_id": fs[3].0}));
    let (st, v) = send_cookie(&app, "PATCH", &format!("/assets/people/{p}"), Some(&cookie), body).await;
    assert_eq!(
        (st, v["cover"]["face_id"].as_str()),
        (StatusCode::OK, Some(fs[3].0.to_string().as_str())),
        "{v}"
    );
    assert_eq!(get(&app, &other, &path).await.0, StatusCode::NOT_FOUND);
    let ghost = format!("/assets/people/{}/faces", Uuid::new_v4());
    assert_eq!(get(&app, &other, &ghost).await.0, StatusCode::NOT_FOUND);
    t.teardown().await;
}
