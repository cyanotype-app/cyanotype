//! Job queue integration tests. Need a real Postgres: set `DATABASE_URL` (the role must be
//! able to CREATE DATABASE). Each test migrates a fresh throwaway database; without
//! `DATABASE_URL` the tests return early.
use cyanotype_core::{db, jobs};
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

async fn make_asset(pool: &PgPool) -> Uuid {
    let (user,): (Uuid,) = sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ($1, 'x') RETURNING id")
        .bind(format!("{}@example.com", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .unwrap();
    let (asset,): (Uuid,) = sqlx::query_as(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
         VALUES ($1, $2, 'k', 'a.jpg', 'image/jpeg', 1) RETURNING id",
    )
    .bind(user)
    .bind(Uuid::new_v4().as_bytes().repeat(2))
    .fetch_one(pool)
    .await
    .unwrap();
    asset
}

async fn state(pool: &PgPool, id: i64) -> (String, i32) {
    sqlx::query_as("SELECT state, attempts FROM jobs WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn enqueue_is_idempotent_and_claim_is_exclusive() {
    let Some(t) = TestDb::new().await else { return };
    let asset = make_asset(&t.pool).await;
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, asset, 1).await.unwrap();
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, asset, 1).await.unwrap();
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM jobs")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);

    let job = jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().unwrap();
    assert_eq!(job.attempts, 1);
    assert!(jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().is_none());
    assert!(jobs::claim(&t.pool, jobs::KIND_THUMBNAIL).await.unwrap().is_none());
    jobs::complete(&t.pool, job.id).await.unwrap();
    assert_eq!(state(&t.pool, job.id).await.0, "done");
    assert!(jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().is_none());
    t.teardown().await;
}

#[tokio::test]
async fn always_failing_job_ends_failed_and_is_not_claimable() {
    let Some(t) = TestDb::new().await else { return };
    let asset = make_asset(&t.pool).await;
    jobs::enqueue(&t.pool, jobs::KIND_THUMBNAIL, asset, 1).await.unwrap();

    let mut id = 0;
    for attempt in 1..=jobs::MAX_ATTEMPTS {
        // Skip the backoff delay.
        sqlx::query("UPDATE jobs SET run_after = now()")
            .execute(&t.pool)
            .await
            .unwrap();
        let job = jobs::claim(&t.pool, jobs::KIND_THUMBNAIL)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("attempt {attempt} should be claimable"));
        assert_eq!(job.attempts, attempt);
        id = job.id;
        jobs::fail(&t.pool, &job, "boom").await.unwrap();
        let (st, _) = state(&t.pool, id).await;
        assert_eq!(
            st,
            if attempt == jobs::MAX_ATTEMPTS {
                "failed"
            } else {
                "queued"
            }
        );
    }
    sqlx::query("UPDATE jobs SET run_after = now()")
        .execute(&t.pool)
        .await
        .unwrap();
    assert!(jobs::claim(&t.pool, jobs::KIND_THUMBNAIL).await.unwrap().is_none());
    assert_eq!(state(&t.pool, id).await, ("failed".into(), jobs::MAX_ATTEMPTS));
    t.teardown().await;
}

#[tokio::test]
async fn failure_backs_off() {
    let Some(t) = TestDb::new().await else { return };
    let asset = make_asset(&t.pool).await;
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, asset, 1).await.unwrap();
    let job = jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().unwrap();
    jobs::fail(&t.pool, &job, "boom").await.unwrap();
    // Requeued but not yet due.
    assert!(jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().is_none());
    assert_eq!(state(&t.pool, job.id).await.0, "queued");
    t.teardown().await;
}

#[tokio::test]
async fn stale_running_jobs_are_reclaimed_then_reaped() {
    let Some(t) = TestDb::new().await else { return };
    let asset = make_asset(&t.pool).await;
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, asset, 1).await.unwrap();
    let job = jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().unwrap();

    // Fresh lock: not reclaimable.
    assert!(jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().is_none());

    // Worker "dies": lock goes stale, job is reclaimed with attempts incremented.
    sqlx::query("UPDATE jobs SET locked_at = now() - interval '1 hour'")
        .execute(&t.pool)
        .await
        .unwrap();
    let again = jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().unwrap();
    assert_eq!((again.id, again.attempts), (job.id, 2));

    // Dies on the final attempt: must end up failed instead of stuck in 'running'.
    sqlx::query("UPDATE jobs SET attempts = $1, locked_at = now() - interval '1 hour'")
        .bind(jobs::MAX_ATTEMPTS)
        .execute(&t.pool)
        .await
        .unwrap();
    assert!(jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().is_none());
    assert_eq!(state(&t.pool, job.id).await.0, "failed");
    t.teardown().await;
}

#[tokio::test]
async fn permanent_failure_is_not_retried() {
    let Some(t) = TestDb::new().await else { return };
    let asset = make_asset(&t.pool).await;
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, asset, 1).await.unwrap();
    let job = jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().unwrap();
    jobs::fail_permanent(&t.pool, job.id, "corrupt").await.unwrap();
    assert_eq!(state(&t.pool, job.id).await, ("failed".into(), 1));
    sqlx::query("UPDATE jobs SET run_after = now()")
        .execute(&t.pool)
        .await
        .unwrap();
    assert!(jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().is_none());
    t.teardown().await;
}
