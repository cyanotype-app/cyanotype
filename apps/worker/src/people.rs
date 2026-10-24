//! The `people` run: grouping each owner's faces into people (`cyanotype_core::people`, docs/faces.md, "People"). Like the near-duplicate
//! scan (`dup`) it is per owner, not per photo, so it is not a row of the job queue: `people_runs` holds the requests, which the faces job
//! makes as it stores faces and the API makes when the owner changes a person. It waits until an owner's faces have stopped
//! arriving for a quiet spell (and none is queued or running for them), so a backfill groups once, and, like the faces job, it yields
//! to every other kind of work. It only reads and writes the database: no model, no photo, no byte budget.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use cyanotype_core::{jobs, metrics, people};

use crate::{faces, handlers::Ctx};

/// How often the table of requests is looked at.
const EVERY: Duration = Duration::from_secs(20);
/// How often owners whose faces were stored before People existed are asked for a first run.
const ASK_EVERY: Duration = Duration::from_secs(3600);

pub async fn run_loop(ctx: Arc<Ctx>) {
    let foreground: Vec<&str> = faces::foreground_kinds().collect();
    let mut tick = tokio::time::interval(EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_ask: Option<Instant> = None;
    loop {
        tick.tick().await;
        if last_ask.is_none_or(|t| t.elapsed() >= ASK_EVERY) {
            last_ask = Some(Instant::now());
            match people::request_missing(&ctx.db).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(owners = n, "asked for a first grouping of faces into people"),
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "could not ask for a grouping of faces"),
            }
        }
        match jobs::runnable_waiting(&ctx.db, &foreground).await {
            Ok(false) => {}
            Ok(true) => continue,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "could not check for foreground jobs");
                continue;
            }
        }
        // One owner per tick at most, so a long run does not pile up behind itself.
        let due = match people::claim_due(&ctx.db).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "could not look for a grouping of faces");
                continue;
            }
        };
        let Some((owner, requested)) = due else { continue };
        let started = Instant::now();
        let result = people::run(&ctx.db, owner).await;
        let m = metrics::global();
        m.histogram("people.run_ms", started.elapsed().as_secs_f64() * 1000.0, &[]);
        match &result {
            Ok(r) => {
                tracing::info!(
                    user_id = %owner,
                    looked_at = r.looked_at,
                    assigned = r.assigned,
                    people_made = r.created,
                    grouped = r.grouped,
                    left = r.left,
                    "faces grouped into people"
                );
                m.count("people.assigned", r.assigned as u64, &[]);
                m.count("people.created", r.created as u64, &[]);
                m.gauge("people.ungrouped_faces", r.left as f64, &[]);
            }
            Err(e) => tracing::warn!(user_id = %owner, error = %format!("{e:#}"), "grouping faces into people failed"),
        }
        if let Err(e) = people::finish(&ctx.db, owner, requested, result.map_err(|e| format!("{e:#}"))).await {
            tracing::warn!(error = %format!("{e:#}"), "could not record a grouping of faces");
        }
    }
}
