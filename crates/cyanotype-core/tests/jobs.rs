//! Job queue integration tests. Need a real Postgres: set `DATABASE_URL` (the role must be
//! able to CREATE DATABASE). Each test migrates a fresh throwaway database; without
//! `DATABASE_URL` the tests return early.
use std::time::{Duration, Instant};

use cyanotype_core::{db, jobs};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

struct TestDb {
    pool: PgPool,
    url: String,
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
        let url = format!("{base}/{name}");
        let pool = db::connect(&url).await.unwrap();
        db::migrate(&pool).await.unwrap();
        Some(Self {
            pool,
            url,
            admin_url,
            name,
        })
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

const KINDS: &[&str] = &[jobs::KIND_METADATA, jobs::KIND_THUMBNAIL, jobs::KIND_PAIR];

/// Wait for the waker, failing the test if it takes longer than `within`. Returns how long it took.
async fn expect_wake(waker: &jobs::JobWaker, within: Duration) -> Duration {
    let start = Instant::now();
    tokio::time::timeout(within, waker.woken())
        .await
        .unwrap_or_else(|_| panic!("no wake-up within {within:?}"));
    start.elapsed()
}

/// A listening waker that has finished connecting (it signals once on connect), with that signal consumed.
async fn ready_waker(url: &str) -> jobs::JobWaker {
    let waker = jobs::JobWaker::spawn(url);
    expect_wake(&waker, Duration::from_secs(10)).await;
    waker
}

#[tokio::test]
async fn notify_wakes_a_listening_worker_quickly_for_every_enqueue_path() {
    let Some(t) = TestDb::new().await else { return };
    let asset = make_asset(&t.pool).await;
    let waker = ready_waker(&t.url).await;

    // Plain enqueue (upload complete, pairing hand-off, reindex --rethumb all go through this INSERT).
    let wait = tokio::spawn({
        let w = waker.clone();
        async move { expect_wake(&w, Duration::from_secs(1)).await }
    });
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, asset, 1).await.unwrap();
    let took = wait.await.unwrap();
    assert!(took < Duration::from_secs(1), "woke in {took:?}");

    // Ad-hoc SQL insert (a migration backfill, a manual requeue): the trigger catches it too.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = tokio::time::timeout(Duration::from_millis(50), waker.woken()).await; // drain
    sqlx::query("INSERT INTO jobs (kind, asset_id) VALUES ('pair', $1)")
        .bind(asset)
        .execute(&t.pool)
        .await
        .unwrap();
    expect_wake(&waker, Duration::from_secs(1)).await;

    // A failed job coming back via the retry endpoint's UPDATE.
    let _ = tokio::time::timeout(Duration::from_millis(50), waker.woken()).await;
    sqlx::query("UPDATE jobs SET state = 'failed' WHERE kind = 'pair'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), waker.woken())
            .await
            .is_err(),
        "moving a job to failed must not wake workers"
    );
    sqlx::query("UPDATE jobs SET state = 'queued', attempts = 0, run_after = now() WHERE kind = 'pair'")
        .execute(&t.pool)
        .await
        .unwrap();
    expect_wake(&waker, Duration::from_secs(1)).await;

    // The worker's own claim/complete transitions do not wake anyone.
    let _ = tokio::time::timeout(Duration::from_millis(50), waker.woken()).await;
    let job = jobs::claim(&t.pool, jobs::KIND_PAIR).await.unwrap().unwrap();
    jobs::complete(&t.pool, job.id).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), waker.woken())
            .await
            .is_err()
    );
    t.teardown().await;
}

#[tokio::test]
async fn a_notification_is_only_delivered_after_the_enqueuing_transaction_commits() {
    let Some(t) = TestDb::new().await else { return };
    let asset = make_asset(&t.pool).await;
    let waker = ready_waker(&t.url).await;
    let mut tx = t.pool.begin().await.unwrap();
    jobs::enqueue(&mut *tx, jobs::KIND_THUMBNAIL, asset, 1).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), waker.woken())
            .await
            .is_err()
    );
    tx.commit().await.unwrap();
    expect_wake(&waker, Duration::from_secs(1)).await;
    t.teardown().await;
}

#[tokio::test]
async fn wait_for_work_returns_on_notify_on_a_due_retry_and_on_the_fallback() {
    let Some(t) = TestDb::new().await else { return };
    let asset = make_asset(&t.pool).await;
    let waker = ready_waker(&t.url).await;

    // Notify: far-away fallback, returns as soon as a job is queued.
    let waiter = tokio::spawn({
        let (pool, waker) = (t.pool.clone(), waker.clone());
        async move {
            let start = Instant::now();
            jobs::wait_for_work(&pool, &waker, KINDS, Duration::from_secs(30)).await;
            start.elapsed()
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, asset, 1).await.unwrap();
    assert!(waiter.await.unwrap() < Duration::from_secs(1));
    let job = jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().unwrap();

    // Delayed retry: the failure requeues with a backoff. Shorten it to ~1.5 s; the wait must end when it is due,
    // not at the 30 s fallback. (The requeue itself notifies once; the first wake finds nothing due and we wait again.)
    jobs::fail(&t.pool, &job, "boom").await.unwrap();
    sqlx::query("UPDATE jobs SET run_after = now() + interval '1500 milliseconds' WHERE id = $1")
        .bind(job.id)
        .execute(&t.pool)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = tokio::time::timeout(Duration::from_millis(50), waker.woken()).await; // drain the requeue notifications
    let started = Instant::now();
    loop {
        jobs::wait_for_work(&t.pool, &waker, KINDS, Duration::from_secs(30)).await;
        if jobs::claim(&t.pool, jobs::KIND_METADATA).await.unwrap().is_some() {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "retry was never picked up");
    }
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(1200),
        "claimed before it was due: {took:?}"
    );
    assert!(took < Duration::from_secs(4), "slept past the due time: {took:?}");

    // Fallback poll still works with no listener at all (an inert waker): a job that is already claimable is
    // found within the fallback interval.
    let inert = jobs::JobWaker::inert();
    let start = Instant::now();
    jobs::wait_for_work(&t.pool, &inert, KINDS, Duration::from_millis(400)).await;
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(350) && took < Duration::from_secs(2),
        "{took:?}"
    );
    t.teardown().await;
}

#[tokio::test]
async fn next_due_in_reports_the_earliest_delayed_job_only() {
    let Some(t) = TestDb::new().await else { return };
    assert_eq!(jobs::next_due_in(&t.pool, KINDS).await.unwrap(), None);
    let asset = make_asset(&t.pool).await;
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, asset, 1).await.unwrap();
    assert_eq!(
        jobs::next_due_in(&t.pool, KINDS).await.unwrap(),
        None,
        "a runnable job is not 'delayed'"
    );
    sqlx::query("UPDATE jobs SET run_after = now() + interval '40 seconds'")
        .execute(&t.pool)
        .await
        .unwrap();
    let d = jobs::next_due_in(&t.pool, KINDS).await.unwrap().unwrap();
    assert!(d > Duration::from_secs(30) && d <= Duration::from_secs(40), "{d:?}");
    assert_eq!(jobs::next_due_in(&t.pool, &[jobs::KIND_THUMBNAIL]).await.unwrap(), None);
    t.teardown().await;
}

#[tokio::test]
async fn the_listener_reconnects_after_its_connection_is_killed() {
    let Some(t) = TestDb::new().await else { return };
    let asset = make_asset(&t.pool).await;
    let waker = ready_waker(&t.url).await;

    // Kill the backend holding our LISTEN (the only session whose last statement is LISTEN).
    let (killed,): (i64,) = sqlx::query_as(
        "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity
         WHERE datname = current_database() AND pid <> pg_backend_pid() AND query LIKE 'LISTEN%'",
    )
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert_eq!(killed, 1, "exactly one listening session");

    // Losing the connection means missed notifications are possible, so the waker signals a wake-up on reconnect.
    expect_wake(&waker, Duration::from_secs(10)).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = tokio::time::timeout(Duration::from_millis(100), waker.woken()).await; // drain extra signals

    // And it is listening again: a new job wakes it promptly.
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, asset, 1).await.unwrap();
    let took = expect_wake(&waker, Duration::from_secs(2)).await;
    assert!(took < Duration::from_secs(1), "{took:?}");
    t.teardown().await;
}

#[tokio::test]
async fn the_listener_does_not_use_the_application_pool() {
    let Some(t) = TestDb::new().await else { return };
    let (before,): (i64,) = sqlx::query_as("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database()")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    let _waker = ready_waker(&t.url).await;
    // One extra dedicated session; the pool itself is untouched (its size and idle count do not change).
    let (after,): (i64,) = sqlx::query_as("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database()")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(after, before + 1);
    assert!(t.pool.size() <= 10);
    t.teardown().await;
}

#[tokio::test]
async fn queue_stats_counts_per_kind_and_ignores_delayed_jobs_for_age() {
    let Some(t) = TestDb::new().await else { return };
    let (a, b, c) = (
        make_asset(&t.pool).await,
        make_asset(&t.pool).await,
        make_asset(&t.pool).await,
    );
    for asset in [a, b, c] {
        jobs::enqueue(&t.pool, jobs::KIND_THUMBNAIL, asset, 1).await.unwrap();
    }
    sqlx::query("UPDATE jobs SET run_after = now() - interval '90 seconds' WHERE asset_id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state = 'running' WHERE asset_id = $1")
        .bind(b)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state = 'queued', run_after = now() + interval '1 hour' WHERE asset_id = $1")
        .bind(c)
        .execute(&t.pool)
        .await
        .unwrap();
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, a, 1).await.unwrap();
    sqlx::query("UPDATE jobs SET state = 'failed' WHERE kind = 'metadata'")
        .execute(&t.pool)
        .await
        .unwrap();
    let stats = jobs::queue_stats(&t.pool).await.unwrap();
    let thumb = stats.iter().find(|s| s.kind == "thumbnail").unwrap();
    assert_eq!((thumb.queued, thumb.running, thumb.failed), (2, 1, 0));
    assert!(
        thumb.oldest_queued_age_secs >= 89.0 && thumb.oldest_queued_age_secs < 120.0,
        "{}",
        thumb.oldest_queued_age_secs
    );
    let meta = stats.iter().find(|s| s.kind == "metadata").unwrap();
    assert_eq!((meta.queued, meta.running, meta.failed), (0, 0, 1));
    assert_eq!(meta.oldest_queued_age_secs, 0.0);
    t.teardown().await;
}

#[tokio::test]
async fn stalled_seconds_is_zero_when_idle_or_progressing_and_grows_when_wedged() {
    let Some(t) = TestDb::new().await else { return };
    assert_eq!(jobs::stalled_seconds(&t.pool).await.unwrap(), 0.0);
    let (a, b) = (make_asset(&t.pool).await, make_asset(&t.pool).await);
    jobs::enqueue(&t.pool, jobs::KIND_METADATA, a, 1).await.unwrap();
    // Long-idle queue that just received work: not stalled.
    sqlx::query("UPDATE jobs SET state = 'done', finished_at = now() - interval '2 hours'")
        .execute(&t.pool)
        .await
        .unwrap();
    jobs::enqueue(&t.pool, jobs::KIND_THUMBNAIL, b, 1).await.unwrap();
    assert!(jobs::stalled_seconds(&t.pool).await.unwrap() < 5.0);
    // Work has waited 40 minutes and nothing finished for 2 hours: stalled for the shorter, 40 minutes.
    sqlx::query("UPDATE jobs SET run_after = now() - interval '40 minutes' WHERE state = 'queued'")
        .execute(&t.pool)
        .await
        .unwrap();
    let s = jobs::stalled_seconds(&t.pool).await.unwrap();
    assert!((2395.0..2500.0).contains(&s), "{s}");
    // A recent completion means the queue is moving.
    sqlx::query("UPDATE jobs SET finished_at = now() - interval '1 minute' WHERE state = 'done'")
        .execute(&t.pool)
        .await
        .unwrap();
    let s = jobs::stalled_seconds(&t.pool).await.unwrap();
    assert!((55.0..90.0).contains(&s), "{s}");
    t.teardown().await;
}

/// Migration 0010 requeues the thumbnail jobs the worker refused for memory, and only those.
#[tokio::test]
async fn migration_0010_requeues_only_memory_refusals() {
    let Some(db) = TestDb::new().await else { return };
    let asset = make_asset(&db.pool).await;
    let memory =
        "decoding a 14000x7000 image needs about 477 MB, over the worker memory budget (493 MB available to this job)";
    let rows = [
        ("thumbnail", memory),
        (
            "thumbnail",
            "decoding a 9000x9000 image needs about 1000 MB, over the worker memory budget (400 MB available to this job)",
        ),
        (
            "thumbnail",
            "original is 700 MB, which does not fit the worker memory budget of 640 MB",
        ),
        ("thumbnail", "decode image: unexpected EOF"),
        ("metadata", memory), // same words, different job: left alone
    ];
    // One asset can only have one job per kind and version; use separate versions for the thumbnail rows.
    for (i, (kind, err)) in rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO jobs (kind, asset_id, version, state, attempts, last_error, finished_at)
             VALUES ($1, $2, $3, 'failed', 3, $4, now())",
        )
        .bind(kind)
        .bind(asset)
        .bind(i as i32 + 1)
        .bind(err)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let done: (i64,) = sqlx::query_as("SELECT count(*) FROM jobs")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(done.0, 5);

    let sql = include_str!("../migrations/0010_requeue_memory_refusals.sql");
    let state = || async {
        sqlx::query_as::<_, (i32, String, i32, Option<String>)>(
            "SELECT version, state, attempts, last_error FROM jobs ORDER BY version",
        )
        .fetch_all(&db.pool)
        .await
        .unwrap()
    };
    sqlx::raw_sql(sql).execute(&db.pool).await.unwrap();
    let after = state().await;
    assert_eq!(
        after.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(),
        ["queued", "queued", "failed", "failed", "failed"]
    );
    assert_eq!(after[0].2, 0, "a fresh set of attempts");
    assert_eq!(after[0].3, None);
    assert!(
        after[2].3.as_deref().unwrap().starts_with("original is"),
        "other errors are kept"
    );

    // Idempotent: a second run changes nothing.
    sqlx::raw_sql(sql).execute(&db.pool).await.unwrap();
    assert_eq!(state().await, after);
    db.teardown().await;
}
