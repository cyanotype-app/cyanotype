mod exif;
mod handlers;
mod imaging;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use cyanotype_core::{
    config::{self, DatabaseConfig, S3Config},
    db, jobs,
    storage::Storage,
    telemetry,
};
use handlers::{Ctx, Outcome, Permanent};
use tokio::sync::Semaphore;
use tracing::Instrument;

const DEFAULT_HEAVY_CONCURRENCY: usize = 2;
const DEFAULT_MAX_ORIGINAL_BYTES: u64 = 200 * 1024 * 1024;

fn env_parse<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[tokio::main]
async fn main() -> Result<()> {
    telemetry::init("cyanotype-worker");
    let db_cfg = DatabaseConfig::from_env()?;
    let s3_cfg = S3Config::from_env()?;
    tracing::info!(database = %config::redact_url(&db_cfg.url), s3_bucket = %s3_cfg.bucket, "starting");
    let pool = db::connect(&db_cfg.url).await?;
    let concurrency = env_parse("WORKER_HEAVY_CONCURRENCY", DEFAULT_HEAVY_CONCURRENCY).max(1);
    let ctx = Arc::new(Ctx {
        db: pool.clone(),
        storage: Storage::new(&s3_cfg),
        max_original_bytes: env_parse("WORKER_MAX_ORIGINAL_BYTES", DEFAULT_MAX_ORIGINAL_BYTES),
    });
    // Bounds in-flight jobs (each holds a whole original plus decoded pixels in memory).
    let heavy = Arc::new(Semaphore::new(concurrency));

    tracing::info!(
        concurrency,
        max_original_bytes = ctx.max_original_bytes,
        "worker started"
    );
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    loop {
        let mut claimed = false;
        for kind in jobs::DEFAULT_PIPELINE {
            // Take a permit before claiming so we never hold a claimed job idle.
            let permit = tokio::select! {
                p = heavy.clone().acquire_owned() => p?,
                _ = &mut shutdown => return drain(heavy, concurrency).await,
            };
            match jobs::claim(&pool, kind).await {
                Ok(Some(job)) => {
                    claimed = true;
                    let ctx = ctx.clone();
                    tokio::spawn(async move {
                        execute(&ctx, job).await;
                        drop(permit);
                    });
                }
                Ok(None) => drop(permit),
                Err(e) => {
                    drop(permit);
                    tracing::error!(error = %format!("{e:#}"), "claim failed");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
        if !claimed {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                _ = &mut shutdown => return drain(heavy, concurrency).await,
            }
        }
    }
}

/// Wait (bounded) for in-flight jobs; anything unfinished is reclaimed after the stale timeout.
async fn drain(heavy: Arc<Semaphore>, concurrency: usize) -> Result<()> {
    tracing::info!("shutting down; waiting for in-flight jobs");
    let _ = tokio::time::timeout(Duration::from_secs(30), heavy.acquire_many(concurrency as u32)).await;
    Ok(())
}

async fn execute(ctx: &Arc<Ctx>, job: jobs::Job) {
    let span = tracing::info_span!("job", job_id = job.id, kind = %job.kind, asset_id = %job.asset_id, attempts = job.attempts);
    execute_inner(ctx, job).instrument(span).await;
}

/// Runs one job and logs exactly one summary line with `outcome` and `duration_ms`.
/// `outcome` is one of: done, skipped, retry, failed.
async fn execute_inner(ctx: &Arc<Ctx>, job: jobs::Job) {
    let started = std::time::Instant::now();
    let result = handlers::run(ctx, &job).await;
    let duration_ms = || started.elapsed().as_millis() as u64;
    let recorded = match result {
        Ok(outcome) => {
            if let Outcome::Skipped(reason) = outcome {
                tracing::info!(outcome = "skipped", reason, duration_ms = duration_ms(), "job finished");
            } else {
                tracing::info!(outcome = "done", duration_ms = duration_ms(), "job finished");
            }
            jobs::complete(&ctx.db, job.id).await
        }
        Err(e) if e.is::<Permanent>() => {
            let msg = format!("{e:#}");
            tracing::error!(outcome = "failed", permanent = true, error = %msg, duration_ms = duration_ms(), "job finished");
            jobs::fail_permanent(&ctx.db, job.id, &msg).await
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if job.attempts >= jobs::MAX_ATTEMPTS {
                tracing::error!(outcome = "failed", permanent = false, error = %msg, duration_ms = duration_ms(), "job finished; attempts exhausted");
            } else {
                tracing::warn!(outcome = "retry", error = %msg, duration_ms = duration_ms(), "job finished; will retry");
            }
            jobs::fail(&ctx.db, &job, &msg).await
        }
    };
    if let Err(e) = recorded {
        tracing::error!(error = %format!("{e:#}"), "could not record job outcome");
    }
}
