mod budget;
mod exif;
mod formats;
mod handlers;
mod heif;
mod imaging;
mod media;
#[cfg(test)]
mod memcal;
mod raw;
#[cfg(test)]
mod raw_samples;
mod strip;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use budget::MemoryBudget;
use cyanotype_core::{
    config::{self, DatabaseConfig, S3Config},
    db, jobs, metrics,
    storage::Storage,
    telemetry,
};
use handlers::{Ctx, JobInfo, Outcome, Permanent};
use tokio::sync::Semaphore;
use tracing::Instrument;

const DEFAULT_HEAVY_CONCURRENCY: usize = 2;
const DEFAULT_MAX_ORIGINAL_BYTES: u64 = 200 * 1024 * 1024;
/// Total decode memory (MB) all in-flight jobs may need together. The production container limit is
/// 1024m; the rest is headroom for the runtime, libheif/libwebp internals and allocator slack.
const DEFAULT_MEMORY_BUDGET_MB: u64 = 640;
/// Job notifications (LISTEN/NOTIFY) wake the worker instantly; this poll is only the safety net for a missed
/// notification and for stale-job reclaim.
const FALLBACK_POLL: Duration = Duration::from_secs(30);
/// How often the queue gauges are sampled.
const GAUGE_EVERY: Duration = Duration::from_secs(15);

/// Every kind this worker claims, in claim order.
fn handled_kinds() -> impl Iterator<Item = &'static str> {
    jobs::DEFAULT_PIPELINE
        .iter()
        .copied()
        .chain([jobs::KIND_PAIR, jobs::KIND_GEOCODE, jobs::KIND_SHARE_COPY])
}

fn env_parse<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[tokio::main]
async fn main() -> Result<()> {
    telemetry::init("cyanotype-worker");
    metrics::init("cyanotype-worker");
    let db_cfg = DatabaseConfig::from_env()?;
    let s3_cfg = S3Config::from_env()?;
    tracing::info!(database = %config::redact_url(&db_cfg.url), s3_bucket = %s3_cfg.bucket, "starting");
    let pool = db::connect(&db_cfg.url).await?;
    let concurrency = env_parse("WORKER_HEAVY_CONCURRENCY", DEFAULT_HEAVY_CONCURRENCY).max(1);
    let ctx = Arc::new(Ctx {
        db: pool.clone(),
        storage: Storage::new(&s3_cfg),
        max_original_bytes: env_parse("WORKER_MAX_ORIGINAL_BYTES", DEFAULT_MAX_ORIGINAL_BYTES),
        budget: MemoryBudget::new(env_parse("WORKER_MEMORY_BUDGET_MB", DEFAULT_MEMORY_BUDGET_MB) << 20),
        raw_develop: env_parse("WORKER_RAW_DEVELOP", 1u8) != 0,
    });
    // Bounds in-flight jobs (each holds a whole original plus decoded pixels in memory).
    let heavy = Arc::new(Semaphore::new(concurrency));

    tracing::info!(
        concurrency,
        max_original_bytes = ctx.max_original_bytes,
        memory_budget_mb = ctx.budget.total_bytes() >> 20,
        raw_develop = ctx.raw_develop,
        "worker started"
    );
    // Instant pickup: a dedicated LISTEN connection (not from the pool) wakes the loop below.
    let waker = jobs::JobWaker::spawn(&db_cfg.url);
    tokio::spawn(sample_gauges(ctx.clone()));
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    loop {
        let mut claimed = false;
        for kind in handled_kinds() {
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
            // Idle: sleep until a notification, the earliest delayed retry coming due, or the fallback poll.
            let kinds: Vec<&str> = handled_kinds().collect();
            tokio::select! {
                () = jobs::wait_for_work(&pool, &waker, &kinds, FALLBACK_POLL) => {}
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

/// Queue gauges, per kind: queued, running, failed, and the age of the oldest runnable queued job. Every handled
/// kind is reported every time (zeros included), so a drained queue reads 0 instead of going stale.
async fn sample_gauges(ctx: Arc<Ctx>) {
    let m = metrics::global();
    if !m.is_enabled() {
        return;
    }
    let mut tick = tokio::time::interval(GAUGE_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        match jobs::queue_stats(&ctx.db).await {
            Ok(stats) => {
                for kind in handled_kinds() {
                    let s = stats.iter().find(|s| s.kind == kind);
                    let tags = [("kind", kind)];
                    m.gauge("queue.queued", s.map_or(0.0, |s| s.queued as f64), &tags);
                    m.gauge("queue.running", s.map_or(0.0, |s| s.running as f64), &tags);
                    m.gauge("queue.failed", s.map_or(0.0, |s| s.failed as f64), &tags);
                    m.gauge(
                        "queue.oldest_queued_age_seconds",
                        s.map_or(0.0, |s| s.oldest_queued_age_secs),
                        &tags,
                    );
                }
            }
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "could not sample queue gauges"),
        }
        match jobs::stalled_seconds(&ctx.db).await {
            Ok(secs) => m.gauge("queue.stalled_seconds", secs, &[]),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "could not sample stalled seconds"),
        }
        m.gauge("worker.memory_budget.used_bytes", ctx.budget.used_bytes() as f64, &[]);
        m.gauge("worker.memory_budget.total_bytes", ctx.budget.total_bytes() as f64, &[]);
    }
}

async fn execute(ctx: &Arc<Ctx>, job: jobs::Job) {
    let span = tracing::info_span!(
        "job",
        job.id = job.id,
        job.kind = %job.kind,
        job.attempts = job.attempts,
        asset.id = %job.asset_id
    );
    execute_inner(ctx, job).instrument(span).await;
}

/// Runs one job and logs exactly one summary line with `outcome` and `duration_ms` (inside the `job` span, so it
/// also carries `job.id`, `job.kind`, `job.attempts` and `asset.id`). Also emits the job metrics.
/// `outcome` is one of: done, skipped, retry, failed.
async fn execute_inner(ctx: &Arc<Ctx>, job: jobs::Job) {
    let started = std::time::Instant::now();
    let info = JobInfo::default();
    let result = handlers::run(ctx, &job, &info).await;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let duration_ms = elapsed_ms as u64;
    let m = metrics::global();
    let family = info.family();
    // `outcome` tags the duration histogram; the counters are separate metrics so they can be summed simply.
    let record = |counter: &str, outcome: &str, extra: &[(&str, &str)]| {
        let mut tags = vec![("kind", job.kind.as_str()), ("family", family), ("outcome", outcome)];
        tags.extend_from_slice(extra);
        m.count(counter, 1, &tags);
        m.histogram(
            "jobs.duration",
            elapsed_ms,
            &[("kind", &job.kind), ("family", family), ("outcome", outcome)],
        );
    };
    let recorded = match result {
        Ok(outcome) => {
            if let Outcome::Skipped(reason) = outcome {
                tracing::info!(outcome = "skipped", reason, duration_ms, "job finished");
                record("jobs.completed", "skipped", &[]);
            } else {
                tracing::info!(outcome = "done", duration_ms, "job finished");
                record("jobs.completed", "done", &[]);
            }
            jobs::complete(&ctx.db, job.id).await
        }
        Err(e) if e.is::<Permanent>() => {
            let msg = format!("{e:#}");
            tracing::error!(outcome = "failed", permanent = true, error = %msg, duration_ms, "job finished");
            record("jobs.failed", "failed", &[("permanent", "true")]);
            jobs::fail_permanent(&ctx.db, job.id, &msg).await
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if job.attempts >= jobs::MAX_ATTEMPTS {
                tracing::error!(outcome = "failed", permanent = false, error = %msg, duration_ms, "job finished; attempts exhausted");
                record("jobs.failed", "failed", &[("permanent", "false")]);
            } else {
                tracing::warn!(outcome = "retry", error = %msg, duration_ms, "job finished; will retry");
                record("jobs.retried", "retry", &[]);
            }
            jobs::fail(&ctx.db, &job, &msg).await
        }
    };
    if let Err(e) = recorded {
        tracing::error!(error = %format!("{e:#}"), "could not record job outcome");
    }
}
