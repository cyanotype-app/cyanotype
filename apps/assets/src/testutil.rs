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

pub fn app(pool: &PgPool) -> Router {
    crate::routes::routes().with_state(AppState {
        db: pool.clone(),
        storage: dummy_storage(),
        trash_retention_days: 30,
        purge_wake: Default::default(),
        text: Arc::new(FakeText),
    })
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
    sqlx::query("INSERT INTO asset_embeddings (asset_id, model, version, embedding) VALUES ($1, $2, $3, array_fill(0.1::real, ARRAY[512])::vector)")
        .bind(id)
        .bind(cyanotype_clip::MODEL_ID)
        .bind(cyanotype_clip::MODEL_VERSION)
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

/// A unit vector with weight on the given words' axes (the "image embedding" of a photo of those things).
pub fn embedding_of(words: &[(&str, f32)]) -> Vec<f32> {
    let mut v = vec![0.0f32; 512];
    for (w, weight) in words {
        let axis = VOCAB.iter().position(|x| x == w).expect("known word");
        v[axis] += weight;
    }
    // A little constant so no vector is the zero vector (and nothing is exactly orthogonal).
    v[511] += 0.01;
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / n).collect()
}

/// A text model that maps the words of a query onto [`VOCAB`] axes, so ranking tests need no ONNX model.
pub struct FakeText;

impl crate::search::TextEmbedder for FakeText {
    fn embed(&self, query: &str) -> anyhow::Result<Arc<Vec<f32>>> {
        let words: Vec<(&str, f32)> = query
            .split_whitespace()
            .filter_map(|w| VOCAB.iter().find(|v| **v == w.to_lowercase()).map(|v| (*v, 1.0)))
            .collect();
        Ok(Arc::new(embedding_of(&words)))
    }
}

/// Replace a photo's embedding (current model and version) and mark it as having derived images.
pub async fn set_embedding(pool: &PgPool, asset: Uuid, words: &[(&str, f32)]) {
    cyanotype_core::embeddings::store(
        pool,
        asset,
        cyanotype_clip::MODEL_ID,
        cyanotype_clip::MODEL_VERSION,
        &embedding_of(words),
    )
    .await
    .unwrap();
}
