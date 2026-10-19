//! Deleting an account for good (`accounts::delete_account`) against a real Postgres and S3 API: everything the account
//! stored goes (every version of every object, every row) and nothing of another account's is touched.
//! Needs `DATABASE_URL` and the `S3_*` variables (each test makes its own bucket with versioning on, like production);
//! without them the tests return early.
use std::time::Duration;

use aws_sdk_s3::{
    Client,
    config::{Credentials, Region},
    types::{BucketVersioningStatus, VersioningConfiguration},
};
use cyanotype_core::{
    accounts::{self, JobsStillRunning},
    config::S3Config,
    db, keys,
    storage::Storage,
};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

struct Fx {
    pool: PgPool,
    admin_url: String,
    db_name: String,
    storage: Storage,
    client: Client,
    bucket: String,
}

impl Fx {
    async fn new() -> Option<Self> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty())?;
        let mut cfg = S3Config::from_env().expect("S3_* env");
        cfg.bucket = format!("cyx-{}", Uuid::new_v4().simple());
        let client = Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version_latest()
                .region(Region::new(cfg.region.clone()))
                .credentials_provider(Credentials::new(&cfg.access_key, &cfg.secret_key, None, None, "static"))
                .force_path_style(cfg.force_path_style)
                .endpoint_url(cfg.endpoint.clone().unwrap())
                .build(),
        );
        client
            .create_bucket()
            .bucket(&cfg.bucket)
            .send()
            .await
            .expect("create bucket");
        client
            .put_bucket_versioning()
            .bucket(&cfg.bucket)
            .versioning_configuration(
                VersioningConfiguration::builder()
                    .status(BucketVersioningStatus::Enabled)
                    .build(),
            )
            .send()
            .await
            .expect("enable versioning");
        let db_name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin
            .execute(format!("CREATE DATABASE {db_name}").as_str())
            .await
            .unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{db_name}")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        Some(Self {
            pool,
            admin_url,
            db_name,
            storage: Storage::new(&cfg),
            client,
            bucket: cfg.bucket,
        })
    }

    async fn teardown(self) {
        self.pool.close().await;
        let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
        admin
            .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.db_name).as_str())
            .await
            .unwrap();
    }

    async fn put(&self, key: &str, body: &[u8]) {
        self.storage
            .put_bytes(key, body.to_vec(), "application/octet-stream")
            .await
            .unwrap();
    }

    /// Every version and delete marker in the bucket: `(key, is_delete_marker)`.
    async fn everything(&self) -> Vec<(String, bool)> {
        let out = self
            .client
            .list_object_versions()
            .bucket(&self.bucket)
            .send()
            .await
            .unwrap();
        let mut v: Vec<_> = out
            .versions()
            .iter()
            .map(|x| (x.key().unwrap().to_string(), false))
            .collect();
        v.extend(
            out.delete_markers()
                .iter()
                .map(|x| (x.key().unwrap().to_string(), true)),
        );
        v.sort();
        v
    }

    async fn count(&self, sql: &str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql).bind(id).fetch_one(&self.pool).await.unwrap()
    }
}

async fn user(pool: &PgPool, email: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ($1, 'x') RETURNING id")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A file with its objects in the bucket (an original written twice, so an old version stays, a thumbnail and a preview), a
/// job, an embedding-less row of everything else an account owns: album, share, API key, session.
async fn populate(fx: &Fx, owner: Uuid, tag: &str) -> Vec<String> {
    let sha = format!("{:064x}", tag.bytes().fold(7u128, |a, b| a * 131 + b as u128));
    let asset: Uuid = sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash)
         VALUES ($1, decode($2, 'hex'), $3, $4, 'image/jpeg', 100, '\\x01') RETURNING id",
    )
    .bind(owner)
    .bind(&sha)
    .bind(format!("originals/{owner}/{}/{tag}.jpg", &sha[..2]))
    .bind(format!("{tag}.jpg"))
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    let keys_written = vec![
        format!("originals/{owner}/{}/{tag}.jpg", &sha[..2]),
        keys::thumb(owner, asset),
        keys::preview(owner, asset),
        format!("manifests/{owner}/20260101.json"),
        format!("edits/{owner}/{}/{sha}.json", &sha[..2]),
    ];
    for k in &keys_written {
        fx.put(k, b"v1").await;
    }
    fx.put(&keys_written[0], b"v2").await; // an old version stays behind
    sqlx::query("INSERT INTO jobs (kind, asset_id) VALUES ('thumbnail', $1)")
        .bind(asset)
        .execute(&fx.pool)
        .await
        .unwrap();
    let album: Uuid = sqlx::query_scalar("INSERT INTO albums (owner_id, title) VALUES ($1, 'Album') RETURNING id")
        .bind(owner)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO album_assets (album_id, asset_id) VALUES ($1, $2)")
        .bind(album)
        .bind(asset)
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO shares (owner_id, token, kind, album_id) VALUES ($1, $2, 'album', $3)")
        .bind(owner)
        .bind(format!("{tag:x<30}"))
        .bind(album)
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO api_keys (user_id, name, key_hash) VALUES ($1, 'k', $2)")
        .bind(owner)
        .bind(tag.as_bytes())
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, now() + interval '1 day')")
        .bind(tag.as_bytes())
        .bind(owner)
        .execute(&fx.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO upload_log (owner_id, bytes) VALUES ($1, 100)")
        .bind(owner)
        .execute(&fx.pool)
        .await
        .unwrap();
    keys_written
}

#[test]
fn prefixes_end_with_the_account_and_a_slash() {
    let id = Uuid::new_v4();
    for p in accounts::prefixes(id) {
        assert!(p.ends_with(&format!("/{id}/")), "{p}");
    }
}

#[tokio::test]
async fn deleting_an_account_removes_all_of_its_objects_and_rows_and_nothing_of_anybody_elses() {
    let Some(fx) = Fx::new().await else { return };
    let (gone, kept) = (
        user(&fx.pool, "gone@example.com").await,
        user(&fx.pool, "kept@example.com").await,
    );
    let gone_keys = populate(&fx, gone, "gone").await;
    let kept_keys = populate(&fx, kept, "kept").await;
    // A prefix that merely starts like the account's id must survive too (ids never prefix each other, but the check is cheap).
    let neighbour = format!("originals/{gone}x/aa/neighbour.jpg");
    fx.put(&neighbour, b"n").await;
    let before = fx.everything().await;
    assert!(
        before.len() > gone_keys.len() + kept_keys.len(),
        "the old version of each original is in the bucket"
    );

    let report = accounts::delete_account(&fx.pool, &fx.storage, gone, Duration::ZERO)
        .await
        .unwrap();
    assert!(report.existed);
    assert_eq!((report.files, report.objects_deleted), (1, 6), "{report:?}"); // 5 objects + the old version of the original

    // The bucket: none of the account's keys in any version; the other account's and the neighbour's are exactly as they were.
    let after = fx.everything().await;
    let mine = |k: &str| accounts::prefixes(gone).iter().any(|p| k.starts_with(p.as_str()));
    assert!(after.iter().all(|(k, _)| !mine(k)), "{after:?}");
    let expected: Vec<_> = before.iter().filter(|(k, _)| !mine(k)).cloned().collect();
    assert_eq!(after, expected);
    assert!(after.iter().any(|(k, _)| k == &neighbour));
    for k in &kept_keys {
        assert!(fx.storage.head_size(k).await.unwrap().is_some(), "{k}");
    }

    // The rows: every one of the account's is gone; the other account's are all still there.
    for table in [
        "assets",
        "albums",
        "shares",
        "api_keys",
        "sessions",
        "upload_log",
        "user_usage",
    ] {
        let col = match table {
            "api_keys" | "sessions" | "user_usage" => "user_id",
            _ => "owner_id",
        };
        assert_eq!(
            fx.count(&format!("SELECT count(*) FROM {table} WHERE {col} = $1"), gone)
                .await,
            0,
            "{table}"
        );
        assert!(
            fx.count(&format!("SELECT count(*) FROM {table} WHERE {col} = $1"), kept)
                .await
                >= 1,
            "{table} of the other account"
        );
    }
    assert_eq!(fx.count("SELECT count(*) FROM users WHERE id = $1", gone).await, 0);
    assert_eq!(fx.count("SELECT count(*) FROM users WHERE id = $1", kept).await, 1);
    assert_eq!(
        fx.count(
            "SELECT count(*) FROM jobs WHERE asset_id IN (SELECT id FROM assets WHERE owner_id = $1)",
            kept
        )
        .await,
        1
    );
    assert_eq!(
        fx.count(
            "SELECT count(*) FROM album_assets WHERE album_id IN (SELECT id FROM albums WHERE owner_id = $1)",
            kept
        )
        .await,
        1
    );
    // The remaining account's counters are intact (the cascade did not touch them).
    let u = cyanotype_core::usage::of(&fx.pool, kept).await.unwrap();
    assert_eq!((u.originals_bytes, u.file_count), (100, 1));

    // Running it again is harmless (the account is gone, the objects too).
    let again = accounts::delete_account(&fx.pool, &fx.storage, gone, Duration::ZERO)
        .await
        .unwrap();
    assert!(!again.existed);
    assert_eq!(fx.everything().await, expected);
    fx.teardown().await;
}

#[tokio::test]
async fn a_failed_deletion_can_be_retried_and_unfinished_uploads_are_aborted() {
    let Some(fx) = Fx::new().await else { return };
    let gone = user(&fx.pool, "gone@example.com").await;
    populate(&fx, gone, "gone").await;
    // An upload in progress, with a part in the bucket.
    let key = format!("originals/{gone}/aa/pending.jpg");
    let upload_id = fx
        .storage
        .create_multipart(&key, "image/jpeg", Default::default())
        .await
        .unwrap();
    fx.storage
        .upload_part(&key, &upload_id, 1, vec![1u8; 10].into())
        .await
        .unwrap();
    sqlx::query("INSERT INTO uploads (upload_id, owner_id, sha256, key, size_bytes, part_size, original_name, content_type) VALUES ($1, $2, $3, $4, 10, 10, 'p.jpg', 'image/jpeg')")
        .bind(&upload_id)
        .bind(gone)
        .bind(vec![9u8; 32])
        .bind(&key)
        .execute(&fx.pool)
        .await
        .unwrap();
    // A job is running on one of its files: the deletion waits, gives up, and has changed nothing it cannot repeat.
    sqlx::query("UPDATE jobs SET state = 'running'")
        .execute(&fx.pool)
        .await
        .unwrap();
    let err = accounts::delete_account(&fx.pool, &fx.storage, gone, Duration::ZERO)
        .await
        .unwrap_err();
    assert!(err.downcast_ref::<JobsStillRunning>().is_some(), "{err:#}");
    // Already disabled, signed out, and its files marked so the worker stops making derived images for them.
    let disabled: bool = sqlx::query_scalar("SELECT disabled_at IS NOT NULL FROM users WHERE id = $1")
        .bind(gone)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert!(disabled);
    assert_eq!(
        fx.count("SELECT count(*) FROM sessions WHERE user_id = $1", gone).await,
        0
    );
    assert_eq!(
        fx.count(
            "SELECT count(*) FROM assets WHERE owner_id = $1 AND purging_at IS NOT NULL AND deleted_at IS NOT NULL",
            gone
        )
        .await,
        1
    );
    assert_eq!(
        fx.count("SELECT count(*) FROM users WHERE id = $1", gone).await,
        1,
        "not deleted yet"
    );

    // The job finishes; the retry completes.
    sqlx::query("UPDATE jobs SET state = 'done'")
        .execute(&fx.pool)
        .await
        .unwrap();
    let r = accounts::delete_account(&fx.pool, &fx.storage, gone, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(r.uploads_aborted, 1);
    assert!(fx.everything().await.is_empty(), "{:?}", fx.everything().await);
    assert!(
        fx.storage.list_parts(&key, &upload_id).await.unwrap().is_none(),
        "the multipart upload is gone"
    );
    assert_eq!(fx.count("SELECT count(*) FROM users WHERE id = $1", gone).await, 0);
    fx.teardown().await;
}
