//! Embedding storage, the embed-job sweep, the backfill migration and job priority. Need a real Postgres with
//! pgvector: set `DATABASE_URL` (see `tests/jobs.rs`); without it the tests return early.
use cyanotype_core::{db, embeddings, jobs};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

const MODEL: &str = "test-model";

struct TestDb {
    pool: PgPool,
    admin_url: String,
    name: String,
}

impl TestDb {
    async fn new() -> Option<Self> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
        db::migrate(&pool).await.unwrap();
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

/// A photo with derived images (thumbhash set) unless `preview` is false.
async fn asset(pool: &PgPool, owner: Uuid, name: &str, preview: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash)
         VALUES ($1, sha256($2::bytea), 'k', $2, 'image/jpeg', 1, CASE WHEN $3 THEN '\\x01'::bytea END) RETURNING id",
    )
    .bind(owner)
    .bind(format!("{name}{}", Uuid::new_v4()).as_bytes())
    .bind(preview)
    .fetch_one(pool)
    .await
    .map(|id: Uuid| id)
    .unwrap()
}

fn unit(seed: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..512).map(|i| ((i * 31 + seed * 17) % 97) as f32 - 48.0).collect();
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.into_iter().map(|x| x / n).collect()
}

async fn queued_embeds(pool: &PgPool) -> Vec<(Uuid, i32, i16)> {
    sqlx::query_as(
        "SELECT asset_id, version, priority FROM jobs WHERE kind = 'embed' AND state = 'queued' ORDER BY asset_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn schema_has_a_half_precision_column_and_an_hnsw_index() {
    let Some(t) = TestDb::new().await else { return };
    let (ty,): (String,) = sqlx::query_as(
        "SELECT format_type(atttypid, atttypmod) FROM pg_attribute
         WHERE attrelid = 'asset_embeddings'::regclass AND attname = 'embedding'",
    )
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(ty, "halfvec(512)");
    let (def,): (String,) =
        sqlx::query_as("SELECT indexdef FROM pg_indexes WHERE indexname = 'asset_embeddings_hnsw_idx'")
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert!(def.contains("hnsw") && def.contains("halfvec_cosine_ops"), "{def}");
    // pgvector 0.8 is what has iterative index scans, which filtered searches rely on.
    let (v,): (String,) = sqlx::query_as("SELECT extversion FROM pg_extension WHERE extname = 'vector'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let minor: u32 = v.split('.').nth(1).unwrap().parse().unwrap();
    assert!(v.starts_with("0.") && minor >= 8, "pgvector {v}");
    t.teardown().await;
}

#[tokio::test]
async fn store_writes_the_vector_and_a_new_model_replaces_the_old_one() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, "a", true).await;
    embeddings::store(&t.pool, a, MODEL, 1, &unit(1)).await.unwrap();
    // Same model again: updated in place (a re-run of the job), not duplicated.
    embeddings::store(&t.pool, a, MODEL, 2, &unit(2)).await.unwrap();
    let rows: Vec<(String, i32, f64)> = sqlx::query_as(
        "SELECT model, version, (embedding <=> $1::text::halfvec)::float8 FROM asset_embeddings WHERE asset_id = $2",
    )
    .bind(embeddings::pg_literal(&unit(2)))
    .bind(a)
    .fetch_all(&t.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].0.as_str(), rows[0].1), (MODEL, 2));
    assert!(
        rows[0].2 < 0.001,
        "stored vector is the one written (distance {})",
        rows[0].2
    );

    // A different model: the old generation goes in the same statement.
    embeddings::store(&t.pool, a, "other-model", 1, &unit(3)).await.unwrap();
    let models: Vec<(String,)> = sqlx::query_as("SELECT model FROM asset_embeddings WHERE asset_id = $1")
        .bind(a)
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(models, [("other-model".to_string(),)]);

    // Deleting the photo takes its vector with it.
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM asset_embeddings")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    t.teardown().await;
}

#[tokio::test]
async fn the_sweep_queues_exactly_the_visible_photos_without_a_current_vector() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let plain = asset(&t.pool, owner, "plain", true).await;
    let has_vector = asset(&t.pool, owner, "has", true).await;
    let trashed = asset(&t.pool, owner, "trashed", true).await;
    let _no_preview = asset(&t.pool, owner, "nopreview", false).await; // no preview yet: not queued
    let purging = asset(&t.pool, owner, "purging", true).await;
    let hidden_raw = asset(&t.pool, owner, "raw", true).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET deleted_at = now(), purging_at = now() WHERE id = $1")
        .bind(purging)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
        .bind(hidden_raw)
        .bind(plain)
        .execute(&t.pool)
        .await
        .unwrap();
    embeddings::store(&t.pool, has_vector, MODEL, 1, &unit(1))
        .await
        .unwrap();

    let n = embeddings::enqueue_missing(&t.pool, MODEL, 1).await.unwrap();
    let mut want = vec![plain, trashed]; // trashed photos too: restoring one needs no further work
    want.sort();
    let got: Vec<Uuid> = queued_embeds(&t.pool).await.into_iter().map(|r| r.0).collect();
    assert_eq!(got, want);
    assert_eq!(n, 2);
    assert!(
        queued_embeds(&t.pool)
            .await
            .iter()
            .all(|r| r.1 == 1 && r.2 == jobs::PRIORITY_BACKFILL)
    );

    // Idempotent: nothing new, nothing duplicated.
    assert_eq!(embeddings::enqueue_missing(&t.pool, MODEL, 1).await.unwrap(), 0);
    assert_eq!(queued_embeds(&t.pool).await.len(), 2);
    t.teardown().await;
}

#[tokio::test]
async fn a_model_version_bump_queues_every_photo_again_and_leaves_failures_alone() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, "a", true).await;
    let b = asset(&t.pool, owner, "b", true).await;
    let corrupt = asset(&t.pool, owner, "corrupt", true).await;
    for (id, ver) in [(a, 1), (b, 1)] {
        embeddings::store(&t.pool, id, MODEL, ver, &unit(1)).await.unwrap();
        jobs::enqueue(&t.pool, jobs::KIND_EMBED, id, ver).await.unwrap();
    }
    sqlx::query("UPDATE jobs SET state = 'done'")
        .execute(&t.pool)
        .await
        .unwrap();
    // A photo whose preview could not be decoded failed permanently: a sweep must not retry it forever.
    jobs::enqueue(&t.pool, jobs::KIND_EMBED, corrupt, 1).await.unwrap();
    sqlx::query("UPDATE jobs SET state = 'failed', last_error = 'not a WebP' WHERE asset_id = $1")
        .bind(corrupt)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        embeddings::enqueue_missing(&t.pool, MODEL, 1).await.unwrap(),
        0,
        "current: nothing to do"
    );

    // Version 2 of the same model, and a different model id at the same version: both mean "re-embed everything".
    for (model, version) in [(MODEL, 2), ("a-better-model", 1)] {
        sqlx::query("DELETE FROM jobs WHERE state = 'queued'")
            .execute(&t.pool)
            .await
            .unwrap();
        let n = embeddings::enqueue_missing(&t.pool, model, version).await.unwrap();
        let queued = queued_embeds(&t.pool).await;
        // The corrupt photo has a failed job only at version 1: at a new version it gets a fresh chance, as intended
        // (a new model may well decode what the old pipeline could not).
        assert!(
            queued.iter().any(|r| r.0 == a) && queued.iter().any(|r| r.0 == b),
            "{model} v{version}: {queued:?}"
        );
        assert!(n >= 2);
    }

    // The vectors are still the old ones until the worker replaces them; search keeps working on them meanwhile.
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM asset_embeddings WHERE model = $1 AND version = 1")
        .bind(MODEL)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 2);

    // A job that finished without producing a vector (skipped while the photo was a hidden RAW) is requeued once the
    // photo is visible and still has no vector for the same version.
    sqlx::query("DELETE FROM jobs").execute(&t.pool).await.unwrap();
    jobs::enqueue(&t.pool, jobs::KIND_EMBED, corrupt, 7).await.unwrap();
    sqlx::query("UPDATE jobs SET state = 'done'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(embeddings::enqueue_missing(&t.pool, MODEL, 7).await.unwrap(), 3);
    t.teardown().await;
}

#[tokio::test]
async fn migration_0012_backfills_low_priority_jobs_and_is_idempotent() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let plain = asset(&t.pool, owner, "plain", true).await;
    let _no_preview = asset(&t.pool, owner, "nopreview", false).await;
    let hidden = asset(&t.pool, owner, "raw", true).await;
    sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
        .bind(hidden)
        .bind(plain)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM jobs").execute(&t.pool).await.unwrap();

    let sql = include_str!("../migrations/0012_embed_backfill.sql");
    sqlx::raw_sql(sql).execute(&t.pool).await.unwrap();
    assert_eq!(queued_embeds(&t.pool).await, [(plain, 1, 10)]);
    sqlx::raw_sql(sql).execute(&t.pool).await.unwrap();
    assert_eq!(
        queued_embeds(&t.pool).await,
        [(plain, 1, 10)],
        "a second run adds nothing"
    );
    t.teardown().await;
}

#[tokio::test]
async fn jobs_are_claimed_by_priority_and_embeds_wait_for_foreground_work() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let backfill = asset(&t.pool, owner, "old", true).await;
    let fresh = asset(&t.pool, owner, "new", true).await;
    let thumb = asset(&t.pool, owner, "up", true).await;
    // The backfill job is older (lower id) than the fresh upload's, but the upload's is claimed first.
    jobs::enqueue_with_priority(&t.pool, jobs::KIND_EMBED, backfill, 1, jobs::PRIORITY_BACKFILL)
        .await
        .unwrap();
    jobs::enqueue(&t.pool, jobs::KIND_EMBED, fresh, 1).await.unwrap();
    let got = jobs::claim_batch(&t.pool, jobs::KIND_EMBED, 1).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].asset_id, fresh);
    let rest = jobs::claim_batch(&t.pool, jobs::KIND_EMBED, 5).await.unwrap();
    assert_eq!(rest.iter().map(|j| j.asset_id).collect::<Vec<_>>(), [backfill]);
    assert!(
        jobs::claim_batch(&t.pool, jobs::KIND_EMBED, 5)
            .await
            .unwrap()
            .is_empty()
    );

    // Within one priority: oldest first; batches are exclusive between claimers.
    sqlx::query("DELETE FROM jobs").execute(&t.pool).await.unwrap();
    for id in [backfill, fresh, thumb] {
        jobs::enqueue_with_priority(&t.pool, jobs::KIND_EMBED, id, 1, 10)
            .await
            .unwrap();
    }
    let (x, y) = tokio::join!(
        jobs::claim_batch(&t.pool, jobs::KIND_EMBED, 2),
        jobs::claim_batch(&t.pool, jobs::KIND_EMBED, 2)
    );
    let mut all: Vec<i64> = x.unwrap().into_iter().chain(y.unwrap()).map(|j| j.id).collect();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 3, "no job claimed twice, none lost");

    // The worker's embed loop asks this before every batch: any runnable foreground job means "not now".
    let fg = [jobs::KIND_METADATA, jobs::KIND_THUMBNAIL];
    assert!(!jobs::runnable_waiting(&t.pool, &fg).await.unwrap());
    jobs::enqueue(&t.pool, jobs::KIND_THUMBNAIL, thumb, 1).await.unwrap();
    assert!(jobs::runnable_waiting(&t.pool, &fg).await.unwrap());
    // A delayed retry is not "waiting".
    sqlx::query("UPDATE jobs SET run_after = now() + interval '1 hour' WHERE kind = 'thumbnail'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert!(!jobs::runnable_waiting(&t.pool, &fg).await.unwrap());
    t.teardown().await;
}

#[tokio::test]
async fn progress_counts_visible_live_photos_with_previews() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let other = user(&t.pool).await;
    let a = asset(&t.pool, owner, "a", true).await;
    let _b = asset(&t.pool, owner, "b", true).await;
    let trashed = asset(&t.pool, owner, "t", true).await;
    let _no_preview = asset(&t.pool, owner, "n", false).await;
    let _theirs = asset(&t.pool, other, "x", true).await;
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&t.pool)
        .await
        .unwrap();
    embeddings::store(&t.pool, a, MODEL, 1, &unit(1)).await.unwrap();
    assert_eq!(
        embeddings::progress(&t.pool, owner, MODEL, 1).await.unwrap(),
        embeddings::Progress { indexed: 1, total: 2 }
    );
    // Vectors from another model or version do not count as indexed for this one.
    assert_eq!(embeddings::progress(&t.pool, owner, MODEL, 2).await.unwrap().indexed, 0);
    t.teardown().await;
}
