//! Image embeddings in `asset_embeddings` and the jobs that produce them. The model itself lives in the
//! `cyanotype-clip` crate (this crate must not link ONNX Runtime), so callers pass the model id and version in.
use anyhow::Result;
use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use crate::jobs;

/// pgvector's text input format (`[0.1,0.2,...]`), for `vector` and `halfvec` alike: bind as text, cast in SQL.
pub fn pg_literal(v: &[f32]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(v.len() * 10 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "{x}");
    }
    s.push(']');
    s
}

/// Store a photo's vector for `(model, version)`, replacing that model's previous vector for it. Vectors of other
/// models stay until [`prune_superseded`] removes them: search keeps answering from them while the new model's
/// vectors fill in.
pub async fn store(db: &PgPool, asset_id: Uuid, model: &str, version: i32, embedding: &[f32]) -> Result<()> {
    sqlx::query(
        "INSERT INTO asset_embeddings (asset_id, model, version, embedding)
         VALUES ($1, $2, $3, $4::text::halfvec)
         ON CONFLICT (asset_id, model) DO UPDATE SET version = EXCLUDED.version, embedding = EXCLUDED.embedding,
             created_at = now()",
    )
    .bind(asset_id)
    .bind(model)
    .bind(version)
    .bind(pg_literal(embedding))
    .execute(db)
    .await?;
    Ok(())
}

/// How many visible photos with derived images have no vector for `(model, version)` (what the sweep would queue).
pub async fn missing_count(db: impl PgExecutor<'_>, model: &str, version: i32) -> Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM assets a
         WHERE a.thumbhash IS NOT NULL AND a.primary_id IS NULL AND a.purging_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM asset_embeddings e
                           WHERE e.asset_id = a.id AND e.model = $1 AND e.version = $2)",
    )
    .bind(model)
    .bind(version)
    .fetch_one(db)
    .await?;
    Ok(n)
}

/// Delete the vectors of every model other than `keep`, and older versions of `keep` itself. Called once every photo has
/// a vector for `(keep, version)`, so nothing is left to serve from the old ones. Returns the rows removed.
pub async fn prune_superseded(db: impl PgExecutor<'_>, keep: &str, version: i32) -> Result<u64> {
    Ok(
        sqlx::query("DELETE FROM asset_embeddings WHERE model <> $1 OR version <> $2")
            .bind(keep)
            .bind(version)
            .execute(db)
            .await?
            .rows_affected(),
    )
}

/// Queue an embed job for every visible photo that has derived images and no vector for `(model, version)`: the
/// catch-up after a model swap, after a photo's hidden-RAW status ended (unpair), and for jobs that were lost. A job
/// already queued, running or failed is left alone; one that finished without producing a vector is requeued.
/// Returns how many jobs were queued.
pub async fn enqueue_missing(db: impl PgExecutor<'_>, model: &str, version: i32) -> Result<u64> {
    let done = sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version, priority)
         SELECT 'embed', a.id, $2, $3 FROM assets a
         WHERE a.thumbhash IS NOT NULL AND a.primary_id IS NULL AND a.purging_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM asset_embeddings e
                           WHERE e.asset_id = a.id AND e.model = $1 AND e.version = $2)
         ON CONFLICT (kind, asset_id, version) DO UPDATE
             SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL,
                 last_error = NULL, priority = EXCLUDED.priority
             WHERE jobs.state = 'done'",
    )
    .bind(model)
    .bind(version)
    .bind(jobs::PRIORITY_BACKFILL)
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

/// Embedding progress for one owner's visible, live photos that have derived images (what search can cover).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub indexed: i64,
    pub total: i64,
}

pub async fn progress(db: impl PgExecutor<'_>, owner: Uuid, model: &str, version: i32) -> Result<Progress> {
    let (indexed, total): (i64, i64) = sqlx::query_as(
        "SELECT count(e.asset_id), count(*) FROM assets a
         LEFT JOIN asset_embeddings e ON e.asset_id = a.id AND e.model = $2 AND e.version = $3
         WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.primary_id IS NULL AND a.thumbhash IS NOT NULL",
    )
    .bind(owner)
    .bind(model)
    .bind(version)
    .fetch_one(db)
    .await?;
    Ok(Progress { indexed, total })
}

/// How far one model's vectors cover an owner's library, and whether this deployment can embed queries with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCoverage {
    pub model: String,
    pub version: i32,
    pub progress: Progress,
    /// The text tower's files are installed here, so queries can be embedded for this model.
    pub available: bool,
}

impl ModelCoverage {
    /// Whole percent of the library covered (100 for an empty library: nothing is missing).
    pub fn percent(&self) -> u8 {
        if self.progress.total <= 0 {
            return 100;
        }
        (self.progress.indexed * 100 / self.progress.total).clamp(0, 100) as u8
    }
}

/// Which model answers a search: the configured one once it covers at least `min_percent` of the library, until then
/// whichever installed model has the most photos indexed (the previous model, while a swap re-indexes). Returns the
/// index into `coverage`, or `None` when nothing is installed or `configured` is not listed.
pub fn choose_serving(coverage: &[ModelCoverage], configured: &str, min_percent: u8) -> Option<usize> {
    let target = coverage.iter().position(|c| c.model == configured)?;
    let t = &coverage[target];
    if t.available && (t.progress.total <= 0 || t.percent() >= min_percent) {
        return Some(target);
    }
    // Most vectors wins; the configured model wins a tie (an empty library, or two models equally far along).
    coverage
        .iter()
        .enumerate()
        .filter(|(_, c)| c.available)
        .max_by_key(|(i, c)| (c.progress.indexed, *i == target))
        .map(|(i, _)| i)
}

/// Coverage of each `(model id, version, installed)` for one owner.
pub async fn coverage(db: &PgPool, owner: Uuid, models: &[(&str, i32, bool)]) -> Result<Vec<ModelCoverage>> {
    let mut out = Vec::with_capacity(models.len());
    for &(model, version, available) in models {
        out.push(ModelCoverage {
            model: model.to_string(),
            version,
            progress: progress(db, owner, model, version).await?,
            available,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cov(model: &str, indexed: i64, total: i64, available: bool) -> ModelCoverage {
        ModelCoverage {
            model: model.into(),
            version: 1,
            progress: Progress { indexed, total },
            available,
        }
    }

    #[test]
    fn the_old_model_serves_until_the_configured_one_is_far_enough() {
        let pick = |new: i64| {
            let c = [cov("old", 1000, 1000, true), cov("new", new, 1000, true)];
            choose_serving(&c, "new", 95).map(|i| c[i].model.clone())
        };
        assert_eq!(pick(0).as_deref(), Some("old"));
        assert_eq!(pick(500).as_deref(), Some("old"));
        assert_eq!(pick(949).as_deref(), Some("old"));
        assert_eq!(pick(950).as_deref(), Some("new"));
        assert_eq!(pick(1000).as_deref(), Some("new"));
    }

    #[test]
    fn a_fresh_or_empty_library_uses_the_configured_model() {
        let c = [cov("old", 0, 0, true), cov("new", 0, 0, true)];
        assert_eq!(choose_serving(&c, "new", 95), Some(1));
        // Photos but no vectors anywhere: the configured model (search answers "indexing", not an error).
        let c = [cov("old", 0, 40, true), cov("new", 0, 40, true)];
        assert_eq!(choose_serving(&c, "new", 95), Some(1));
        // The configured model has started, the old one has nothing: no reason to look back.
        let c = [cov("old", 0, 40, true), cov("new", 3, 40, true)];
        assert_eq!(choose_serving(&c, "new", 95), Some(1));
    }

    #[test]
    fn a_model_that_is_not_installed_is_never_chosen() {
        let c = [cov("old", 1000, 1000, false), cov("new", 400, 1000, true)];
        assert_eq!(choose_serving(&c, "new", 95), Some(1));
        let c = [cov("old", 400, 1000, true), cov("new", 1000, 1000, false)];
        assert_eq!(choose_serving(&c, "new", 95), Some(0));
        let c = [cov("old", 400, 1000, false), cov("new", 1000, 1000, false)];
        assert_eq!(choose_serving(&c, "new", 95), None);
        assert_eq!(choose_serving(&c, "unknown", 95), None);
    }

    #[test]
    fn percent_rounds_down_so_ninety_five_means_ninety_five() {
        assert_eq!(cov("m", 949, 1000, true).percent(), 94);
        assert_eq!(cov("m", 1, 3, true).percent(), 33);
        assert_eq!(cov("m", 0, 0, true).percent(), 100);
    }
}
