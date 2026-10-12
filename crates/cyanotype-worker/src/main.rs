mod exif;
mod handlers;
mod imaging;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use cyanotype_core::{config::Config, db, jobs, storage::Storage};
use handlers::{Ctx, Outcome, Permanent};
use tokio::sync::Semaphore;

const DEFAULT_HEAVY_CONCURRENCY: usize = 2;
const DEFAULT_MAX_ORIGINAL_BYTES: u64 = 200 * 1024 * 1024;

fn env_parse<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cfg = Config::from_env()?;
    let pool = db::connect(&cfg.database_url).await?;
    let concurrency = env_parse("WORKER_HEAVY_CONCURRENCY", DEFAULT_HEAVY_CONCURRENCY).max(1);
    let ctx = Arc::new(Ctx {
        db: pool.clone(),
        storage: Storage::new(&cfg),
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
    let started = std::time::Instant::now();
    let result = handlers::run(ctx, &job).await;
    let recorded = match result {
        Ok(outcome) => {
            if let Outcome::Skipped(reason) = outcome {
                tracing::info!(job = job.id, kind = %job.kind, asset = %job.asset_id, reason, "job skipped");
            } else {
                tracing::info!(job = job.id, kind = %job.kind, asset = %job.asset_id,
                    ms = started.elapsed().as_millis() as u64, "job done");
            }
            jobs::complete(&ctx.db, job.id).await
        }
        Err(e) if e.is::<Permanent>() => {
            let msg = format!("{e:#}");
            tracing::error!(job = job.id, kind = %job.kind, asset = %job.asset_id, error = %msg, "job failed permanently");
            jobs::fail_permanent(&ctx.db, job.id, &msg).await
        }
        Err(e) => {
            let msg = format!("{e:#}");
            tracing::warn!(job = job.id, kind = %job.kind, asset = %job.asset_id, attempt = job.attempts,
                error = %msg, "job failed; will retry unless attempts are exhausted");
            jobs::fail(&ctx.db, &job, &msg).await
        }
    };
    if let Err(e) = recorded {
        tracing::error!(job = job.id, error = %format!("{e:#}"), "could not record job outcome");
    }
}
