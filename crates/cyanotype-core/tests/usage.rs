//! Usage counters (`user_usage`, migration 0017) over a real Postgres: the triggers under every way a file is added, changed
//! and removed, the nightly reconcile, the backfill by migration, and the bucket check against a real S3 API.
//! Need `DATABASE_URL` (see `tests/jobs.rs`) and, for the bucket tests, the `S3_*` variables; without them the tests
//! return early.
use aws_sdk_s3::{
    Client,
    config::{Credentials, Region},
    types::{BucketVersioningStatus, VersioningConfiguration},
};
use cyanotype_core::{
    admin_tasks,
    config::S3Config,
    db, keys, pairing,
    reindex::{self, Options},
    storage::Storage,
    usage::{self, Usage},
};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

struct TestDb {
    pool: PgPool,
    admin_url: String,
    name: String,
}

impl TestDb {
    async fn create() -> Option<(String, String, String)> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        Some((admin_url.clone(), name.clone(), format!("{base}/{name}")))
    }

    async fn new() -> Option<Self> {
        let (admin_url, name, url) = Self::create().await?;
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        Some(Self { pool, admin_url, name })
    }

    /// Migrated only up to `version`, to test what a migration does to rows that exist already.
    async fn new_at(version: i64) -> Option<Self> {
        let (admin_url, name, url) = Self::create().await?;
        let pool = db::connect(&url).await.unwrap();
        let upto: Vec<_> = db::MIGRATOR.iter().filter(|m| m.version <= version).cloned().collect();
        let partial = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(upto),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        partial.run(&pool).await.unwrap();
        Some(Self { pool, admin_url, name })
    }

    async fn teardown(self) {
        self.pool.close().await;
        let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
        admin
            .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str())
            .await
            .unwrap();
    }
}

async fn user(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ($1, 'x') RETURNING id")
        .bind(format!("{}@example.com", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn file(pool: &PgPool, owner: Uuid, name: &str, size: i64) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
         VALUES ($1, sha256($2::bytea), $3, $2, 'image/jpeg', $4) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(format!("originals/{owner}/{name}"))
    .bind(size)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn exec(pool: &PgPool, sql: &str, id: Uuid) {
    sqlx::query(sql).bind(id).execute(pool).await.unwrap();
}

/// The maintained row must equal the numbers recomputed from `assets`, and (when given) the expected ones.
async fn check(pool: &PgPool, owner: Uuid, what: &str) -> Usage {
    let kept = usage::of(pool, owner).await.unwrap();
    let truth = usage::truth_of(pool, owner).await.unwrap();
    assert_eq!(kept, truth, "{what}: the trigger-maintained row differs from the files");
    kept
}

#[tokio::test]
async fn a_new_account_has_no_usage() {
    let Some(t) = TestDb::new().await else { return };
    let u = user(&t.pool).await;
    assert_eq!(usage::of(&t.pool, u).await.unwrap(), Usage::default());
    t.teardown().await;
}

#[tokio::test]
async fn adding_trashing_restoring_pairing_and_purging_keep_the_counters_exact() {
    let Some(t) = TestDb::new().await else { return };
    let p = &t.pool;
    let (a, b) = (user(p).await, user(p).await);

    // Insert: a JPEG and a RAW (by extension), 1000 and 5000 bytes. Another account's file stays out of it.
    let jpg = file(p, a, "IMG_1.jpg", 1000).await;
    let raw = file(p, a, "IMG_1.CR3", 5000).await;
    file(p, b, "other.jpg", 70).await;
    let u = check(p, a, "insert").await;
    assert_eq!(
        (u.originals_bytes, u.photo_count, u.raw_count, u.file_count),
        (6000, 2, 1, 2)
    );
    assert_eq!((u.trash_bytes, u.trash_count), (0, 0));
    assert_eq!(check(p, b, "other account").await.originals_bytes, 70);

    // Pairing hides the RAW half: the library lists one photo, both files still count and the RAW is still a RAW.
    exec(p, &format!("UPDATE assets SET primary_id = '{jpg}' WHERE id = $1"), raw).await;
    let u = check(p, a, "pair").await;
    assert_eq!((u.originals_bytes, u.photo_count, u.raw_count), (6000, 1, 1));
    exec(p, "UPDATE assets SET primary_id = NULL WHERE id = $1", raw).await;
    assert_eq!(check(p, a, "unpair").await.photo_count, 2);

    // Trash: bytes move from originals to trash (counted until purged), counts follow.
    exec(p, "UPDATE assets SET deleted_at = now() WHERE id = $1", raw).await;
    let u = check(p, a, "trash").await;
    assert_eq!(
        (
            u.originals_bytes,
            u.trash_bytes,
            u.photo_count,
            u.raw_count,
            u.trash_count
        ),
        (1000, 5000, 1, 0, 1)
    );
    // Trashing again changes nothing; restore puts it back.
    exec(p, "UPDATE assets SET deleted_at = now() WHERE id = $1", raw).await;
    check(p, a, "trash twice").await;
    exec(p, "UPDATE assets SET deleted_at = NULL WHERE id = $1", raw).await;
    let u = check(p, a, "restore").await;
    assert_eq!((u.originals_bytes, u.trash_bytes, u.raw_count), (6000, 0, 1));

    // Delete forever: marked for purging (still in the trash), then the row goes.
    exec(
        p,
        "UPDATE assets SET deleted_at = now(), purging_at = now() WHERE id = $1",
        raw,
    )
    .await;
    assert_eq!(check(p, a, "marked for purge").await.trash_bytes, 5000);
    exec(p, "DELETE FROM assets WHERE id = $1", raw).await;
    let u = check(p, a, "purged").await;
    assert_eq!(
        (
            u.originals_bytes,
            u.trash_bytes,
            u.trash_count,
            u.raw_count,
            u.photo_count
        ),
        (1000, 0, 0, 0, 1)
    );

    // Deleting a live row and trashed-then-deleted ones.
    exec(p, "DELETE FROM assets WHERE id = $1", jpg).await;
    assert_eq!(check(p, a, "deleted live").await, Usage::default());
    t.teardown().await;
}

#[tokio::test]
async fn bulk_changes_and_ownership_moves_are_counted() {
    let Some(t) = TestDb::new().await else { return };
    let p = &t.pool;
    let (a, b) = (user(p).await, user(p).await);
    for i in 0..300 {
        file(p, a, &format!("f{i}.jpg"), 10).await;
    }
    assert_eq!(check(p, a, "bulk insert").await.originals_bytes, 3000);
    // "Empty trash" style: one statement trashes everything, then another deletes it.
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE owner_id = $1")
        .bind(a)
        .execute(p)
        .await
        .unwrap();
    let u = check(p, a, "bulk trash").await;
    assert_eq!((u.originals_bytes, u.trash_bytes, u.photo_count), (0, 3000, 0));
    // A file changing owner moves its bytes.
    let one = file(p, a, "move.jpg", 500).await;
    sqlx::query("UPDATE assets SET owner_id = $2 WHERE id = $1")
        .bind(one)
        .bind(b)
        .execute(p)
        .await
        .unwrap();
    assert_eq!(check(p, a, "owner left").await.originals_bytes, 0);
    assert_eq!(check(p, b, "owner arrived").await.originals_bytes, 500);
    sqlx::query("DELETE FROM assets WHERE owner_id = $1")
        .bind(a)
        .execute(p)
        .await
        .unwrap();
    assert_eq!(check(p, a, "bulk delete").await, Usage::default());
    // A size correction counts.
    sqlx::query("UPDATE assets SET size_bytes = 800 WHERE id = $1")
        .bind(one)
        .execute(p)
        .await
        .unwrap();
    assert_eq!(check(p, b, "resize").await.originals_bytes, 800);
    t.teardown().await;
}

#[tokio::test]
async fn derived_sizes_are_counted_as_the_worker_records_them() {
    let Some(t) = TestDb::new().await else { return };
    let p = &t.pool;
    let a = user(p).await;
    let x = file(p, a, "x.jpg", 100).await;
    let y = file(p, a, "y.jpg", 100).await;
    let u = check(p, a, "unmeasured").await;
    assert_eq!((u.derived_bytes, u.derived_files, u.file_count), (0, 0, 2));
    sqlx::query("UPDATE assets SET derived_bytes = 30 WHERE id = $1")
        .bind(x)
        .execute(p)
        .await
        .unwrap();
    let u = check(p, a, "thumbnail written").await;
    assert_eq!((u.derived_bytes, u.derived_files), (30, 1));
    // The share copy adds to the same file; a re-render replaces the thumbnail part only.
    sqlx::query("UPDATE assets SET share_copy_bytes = 90 WHERE id = $1")
        .bind(x)
        .execute(p)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET derived_bytes = 40 WHERE id = $1")
        .bind(x)
        .execute(p)
        .await
        .unwrap();
    assert_eq!(check(p, a, "share copy and re-render").await.derived_bytes, 130);
    // Derived images stay while the photo is in the trash and go when it does.
    exec(p, "UPDATE assets SET deleted_at = now() WHERE id = $1", x).await;
    assert_eq!(check(p, a, "trashed").await.derived_bytes, 130);
    exec(p, "DELETE FROM assets WHERE id = $1", x).await;
    assert_eq!(check(p, a, "purged").await.derived_bytes, 0);
    let _ = y;
    t.teardown().await;
}

#[tokio::test]
async fn deleting_an_account_with_files_works() {
    let Some(t) = TestDb::new().await else { return };
    let p = &t.pool;
    let (a, b) = (user(p).await, user(p).await);
    file(p, a, "a.jpg", 10).await;
    file(p, b, "b.jpg", 20).await;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(a)
        .execute(p)
        .await
        .unwrap();
    let (rows,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_usage WHERE user_id = $1")
        .bind(a)
        .fetch_one(p)
        .await
        .unwrap();
    assert_eq!(rows, 0, "no usage row is left (or created) for a deleted account");
    assert_eq!(check(p, b, "the other account").await.originals_bytes, 20);
    t.teardown().await;
}

#[tokio::test]
async fn the_sql_raw_list_matches_the_pairing_list() {
    let Some(t) = TestDb::new().await else { return };
    for ext in pairing::RAW_EXTS {
        let raw: bool = sqlx::query_scalar("SELECT asset_is_raw($1)")
            .bind(*ext)
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert!(raw, "{ext}");
    }
    for ext in pairing::PRIMARY_EXTS.iter().chain(&["png", "webp", "tif", "mp4"]) {
        let raw: bool = sqlx::query_scalar("SELECT asset_is_raw($1)")
            .bind(*ext)
            .fetch_one(&t.pool)
            .await
            .unwrap();
        assert!(!raw, "{ext}");
    }
    let none: Option<bool> = sqlx::query_scalar("SELECT asset_is_raw(NULL)")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(none, None);
    t.teardown().await;
}

#[tokio::test]
async fn the_migration_fills_the_table_from_existing_files() {
    let Some(t) = TestDb::new_at(16).await else { return };
    let p = &t.pool;
    let a: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ('a@example.com', 'x') RETURNING id")
            .fetch_one(p)
            .await
            .unwrap();
    for (name, size, trashed) in [("1.jpg", 100, false), ("2.dng", 400, false), ("3.jpg", 30, true)] {
        let id = file_old(p, a, name, size).await;
        if trashed {
            exec(p, "UPDATE assets SET deleted_at = now() WHERE id = $1", id).await;
        }
    }
    db::migrate(p).await.unwrap();
    let u = check(p, a, "after the migration").await;
    assert_eq!(
        (
            u.originals_bytes,
            u.trash_bytes,
            u.photo_count,
            u.raw_count,
            u.trash_count
        ),
        (500, 30, 2, 1, 1)
    );
    t.teardown().await;
}

/// `file` before the migration exists (the same insert; named apart because it runs on the older schema).
async fn file_old(pool: &PgPool, owner: Uuid, name: &str, size: i64) -> Uuid {
    file(pool, owner, name, size).await
}

#[tokio::test]
async fn reconcile_finds_and_repairs_drift_and_is_quiet_otherwise() {
    let Some(t) = TestDb::new().await else { return };
    let p = &t.pool;
    let (a, b, c) = (user(p).await, user(p).await, user(p).await);
    file(p, a, "a.jpg", 100).await;
    file(p, b, "b.jpg", 200).await;
    let r = usage::reconcile(p).await.unwrap();
    assert_eq!((r.users_checked, r.drifted.len()), (3, 0), "{:?}", r.drifted);

    // Counters that are wrong (as if a path had been missed, or SQL ran with triggers disabled).
    sqlx::query("UPDATE user_usage SET originals_bytes = originals_bytes + 55, photo_count = 9 WHERE user_id = $1")
        .bind(a)
        .execute(p)
        .await
        .unwrap();
    sqlx::query("DELETE FROM user_usage WHERE user_id = $1")
        .bind(b)
        .execute(p)
        .await
        .unwrap();
    // And a row for an account with no files that claims some.
    sqlx::query("UPDATE user_usage SET trash_bytes = 77, trash_count = 1 WHERE user_id = $1")
        .bind(c)
        .execute(p)
        .await
        .unwrap();

    let r = usage::reconcile(p).await.unwrap();
    let mut drifted: Vec<_> = r.drifted.iter().map(|d| d.user_id).collect();
    drifted.sort();
    let mut want = vec![a, b, c];
    want.sort();
    assert_eq!(drifted, want);
    let da = r.drifted.iter().find(|d| d.user_id == a).unwrap();
    assert_eq!(da.stored_bytes_off(), 55);
    assert_eq!((da.stored.photo_count, da.truth.photo_count), (9, 1));
    assert_eq!(check(p, a, "repaired").await.originals_bytes, 100);
    assert_eq!(check(p, b, "restored").await.originals_bytes, 200);
    assert_eq!(check(p, c, "emptied").await, Usage::default());

    // Repaired: the next run is quiet.
    assert!(usage::reconcile(p).await.unwrap().drifted.is_empty());
    // And the report is recorded for the admin page.
    usage::reconcile_and_report(p).await.unwrap();
    let (drift,): (i64,) =
        sqlx::query_as("SELECT (value->>'drifted_users')::bigint FROM server_state WHERE key = 'usage_reconcile'")
            .fetch_one(p)
            .await
            .unwrap();
    assert_eq!(drift, 0);
    t.teardown().await;
}

#[tokio::test]
async fn reconcile_prunes_only_old_upload_log_rows() {
    let Some(t) = TestDb::new().await else { return };
    let p = &t.pool;
    let a = user(p).await;
    sqlx::query("INSERT INTO upload_log (owner_id, at, bytes) VALUES ($1, now() - interval '91 days', 5), ($1, now() - interval '31 days', 6), ($1, now(), 7)")
        .bind(a)
        .execute(p)
        .await
        .unwrap();
    let r = usage::reconcile(p).await.unwrap();
    assert_eq!(r.log_rows_pruned, 1);
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM upload_log")
        .fetch_one(p)
        .await
        .unwrap();
    assert_eq!(n, 2);
    t.teardown().await;
}

// ---- admin tasks

#[tokio::test]
async fn admin_tasks_queue_one_bucket_check_at_a_time_and_reclaim_abandoned_ones() {
    let Some(t) = TestDb::new().await else { return };
    let p = &t.pool;
    let (first, created) = admin_tasks::enqueue(p, admin_tasks::KIND_BUCKET_CHECK, None, serde_json::json!({}))
        .await
        .unwrap();
    assert!(created);
    // A second request while one is open returns the open one.
    let (again, created) = admin_tasks::enqueue(p, admin_tasks::KIND_BUCKET_CHECK, None, serde_json::json!({}))
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(again.id, first.id);

    let claimed = admin_tasks::claim(p, &[admin_tasks::KIND_BUCKET_CHECK])
        .await
        .unwrap()
        .unwrap();
    assert_eq!((claimed.id, claimed.state.as_str()), (first.id, "running"));
    // Running: nobody else gets it, until it is old enough to be taken for abandoned.
    assert!(
        admin_tasks::claim(p, &[admin_tasks::KIND_BUCKET_CHECK])
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("UPDATE admin_tasks SET started_at = now() - interval '3 hours'")
        .execute(p)
        .await
        .unwrap();
    let again = admin_tasks::claim(p, &[admin_tasks::KIND_BUCKET_CHECK])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.id, first.id);

    admin_tasks::finish(p, first.id, Ok(serde_json::json!({"ok": true})))
        .await
        .unwrap();
    let last = admin_tasks::latest(p, admin_tasks::KIND_BUCKET_CHECK)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (last.state.as_str(), last.result),
        ("done", Some(serde_json::json!({"ok": true})))
    );
    // Done: a new one can be queued, and a failure keeps its message.
    let (next, created) = admin_tasks::enqueue(p, admin_tasks::KIND_BUCKET_CHECK, None, serde_json::json!({}))
        .await
        .unwrap();
    assert!(created && next.id != first.id);
    admin_tasks::finish(p, next.id, Err("storage is down".into()))
        .await
        .unwrap();
    let last = admin_tasks::latest(p, admin_tasks::KIND_BUCKET_CHECK)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (last.state.as_str(), last.error.as_deref()),
        ("failed", Some("storage is down"))
    );
    // Other kinds are not claimed by a worker that does not handle them.
    admin_tasks::enqueue(p, "something_else", None, serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        admin_tasks::claim(p, &[admin_tasks::KIND_BUCKET_CHECK])
            .await
            .unwrap()
            .is_none()
    );
    t.teardown().await;
}

// ---- against a real bucket

struct S3Fx {
    db: TestDb,
    storage: Storage,
    client: Client,
    bucket: String,
}

impl S3Fx {
    async fn new() -> Option<Self> {
        std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty())?;
        let db = TestDb::new().await?;
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
        Some(Self {
            db,
            storage: Storage::new(&cfg),
            client,
            bucket: cfg.bucket,
        })
    }

    async fn put(&self, key: &str, body: Vec<u8>) {
        self.storage
            .put_bytes(key, body, "application/octet-stream")
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn reindex_counts_what_it_finds_in_the_bucket() {
    let Some(fx) = S3Fx::new().await else { return };
    let owner = user(&fx.db.pool).await;
    let body = vec![7u8; 1234];
    let sha: [u8; 32] = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&body).into()
    };
    fx.put(&keys::original(owner, &sha, "jpg"), body).await;
    reindex::run(
        &fx.db.pool,
        &fx.storage,
        &Options {
            apply: true,
            min_age: std::time::Duration::ZERO,
            ..Options::default()
        },
    )
    .await
    .unwrap();
    let u = check(&fx.db.pool, owner, "after reindex").await;
    assert_eq!((u.originals_bytes, u.file_count), (1234, 1));
    fx.db.teardown().await;
}

#[tokio::test]
async fn measuring_derived_sizes_catches_up_files_rendered_before_sizes_were_recorded() {
    let Some(fx) = S3Fx::new().await else { return };
    let p = &fx.db.pool;
    let a = user(p).await;
    let x = file(p, a, "x.jpg", 10).await;
    let y = file(p, a, "y.jpg", 10).await; // no thumbnail yet: left alone
    let z = file(p, a, "z.jpg", 10).await; // thumbnail row says done but the objects are gone: left alone
    for id in [x, z] {
        sqlx::query("UPDATE assets SET thumbhash = '\\x01' WHERE id = $1")
            .bind(id)
            .execute(p)
            .await
            .unwrap();
    }
    fx.put(&keys::thumb(a, x), vec![0; 100]).await;
    fx.put(&keys::preview(a, x), vec![0; 400]).await;
    let n = usage::measure_derived(p, &fx.storage, 100).await.unwrap();
    assert_eq!(n, 1);
    let u = check(p, a, "measured").await;
    assert_eq!((u.derived_bytes, u.derived_files), (500, 1));
    // Idempotent.
    assert_eq!(usage::measure_derived(p, &fx.storage, 100).await.unwrap(), 0);
    let _ = y;
    fx.db.teardown().await;
}

#[tokio::test]
async fn bucket_check_compares_the_bucket_with_the_database() {
    let Some(fx) = S3Fx::new().await else { return };
    let p = &fx.db.pool;
    let (a, b) = (user(p).await, user(p).await);
    let stranger = Uuid::new_v4();

    // Account a: two files in the database, both in the bucket, one overwritten once (an old version stays).
    for (i, size) in [(1u8, 300usize), (2, 200)] {
        let name = format!("{i}.jpg");
        let id = file(p, a, &name, size as i64).await;
        sqlx::query("UPDATE assets SET original_key = $2 WHERE id = $1")
            .bind(id)
            .bind(format!("originals/{a}/{i:02}/{name}"))
            .execute(p)
            .await
            .unwrap();
        fx.put(&format!("originals/{a}/{i:02}/{name}"), vec![i; size]).await;
    }
    fx.put(&format!("originals/{a}/01/1.jpg"), vec![1; 300]).await;
    // Account b: the database says 50 bytes, the bucket holds nothing for it.
    file(p, b, "gone.jpg", 50).await;
    // An original under an account that does not exist, a derived file, a manifest and an unrelated key.
    fx.put(&format!("originals/{stranger}/aa/x.jpg"), vec![0; 40]).await;
    fx.put(&format!("derived/{a}/{}/thumb.webp", Uuid::new_v4()), vec![0; 25])
        .await;
    fx.put(&format!("manifests/{a}/m.json"), vec![0; 10]).await;
    fx.put("backups/pg/x.dump", vec![0; 5]).await;

    let r = usage::bucket_check(p, &fx.storage).await.unwrap();
    assert!(r.versioned);
    assert_eq!(r.originals_bytes, 300 + 200 + 40);
    assert_eq!((r.derived_bytes, r.manifests_bytes, r.other_bytes), (25, 10, 5));
    assert_eq!((r.noncurrent_versions, r.noncurrent_bytes), (1, 300));
    assert_eq!(r.unknown_account_bytes, 40);
    assert_eq!(r.db_originals_bytes, 500 + 50);
    assert_eq!(r.mismatched.len(), 1);
    assert_eq!(
        (
            r.mismatched[0].user_id,
            r.mismatched[0].bucket_originals_bytes,
            r.mismatched[0].db_originals_bytes
        ),
        (b, 0, 50)
    );
    let _ = &fx.client;
    let _ = &fx.bucket;
    fx.db.teardown().await;
}
