//! ZIP downloads (`zip.rs`, and "Download all" of `share_public.rs`) against a real Postgres and a real S3 API: skipped without
//! `DATABASE_URL` and the `S3_*` variables, like the other tests that write objects.
use std::net::SocketAddr;

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode, header},
    response::Response,
};
use cyanotype_core::{config::S3Config, keys, storage::Storage};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::{routes_with, share_public::ShareState};
use crate::{
    AppState,
    testutil::{FakeText, TestDb, default_search, send, session, user},
    zip::tests::{entry_bytes, read_directory},
};

struct Env {
    t: TestDb,
    storage: Storage,
}

impl Env {
    async fn new() -> Option<Self> {
        std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty())?;
        let t = TestDb::new().await?;
        let storage = Storage::new(&S3Config::from_env().expect("S3_* env"));
        Some(Self { t, storage })
    }

    fn app(&self) -> Router {
        self.app_with(ShareState::new(false, vec![]).with_limits((10_000, 10_000), (10_000, 10_000), (10_000, 10_000)))
    }

    fn app_with(&self, st: ShareState) -> Router {
        routes_with(st).with_state(AppState {
            db: self.t.pool.clone(),
            storage: self.storage.clone(),
            trash_retention_days: 30,
            purge_wake: Default::default(),
            text: std::sync::Arc::new(FakeText::default()),
            search: default_search(),
            faces_enabled: false,
            pets_enabled: false,
        })
    }

    /// A photo with real bytes in the bucket.
    async fn photo(&self, owner: Uuid, name: &str, data: &[u8], taken: Option<&str>) -> Uuid {
        let key = format!("originals/{owner}/zip-{}", Uuid::new_v4());
        self.storage.put_bytes(&key, data.to_vec(), "image/jpeg").await.unwrap();
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, taken_at)
             VALUES ($1, sha256($2::text::bytea), $3, $4, 'image/jpeg', $5, $6::timestamptz) RETURNING id",
        )
        .bind(owner)
        .bind(Uuid::new_v4().to_string())
        .bind(key)
        .bind(name)
        .bind(data.len() as i64)
        .bind(taken)
        .fetch_one(&self.t.pool)
        .await
        .unwrap();
        id
    }

    /// The RAW behind `primary`.
    async fn raw_of(&self, owner: Uuid, primary: Uuid, name: &str, data: &[u8]) -> Uuid {
        let id = self.photo(owner, name, data, None).await;
        sqlx::query("UPDATE assets SET primary_id = $2, content_type = 'image/x-raw' WHERE id = $1")
            .bind(id)
            .bind(primary)
            .execute(&self.t.pool)
            .await
            .unwrap();
        id
    }

    /// The JPEG copy a share link serves (the worker's job), with these bytes.
    async fn share_copy(&self, owner: Uuid, id: Uuid, data: &[u8]) {
        self.storage
            .put_bytes(&keys::share_copy(owner, id), data.to_vec(), "image/jpeg")
            .await
            .unwrap();
        sqlx::query("UPDATE assets SET share_copy_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.t.pool)
            .await
            .unwrap();
    }
}

/// Deterministic bytes that differ per seed (so a mix-up of two files shows).
fn bytes(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn bearer(key: &str) -> (&'static str, String) {
    ("authorization", format!("Bearer {key}"))
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    auth: Option<(&str, String)>,
    body: Option<(&str, Vec<u8>)>,
) -> Response {
    let mut b = Request::builder().method(method).uri(path);
    if let Some((n, v)) = auth {
        b = b.header(n, v);
    }
    let body = match body {
        Some((ct, data)) => {
            b = b.header(header::CONTENT_TYPE, ct);
            Body::from(data)
        }
        None => Body::empty(),
    };
    let mut req = b.body(body).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("203.0.113.9:1234".parse::<SocketAddr>().unwrap()));
    app.clone().oneshot(req).await.unwrap()
}

async fn collect(res: Response) -> (StatusCode, HeaderMap, Vec<u8>) {
    let (status, headers) = (res.status(), res.headers().clone());
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, body)
}

async fn post_zip(app: &Router, key: &str, body: Value) -> (StatusCode, HeaderMap, Vec<u8>) {
    collect(
        call(
            app,
            "POST",
            "/assets/zip",
            Some(bearer(key)),
            Some(("application/json", body.to_string().into_bytes())),
        )
        .await,
    )
    .await
}

/// `(name, bytes)` of every entry, after checking the archive is whole: exact `Content-Length`, a readable directory, every
/// CRC right.
fn entries(headers: &HeaderMap, z: &[u8]) -> Vec<(String, Vec<u8>)> {
    assert_eq!(headers[header::CONTENT_TYPE], "application/zip");
    assert_eq!(headers[header::CONTENT_LENGTH].to_str().unwrap(), z.len().to_string());
    read_directory(z)
        .into_iter()
        .map(|(name, crc, size, off, _)| {
            let data = entry_bytes(z, off, size).to_vec();
            assert_eq!(crc32fast::hash(&data), crc, "crc of {name}");
            (name, data)
        })
        .collect()
}

fn names(e: &[(String, Vec<u8>)]) -> Vec<&str> {
    e.iter().map(|(n, _)| n.as_str()).collect()
}

fn disposition(h: &HeaderMap) -> String {
    h[header::CONTENT_DISPOSITION].to_str().unwrap().to_string()
}

/// `unzip -t` on the archive, when `unzip` is installed: an independent reader agrees it is a good ZIP.
fn unzip_test(z: &[u8]) {
    let path = std::env::temp_dir().join(format!("cyanotype-zip-test-{}.zip", Uuid::new_v4()));
    std::fs::write(&path, z).unwrap();
    match std::process::Command::new("unzip").arg("-tq").arg(&path).output() {
        Ok(out) => assert!(
            out.status.success(),
            "unzip -t: {}",
            String::from_utf8_lossy(&out.stdout)
        ),
        Err(_) => eprintln!("unzip is not installed: skipped the external check"),
    }
    std::fs::remove_file(path).ok();
}

async fn album_with(app: &Router, key: &str, title: &str, ids: &[Uuid]) -> Uuid {
    let (st, a) = send(app, "POST", "/assets/albums", key, Some(json!({"title": title}))).await;
    assert_eq!(st, StatusCode::CREATED, "{a}");
    let id: Uuid = a["id"].as_str().unwrap().parse().unwrap();
    let (st, v) = send(
        app,
        "POST",
        &format!("/assets/albums/{id}/assets"),
        key,
        Some(json!({"ids": ids})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    id
}

#[tokio::test]
async fn album_zip_is_a_valid_archive_of_the_originals() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    let (d1, d2, d3) = (bytes(1, 70_000), bytes(2, 5), bytes(3, 0));
    let a = e.photo(uid, "IMG_0001.jpg", &d1, Some("2024-05-01T12:00:00Z")).await;
    let b = e.photo(uid, "IMG_0001.jpg", &d2, Some("2024-05-02T12:00:00Z")).await;
    let c = e.photo(uid, "../../Evil\\name.jpg", &d3, None).await;
    let album = album_with(&app, &key, "Alpha trip", &[a, b, c]).await;

    let (st, h, z) = collect(
        call(
            &app,
            "GET",
            &format!("/assets/albums/{album}/zip"),
            Some(bearer(&key)),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(disposition(&h), "attachment; filename=\"Alpha trip.zip\"");
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    let got = entries(&h, &z);
    // Oldest first; the repeated name gets a counter; the path-like name is a plain file name.
    assert_eq!(names(&got), ["IMG_0001.jpg", "IMG_0001 (2).jpg", "name.jpg"]);
    let by_name = |n: &str| got.iter().find(|(x, _)| x == n).unwrap().1.clone();
    assert_eq!(by_name("IMG_0001.jpg"), d1);
    assert_eq!(by_name("IMG_0001 (2).jpg"), d2);
    assert_eq!(by_name("name.jpg"), d3);
    // Entry times are the capture times.
    let dir = read_directory(&z);
    let mtime = |n: &str| dir.iter().find(|d| d.0 == n).unwrap().4;
    assert_eq!(mtime("IMG_0001.jpg"), Some(1_714_564_800));
    assert_eq!(mtime("name.jpg"), None);
    unzip_test(&z);

    // The same through the selection endpoint with the album scope: the same archive.
    let (st, h2, z2) = post_zip(&app, &key, json!({"scope": {"album": album}})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(disposition(&h2), "attachment; filename=\"Alpha trip.zip\"");
    assert_eq!(names(&entries(&h2, &z2)), names(&got));
    e.t.teardown().await;
}

#[tokio::test]
async fn empty_archive_name_falls_back() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    let a = e.photo(uid, "a.jpg", b"aaa", None).await;
    let (st, h, z) = post_zip(&app, &key, json!({"ids": [a]})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(disposition(&h), "attachment; filename=\"cyanotype-photos.zip\"");
    assert_eq!(entries(&h, &z), [("a.jpg".to_string(), b"aaa".to_vec())]);
    // A name from the page (the album the selection was made in), made safe.
    let (_, h, _) = post_zip(&app, &key, json!({"ids": [a], "name": "Bravo/../set"})).await;
    assert_eq!(disposition(&h), "attachment; filename=\"Bravo-..-set.zip\"");
    e.t.teardown().await;
}

#[tokio::test]
async fn a_pair_is_both_files_and_a_form_post_works() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    let jpg = e
        .photo(uid, "IMG_0007.JPG", b"jpeg", Some("2024-01-01T00:00:00Z"))
        .await;
    let raw = e.raw_of(uid, jpg, "IMG_0007.CR3", b"rawbytes").await;
    let other = e
        .photo(uid, "IMG_0008.JPG", b"other", Some("2024-01-02T00:00:00Z"))
        .await;
    // Asking for the RAW's own id is not a way in: only visible photos are selectable.
    let body = serde_urlencoded::to_string([("selection", json!({"ids": [jpg, raw, other]}).to_string())]).unwrap();
    let (st, h, z) = collect(
        call(
            &app,
            "POST",
            "/assets/zip",
            Some(bearer(&key)),
            Some(("application/x-www-form-urlencoded", body.into_bytes())),
        )
        .await,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let got = entries(&h, &z);
    assert_eq!(names(&got), ["IMG_0007.JPG", "IMG_0007.CR3", "IMG_0008.JPG"]);
    assert_eq!(got[1].1, b"rawbytes");
    unzip_test(&z);
    e.t.teardown().await;
}

#[tokio::test]
async fn scopes_ids_and_the_plan() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    let a = e.photo(uid, "a.jpg", b"a", Some("2024-01-01T00:00:00Z")).await;
    let b = e.photo(uid, "b.jpg", b"bb", Some("2024-01-02T00:00:00Z")).await;
    let c = e.photo(uid, "c.jpg", b"ccc", Some("2024-01-03T00:00:00Z")).await;
    // Select all of the library except one: no ids are sent for the photos that are in.
    let (st, h, z) = post_zip(&app, &key, json!({"scope": "library", "except": [b]})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(names(&entries(&h, &z)), ["a.jpg", "c.jpg"]);
    // Ids and a scope together are refused, as everywhere.
    assert_eq!(
        post_zip(&app, &key, json!({"ids": [a], "scope": "library"})).await.0,
        StatusCode::BAD_REQUEST
    );
    // The plan says what the archive holds, without the bytes.
    let (st, v) = send(
        &app,
        "POST",
        "/assets/zip/plan",
        &key,
        Some(json!({"scope": "library", "name": "Alpha"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        (v["files"].clone(), v["bytes"].clone(), v["name"].clone()),
        (json!(3), json!(6), json!("Alpha.zip"))
    );
    // Nothing to download is a 404, not an empty archive.
    assert_eq!(post_zip(&app, &key, json!({"ids": []})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        post_zip(&app, &key, json!({"ids": [Uuid::new_v4()]})).await.0,
        StatusCode::NOT_FOUND
    );
    // Garbage is a 400.
    let res = call(
        &app,
        "POST",
        "/assets/zip",
        Some(bearer(&key)),
        Some(("application/json", b"{nope".to_vec())),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = call(
        &app,
        "POST",
        "/assets/zip",
        Some(bearer(&key)),
        Some(("text/plain", b"x".to_vec())),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    // Not signed in.
    assert_eq!(
        post_zip(&app, "cyk_nope", json!({"ids": [c]})).await.0,
        StatusCode::UNAUTHORIZED
    );
    e.t.teardown().await;
}

#[tokio::test]
async fn other_peoples_trashed_purging_and_hidden_photos_are_left_out() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let (other, other_key) = user(&e.t.pool, "bravo@example.com").await;
    let app = e.app();
    let mine = e.photo(uid, "mine.jpg", b"mine", None).await;
    let theirs = e.photo(other, "theirs.jpg", b"theirs", None).await;
    let trashed = e.photo(uid, "trashed.jpg", b"t", None).await;
    let purging = e.photo(uid, "purging.jpg", b"p", None).await;
    let hidden = e.photo(uid, "hidden.jpg", b"h", None).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&e.t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET deleted_at = now(), purging_at = now() WHERE id = $1")
        .bind(purging)
        .execute(&e.t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(hidden)
        .execute(&e.t.pool)
        .await
        .unwrap();

    let all = json!({"ids": [mine, theirs, trashed, purging, hidden]});
    let (st, h, z) = post_zip(&app, &key, all.clone()).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(names(&entries(&h, &z)), ["mine.jpg"]);
    // The other person asking for my photo gets nothing: a 404, the same as for a photo that does not exist.
    assert_eq!(
        post_zip(&app, &other_key, json!({"ids": [mine, trashed, hidden]}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // Scopes: the trash scope never downloads (the trash is for restoring), and no scope reaches a hidden photo.
    assert_eq!(
        post_zip(&app, &key, json!({"scope": "trash"})).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, h, z) = post_zip(&app, &key, json!({"scope": "library"})).await;
    assert_eq!(names(&entries(&h, &z)), ["mine.jpg"]);
    // Somebody else's album is a 404.
    let album = album_with(&app, &other_key, "Bravo", &[theirs]).await;
    let res = call(
        &app,
        "GET",
        &format!("/assets/albums/{album}/zip"),
        Some(bearer(&key)),
        None,
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // The Hidden view: explicit ids work only for a session that unlocked it (an API key never does).
    let (sid, cookie) = session(&e.t.pool, "charlie@example.com", "member").await;
    let secret = e.photo(sid, "secret.jpg", b"secret", None).await;
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(secret)
        .execute(&e.t.pool)
        .await
        .unwrap();
    let ask = |body: Value| {
        call(
            &app,
            "POST",
            "/assets/zip",
            Some(("cookie", cookie.clone())),
            Some(("application/json", body.to_string().into_bytes())),
        )
    };
    assert_eq!(
        ask(json!({"ids": [secret]})).await.status(),
        StatusCode::NOT_FOUND,
        "locked"
    );
    sqlx::query("UPDATE sessions SET hidden_unlocked_until = now() + interval '1 hour' WHERE user_id = $1")
        .bind(sid)
        .execute(&e.t.pool)
        .await
        .unwrap();
    let (st, hd, z) = collect(ask(json!({"ids": [secret]})).await).await;
    assert_eq!(st, StatusCode::OK, "unlocked");
    assert_eq!(entries(&hd, &z), [("secret.jpg".to_string(), b"secret".to_vec())]);
    // Still not through the library scope, which never lists hidden photos.
    assert_eq!(ask(json!({"scope": "library"})).await.status(), StatusCode::NOT_FOUND);
    e.t.teardown().await;
}

#[tokio::test]
async fn too_many_files_is_a_413() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    sqlx::query(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
         SELECT $1, sha256(g::text::bytea), 'originals/none/' || g, 'p' || g || '.jpg', 'image/jpeg', 1
         FROM generate_series(1, $2) g",
    )
    .bind(uid)
    .bind(super::zip::MAX_FILES as i32 + 1)
    .execute(&e.t.pool)
    .await
    .unwrap();
    let (st, _, body) = post_zip(&app, &key, json!({"scope": "library"})).await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(String::from_utf8_lossy(&body).contains("in parts"));
    e.t.teardown().await;
}

#[tokio::test]
async fn a_smart_album_downloads_what_its_rules_match() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    let a = e.photo(uid, "fav.jpg", b"fav", Some("2024-01-01T00:00:00Z")).await;
    let _b = e.photo(uid, "plain.jpg", b"plain", Some("2024-01-02T00:00:00Z")).await;
    sqlx::query("UPDATE assets SET favorite = true WHERE id = $1")
        .bind(a)
        .execute(&e.t.pool)
        .await
        .unwrap();
    let (st, album) = send(
        &app,
        "POST",
        "/assets/albums",
        &key,
        Some(
            json!({"title": "Favourites", "rules": {"match": "all", "rules": [{"field": "favorite", "value": true}]}}),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{album}");
    let id = album["id"].as_str().unwrap();
    let (st, h, z) = collect(
        call(
            &app,
            "GET",
            &format!("/assets/albums/{id}/zip"),
            Some(bearer(&key)),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(names(&entries(&h, &z)), ["fav.jpg"]);
    e.t.teardown().await;
}

#[tokio::test]
async fn the_body_is_streamed_in_small_pieces_and_a_broken_original_aborts_it() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    // 3 x 8 MiB: if any layer buffered a file, a frame would be that big.
    let mut ids = Vec::new();
    for i in 0..3u8 {
        ids.push(
            e.photo(
                uid,
                &format!("big{i}.jpg"),
                &bytes(i, 8 << 20),
                Some(&format!("2024-01-0{}T00:00:00Z", i + 1)),
            )
            .await,
        );
    }
    let res = call(
        &app,
        "POST",
        "/assets/zip",
        Some(bearer(&key)),
        Some(("application/json", json!({"ids": ids}).to_string().into_bytes())),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let total: u64 = res.headers()[header::CONTENT_LENGTH].to_str().unwrap().parse().unwrap();
    assert!(total > 24 << 20);
    let mut body = res.into_body();
    // The response is a stream: the first frame (the first entry's header) comes without any file having been read.
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert!(
        first.len() < 200,
        "first frame is the local header, got {} bytes",
        first.len()
    );
    let (mut seen, mut max, mut frames) = (first.len() as u64, first.len(), 1);
    while let Some(f) = body.frame().await {
        let d = f.unwrap().into_data().unwrap();
        seen += d.len() as u64;
        max = max.max(d.len());
        frames += 1;
    }
    assert_eq!(seen, total);
    assert!(
        max <= 1 << 20,
        "largest frame {max} bytes: files are relayed in pieces, never buffered whole"
    );
    assert!(frames > 24, "{frames} frames");

    // An original that is gone from the bucket: the response must fail, not end as a smaller "complete" archive.
    let gone = e.photo(uid, "gone.jpg", b"x", Some("2024-02-01T00:00:00Z")).await;
    let key_of: String = sqlx::query_scalar("SELECT original_key FROM assets WHERE id = $1")
        .bind(gone)
        .fetch_one(&e.t.pool)
        .await
        .unwrap();
    e.storage.delete_all_versions(&[key_of]).await.unwrap();
    let res = call(
        &app,
        "POST",
        "/assets/zip",
        Some(bearer(&key)),
        Some(("application/json", json!({"ids": [gone]}).to_string().into_bytes())),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(axum::body::to_bytes(res.into_body(), usize::MAX).await.is_err());
    e.t.teardown().await;
}

// --- share links ---

async fn visit(app: &Router, path: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    collect(call(app, "GET", path, None, None).await).await
}

#[tokio::test]
async fn a_password_link_gives_no_archive_until_it_is_unlocked() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    let a = e.photo(uid, "a.jpg", b"a", None).await;
    e.share_copy(uid, a, b"a").await;
    let body = json!({"asset_ids": [a], "password": "bravo-charlie"});
    let (_, s) = send(&app, "POST", "/assets/shares", &key, Some(body)).await;
    let token = s["token"].as_str().unwrap();
    let url = format!("/assets/public/s/{token}/zip");
    assert_eq!(visit(&app, &url).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(visit(&app, &format!("{url}/plan")).await.0, StatusCode::UNAUTHORIZED);
    let unlock = json!({"password": "bravo-charlie"}).to_string().into_bytes();
    let path = format!("/assets/public/s/{token}/unlock");
    let res = call(&app, "POST", &path, None, Some(("application/json", unlock))).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let cookie = res.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let (st, h, z) = collect(call(&app, "GET", &url, Some(("cookie", cookie)), None).await).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(entries(&h, &z), [("a.jpg".to_string(), b"a".to_vec())]);
    e.t.teardown().await;
}

#[tokio::test]
async fn a_share_downloads_its_own_copies_and_nothing_else() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    let a = e
        .photo(uid, "IMG_1.jpg", &bytes(1, 400), Some("2024-01-01T00:00:00Z"))
        .await;
    let b = e
        .photo(uid, "IMG_2.jpg", &bytes(2, 300), Some("2024-01-02T00:00:00Z"))
        .await;
    let heic = e.photo(uid, "IMG_3.heic", &bytes(3, 300), None).await;
    sqlx::query("UPDATE assets SET content_type = 'image/heic' WHERE id = $1")
        .bind(heic)
        .execute(&e.t.pool)
        .await
        .unwrap();
    let outside = e.photo(uid, "outside.jpg", b"not shared", None).await;
    let hidden = e.photo(uid, "hidden.jpg", b"hidden", None).await;
    sqlx::query("UPDATE assets SET hidden = true WHERE id = $1")
        .bind(hidden)
        .execute(&e.t.pool)
        .await
        .unwrap();
    let (copy_a, copy_b) = (bytes(11, 100), bytes(12, 50));
    for (id, data) in [(a, &copy_a), (b, &copy_b), (outside, &bytes(13, 10))] {
        e.share_copy(uid, id, data).await;
    }
    let (st, s) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"asset_ids": [a, b, heic, hidden], "title": "Alpha set"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{s}");
    let token = s["token"].as_str().unwrap();

    let (st, _, plan) = visit(&app, &format!("/assets/public/s/{token}/zip/plan")).await;
    assert_eq!(st, StatusCode::OK);
    let plan: Value = serde_json::from_slice(&plan).unwrap();
    assert_eq!(
        (
            plan["files"].clone(),
            plan["bytes"].clone(),
            plan["pending"].clone(),
            plan["skipped"].clone()
        ),
        (json!(2), json!(150), json!(0), json!(1))
    );
    assert_eq!(plan["name"], "Alpha set.zip");

    let (st, h, z) = visit(&app, &format!("/assets/public/s/{token}/zip")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(disposition(&h), "attachment; filename=\"Alpha set.zip\"");
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    // The location-free copies under the original names; not the originals, not the HEIC, not the hidden photo, not other photos.
    let got = entries(&h, &z);
    assert_eq!(
        got,
        [
            ("IMG_1.jpg".to_string(), copy_a.clone()),
            ("IMG_2.jpg".to_string(), copy_b.clone())
        ]
    );
    unzip_test(&z);

    // Unknown and revoked tokens are the same 404; the token of one link does not reach another link's photos.
    assert_eq!(visit(&app, "/assets/public/s/nope/zip").await.0, StatusCode::NOT_FOUND);
    let (st, s2) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"asset_ids": [outside]})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{s2}");
    let (_, _, z2) = visit(&app, &format!("/assets/public/s/{}/zip", s2["token"].as_str().unwrap())).await;
    let dir2 = read_directory(&z2);
    assert_eq!(dir2.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["outside.jpg"]);
    let (st, _) = send(
        &app,
        "DELETE",
        &format!("/assets/shares/{}", s["id"].as_str().unwrap()),
        &key,
        None,
    )
    .await;
    assert!(st.is_success());
    assert_eq!(
        visit(&app, &format!("/assets/public/s/{token}/zip")).await.0,
        StatusCode::NOT_FOUND
    );
    e.t.teardown().await;
}

#[tokio::test]
async fn a_share_without_downloads_gives_no_archive() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    let a = e.photo(uid, "a.jpg", b"a", None).await;
    e.share_copy(uid, a, b"a").await;
    let (_, s) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"asset_ids": [a], "allow_download": false})),
    )
    .await;
    let token = s["token"].as_str().unwrap();
    assert_eq!(
        visit(&app, &format!("/assets/public/s/{token}/zip")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        visit(&app, &format!("/assets/public/s/{token}/zip/plan")).await.0,
        StatusCode::FORBIDDEN
    );
    // Turning downloads on takes effect at once; turning them off again closes it again.
    let id = s["id"].as_str().unwrap();
    send(
        &app,
        "PATCH",
        &format!("/assets/shares/{id}"),
        &key,
        Some(json!({"allow_download": true})),
    )
    .await;
    assert_eq!(
        visit(&app, &format!("/assets/public/s/{token}/zip")).await.0,
        StatusCode::OK
    );
    send(
        &app,
        "PATCH",
        &format!("/assets/shares/{id}"),
        &key,
        Some(json!({"allow_download": false})),
    )
    .await;
    assert_eq!(
        visit(&app, &format!("/assets/public/s/{token}/zip")).await.0,
        StatusCode::FORBIDDEN
    );
    e.t.teardown().await;
}

#[tokio::test]
async fn copies_that_are_not_made_yet_are_pending_and_queued() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    let ready = e.photo(uid, "ready.jpg", b"r", None).await;
    let later = e.photo(uid, "later.jpg", b"l", None).await;
    e.share_copy(uid, ready, b"r").await;
    let (_, s) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"asset_ids": [ready, later]})),
    )
    .await;
    let token = s["token"].as_str().unwrap();
    let (st, _, plan) = visit(&app, &format!("/assets/public/s/{token}/zip/plan")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(serde_json::from_slice::<Value>(&plan).unwrap()["pending"], 1);
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'share_copy' AND asset_id = $1")
        .bind(later)
        .fetch_one(&e.t.pool)
        .await
        .unwrap();
    assert_eq!(queued, 1);
    assert_eq!(
        visit(&app, &format!("/assets/public/s/{token}/zip")).await.0,
        StatusCode::CONFLICT
    );
    e.t.teardown().await;
}

#[tokio::test]
async fn a_share_with_originals_gets_the_files_untouched_pairs_included() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app();
    let jpg = e
        .photo(uid, "IMG_5.JPG", b"jpeg-original", Some("2024-01-01T00:00:00Z"))
        .await;
    let _raw = e.raw_of(uid, jpg, "IMG_5.CR3", b"raw-original").await;
    let heic = e
        .photo(uid, "IMG_6.heic", b"heic-original", Some("2024-01-02T00:00:00Z"))
        .await;
    let album = album_with(&app, &key, "Bravo", &[jpg, heic]).await;
    let (st, s) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"album_id": album, "include_originals": true})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{s}");
    let (st, h, z) = visit(&app, &format!("/assets/public/s/{}/zip", s["token"].as_str().unwrap())).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(disposition(&h), "attachment; filename=\"Bravo.zip\"");
    let got = entries(&h, &z);
    assert_eq!(
        got,
        [
            ("IMG_5.JPG".to_string(), b"jpeg-original".to_vec()),
            ("IMG_5.CR3".to_string(), b"raw-original".to_vec()),
            ("IMG_6.heic".to_string(), b"heic-original".to_vec()),
        ]
    );
    e.t.teardown().await;
}

#[tokio::test]
async fn public_archives_are_limited_in_size_count_and_rate() {
    let Some(e) = Env::new().await else { return };
    let (uid, key) = user(&e.t.pool, "alpha@example.com").await;
    let app = e.app_with(
        ShareState::new(false, vec![])
            .with_limits((10_000, 10_000), (10_000, 10_000), (10_000, 10_000))
            .with_zip_limit(3, 1),
    );
    // More files than a link may download at once (originals, so no copies are needed).
    sqlx::query(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
         SELECT $1, sha256(g::text::bytea), 'originals/none/' || g, 'p' || g || '.jpg', 'image/jpeg', 1
         FROM generate_series(1, 1001) g",
    )
    .bind(uid)
    .execute(&e.t.pool)
    .await
    .unwrap();
    let (st, s) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"scope": "library", "include_originals": true})),
    )
    .await;
    // A share of a view is capped at 1000 photos when it is made; add them through an album instead.
    let _ = (st, s);
    let all: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM assets WHERE owner_id = $1")
        .bind(uid)
        .fetch_all(&e.t.pool)
        .await
        .unwrap();
    let (_, album) = send(&app, "POST", "/assets/albums", &key, Some(json!({"title": "Big"}))).await;
    let album_id = album["id"].as_str().unwrap().to_string();
    sqlx::query("INSERT INTO album_assets (album_id, asset_id) SELECT $1::uuid, unnest($2::uuid[])")
        .bind(&album_id)
        .bind(&all)
        .execute(&e.t.pool)
        .await
        .unwrap();
    let (st, s) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"album_id": album_id, "include_originals": true})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{s}");
    let token = s["token"].as_str().unwrap();
    let (st, _, body) = visit(&app, &format!("/assets/public/s/{token}/zip")).await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(String::from_utf8_lossy(&body).contains("one by one"));

    // The per-visitor budget (3 archives, the refused one above spent one, then 1 a minute): the third request in a row is a 429 with Retry-After.
    let small = e.photo(uid, "s.jpg", b"s", None).await;
    let (_, s) = send(
        &app,
        "POST",
        "/assets/shares",
        &key,
        Some(json!({"asset_ids": [small], "include_originals": true})),
    )
    .await;
    let url = format!("/assets/public/s/{}/zip", s["token"].as_str().unwrap());
    assert_eq!(visit(&app, &url).await.0, StatusCode::OK);
    assert_eq!(visit(&app, &url).await.0, StatusCode::OK);
    let (st, h, _) = visit(&app, &url).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.contains_key(header::RETRY_AFTER));
    e.t.teardown().await;
}
