//! Face storage, the `faces` job sweep, progress, the purge cascade and the delete-everything command. Need a real Postgres with
//! pgvector: set `DATABASE_URL` (see `tests/jobs.rs`); without it the tests return early.
use cyanotype_core::{
    db,
    faces::{self, DIM, MODEL, NewFace, VERSION},
    jobs,
};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

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

/// A photo with a preview (thumbhash set) unless `preview` is false.
async fn asset(pool: &PgPool, owner: Uuid, preview: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash)
         VALUES ($1, sha256($2::bytea), 'k', 'p.jpg', 'image/jpeg', 1, CASE WHEN $3 THEN '\\x01'::bytea END) RETURNING id",
    )
    .bind(owner)
    .bind(Uuid::new_v4().as_bytes().to_vec())
    .bind(preview)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn unit(seed: usize) -> Vec<f32> {
    let v: Vec<f32> = (0..DIM).map(|i| ((i * 31 + seed * 17) % 97) as f32 - 48.0).collect();
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.into_iter().map(|x| x / n).collect()
}

fn face(seed: usize, embedding: bool) -> NewFace {
    NewFace {
        bbox: [0.1 * seed as f32, 0.2, 0.2, 0.25],
        landmarks: [0.1, 0.2, 0.3, 0.2, 0.2, 0.3, 0.12, 0.4, 0.28, 0.4],
        score: 0.9,
        embedding: embedding.then(|| unit(seed)),
    }
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn storing_twice_replaces_the_rows_and_never_duplicates() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, true).await;
    assert!(
        faces::store(
            &t.pool,
            a,
            MODEL,
            VERSION,
            &[face(1, true), face(2, true), face(3, false)]
        )
        .await
        .unwrap()
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 3);
    // The same job again with one face fewer: the old rows are gone, not added to.
    faces::store(&t.pool, a, MODEL, VERSION, &[face(4, true), face(5, true)])
        .await
        .unwrap();
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 2);
    assert_eq!(count(&t.pool, "SELECT faces::bigint FROM face_scans").await, 2);
    // Owner and model came along; the box and landmarks round-trip; a face without a fingerprint has a NULL one.
    let (o, model, idx, x, lm): (Uuid, String, i16, f32, Vec<f32>) = sqlx::query_as(
        "SELECT owner_id, model, idx, box_x, landmarks FROM faces WHERE asset_id = $1 ORDER BY idx LIMIT 1",
    )
    .bind(a)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!((o, model.as_str(), idx), (owner, MODEL, 0));
    assert!((x - 0.4).abs() < 1e-6 && lm.len() == 10);
    faces::store(&t.pool, a, MODEL, VERSION, &[face(1, false)])
        .await
        .unwrap();
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM faces WHERE embedding IS NULL").await,
        1
    );
    // A photo with no faces still counts as scanned.
    let b = asset(&t.pool, owner, true).await;
    faces::store(&t.pool, b, MODEL, VERSION, &[]).await.unwrap();
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_scans").await, 2);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 1);
    t.teardown().await;
}

#[tokio::test]
async fn a_fingerprint_is_stored_as_a_halfvec_the_index_can_search() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let mut ids = Vec::new();
    for s in 1..=5 {
        let a = asset(&t.pool, owner, true).await;
        faces::store(&t.pool, a, MODEL, VERSION, &[face(s, true)])
            .await
            .unwrap();
        ids.push(a);
    }
    // The nearest face to photo 3's own fingerprint is photo 3's face (the query repeats the index's expression and predicate).
    let q = cyanotype_core::embeddings::pg_literal(&unit(3));
    let nearest: Uuid = sqlx::query_scalar(&format!(
        "SELECT asset_id FROM faces WHERE model = '{MODEL}' AND embedding IS NOT NULL
         ORDER BY embedding::halfvec({DIM}) <=> $1::text::halfvec({DIM}) LIMIT 1"
    ))
    .bind(&q)
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(nearest, ids[2]);
    // A vector of the wrong size for the model is refused by the index's cast, not stored silently.
    let wrong = faces::store(
        &t.pool,
        ids[0],
        MODEL,
        VERSION,
        &[NewFace {
            embedding: Some(vec![0.5; 7]),
            ..face(1, false)
        }],
    )
    .await;
    assert!(wrong.is_err());
    t.teardown().await;
}

#[tokio::test]
async fn nothing_is_stored_for_a_photo_that_is_being_deleted() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, true).await;
    sqlx::query("UPDATE assets SET purging_at = now(), deleted_at = now() WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    assert!(
        !faces::store(&t.pool, a, MODEL, VERSION, &[face(1, true)])
            .await
            .unwrap()
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 0);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_scans").await, 0);
    t.teardown().await;
}

#[tokio::test]
async fn the_sweep_queues_unscanned_photos_once_at_backfill_priority() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let scanned = asset(&t.pool, owner, true).await;
    let waiting = asset(&t.pool, owner, true).await;
    let no_preview = asset(&t.pool, owner, false).await;
    let trashed_for_good = asset(&t.pool, owner, true).await;
    sqlx::query("UPDATE assets SET purging_at = now(), deleted_at = now() WHERE id = $1")
        .bind(trashed_for_good)
        .execute(&t.pool)
        .await
        .unwrap();
    faces::store(&t.pool, scanned, MODEL, VERSION, &[]).await.unwrap();

    assert_eq!(faces::missing_count(&t.pool, MODEL, VERSION).await.unwrap(), 1);
    assert_eq!(faces::enqueue_missing(&t.pool, MODEL, VERSION).await.unwrap(), 1);
    // Running the sweep again queues nothing new.
    assert_eq!(faces::enqueue_missing(&t.pool, MODEL, VERSION).await.unwrap(), 0);
    let queued: Vec<(Uuid, i32, i16)> =
        sqlx::query_as("SELECT asset_id, version, priority FROM jobs WHERE kind = 'faces'")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert_eq!(queued, vec![(waiting, VERSION, jobs::PRIORITY_BACKFILL)]);
    assert!(!queued.iter().any(|q| q.0 == no_preview));

    // A job that finished without a scan (lost work) is queued again; a failed one is left for the owner's retry.
    sqlx::query("UPDATE jobs SET state = 'done' WHERE kind = 'faces'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(faces::enqueue_missing(&t.pool, MODEL, VERSION).await.unwrap(), 1);
    sqlx::query("UPDATE jobs SET state = 'failed' WHERE kind = 'faces'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(faces::enqueue_missing(&t.pool, MODEL, VERSION).await.unwrap(), 0);

    // A new model version scans everything again.
    assert_eq!(faces::missing_count(&t.pool, MODEL, VERSION + 1).await.unwrap(), 2);
    t.teardown().await;
}

#[tokio::test]
async fn progress_counts_one_owners_visible_photos_only() {
    let Some(t) = TestDb::new().await else { return };
    let (me, other) = (user(&t.pool).await, user(&t.pool).await);
    let a = asset(&t.pool, me, true).await;
    let _b = asset(&t.pool, me, true).await;
    let _no_preview = asset(&t.pool, me, false).await;
    let theirs = asset(&t.pool, other, true).await;
    faces::store(&t.pool, a, MODEL, VERSION, &[face(1, true), face(2, true)])
        .await
        .unwrap();
    faces::store(&t.pool, theirs, MODEL, VERSION, &[face(3, true)])
        .await
        .unwrap();
    let p = faces::progress(&t.pool, me, MODEL, VERSION).await.unwrap();
    assert_eq!((p.scanned, p.total, p.faces), (1, 2, 2));
    // An older version's scan does not count for the current one.
    let p = faces::progress(&t.pool, me, MODEL, VERSION + 1).await.unwrap();
    assert_eq!((p.scanned, p.total, p.faces), (0, 2, 0));
    t.teardown().await;
}

#[tokio::test]
async fn deleting_a_photo_or_an_account_takes_its_faces_with_it() {
    let Some(t) = TestDb::new().await else { return };
    let (me, other) = (user(&t.pool).await, user(&t.pool).await);
    let a = asset(&t.pool, me, true).await;
    let b = asset(&t.pool, me, true).await;
    let c = asset(&t.pool, other, true).await;
    for x in [a, b, c] {
        faces::store(&t.pool, x, MODEL, VERSION, &[face(1, true)])
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 2);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_scans").await, 2);
    sqlx::query("DELETE FROM assets WHERE owner_id = $1")
        .bind(me)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(me)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 1);
    t.teardown().await;
}

#[tokio::test]
async fn delete_all_counts_first_and_removes_only_face_data() {
    let Some(t) = TestDb::new().await else { return };
    let (me, other) = (user(&t.pool).await, user(&t.pool).await);
    let a = asset(&t.pool, me, true).await;
    let c = asset(&t.pool, other, true).await;
    faces::store(&t.pool, a, MODEL, VERSION, &[face(1, true), face(2, true)])
        .await
        .unwrap();
    faces::store(&t.pool, c, MODEL, VERSION, &[face(3, true)])
        .await
        .unwrap();
    faces::enqueue_for_asset(&t.pool, a, VERSION).await.unwrap();
    jobs::enqueue(&t.pool, jobs::KIND_EMBED, a, 2).await.unwrap();

    let d = faces::delete_all(&t.pool, Some(me), true).await.unwrap();
    assert_eq!((d.faces, d.scans, d.jobs), (2, 1, 1));
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM faces").await,
        3,
        "a dry run deletes nothing"
    );

    let d = faces::delete_all(&t.pool, Some(me), false).await.unwrap();
    assert_eq!((d.faces, d.scans, d.jobs), (2, 1, 1));
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM faces").await,
        1,
        "the other account's faces stay"
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM jobs WHERE kind = 'embed'").await,
        1
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM assets").await,
        2,
        "photos are untouched"
    );

    let d = faces::delete_all(&t.pool, None, false).await.unwrap();
    assert_eq!((d.faces, d.scans), (1, 1));
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 0);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM face_scans").await, 0);
    t.teardown().await;
}

#[tokio::test]
async fn forgetting_a_photos_faces_queues_a_new_scan_only_if_it_had_one() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let scanned = asset(&t.pool, owner, true).await;
    let never = asset(&t.pool, owner, true).await;
    faces::store(&t.pool, scanned, MODEL, VERSION, &[face(1, true)])
        .await
        .unwrap();
    assert!(!faces::forget_and_requeue(&t.pool, never, VERSION, true).await.unwrap());
    assert!(
        faces::forget_and_requeue(&t.pool, scanned, VERSION, true)
            .await
            .unwrap()
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 0);
    assert_eq!(
        count(
            &t.pool,
            "SELECT count(*) FROM jobs WHERE kind = 'faces' AND state = 'queued'"
        )
        .await,
        1
    );
    t.teardown().await;
}

#[tokio::test]
async fn superseded_models_are_pruned_when_the_new_one_covers_the_library() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, true).await;
    // An older version's rows (same model id, version 0) and the current ones.
    faces::store(&t.pool, a, MODEL, VERSION - 1, &[face(1, true), face(2, true)])
        .await
        .unwrap();
    assert_eq!(faces::missing_count(&t.pool, MODEL, VERSION).await.unwrap(), 1);
    faces::store(&t.pool, a, MODEL, VERSION, &[face(3, true)])
        .await
        .unwrap();
    assert_eq!(faces::missing_count(&t.pool, MODEL, VERSION).await.unwrap(), 0);
    // Same (asset, model) key: storing replaced the old rows already, so there is nothing left to prune.
    assert_eq!(faces::prune_superseded(&t.pool, MODEL, VERSION).await.unwrap(), 0);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM faces").await, 1);
    t.teardown().await;
}
