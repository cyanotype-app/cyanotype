//! Postgres job queue. Enqueue is idempotent per (kind, asset, version);
//! claiming uses `FOR UPDATE SKIP LOCKED` so any number of workers can run.
use std::{sync::Arc, time::Duration};

use anyhow::Result;
use sqlx::{PgExecutor, PgPool, postgres::PgListener};
use tokio::sync::Notify;
use uuid::Uuid;

/// The NOTIFY channel a trigger on `jobs` (migration 0007) signals whenever a job becomes queued.
pub const NOTIFY_CHANNEL: &str = "jobs_queued";

pub const KIND_METADATA: &str = "metadata";
pub const KIND_THUMBNAIL: &str = "thumbnail";
pub const KIND_GEOCODE: &str = "geocode";
pub const KIND_EMBED: &str = "embed";
/// Backfill: try to pair an existing asset with its RAW/JPEG counterpart (`pairing`). New uploads pair inline when
/// their metadata job finishes, so this is never in `DEFAULT_PIPELINE`.
pub const KIND_PAIR: &str = "pair";
/// Write the location-free JPEG that share links serve for downloads (`derived/{user}/{asset}/share.jpg`).
/// Enqueued when a share that allows downloads is created or viewed, never for every upload.
pub const KIND_SHARE_COPY: &str = "share_copy";

/// Jobs enqueued for every new asset. Later pipeline stages are enqueued by earlier ones
/// or by the milestones that implement them.
pub const DEFAULT_PIPELINE: &[&str] = &[KIND_METADATA, KIND_THUMBNAIL];

/// A running job older than this is assumed to belong to a dead worker.
const STALE_AFTER_SECS: i64 = 600;
pub const MAX_ATTEMPTS: i32 = 5;

#[derive(Debug, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub asset_id: Uuid,
    pub version: i32,
    pub attempts: i32,
}

pub async fn enqueue(db: impl PgExecutor<'_>, kind: &str, asset_id: Uuid, version: i32) -> Result<()> {
    enqueue_with_priority(db, kind, asset_id, version, 0).await
}

/// Lower `priority` is claimed first within a kind (the default is 0). Background work such as the embedding
/// backfill uses [`PRIORITY_BACKFILL`] so photos uploaded meanwhile are embedded before it.
pub async fn enqueue_with_priority(
    db: impl PgExecutor<'_>,
    kind: &str,
    asset_id: Uuid,
    version: i32,
    priority: i16,
) -> Result<()> {
    sqlx::query("INSERT INTO jobs (kind, asset_id, version, priority) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
        .bind(kind)
        .bind(asset_id)
        .bind(version)
        .bind(priority)
        .execute(db)
        .await?;
    Ok(())
}

/// Priority of backfill jobs (migrations, the model-swap sweep): after everything enqueued by a new upload.
pub const PRIORITY_BACKFILL: i16 = 10;

/// Mark jobs whose worker died on their final attempt as failed. Without this they would
/// sit in 'running' forever: `claim` refuses them (attempts exhausted) and nobody calls `fail`.
async fn reap_exhausted(pool: &PgPool, kind: &str) -> Result<()> {
    sqlx::query(
        "UPDATE jobs SET state = 'failed', finished_at = now(), locked_at = NULL,
             last_error = coalesce(last_error, 'worker died on final attempt')
         WHERE kind = $1 AND state = 'running' AND attempts >= $3
           AND locked_at < now() - make_interval(secs => $2)",
    )
    .bind(kind)
    .bind(STALE_AFTER_SECS as f64)
    .bind(MAX_ATTEMPTS)
    .execute(pool)
    .await?;
    Ok(())
}

/// Claim one job of `kind`, also reclaiming jobs whose worker died mid-run.
/// A job is claimed at most `MAX_ATTEMPTS` times.
pub async fn claim(pool: &PgPool, kind: &str) -> Result<Option<Job>> {
    Ok(claim_batch(pool, kind, 1).await?.pop())
}

/// Claim up to `limit` jobs of `kind` in one statement, best first: lowest `priority`, then the earliest due, then
/// oldest. Same rules as [`claim`].
pub async fn claim_batch(pool: &PgPool, kind: &str, limit: i64) -> Result<Vec<Job>> {
    reap_exhausted(pool, kind).await?;
    let mut jobs = sqlx::query_as::<_, Job>(
        "UPDATE jobs SET state = 'running', locked_at = now(), attempts = attempts + 1
         WHERE id IN (
             SELECT id FROM jobs
             WHERE kind = $1 AND attempts < $3 AND run_after <= now()
               AND (state = 'queued' OR (state = 'running' AND locked_at < now() - make_interval(secs => $2)))
             ORDER BY priority, run_after, id
             FOR UPDATE SKIP LOCKED
             LIMIT $4)
         RETURNING id, kind, asset_id, version, attempts",
    )
    .bind(kind)
    .bind(STALE_AFTER_SECS as f64)
    .bind(MAX_ATTEMPTS)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    // RETURNING has no order; hand them back in id order so a batch is processed predictably.
    jobs.sort_by_key(|j| j.id);
    Ok(jobs)
}

/// Is any job of `kinds` runnable right now? Background work (embedding) checks this before taking a job, so a
/// burst of uploads has its thumbnails made before the backfill continues.
pub async fn runnable_waiting(pool: &PgPool, kinds: &[&str]) -> Result<bool> {
    let kinds: Vec<String> = kinds.iter().map(|k| (*k).to_string()).collect();
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM jobs WHERE kind = ANY($1) AND state = 'queued' AND attempts < $2 AND run_after <= now())",
    )
    .bind(&kinds)
    .bind(MAX_ATTEMPTS)
    .fetch_one(pool)
    .await?)
}

pub async fn complete(pool: &PgPool, id: i64) -> Result<()> {
    sqlx::query("UPDATE jobs SET state = 'done', finished_at = now(), last_error = NULL WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Requeue with exponential backoff, or mark failed once attempts are exhausted.
pub async fn fail(pool: &PgPool, job: &Job, error: &str) -> Result<()> {
    let exhausted = job.attempts >= MAX_ATTEMPTS;
    sqlx::query(
        "UPDATE jobs SET state = CASE WHEN $2 THEN 'failed' ELSE 'queued' END,
             last_error = $3, locked_at = NULL,
             run_after = now() + make_interval(secs => $4)
         WHERE id = $1",
    )
    .bind(job.id)
    .bind(exhausted)
    .bind(error)
    .bind(f64::from(2_i32.pow(job.attempts.clamp(1, 10) as u32)) * 5.0)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark failed immediately, without retries. For errors that retrying cannot fix
/// (corrupt file, checksum mismatch, over the size limit).
pub async fn fail_permanent(pool: &PgPool, id: i64, error: &str) -> Result<()> {
    sqlx::query(
        "UPDATE jobs SET state = 'failed', finished_at = now(), locked_at = NULL, last_error = $2 WHERE id = $1",
    )
    .bind(id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// How long until the earliest delayed job (a retry waiting out its backoff) among `kinds` becomes due.
/// `None` when nothing is waiting. Used to sleep exactly as long as needed instead of polling.
pub async fn next_due_in(pool: &PgPool, kinds: &[&str]) -> Result<Option<Duration>> {
    let kinds: Vec<String> = kinds.iter().map(|k| (*k).to_string()).collect();
    let secs: Option<f64> = sqlx::query_scalar(
        "SELECT extract(epoch FROM min(run_after) - now())::float8 FROM jobs
         WHERE state = 'queued' AND attempts < $2 AND kind = ANY($1) AND run_after > now()",
    )
    .bind(&kinds)
    .bind(MAX_ATTEMPTS)
    .fetch_one(pool)
    .await?;
    Ok(secs.map(|s| Duration::from_secs_f64(s.max(0.0))))
}

/// The worker's idle wait: returns when a job was queued (notification), when the earliest delayed retry among
/// `kinds` comes due, or after `fallback` (the safety net for a missed notification and for stale-job reclaim),
/// whichever is first. Callers claim again afterwards.
pub async fn wait_for_work(pool: &PgPool, waker: &JobWaker, kinds: &[&str], fallback: Duration) {
    let nap = match next_due_in(pool, kinds).await {
        // A tiny floor so a due-now job (or clock skew) cannot spin the loop.
        Ok(Some(due)) => due.max(Duration::from_millis(50)).min(fallback),
        Ok(None) => fallback,
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "could not look up the next due job");
            Duration::from_secs(5).min(fallback)
        }
    };
    tokio::select! {
        () = waker.woken() => {}
        () = tokio::time::sleep(nap) => {}
    }
}

/// Wakes the worker the moment a job is queued. A background task owns one **dedicated** Postgres connection
/// (not taken from the pool, so it can never starve it) that LISTENs on [`NOTIFY_CHANNEL`] and signals a
/// [`Notify`]. Wake-ups coalesce: any number of notifications while the worker is busy leave one pending
/// permit, and the worker then claims until the queue is empty.
///
/// Notifications are a latency optimisation, never the source of truth: the worker also polls on a fallback
/// interval, and the listener signals a wake-up after every (re)connect because notifications sent while it was
/// disconnected are lost.
#[derive(Clone)]
pub struct JobWaker {
    notify: Arc<Notify>,
}

impl JobWaker {
    /// Start listening. Returns immediately; the connection (and any reconnection) happens in the background.
    pub fn spawn(database_url: &str) -> Self {
        let notify = Arc::new(Notify::new());
        tokio::spawn(listen_forever(database_url.to_string(), notify.clone()));
        Self { notify }
    }

    /// A waker nothing signals (tests, or a caller that only wants the fallback poll).
    pub fn inert() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
        }
    }

    /// Wake the waiting worker loop from inside the process (a background loop that finished a job does this so
    /// the next claim happens at once instead of after the fallback poll).
    pub fn wake(&self) {
        self.notify.notify_one();
    }

    /// Completes when a job was queued since the last call (or the listener (re)connected).
    pub async fn woken(&self) {
        self.notify.notified().await;
    }
}

async fn listen_forever(url: String, notify: Arc<Notify>) {
    let mut backoff = Duration::from_millis(500);
    loop {
        // (Re)connect. `PgListener` also reconnects by itself after a dropped connection; this loop covers
        // the initial connect and a listener that could not recover.
        let mut listener = match connect_listener(&url).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), retry_in_ms = backoff.as_millis() as u64, "job listener could not connect");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
                continue;
            }
        };
        backoff = Duration::from_millis(500);
        tracing::info!(channel = NOTIFY_CHANNEL, "listening for new jobs");
        // Anything queued while we were not listening produced no message we can see.
        notify.notify_one();
        loop {
            match listener.try_recv().await {
                Ok(Some(_)) => notify.notify_one(),
                // The connection dropped and sqlx re-established it (and re-LISTENed): messages may have been lost.
                Ok(None) => {
                    tracing::warn!("job listener reconnected");
                    notify.notify_one();
                }
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), "job listener failed; reconnecting");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    notify.notify_one();
                    break;
                }
            }
        }
    }
}

async fn connect_listener(url: &str) -> Result<PgListener> {
    let mut l = PgListener::connect(url).await?;
    l.listen(NOTIFY_CHANNEL).await?;
    Ok(l)
}

/// Unfinished (and failed) job counts per kind and state, for the queue gauges.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueStats {
    pub kind: String,
    pub queued: i64,
    pub running: i64,
    pub failed: i64,
    /// Seconds the oldest *runnable* queued job has been waiting (0 when none; delayed retries do not count).
    pub oldest_queued_age_secs: f64,
}

pub async fn queue_stats(pool: &PgPool) -> Result<Vec<QueueStats>> {
    let rows: Vec<(String, i64, i64, i64, f64)> = sqlx::query_as(
        "SELECT kind,
                count(*) FILTER (WHERE state = 'queued'),
                count(*) FILTER (WHERE state = 'running'),
                count(*) FILTER (WHERE state = 'failed'),
                coalesce(extract(epoch FROM now() - min(run_after) FILTER (WHERE state = 'queued' AND run_after <= now())), 0)::float8
         FROM jobs WHERE state <> 'done' GROUP BY kind ORDER BY kind",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, queued, running, failed, oldest)| QueueStats {
            kind,
            queued,
            running,
            failed,
            oldest_queued_age_secs: oldest.max(0.0),
        })
        .collect())
}

/// Seconds the queue has had runnable work and made no progress: the lesser of "how long the oldest runnable job
/// has waited" and "how long since any job finished successfully". 0 when nothing is waiting. A healthy busy
/// queue stays near 0; a dead or wedged worker makes it grow, which is what the "no jobs completed" monitor watches.
pub async fn stalled_seconds(pool: &PgPool) -> Result<f64> {
    let secs: Option<f64> = sqlx::query_scalar(
        "SELECT CASE WHEN w.oldest IS NULL THEN 0
                     ELSE least(w.oldest, coalesce(d.last_done, w.oldest)) END::float8
         FROM (SELECT extract(epoch FROM now() - min(run_after)) AS oldest
               FROM jobs WHERE state = 'queued' AND attempts < $1 AND run_after <= now()) w,
              (SELECT extract(epoch FROM now() - max(finished_at)) AS last_done
               FROM jobs WHERE state = 'done') d",
    )
    .bind(MAX_ATTEMPTS)
    .fetch_one(pool)
    .await?;
    Ok(secs.unwrap_or(0.0).max(0.0))
}
