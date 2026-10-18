//! `burst` jobs: regroup the burst neighbourhood of one photo (`cyanotype_core::bursts`, `docs/library.md`, "Bursts").
//! Database only: the vectors the `embed` jobs stored are compared in SQL.
use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result};
use cyanotype_clip::Model;
use cyanotype_core::{bursts, jobs, metrics};

use crate::handlers::{Ctx, Outcome};

/// The thresholds in the order they are tried: the configured model first, then the others (which judge a pair of photos
/// the configured model has not embedded yet, while a library is re-indexed after a model change).
pub fn rules(configured: Model) -> Vec<bursts::Rule> {
    std::iter::once(configured)
        .chain(Model::ALL.into_iter().filter(|m| *m != configured))
        .map(|m| bursts::Rule {
            model: m.id().to_string(),
            version: m.version(),
            min_cosine: m.burst_rules().min_cosine,
        })
        .collect()
}

pub async fn run(ctx: &Arc<Ctx>, job: &jobs::Job) -> Result<Outcome> {
    if bursts::is_current(&ctx.db, job.asset_id)
        .await
        .context("check the photo")?
    {
        return Ok(Outcome::Skipped("already grouped"));
    }
    let started = Instant::now();
    let r = bursts::regroup(&ctx.db, job.asset_id, &rules(ctx.search_model))
        .await
        .context("regroup bursts")?;
    let m = metrics::global();
    m.histogram("bursts.regroup_ms", started.elapsed().as_secs_f64() * 1000.0, &[]);
    m.histogram("bursts.window_photos", r.photos as f64, &[]);
    if r.changed > 0 {
        m.count("bursts.photos_changed", r.changed, &[]);
    }
    tracing::info!(
        photos = r.photos,
        in_bursts = r.in_bursts,
        changed = r.changed,
        "regrouped bursts"
    );
    Ok(Outcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configured_model_judges_first_and_every_model_has_a_threshold() {
        for configured in Model::ALL {
            let r = rules(configured);
            assert_eq!(r.len(), Model::ALL.len());
            assert_eq!(r[0].model, configured.id());
            assert_eq!(r[0].version, configured.version());
            assert!(r.iter().all(|x| (0.5..1.0).contains(&x.min_cosine)));
        }
        assert_eq!(rules(Model::Siglip2B16)[1].model, Model::ClipB32.id());
    }
}
