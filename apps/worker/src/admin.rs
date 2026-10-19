//! The worker's admin-side background work: the nightly usage reconcile and the tasks an admin queues (`admin_tasks`).
use std::{sync::Arc, time::Duration};

use cyanotype_core::{admin_tasks, usage};
use sqlx::PgPool;

use crate::handlers::Ctx;

/// How often the worker checks whether the reconcile is due.
const CHECK_EVERY: Duration = Duration::from_secs(3600);
/// Reconcile when the last run is older than this (a day, less the check interval, so it lands once a day).
const DUE_AFTER_HOURS: i32 = 23;
/// How often the worker looks for a queued admin task.
const TASK_POLL: Duration = Duration::from_secs(10);
/// Derived-size catch-up per reconcile (two or three HEAD requests per file).
const DEFAULT_MEASURE_PER_RUN: i64 = 2000;
/// Session-level advisory lock so only one worker replica reconciles at a time.
const RECONCILE_LOCK: i64 = 0x6379_616e_6f75_7367; // "cyanousg"

pub fn spawn(ctx: Arc<Ctx>) {
    tokio::spawn(reconcile_loop(ctx.clone()));
    tokio::spawn(task_loop(ctx));
}

async fn due(db: &PgPool) -> anyhow::Result<bool> {
    let (due,): (bool,) = sqlx::query_as(
        "SELECT NOT EXISTS (SELECT 1 FROM server_state WHERE key = 'usage_reconcile'
                            AND updated_at > now() - make_interval(hours => $1))",
    )
    .bind(DUE_AFTER_HOURS)
    .fetch_one(db)
    .await?;
    Ok(due)
}

/// Runs the reconcile once if it is due. Returns whether it ran.
pub async fn reconcile_if_due(ctx: &Ctx, measure_per_run: i64) -> anyhow::Result<bool> {
    if !due(&ctx.db).await? {
        return Ok(false);
    }
    let mut conn = ctx.db.acquire().await?;
    let (got,): (bool,) = sqlx::query_as("SELECT pg_try_advisory_lock($1)")
        .bind(RECONCILE_LOCK)
        .fetch_one(&mut *conn)
        .await?;
    if !got {
        return Ok(false);
    }
    // Another replica may have finished while this one waited for its turn.
    let result = async {
        if !due(&ctx.db).await? {
            return anyhow::Ok(false);
        }
        usage::reconcile_and_report(&ctx.db).await?;
        match usage::measure_derived(&ctx.db, &ctx.storage, measure_per_run).await {
            Ok(n) if n > 0 => tracing::info!(files = n, "measured derived sizes"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "could not measure derived sizes"),
        }
        Ok(true)
    }
    .await;
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(RECONCILE_LOCK)
        .execute(&mut *conn)
        .await;
    result
}

async fn reconcile_loop(ctx: Arc<Ctx>) {
    let per_run = std::env::var("USAGE_MEASURE_PER_RUN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MEASURE_PER_RUN);
    // Let the worker settle (and migrations finish on a fresh deploy) before the first look.
    tokio::time::sleep(Duration::from_secs(120)).await;
    loop {
        if let Err(e) = reconcile_if_due(&ctx, per_run).await {
            tracing::warn!(error = %format!("{e:#}"), "usage reconcile failed");
        }
        tokio::time::sleep(CHECK_EVERY).await;
    }
}

async fn task_loop(ctx: Arc<Ctx>) {
    loop {
        match run_one(&ctx).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "admin task loop failed"),
        }
        tokio::time::sleep(TASK_POLL).await;
    }
}

/// Claims and runs one admin task. Returns whether there was one.
pub async fn run_one(ctx: &Ctx) -> anyhow::Result<bool> {
    let Some(task) = admin_tasks::claim(&ctx.db, &[admin_tasks::KIND_BUCKET_CHECK]).await? else {
        return Ok(false);
    };
    tracing::info!(task = task.id, kind = %task.kind, "admin task started");
    let started = std::time::Instant::now();
    let outcome = match task.kind.as_str() {
        admin_tasks::KIND_BUCKET_CHECK => match usage::bucket_check(&ctx.db, &ctx.storage).await {
            Ok(r) => serde_json::to_value(r).map_err(|e| e.to_string()),
            Err(e) => Err(format!("{e:#}")),
        },
        other => Err(format!("unknown task kind {other}")),
    };
    if let Err(e) = &outcome {
        tracing::warn!(task = task.id, error = %e, "admin task failed");
    }
    // Keep the failure text short and free of anything but what the worker itself wrote.
    let outcome = outcome.map_err(|e| e.chars().take(500).collect::<String>());
    admin_tasks::finish(&ctx.db, task.id, outcome).await?;
    tracing::info!(
        task = task.id,
        ms = started.elapsed().as_millis() as u64,
        "admin task finished"
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use cyanotype_core::{config::S3Config, db, storage::Storage};
    use sqlx::{Connection, Executor, PgConnection};
    use uuid::Uuid;

    use super::*;
    use crate::budget::MemoryBudget;

    /// A context over a fresh database, and the S3 API when the `S3_*` variables are set (else storage that goes nowhere).
    async fn ctx() -> Option<(Arc<Ctx>, String, String, bool)> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        let real = std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty()).is_some();
        let cfg = if real {
            S3Config::from_env().unwrap()
        } else {
            S3Config {
                endpoint: Some("http://127.0.0.1:1".into()),
                public_endpoint: None,
                region: "us-east-1".into(),
                bucket: "b".into(),
                access_key: "k".into(),
                secret_key: "s".into(),
                force_path_style: true,
                presign_ttl_secs: 60,
            }
        };
        let ctx = Arc::new(Ctx {
            db: pool,
            storage: Storage::new(&cfg),
            max_original_bytes: 1 << 20,
            budget: MemoryBudget::new(64 << 20),
            raw_develop: false,
            search_model: cyanotype_clip::DEFAULT_MODEL,
        });
        Some((ctx, admin_url, name, real))
    }

    async fn drop_db(ctx: Arc<Ctx>, admin_url: String, name: String) {
        ctx.db.close().await;
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin
            .execute(format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)").as_str())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_reconcile_runs_once_a_day_repairs_drift_and_leaves_a_record() {
        let Some((ctx, admin_url, name, _)) = ctx().await else {
            return;
        };
        let (u,): (Uuid,) =
            sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ('w@example.com','x') RETURNING id")
                .fetch_one(&ctx.db)
                .await
                .unwrap();
        sqlx::query("INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes) VALUES ($1, sha256('a'::bytea), 'k', 'a.jpg', 'image/jpeg', 42)")
            .bind(u)
            .execute(&ctx.db)
            .await
            .unwrap();
        sqlx::query("UPDATE user_usage SET originals_bytes = 1 WHERE user_id = $1")
            .bind(u)
            .execute(&ctx.db)
            .await
            .unwrap();

        assert!(reconcile_if_due(&ctx, 10).await.unwrap(), "never ran: due");
        assert_eq!(cyanotype_core::usage::of(&ctx.db, u).await.unwrap().originals_bytes, 42);
        let (drifted,): (i64,) =
            sqlx::query_as("SELECT (value->>'drifted_users')::bigint FROM server_state WHERE key = 'usage_reconcile'")
                .fetch_one(&ctx.db)
                .await
                .unwrap();
        assert_eq!(drifted, 1);
        // Not again within the day...
        assert!(!reconcile_if_due(&ctx, 10).await.unwrap());
        // ...but a day later.
        sqlx::query("UPDATE server_state SET updated_at = now() - interval '24 hours'")
            .execute(&ctx.db)
            .await
            .unwrap();
        assert!(reconcile_if_due(&ctx, 10).await.unwrap());
        drop_db(ctx, admin_url, name).await;
    }

    #[tokio::test]
    async fn a_queued_bucket_check_is_run_and_its_result_stored() {
        let Some((ctx, admin_url, name, real)) = ctx().await else {
            return;
        };
        if !real {
            drop_db(ctx, admin_url, name).await;
            return;
        }
        assert!(!run_one(&ctx).await.unwrap(), "nothing queued");
        admin_tasks::enqueue(&ctx.db, admin_tasks::KIND_BUCKET_CHECK, None, serde_json::json!({}))
            .await
            .unwrap();
        assert!(run_one(&ctx).await.unwrap());
        let t = admin_tasks::latest(&ctx.db, admin_tasks::KIND_BUCKET_CHECK)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(t.state, "done", "{:?}", t.error);
        let r = t.result.unwrap();
        assert!(r["finished_at"].is_string() && r["versioned"].is_boolean());
        assert!(!run_one(&ctx).await.unwrap(), "done tasks are not run again");
        drop_db(ctx, admin_url, name).await;
    }
}
