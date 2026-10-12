use std::time::Duration;

use anyhow::Result;
use cyanotype_core::{config::Config, db, jobs};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cfg = Config::from_env()?;
    let pool = db::connect(&cfg.database_url).await?;

    tracing::info!("worker started");
    loop {
        let mut idle = true;
        for kind in jobs::DEFAULT_PIPELINE {
            let Some(job) = jobs::claim(&pool, kind).await? else {
                continue;
            };
            idle = false;
            // Handlers land in milestone 2 (EXIF, thumbnails, ThumbHash).
            tracing::warn!(job = job.id, kind = %job.kind, "no handler yet; failing job");
            jobs::fail(&pool, &job, "handler not implemented").await?;
        }
        if idle {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}
