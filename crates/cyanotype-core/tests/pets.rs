//! Pet storage, the `pets` job sweep, progress, the purge cascade and the delete-everything command. Need a real Postgres with pgvector:
//! set `DATABASE_URL` (see `tests/jobs.rs`); without it the tests return early.
use cyanotype_core::{
    db, jobs,
    pets::{self, Basis, MODEL, NewDetection, Scan, VERSION},
};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

/// The search vectors the check reads (any model id will do here: the sweep only asks whether a photo has one).
const SEARCH: Basis<'static> = Basis {
    model: "siglip2-base-patch16-224-q4",
    version: 2,
};

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

/// A photo with a preview (thumbhash set) unless `preview` is false, and the search vector the check reads unless `embedded` is false.
async fn asset(pool: &PgPool, owner: Uuid, preview: bool, embedded: bool) -> Uuid {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash)
         VALUES ($1, sha256($2::bytea), 'k', 'p.jpg', 'image/jpeg', 1, CASE WHEN $3 THEN '\\x01'::bytea END) RETURNING id",
    )
    .bind(owner)
    .bind(Uuid::new_v4().as_bytes().to_vec())
    .bind(preview)
    .fetch_one(pool)
    .await
    .unwrap();
    if embedded {
        sqlx::query(
            "INSERT INTO asset_embeddings (asset_id, model, version, embedding)
             VALUES ($1, $2, $3, array_fill(0.1::real, ARRAY[768])::vector::halfvec)",
        )
        .bind(id)
        .bind(SEARCH.model)
        .bind(SEARCH.version)
        .execute(pool)
        .await
        .unwrap();
    }
    id
}

fn found(kind: &str, seed: usize) -> NewDetection {
    NewDetection {
        kind: kind.into(),
        bbox: [0.1 * seed as f32, 0.2, 0.2, 0.25],
        score: 0.5 + 0.1 * seed as f32,
        embedding: None,
    }
}

fn scan(found: Vec<NewDetection>) -> Scan {
    Scan {
        dog_share: if found.iter().any(|d| d.kind == "dog") {
            0.9
        } else {
            0.02
        },
        cat_share: if found.iter().any(|d| d.kind == "cat") {
            0.8
        } else {
            0.01
        },
        found,
        fingerprinted: false,
    }
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn storing_twice_replaces_the_rows_and_never_duplicates() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, true, true).await;
    let both = scan(vec![found("dog", 1), found("cat", 2)]);
    assert!(pets::store(&t.pool, a, MODEL, VERSION, &both).await.unwrap());
    assert!(pets::store(&t.pool, a, MODEL, VERSION, &both).await.unwrap());
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, i16, f32, f32, f32, f32, f32, Uuid)> = sqlx::query_as(
        "SELECT kind, idx, box_x, box_y, box_w, box_h, score, owner_id FROM pet_detections WHERE asset_id = $1 ORDER BY idx",
    )
    .bind(a)
    .fetch_all(&t.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "two animals however often it runs");
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1, rows[1].0.as_str(), rows[1].1),
        ("dog", 0, "cat", 1)
    );
    assert_eq!(
        (rows[0].2, rows[0].3, rows[0].4, rows[0].5, rows[0].6),
        (0.1, 0.2, 0.2, 0.25, 0.6)
    );
    assert_eq!(rows[0].7, owner, "a detection belongs to its photo's owner");
    // The scan row says what was seen, also for a photo without a pet; "scanned, no pets" is not "not scanned".
    let (n, dog, cat): (i32, f32, f32) =
        sqlx::query_as("SELECT detections, dog_share, cat_share FROM pet_scans WHERE asset_id = $1")
            .bind(a)
            .fetch_one(&t.pool)
            .await
            .unwrap();
    assert_eq!((n, dog, cat), (2, 0.9, 0.8));
    // A new scan with one animal replaces both rows.
    assert!(
        pets::store(&t.pool, a, MODEL, VERSION, &scan(vec![found("cat", 3)]))
            .await
            .unwrap()
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 1);
    assert!(pets::store(&t.pool, a, MODEL, VERSION, &scan(vec![])).await.unwrap());
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 0);
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_scans WHERE detections = 0").await,
        1
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_scan_by_a_newer_model_takes_the_older_ones_rows_and_records_with_it() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, true, true).await;
    assert!(
        pets::store(
            &t.pool,
            a,
            "an-older-model",
            1,
            &scan(vec![found("dog", 1), found("dog", 2)])
        )
        .await
        .unwrap()
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 2);
    assert!(
        pets::store(&t.pool, a, MODEL, VERSION, &scan(vec![found("cat", 1)]))
            .await
            .unwrap()
    );
    assert_eq!(
        count(
            &t.pool,
            "SELECT count(*) FROM pet_detections WHERE model = 'an-older-model'"
        )
        .await,
        0
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_scans WHERE model = 'an-older-model'").await,
        0
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_detections WHERE kind = 'cat'").await,
        1
    );
    // Another version of the same model replaces the same way (the key is the photo and the model).
    assert!(
        pets::store(&t.pool, a, MODEL, VERSION + 1, &scan(vec![found("dog", 1)]))
            .await
            .unwrap()
    );
    let versions: Vec<i32> = sqlx::query_scalar("SELECT version FROM pet_detections")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(versions, vec![VERSION + 1]);
    t.teardown().await;
}

#[tokio::test]
async fn nothing_is_stored_for_a_photo_that_is_being_deleted_or_gone() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, true, true).await;
    sqlx::query("UPDATE assets SET deleted_at = now(), purging_at = now() WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    assert!(
        !pets::store(&t.pool, a, MODEL, VERSION, &scan(vec![found("dog", 1)]))
            .await
            .unwrap()
    );
    assert!(
        !pets::store(&t.pool, Uuid::new_v4(), MODEL, VERSION, &scan(vec![]))
            .await
            .unwrap()
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 0);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_scans").await, 0);
    t.teardown().await;
}

#[tokio::test]
async fn the_kind_is_data_any_word_is_stored_and_an_empty_one_is_not() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, true, true).await;
    // (A later kind is a new word, not a new table: nothing in the schema knows "dog" and "cat".)
    assert!(
        pets::store(&t.pool, a, MODEL, VERSION, &scan(vec![found("rabbit", 1)]))
            .await
            .unwrap()
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_detections WHERE kind = 'rabbit'").await,
        1
    );
    assert!(
        pets::store(&t.pool, a, MODEL, VERSION, &scan(vec![found("", 1)]))
            .await
            .is_err()
    );
    let bad = NewDetection {
        kind: "dog".into(),
        bbox: [0.1, 0.1, 0.0, 0.2],
        score: 0.5,
        embedding: None,
    };
    assert!(
        pets::store(&t.pool, a, MODEL, VERSION, &scan(vec![bad])).await.is_err(),
        "a box has an area"
    );
    t.teardown().await;
}

#[tokio::test]
async fn the_sweep_queues_unscanned_embedded_photos_once_at_backfill_priority() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let scanned = asset(&t.pool, owner, true, true).await;
    let waiting = asset(&t.pool, owner, true, true).await;
    let no_preview = asset(&t.pool, owner, false, true).await;
    let not_embedded = asset(&t.pool, owner, true, false).await;
    let trashed_for_good = asset(&t.pool, owner, true, true).await;
    sqlx::query("UPDATE assets SET purging_at = now(), deleted_at = now() WHERE id = $1")
        .bind(trashed_for_good)
        .execute(&t.pool)
        .await
        .unwrap();
    pets::store(&t.pool, scanned, MODEL, VERSION, &scan(vec![]))
        .await
        .unwrap();

    assert_eq!(pets::missing_count(&t.pool, MODEL, VERSION, SEARCH).await.unwrap(), 1);
    assert_eq!(pets::enqueue_missing(&t.pool, MODEL, VERSION, SEARCH).await.unwrap(), 1);
    // Running the sweep again queues nothing new.
    assert_eq!(pets::enqueue_missing(&t.pool, MODEL, VERSION, SEARCH).await.unwrap(), 0);
    let queued: Vec<(Uuid, i32, i16)> =
        sqlx::query_as("SELECT asset_id, version, priority FROM jobs WHERE kind = 'pets'")
            .fetch_all(&t.pool)
            .await
            .unwrap();
    assert_eq!(queued, vec![(waiting, VERSION, jobs::PRIORITY_BACKFILL)]);
    assert!(!queued.iter().any(|q| q.0 == no_preview || q.0 == not_embedded));

    // A photo whose search vector arrives later is queued by the sweep from then on (and by the embed job directly).
    sqlx::query(
        "INSERT INTO asset_embeddings (asset_id, model, version, embedding)
         VALUES ($1, $2, $3, array_fill(0.1::real, ARRAY[768])::vector::halfvec)",
    )
    .bind(not_embedded)
    .bind(SEARCH.model)
    .bind(SEARCH.version)
    .execute(&t.pool)
    .await
    .unwrap();
    assert_eq!(pets::enqueue_missing(&t.pool, MODEL, VERSION, SEARCH).await.unwrap(), 1);
    // A vector of another search model is not the one the check reads.
    let other = Basis {
        model: "clip-vit-b32-openai-int8",
        version: 1,
    };
    assert_eq!(pets::missing_count(&t.pool, MODEL, VERSION, other).await.unwrap(), 0);

    // A job that finished without a scan (lost work) is queued again; a failed one is left for the owner's retry.
    sqlx::query("UPDATE jobs SET state = 'done' WHERE kind = 'pets'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(pets::enqueue_missing(&t.pool, MODEL, VERSION, SEARCH).await.unwrap(), 2);
    sqlx::query("UPDATE jobs SET state = 'failed' WHERE kind = 'pets'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(pets::enqueue_missing(&t.pool, MODEL, VERSION, SEARCH).await.unwrap(), 0);

    // A new model version scans everything again.
    assert_eq!(
        pets::missing_count(&t.pool, MODEL, VERSION + 1, SEARCH).await.unwrap(),
        3
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_new_embedding_queues_the_job_again_unless_it_is_already_waiting() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, true, true).await;
    let state = || async {
        sqlx::query_scalar::<_, String>("SELECT state FROM jobs WHERE kind = 'pets' AND asset_id = $1")
            .bind(a)
            .fetch_one(&t.pool)
            .await
            .unwrap()
    };
    pets::enqueue_for_asset(&t.pool, a, VERSION).await.unwrap();
    assert_eq!(state().await, "queued");
    // Running: left alone (the job rereads the embedding when it starts, and a newer one is queued after it).
    sqlx::query("UPDATE jobs SET state = 'running', attempts = 1 WHERE kind = 'pets'")
        .execute(&t.pool)
        .await
        .unwrap();
    pets::enqueue_for_asset(&t.pool, a, VERSION).await.unwrap();
    assert_eq!(state().await, "running");
    // Done (it read the embedding of the picture before an edit) or failed: queued again, with fresh attempts.
    for old in ["done", "failed"] {
        sqlx::query("UPDATE jobs SET state = $1, attempts = 3 WHERE kind = 'pets'")
            .bind(old)
            .execute(&t.pool)
            .await
            .unwrap();
        pets::enqueue_for_asset(&t.pool, a, VERSION).await.unwrap();
        assert_eq!(state().await, "queued", "{old}");
    }
    let (attempts, priority): (i32, i16) = sqlx::query_as("SELECT attempts, priority FROM jobs WHERE kind = 'pets'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!((attempts, priority), (0, jobs::PRIORITY_BACKFILL));
    assert_eq!(count(&t.pool, "SELECT count(*) FROM jobs WHERE kind = 'pets'").await, 1);
    t.teardown().await;
}

#[tokio::test]
async fn progress_counts_one_owners_visible_embedded_photos_only() {
    let Some(t) = TestDb::new().await else { return };
    let (me, other) = (user(&t.pool).await, user(&t.pool).await);
    let a = asset(&t.pool, me, true, true).await;
    let _b = asset(&t.pool, me, true, true).await;
    let _no_preview = asset(&t.pool, me, false, true).await;
    let _not_embedded = asset(&t.pool, me, true, false).await;
    let theirs = asset(&t.pool, other, true, true).await;
    pets::store(
        &t.pool,
        a,
        MODEL,
        VERSION,
        &scan(vec![found("dog", 1), found("cat", 2)]),
    )
    .await
    .unwrap();
    pets::store(&t.pool, theirs, MODEL, VERSION, &scan(vec![found("dog", 3)]))
        .await
        .unwrap();
    let p = pets::progress(&t.pool, me, MODEL, VERSION, SEARCH).await.unwrap();
    assert_eq!((p.scanned, p.total, p.found), (1, 2, 2));
    // An older version's scan does not count for the current one.
    let p = pets::progress(&t.pool, me, MODEL, VERSION + 1, SEARCH).await.unwrap();
    assert_eq!((p.scanned, p.total, p.found), (0, 2, 0));
    t.teardown().await;
}

#[tokio::test]
async fn deleting_a_photo_or_an_account_takes_its_pets_with_it() {
    let Some(t) = TestDb::new().await else { return };
    let (me, other) = (user(&t.pool).await, user(&t.pool).await);
    let a = asset(&t.pool, me, true, true).await;
    let b = asset(&t.pool, me, true, true).await;
    let c = asset(&t.pool, other, true, true).await;
    for x in [a, b, c] {
        pets::store(&t.pool, x, MODEL, VERSION, &scan(vec![found("dog", 1)]))
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM assets WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 2);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_scans").await, 2);
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
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 1);
    t.teardown().await;
}

#[tokio::test]
async fn delete_all_counts_first_and_removes_only_pet_data() {
    let Some(t) = TestDb::new().await else { return };
    let (me, other) = (user(&t.pool).await, user(&t.pool).await);
    let a = asset(&t.pool, me, true, true).await;
    let c = asset(&t.pool, other, true, true).await;
    pets::store(
        &t.pool,
        a,
        MODEL,
        VERSION,
        &scan(vec![found("dog", 1), found("cat", 2)]),
    )
    .await
    .unwrap();
    pets::store(&t.pool, c, MODEL, VERSION, &scan(vec![found("dog", 3)]))
        .await
        .unwrap();
    pets::enqueue_for_asset(&t.pool, a, VERSION).await.unwrap();
    jobs::enqueue(&t.pool, jobs::KIND_EMBED, a, 2).await.unwrap();
    jobs::enqueue(&t.pool, jobs::KIND_FACES, a, 1).await.unwrap();

    let d = pets::delete_all(&t.pool, Some(me), true).await.unwrap();
    assert_eq!((d.detections, d.scans, d.jobs), (2, 1, 1));
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_detections").await,
        3,
        "a dry run deletes nothing"
    );

    let d = pets::delete_all(&t.pool, Some(me), false).await.unwrap();
    assert_eq!((d.detections, d.scans, d.jobs), (2, 1, 1));
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM pet_detections").await,
        1,
        "the other account's pets stay"
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM jobs WHERE kind IN ('embed', 'faces')").await,
        2,
        "the other jobs stay"
    );
    assert_eq!(
        count(&t.pool, "SELECT count(*) FROM assets").await,
        2,
        "photos are untouched"
    );

    let d = pets::delete_all(&t.pool, None, false).await.unwrap();
    assert_eq!((d.detections, d.scans), (1, 1));
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 0);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_scans").await, 0);
    t.teardown().await;
}

#[tokio::test]
async fn forgetting_a_photos_pets_queues_a_new_scan_only_if_it_had_one() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let scanned = asset(&t.pool, owner, true, true).await;
    let never = asset(&t.pool, owner, true, true).await;
    pets::store(&t.pool, scanned, MODEL, VERSION, &scan(vec![found("dog", 1)]))
        .await
        .unwrap();
    assert!(!pets::forget_and_requeue(&t.pool, never, VERSION, true).await.unwrap());
    assert!(pets::forget_and_requeue(&t.pool, scanned, VERSION, true).await.unwrap());
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 0);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_scans").await, 0);
    assert_eq!(
        count(
            &t.pool,
            "SELECT count(*) FROM jobs WHERE kind = 'pets' AND state = 'queued'"
        )
        .await,
        1
    );
    // With the feature off the pets are forgotten and nothing is queued.
    pets::store(&t.pool, scanned, MODEL, VERSION, &scan(vec![found("cat", 1)]))
        .await
        .unwrap();
    sqlx::query("DELETE FROM jobs").execute(&t.pool).await.unwrap();
    assert!(
        pets::forget_and_requeue(&t.pool, scanned, VERSION, false)
            .await
            .unwrap()
    );
    assert_eq!(count(&t.pool, "SELECT count(*) FROM jobs").await, 0);
    t.teardown().await;
}

#[tokio::test]
async fn superseded_rows_are_pruned_once_the_library_is_covered() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, true, true).await;
    let gone_preview = asset(&t.pool, owner, true, true).await;
    pets::store(&t.pool, a, MODEL, VERSION - 1, &scan(vec![found("dog", 1)]))
        .await
        .unwrap();
    assert_eq!(pets::missing_count(&t.pool, MODEL, VERSION, SEARCH).await.unwrap(), 2);
    pets::store(&t.pool, a, MODEL, VERSION, &scan(vec![found("dog", 2)]))
        .await
        .unwrap();
    // An older model's rows of a photo the current model has not (and will not) scan are what is left to prune.
    pets::store(&t.pool, gone_preview, "an-older-model", 1, &scan(vec![found("cat", 1)]))
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET thumbhash = NULL WHERE id = $1")
        .bind(gone_preview)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(pets::missing_count(&t.pool, MODEL, VERSION, SEARCH).await.unwrap(), 0);
    assert_eq!(pets::prune_superseded(&t.pool, MODEL, VERSION).await.unwrap(), 1);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_detections").await, 1);
    assert_eq!(count(&t.pool, "SELECT count(*) FROM pet_scans").await, 1);
    t.teardown().await;
}

#[tokio::test]
async fn the_search_vector_is_read_back_as_the_numbers_search_compares() {
    let Some(t) = TestDb::new().await else { return };
    let owner = user(&t.pool).await;
    let a = asset(&t.pool, owner, true, false).await;
    let mut v = vec![0.0f32; 768];
    v[3] = 0.5;
    v[767] = -0.25;
    cyanotype_core::embeddings::store(&t.pool, a, SEARCH.model, SEARCH.version, &v)
        .await
        .unwrap();
    let got = cyanotype_core::embeddings::get(&t.pool, a, SEARCH.model, SEARCH.version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.len(), 768);
    assert!((got[3] - 0.5).abs() < 1e-3 && (got[767] + 0.25).abs() < 1e-3 && got[0] == 0.0);
    // Another model's, or another version's, is not it; a photo without one has none.
    for (model, version) in [("another", SEARCH.version), (SEARCH.model, SEARCH.version + 1)] {
        assert!(
            cyanotype_core::embeddings::get(&t.pool, a, model, version)
                .await
                .unwrap()
                .is_none()
        );
    }
    assert!(
        cyanotype_core::embeddings::get(&t.pool, Uuid::new_v4(), SEARCH.model, SEARCH.version)
            .await
            .unwrap()
            .is_none()
    );
    t.teardown().await;
}
