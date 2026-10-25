//! The storage integrity check's worker side (`cyanotype_core::integrity`, docs/admin.md, "Storage integrity"): the nightly
//! schedule, and the loop that runs the check when a task for it is queued (by the schedule, or by Admin > Health).
//!
//! It has its own loop, apart from the other admin tasks, so a long scan never holds up an account deletion, and it runs behind
//! the upload pipeline: the checksum sample waits while thumbnails or metadata are queued.
use std::{sync::Arc, time::Duration};

use cyanotype_core::{admin_tasks, integrity};

use crate::handlers::Ctx;

/// How often the worker checks whether tonight's run is due.
const CHECK_EVERY: Duration = Duration::from_secs(600);
/// How often it looks for a queued task.
const TASK_POLL: Duration = Duration::from_secs(10);

pub fn spawn(ctx: Arc<Ctx>) {
    match integrity::schedule_from_env() {
        Some(hour) => {
            tracing::info!(hour_utc = hour, "nightly integrity check scheduled");
            tokio::spawn(schedule_loop(ctx.clone(), hour));
        }
        None => tracing::info!("nightly integrity check is off (INTEGRITY_CHECK_ENABLED=0); Run check now still works"),
    }
    tokio::spawn(task_loop(ctx));
}

async fn schedule_loop(ctx: Arc<Ctx>, hour: u8) {
    // Let the worker settle (and migrations finish on a fresh deploy) before the first look.
    tokio::time::sleep(Duration::from_secs(180)).await;
    loop {
        match integrity::schedule_if_due(&ctx.db, hour).await {
            Ok(true) => tracing::info!("queued the nightly integrity check"),
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "could not schedule the integrity check"),
        }
        tokio::time::sleep(CHECK_EVERY).await;
    }
}

async fn task_loop(ctx: Arc<Ctx>) {
    loop {
        match run_one(&ctx).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "integrity task loop failed"),
        }
        tokio::time::sleep(TASK_POLL).await;
    }
}

/// Claims and runs one integrity task. Returns whether there was one.
pub async fn run_one(ctx: &Ctx) -> anyhow::Result<bool> {
    let Some(task) = admin_tasks::claim(&ctx.db, &[admin_tasks::KIND_INTEGRITY_CHECK]).await? else {
        return Ok(false);
    };
    tracing::info!(task = task.id, "integrity check started");
    let trigger = if task.params.get("trigger").and_then(|t| t.as_str()) == Some("schedule") {
        "schedule"
    } else {
        "manual"
    };
    let outcome = match integrity::begin(&ctx.db, trigger, task.requested_by).await {
        Ok(run) => integrity::run(&ctx.db, &ctx.storage, run, &integrity::Options::from_env())
            .await
            .map_err(|e| format!("{e:#}"))
            .and_then(|s| serde_json::to_value(s).map_err(|e| e.to_string())),
        Err(e) => Err(format!("{e:#}")),
    };
    if let Err(e) = &outcome {
        tracing::warn!(task = task.id, error = %e, "integrity check failed");
    }
    // Keep the failure text short and free of anything but what the worker itself wrote.
    let outcome = outcome.map_err(|e| e.chars().take(500).collect::<String>());
    admin_tasks::finish(&ctx.db, task.id, outcome).await?;
    Ok(true)
}
