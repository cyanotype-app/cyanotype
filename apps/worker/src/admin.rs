//! The worker's admin-side background work: the nightly usage reconcile and the tasks an admin queues (`admin_tasks`).
use std::{sync::Arc, time::Duration};

use cyanotype_core::audit;
use cyanotype_core::{accounts, admin_tasks, usage};
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
    tokio::spawn(publish_loop(ctx.clone()));
    tokio::spawn(task_loop(ctx));
}

/// How often the worker writes its own numbers where the admin health card reads them.
const PUBLISH_EVERY: Duration = Duration::from_secs(60);

/// What this worker says about itself: the values the gauges already carry (resident memory and the decode budget), and its
/// version. One row per worker (`worker:<host>` in `server_state`), so the admin page needs no metrics backend.
pub async fn publish(ctx: &Ctx) -> anyhow::Result<()> {
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "worker".into());
    let version = std::env::var("DD_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    sqlx::query(
        "INSERT INTO server_state (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(format!("worker:{host}"))
    .bind(serde_json::json!({
        "version": version,
        "rss_bytes": crate::rss_bytes(),
        "budget_used_bytes": ctx.budget.used_bytes(),
        "budget_total_bytes": ctx.budget.total_bytes(),
    }))
    .execute(&ctx.db)
    .await?;
    Ok(())
}

async fn publish_loop(ctx: Arc<Ctx>) {
    loop {
        if let Err(e) = publish(&ctx).await {
            tracing::warn!(error = %format!("{e:#}"), "could not publish worker stats");
        }
        tokio::time::sleep(PUBLISH_EVERY).await;
    }
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
    let kinds = [admin_tasks::KIND_BUCKET_CHECK, admin_tasks::KIND_DELETE_ACCOUNT];
    let Some(task) = admin_tasks::claim(&ctx.db, &kinds).await? else {
        return Ok(false);
    };
    tracing::info!(task = task.id, kind = %task.kind, "admin task started");
    let started = std::time::Instant::now();
    let outcome = match task.kind.as_str() {
        admin_tasks::KIND_BUCKET_CHECK => match usage::bucket_check(&ctx.db, &ctx.storage).await {
            Ok(r) => serde_json::to_value(r).map_err(|e| e.to_string()),
            Err(e) => Err(format!("{e:#}")),
        },
        admin_tasks::KIND_DELETE_ACCOUNT => delete_account(ctx, &task).await,
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

/// How long a deletion waits for jobs already running on the account's files before it gives up (and an admin retries).
const WAIT_FOR_JOBS: Duration = Duration::from_secs(600);

/// The `delete_account` task: removes the account's objects and rows (`accounts::delete_account`) and records it.
async fn delete_account(ctx: &Ctx, task: &admin_tasks::Task) -> Result<serde_json::Value, String> {
    let user: uuid::Uuid = task
        .params
        .get("user_id")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse().ok())
        .ok_or("the task has no valid user_id")?;
    let email = task
        .params
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let report = accounts::delete_account(&ctx.db, &ctx.storage, user, WAIT_FOR_JOBS)
        .await
        .map_err(|e| format!("{e:#}"))?;
    audit::record(
        &ctx.db,
        task.requested_by,
        "account.delete.finished",
        Some("user"),
        Some(&user.to_string()),
        serde_json::json!({
            "email": email, "files": report.files,
            "objects_deleted": report.objects_deleted, "uploads_aborted": report.uploads_aborted,
        }),
    )
    .await
    .map_err(|e| format!("{e:#}"))?;
    serde_json::to_value(report).map_err(|e| e.to_string())
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

    #[tokio::test]
    async fn a_queued_account_deletion_is_run_audited_and_leaves_other_accounts_alone() {
        let Some((ctx, admin_url, name, real)) = ctx().await else {
            return;
        };
        if !real {
            drop_db(ctx, admin_url, name).await;
            return;
        }
        let user = |email: &'static str| {
            let db = ctx.db.clone();
            async move {
                sqlx::query_scalar::<_, Uuid>("INSERT INTO users (email, password_hash) VALUES ($1,'x') RETURNING id")
                    .bind(email)
                    .fetch_one(&db)
                    .await
                    .unwrap()
            }
        };
        let (admin, gone, kept) = (
            user("admin@example.com").await,
            user("gone@example.com").await,
            user("kept@example.com").await,
        );
        let (gone_key, kept_key) = (
            format!("originals/{gone}/aa/x.jpg"),
            format!("originals/{kept}/aa/y.jpg"),
        );
        for (owner, key) in [(gone, &gone_key), (kept, &kept_key)] {
            ctx.storage
                .put_bytes(key, b"bytes".to_vec(), "image/jpeg")
                .await
                .unwrap();
            sqlx::query("INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes) VALUES ($1, sha256($2::bytea), $2, 'x.jpg', 'image/jpeg', 5)")
                .bind(owner)
                .bind(key.as_bytes())
                .execute(&ctx.db)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO admin_tasks (kind, requested_by, params) VALUES ('delete_account', $1, $2)")
            .bind(admin)
            .bind(serde_json::json!({"user_id": gone, "email": "gone@example.com"}))
            .execute(&ctx.db)
            .await
            .unwrap();
        assert!(run_one(&ctx).await.unwrap());
        let (state, result): (String, Option<serde_json::Value>) =
            sqlx::query_as("SELECT state, result FROM admin_tasks WHERE kind = 'delete_account'")
                .fetch_one(&ctx.db)
                .await
                .unwrap();
        assert_eq!(state, "done");
        assert_eq!(result.unwrap()["files"], 1);
        let (users,): (i64,) = sqlx::query_as("SELECT count(*) FROM users WHERE id = $1")
            .bind(gone)
            .fetch_one(&ctx.db)
            .await
            .unwrap();
        assert_eq!(users, 0);
        assert!(ctx.storage.head_size(&gone_key).await.unwrap().is_none());
        assert!(
            ctx.storage.head_size(&kept_key).await.unwrap().is_some(),
            "another account's object is untouched"
        );
        let (kept_files,): (i64,) = sqlx::query_as("SELECT count(*) FROM assets WHERE owner_id = $1")
            .bind(kept)
            .fetch_one(&ctx.db)
            .await
            .unwrap();
        assert_eq!(kept_files, 1);
        let (actor, detail): (Option<Uuid>, serde_json::Value) =
            sqlx::query_as("SELECT actor_id, detail FROM audit_log WHERE action = 'account.delete.finished'")
                .fetch_one(&ctx.db)
                .await
                .unwrap();
        assert_eq!(
            (actor, detail["email"].as_str()),
            (Some(admin), Some("gone@example.com"))
        );
        ctx.storage.delete_all_versions(&[kept_key]).await.unwrap();
        drop_db(ctx, admin_url, name).await;
    }

    #[tokio::test]
    async fn the_worker_publishes_its_memory_and_version_for_the_health_card() {
        let Some((ctx, admin_url, name, _)) = ctx().await else {
            return;
        };
        publish(&ctx).await.unwrap();
        let (value,): (serde_json::Value,) = sqlx::query_as("SELECT value FROM server_state WHERE key LIKE 'worker:%'")
            .fetch_one(&ctx.db)
            .await
            .unwrap();
        assert!(value["version"].is_string());
        assert_eq!(value["budget_total_bytes"], 64 << 20);
        assert_eq!(value["budget_used_bytes"], 0);
        assert!(value["rss_bytes"].as_u64().unwrap_or(1) > 0);
        publish(&ctx).await.unwrap(); // again: one row, replaced
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM server_state WHERE key LIKE 'worker:%'")
            .fetch_one(&ctx.db)
            .await
            .unwrap();
        assert_eq!(n, 1);
        drop_db(ctx, admin_url, name).await;
    }
}
