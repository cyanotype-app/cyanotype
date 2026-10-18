mod budget;
mod burst;
mod embed;
mod exif;
mod finish;
mod formats;
mod handlers;
mod heif;
mod heif_tiles;
mod imaging;
mod media;
#[cfg(test)]
mod memcal;
mod raw;
#[cfg(test)]
mod raw_samples;
mod stream;
mod strip;
#[cfg(test)]
mod testgen;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use budget::MemoryBudget;
use cyanotype_core::{
    config::{self, DatabaseConfig, S3Config},
    db, jobs, metrics,
    storage::Storage,
    telemetry,
};
use handlers::{Ctx, JobInfo};
use tokio::sync::Semaphore;
use tracing::Instrument;

const DEFAULT_HEAVY_CONCURRENCY: usize = 2;
const DEFAULT_MAX_ORIGINAL_BYTES: u64 = 200 * 1024 * 1024;
/// Total decode memory (MB) all in-flight jobs may need together. The production container limit is
/// 1024m; the rest is headroom for the runtime, libheif/libwebp internals and allocator slack.
const DEFAULT_MEMORY_BUDGET_MB: u64 = 640;
/// Intra-op threads of the embedding session: one leaves the other core of a 2 vCPU host to thumbnails and Postgres.
const DEFAULT_EMBED_THREADS: usize = 1;
/// Job notifications (LISTEN/NOTIFY) wake the worker instantly; this poll is only the safety net for a missed
/// notification and for stale-job reclaim.
const FALLBACK_POLL: Duration = Duration::from_secs(30);
/// How often the queue gauges are sampled.
const GAUGE_EVERY: Duration = Duration::from_secs(15);

/// Every kind the main loop claims, in claim order. Embedding has its own, lower-priority loop (`embed`).
fn handled_kinds() -> impl Iterator<Item = &'static str> {
    jobs::DEFAULT_PIPELINE.iter().copied().chain([
        jobs::KIND_PAIR,
        jobs::KIND_GEOCODE,
        jobs::KIND_SHARE_COPY,
        jobs::KIND_BURST,
    ])
}

/// Every kind that has queue gauges.
fn gauge_kinds() -> impl Iterator<Item = &'static str> {
    handled_kinds().chain([jobs::KIND_EMBED])
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
    let decode_budget_mb = env_parse("WORKER_MEMORY_BUDGET_MB", DEFAULT_MEMORY_BUDGET_MB);
    let search_model = cyanotype_clip::Model::from_env()?;
    let embed_on = env_parse("WORKER_EMBED", 1u8) != 0;
    let embed_model_present = embed_on && embed::Embedder::model_installed(search_model);
    if embed_on && !embed_model_present {
        tracing::warn!(
            model = search_model.name(),
            dir = %search_model.dir().display(),
            "search model files not found: photos will not be embedded for search (run scripts/fetch-models.sh, or set SEARCH_MODEL_DIR)"
        );
    }
    let ctx = Arc::new(Ctx {
        db: pool.clone(),
        storage: Storage::new(&s3_cfg),
        max_original_bytes: env_parse("WORKER_MAX_ORIGINAL_BYTES", DEFAULT_MAX_ORIGINAL_BYTES),
        budget: MemoryBudget::new(decode_budget_mb << 20),
        raw_develop: env_parse("WORKER_RAW_DEVELOP", 1u8) != 0,
        search_model,
    });
    // Embedding (own loop, below). The image tower's resident memory (about 150 MB while the session is loaded) is not part
    // of the decode budget: the container limit is the decode budget plus that plus runtime slack (docker-compose.prod.yml,
    // docs/build.md), and `embed.model_loaded` / `worker.rss_bytes` show it against the limit.
    let embed_busy = Arc::new(Semaphore::new(1));
    let embed_task = if embed_model_present {
        let embedder = Arc::new(embed::Embedder::new(
            search_model,
            env_parse("WORKER_EMBED_THREADS", DEFAULT_EMBED_THREADS).max(1),
        ));
        // Its own LISTEN connection: a shared wake-up would be consumed by whichever loop happened to be waiting.
        let waker = jobs::JobWaker::spawn(&db_cfg.url);
        Some(tokio::spawn(embed::run_loop(
            ctx.clone(),
            embedder,
            waker,
            embed_busy.clone(),
        )))
    } else {
        None
    };
    // Bounds in-flight jobs (each holds a whole original plus decoded pixels in memory).
    let heavy = Arc::new(Semaphore::new(concurrency));

    tracing::info!(
        concurrency,
        max_original_bytes = ctx.max_original_bytes,
        memory_budget_mb = ctx.budget.total_bytes() >> 20,
        raw_develop = ctx.raw_develop,
        embedding = embed_task.is_some(),
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
                _ = &mut shutdown => return drain(heavy, concurrency, embed_busy, embed_task).await,
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
                _ = &mut shutdown => return drain(heavy, concurrency, embed_busy, embed_task).await,
            }
        }
    }
}

/// Wait (bounded) for in-flight jobs; anything unfinished is reclaimed after the stale timeout.
async fn drain(
    heavy: Arc<Semaphore>,
    concurrency: usize,
    embed_busy: Arc<Semaphore>,
    embed_task: Option<tokio::task::JoinHandle<()>>,
) -> Result<()> {
    tracing::info!("shutting down; waiting for in-flight jobs");
    let _ = tokio::time::timeout(Duration::from_secs(30), async {
        let _ = heavy.acquire_many(concurrency as u32).await;
        // The embed loop holds this for the length of a batch; taking it means no batch is in flight (and, held, that
        // none starts).
        let _ = embed_busy.acquire().await;
    })
    .await;
    if let Some(t) = embed_task {
        t.abort();
    }
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
                for kind in gauge_kinds() {
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
        if let Some(rss) = rss_bytes() {
            m.gauge("worker.rss_bytes", rss as f64, &[]);
        }
    }
}

/// This process's resident set, from /proc (Linux); None elsewhere.
fn rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kb * 1024)
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

/// Runs one job and records its outcome (see `finish::record`).
async fn execute_inner(ctx: &Arc<Ctx>, job: jobs::Job) {
    let started = std::time::Instant::now();
    let info = JobInfo::default();
    let result = handlers::run(ctx, &job, &info).await;
    finish::record(
        ctx,
        &job,
        info.family(),
        result,
        started.elapsed().as_secs_f64() * 1000.0,
    )
    .await;
}
