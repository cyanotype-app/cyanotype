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
