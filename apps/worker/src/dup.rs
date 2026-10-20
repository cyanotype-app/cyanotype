//! Near-duplicate scans (`cyanotype_core::duplicates`, `docs/library.md`, "Near-duplicates"): the worker looks every few seconds for an
//! owner whose scan is due (the owner pressed "Find duplicates", or an import has been quiet for a few minutes) and runs it.
use std::{sync::Arc, time::Duration};

use cyanotype_clip::Model;
use cyanotype_core::{duplicates, metrics};

use crate::handlers::Ctx;

/// How often the table of requests is looked at.
const EVERY: Duration = Duration::from_secs(20);

/// The thresholds of the model the library is searched with.
pub fn thresholds(model: Model) -> duplicates::Thresholds {
    let r = model.duplicate_rules();
    duplicates::Thresholds {
        model: model.id().to_string(),
        version: model.version(),
        dim: model.dim(),
        with_metadata: r.with_metadata,
        same_size: r.same_size,
        stripped: r.stripped,
    }
}

pub async fn run_loop(ctx: Arc<Ctx>) {
    let mut tick = tokio::time::interval(EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        // One owner per tick at most, so a long scan does not pile up behind itself.
        let due = match duplicates::claim_due(&ctx.db).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "could not look for a near-duplicate scan");
                continue;
            }
        };
        let Some((owner, requested)) = due else { continue };
        let started = std::time::Instant::now();
        let result = duplicates::scan(&ctx.db, owner, &thresholds(ctx.search_model)).await;
        let m = metrics::global();
        m.histogram("duplicates.scan_ms", started.elapsed().as_secs_f64() * 1000.0, &[]);
        match &result {
            Ok(r) => {
                tracing::info!(user_id = %owner, photos = r.photos, groups = r.groups, too_big = r.too_big, "near-duplicate scan finished");
                m.count("duplicates.groups", r.groups as u64, &[]);
            }
            Err(e) => tracing::warn!(user_id = %owner, error = %format!("{e:#}"), "near-duplicate scan failed"),
        }
        if let Err(e) = duplicates::finish(&ctx.db, owner, requested, result.map_err(|e| format!("{e:#}"))).await {
            tracing::warn!(error = %format!("{e:#}"), "could not record a near-duplicate scan");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thresholds_of_each_model_are_ordered_and_name_its_vectors() {
        for m in Model::ALL {
            let t = thresholds(m);
            assert_eq!((t.model.as_str(), t.version, t.dim), (m.id(), m.version(), m.dim()));
            // Two independent facts agreeing allow the lowest cosine; no fact at all the highest.
            assert!(
                0.5 < t.with_metadata && t.with_metadata < t.same_size && t.same_size < t.stripped && t.stripped < 1.0
            );
        }
    }
}
