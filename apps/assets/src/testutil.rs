//! Helpers for DB-backed tests. They need `DATABASE_URL` (the role must be able to CREATE DATABASE) and are skipped
//! without it: `TestDb::new()` returns None.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use cyanotype_core::{config::S3Config, db, storage::Storage};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use tower::ServiceExt;
use uuid::Uuid;

use crate::{AppState, purge::Objects};

pub struct TestDb {
    pub pool: PgPool,
    admin_url: String,
    name: String,
}

impl TestDb {
    pub async fn new() -> Option<Self> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        Some(Self { pool, admin_url, name })
    }

    pub async fn teardown(self) {
        self.pool.close().await;
        let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
        admin
            .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str())
            .await
            .unwrap();
    }
}

/// Storage pointing nowhere: any call that reaches it fails, presigning (offline) works.
pub fn dummy_storage() -> Storage {
    Storage::new(&S3Config {
        endpoint: Some("http://127.0.0.1:1".into()),
        public_endpoint: Some("http://127.0.0.1:1".into()),
        region: "us-east-1".into(),
        bucket: "b".into(),
        access_key: "k".into(),
        secret_key: "s".into(),
        force_path_style: true,
        presign_ttl_secs: 60,
    })
}

/// The model the search tests run with unless they say otherwise (the default).
pub const TEST_MODEL: cyanotype_clip::Model = cyanotype_clip::DEFAULT_MODEL;

/// The search setup of an app that is not testing search: the test model, no caching.
pub fn default_search() -> Arc<crate::search::SearchState> {
    Arc::new(crate::search::SearchState::uncached(TEST_MODEL, 95))
}

pub fn app(pool: &PgPool) -> Router {
    app_with(pool, default_search(), Arc::new(FakeText::default()))
}

/// An app with its own search model setup (which model is configured, when it takes over) and text model.
pub fn app_with(
    pool: &PgPool,
    search: Arc<crate::search::SearchState>,
    text: Arc<dyn crate::search::TextEmbedder>,
) -> Router {
    crate::routes::routes().with_state(AppState {
        db: pool.clone(),
        storage: dummy_storage(),
        trash_retention_days: 30,
        purge_wake: Default::default(),
        text,
        search,
    })
}

/// The app over a real S3 API (the `S3_*` variables: a bucket that exists), for what writes objects (photo edits); None when
/// they are not set, and the test then has nothing to run against.
pub fn app_with_s3(pool: &PgPool) -> Option<(Router, Storage)> {
    std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty())?;
    let storage = Storage::new(&cyanotype_core::config::S3Config::from_env().expect("S3_* env"));
    let app = crate::routes::routes().with_state(AppState {
        db: pool.clone(),
        storage: storage.clone(),
        trash_retention_days: 30,
        purge_wake: Default::default(),
        text: Arc::new(FakeText::default()),
        search: default_search(),
    });
    Some((app, storage))
}

/// A user with an API key; returns `(user id, bearer key)`.
pub async fn user(pool: &PgPool, email: &str) -> (Uuid, String) {
    let (uid,): (Uuid,) = sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ($1,'x') RETURNING id")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap();
    let key = format!("cyk_{}", Uuid::new_v4().simple());
    sqlx::query("INSERT INTO api_keys (user_id, name, key_hash) VALUES ($1,'t',$2)")
        .bind(uid)
        .bind(cyanotype_service::auth::sha256(key.as_bytes()))
        .execute(pool)
        .await
        .unwrap();
    (uid, key)
}

/// A live asset named `name` (its sha256 is the hash of the name), with a job and an embedding attached.
pub async fn asset(pool: &PgPool, owner: Uuid, name: &str) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
         VALUES ($1, sha256($2::bytea), $3, $2, 'image/jpeg', 1) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(format!("originals/{owner}/{name}"))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO jobs (kind, asset_id) VALUES ('thumbnail', $1)")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO asset_embeddings (asset_id, model, version, embedding) VALUES ($1, $2, $3, array_fill(0.1::real, ARRAY[$4])::vector::halfvec)")
        .bind(id)
        .bind(TEST_MODEL.id())
        .bind(TEST_MODEL.version())
        .bind(TEST_MODEL.dim() as i32)
        .execute(pool)
        .await
        .unwrap();
    id
}

pub async fn send(
    app: &Router,
    method: &str,
    path: &str,
    key: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {key}"));
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// A user with a login session of the given role; returns `(user id, cookie header value)`.
pub async fn session(pool: &PgPool, email: &str, role: &str) -> (Uuid, String) {
    let (uid,): (Uuid,) =
        sqlx::query_as("INSERT INTO users (email, password_hash, role) VALUES ($1,'x',$2) RETURNING id")
            .bind(email)
            .bind(role)
            .fetch_one(pool)
            .await
            .unwrap();
    let token = Uuid::new_v4().simple().to_string();
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1,$2, now() + interval '1 day')")
        .bind(cyanotype_service::auth::sha256(token.as_bytes()))
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
    (uid, format!("{}={token}", cyanotype_service::auth::SESSION_COOKIE))
}

/// Like [`send`], with a session cookie instead of an API key (`None`: no credentials at all).
pub async fn send_cookie(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder().method(method).uri(path);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

pub fn ids(v: &[Uuid]) -> serde_json::Value {
    serde_json::json!({"ids": v})
}

/// Records what a purge asked storage to delete; can be told to fail.
#[derive(Clone, Default)]
pub struct FakeObjects {
    pub deleted: Arc<Mutex<Vec<(String, String)>>>,
    pub fail: Arc<AtomicBool>,
}

impl Objects for FakeObjects {
    async fn delete_asset_objects(&self, original_key: &str, derived_prefix: &str) -> anyhow::Result<usize> {
        if self.fail.load(Ordering::SeqCst) {
            anyhow::bail!("storage is down");
        }
        self.deleted
            .lock()
            .unwrap()
            .push((original_key.to_string(), derived_prefix.to_string()));
        Ok(2)
    }
}

/// Words the fake text model knows, one embedding axis each.
pub const VOCAB: [&str; 6] = ["cat", "dog", "beach", "car", "cake", "night"];

/// A unit vector of `model`'s size with weight on the given words' axes (the "image embedding" of a photo of those things).
pub fn embedding_for(model: cyanotype_clip::Model, words: &[(&str, f32)]) -> Vec<f32> {
    let dim = model.dim();
    let mut v = vec![0.0f32; dim];
    for (w, weight) in words {
        let axis = VOCAB.iter().position(|x| x == w).expect("known word");
        v[axis] += weight;
    }
    // A little constant so no vector is the zero vector (and nothing is exactly orthogonal).
    v[dim - 1] += 0.01;
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / n).collect()
}

/// [`embedding_for`] the test model.
pub fn embedding_of(words: &[(&str, f32)]) -> Vec<f32> {
    embedding_for(TEST_MODEL, words)
}

/// A text model that maps the words of a query onto [`VOCAB`] axes, so ranking tests need no ONNX model. `missing` lists
/// models whose files are "not installed".
#[derive(Default)]
pub struct FakeText {
    pub missing: Vec<cyanotype_clip::Model>,
}

impl crate::search::TextEmbedder for FakeText {
    fn embed(&self, model: cyanotype_clip::Model, query: &str) -> anyhow::Result<Arc<Vec<f32>>> {
        if !self.available(model) {
            return Err(crate::search::ModelMissing(model.dir()).into());
        }
        let words: Vec<(&str, f32)> = query
            .split_whitespace()
            .filter_map(|w| VOCAB.iter().find(|v| **v == w.to_lowercase()).map(|v| (*v, 1.0)))
            .collect();
        Ok(Arc::new(embedding_for(model, &words)))
    }

    fn available(&self, model: cyanotype_clip::Model) -> bool {
        !self.missing.contains(&model)
    }
}

/// Replace a photo's embedding (the test model's current version).
pub async fn set_embedding(pool: &PgPool, asset: Uuid, words: &[(&str, f32)]) {
    set_embedding_for(pool, asset, TEST_MODEL, words).await;
}

/// Give a photo a vector from `model`.
pub async fn set_embedding_for(pool: &PgPool, asset: Uuid, model: cyanotype_clip::Model, words: &[(&str, f32)]) {
    cyanotype_core::embeddings::store(pool, asset, model.id(), model.version(), &embedding_for(model, words))
        .await
        .unwrap();
}
