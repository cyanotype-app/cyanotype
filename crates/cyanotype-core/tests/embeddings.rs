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

const CLIP: &str = "clip-vit-b32-openai-int8";
const SIGLIP: &str = "siglip2-base-patch16-224-q4";

fn vector(dim: usize, seed: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..dim).map(|i| ((i * 31 + seed * 17) % 97) as f32 - 48.0).collect();
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.into_iter().map(|x| x / n).collect()
}

#[tokio::test]
async fn schema_holds_any_dimension_with_one_hnsw_index_per_model() {
    let Some(t) = TestDb::new().await else { return };
    let (ty,): (String,) = sqlx::query_as(
        "SELECT format_type(atttypid, atttypmod) FROM pg_attribute
         WHERE attrelid = 'asset_embeddings'::regclass AND attname = 'embedding'",
    )
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(ty, "halfvec", "no fixed dimension: each model's index fixes its own");
    let defs: Vec<(String, String)> = sqlx::query_as(
        "SELECT indexname, indexdef FROM pg_indexes WHERE tablename = 'asset_embeddings' AND indexdef LIKE '%hnsw%' ORDER BY 1",
    )
    .fetch_all(&t.pool)
    .await
    .unwrap();
    assert_eq!(defs.len(), 2, "{defs:?}");
    for (name, def, dim, model) in [
        ("asset_embeddings_clip_b32_hnsw", &defs[0], 512, CLIP),
        ("asset_embeddings_siglip2_b16_hnsw", &defs[1], 768, SIGLIP),
    ] {
        assert_eq!(def.0, name);
        assert!(def.1.contains("halfvec_cosine_ops"), "{}", def.1);
        assert!(def.1.contains(&format!("halfvec({dim})")), "{}", def.1);
        assert!(def.1.contains(&format!("model = '{model}'")), "{}", def.1);
    }
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
async fn two_models_share_the_table_and_each_nearest_neighbour_search_uses_its_own_index() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, "a", true).await;
    let b = asset(&t.pool, owner, "b", true).await;
    // The same photos, embedded by both models.
    for (i, id) in [a, b].into_iter().enumerate() {
        embeddings::store(&t.pool, id, CLIP, 1, &vector(512, i)).await.unwrap();
        embeddings::store(&t.pool, id, SIGLIP, 2, &vector(768, i))
            .await
            .unwrap();
    }
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM asset_embeddings")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 4, "a new model's vector does not replace the old one");

    // A vector of the wrong size for its model is refused by the model's index, not stored.
    let wrong = embeddings::store(&t.pool, a, SIGLIP, 2, &vector(512, 3)).await;
    assert!(wrong.is_err(), "512 dimensions under the 768-dimension model");
    let wrong = embeddings::store(&t.pool, a, CLIP, 1, &vector(768, 3)).await;
    assert!(wrong.is_err(), "768 dimensions under the 512-dimension model");

    // Each model's search reads only its own vectors, through its own index.
    let mut conn = t.pool.acquire().await.unwrap();
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut *conn)
        .await
        .unwrap();
    for (model, dim, index) in [
        (CLIP, 512, "asset_embeddings_clip_b32_hnsw"),
        (SIGLIP, 768, "asset_embeddings_siglip2_b16_hnsw"),
    ] {
        let q = embeddings::pg_literal(&vector(dim, 0));
        let plan: Vec<(String,)> = sqlx::query_as(&format!(
            "EXPLAIN SELECT asset_id FROM asset_embeddings e WHERE e.model = '{model}'
             ORDER BY (e.embedding::halfvec({dim})) <=> '{q}'::text::halfvec({dim}) LIMIT 5"
        ))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        let text = plan.into_iter().map(|r| r.0).collect::<Vec<_>>().join("\n");
        assert!(text.contains(index), "{model}: {text}");
        let nearest: Vec<(Uuid, f64)> = sqlx::query_as(&format!(
            "SELECT asset_id, (e.embedding::halfvec({dim}) <=> '{q}'::text::halfvec({dim}))::float8
             FROM asset_embeddings e WHERE e.model = '{model}' ORDER BY 2 LIMIT 5"
        ))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(nearest.len(), 2);
        assert_eq!(nearest[0].0, a);
        assert!(nearest[0].1 < 1e-3, "{nearest:?}");
    }
    drop(conn);

    // Once every photo has the new model's vector, the replaced model's rows go (and nothing else does).
    assert_eq!(embeddings::missing_count(&t.pool, SIGLIP, 2).await.unwrap(), 0);
    assert_eq!(embeddings::missing_count(&t.pool, "other", 1).await.unwrap(), 2);
    assert_eq!(embeddings::prune_superseded(&t.pool, SIGLIP, 2).await.unwrap(), 2);
    let left: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT model FROM asset_embeddings")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(left, [(SIGLIP.to_string(),)]);
    assert_eq!(
        embeddings::prune_superseded(&t.pool, SIGLIP, 2).await.unwrap(),
        0,
        "idempotent"
    );
    t.teardown().await;
}

#[tokio::test]
async fn store_replaces_the_same_models_vector_and_keeps_other_models() {
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

    // A different model: both stay (the old one answers searches until the new one has covered the library).
    embeddings::store(&t.pool, a, "other-model", 1, &unit(3)).await.unwrap();
    let models: Vec<(String,)> =
        sqlx::query_as("SELECT model FROM asset_embeddings WHERE asset_id = $1 ORDER BY model")
            .bind(a)
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert_eq!(models, [("other-model".to_string(),), (MODEL.to_string(),)]);

    // Deleting the photo takes its vectors with it.
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

/// What a deploy does to a library that already has CLIP vectors: migrations up to 0012 were applied (the one-model schema),
/// photos have their vectors, then 0013 runs. The vectors must still be there and findable through the rebuilt index, and a
/// second model's vectors must be accepted next to them.
#[tokio::test]
async fn migration_0013_keeps_the_vectors_of_a_library_that_already_has_clip() {
    let Some(admin_url) = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty()) else {
        return;
    };
    let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
    let mut admin = PgConnection::connect(&admin_url).await.unwrap();
    admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
    let (base, _) = admin_url.rsplit_once('/').unwrap();
    let pool = db::connect(&format!("{base}/{name}")).await.unwrap();

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "sql"))
        .collect();
    files.sort();
    let (before, after): (Vec<_>, Vec<_>) = files
        .iter()
        .partition(|p| p.file_name().unwrap().to_str().unwrap() < "0013");
    assert_eq!(after.len(), 1, "0013 is the last migration this test knows about");
    for f in before {
        sqlx::raw_sql(&std::fs::read_to_string(f).unwrap())
            .execute(&pool)
            .await
            .unwrap();
    }

    // The one-model schema: halfvec(512) and one index.
    let owner = user(&pool).await;
    let ids: Vec<Uuid> = {
        let mut v = Vec::new();
        for i in 0..30 {
            let id = asset(&pool, owner, &format!("p{i}"), true).await;
            embeddings::store(&pool, id, CLIP, 1, &vector(512, i)).await.unwrap();
            v.push(id);
        }
        v
    };
    sqlx::raw_sql(&std::fs::read_to_string(after[0]).unwrap())
        .execute(&pool)
        .await
        .unwrap();

    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM asset_embeddings WHERE model = $1 AND version = 1")
        .bind(CLIP)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 30, "no vector was lost");
    let q = embeddings::pg_literal(&vector(512, 7));
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut *conn)
        .await
        .unwrap();
    let nearest: Uuid = sqlx::query_scalar(&format!(
        "SELECT asset_id FROM asset_embeddings e WHERE e.model = '{CLIP}'
         ORDER BY (e.embedding::halfvec(512)) <=> '{q}'::text::halfvec(512) LIMIT 1"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(nearest, ids[7], "the rebuilt CLIP index finds the old vectors");
    drop(conn);
    // The new model's vectors go next to them, and the sweep for it queues every photo.
    embeddings::store(&pool, ids[0], SIGLIP, 2, &vector(768, 0))
        .await
        .unwrap();
    assert_eq!(embeddings::missing_count(&pool, SIGLIP, 2).await.unwrap(), 29);
    assert_eq!(embeddings::enqueue_missing(&pool, SIGLIP, 2).await.unwrap(), 29);
    let (versions,): (Vec<i32>,) =
        sqlx::query_as("SELECT array_agg(DISTINCT version ORDER BY version) FROM jobs WHERE kind = 'embed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(versions.contains(&2), "{versions:?}");

    pool.close().await;
    let mut admin = PgConnection::connect(&admin_url).await.unwrap();
    admin
        .execute(format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)").as_str())
        .await
        .unwrap();
}
