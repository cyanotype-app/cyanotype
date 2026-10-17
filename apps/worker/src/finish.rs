//! How a finished job is recorded: one summary log line, the job metrics, and the queue state.
use std::sync::Arc;

use cyanotype_core::{jobs, metrics};

use crate::handlers::{Ctx, Outcome, Permanent};

/// Logs exactly one summary line with `outcome` and `duration_ms` (inside the `job` span, so it also carries `job.id`,
/// `job.kind`, `job.attempts` and `asset.id`), emits the job metrics and updates the queue row.
/// `outcome` is one of: done, skipped, retry, failed.
pub async fn record(ctx: &Arc<Ctx>, job: &jobs::Job, family: &str, result: anyhow::Result<Outcome>, elapsed_ms: f64) {
    let duration_ms = elapsed_ms as u64;
    let m = metrics::global();
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
            jobs::fail(&ctx.db, job, &msg).await
        }
    };
    if let Err(e) = recorded {
        tracing::error!(error = %format!("{e:#}"), "could not record job outcome");
    }
}
