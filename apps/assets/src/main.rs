//! Assets service: presigned multipart upload flow and asset reads. Routes live under `/assets/*`.
mod janitor;
mod purge;
mod routes;
#[cfg(test)]
mod testutil;

use anyhow::Result;
use axum::{Router, extract::FromRef, routing::get};
use cyanotype_core::{
    config::{self, DatabaseConfig, S3Config},
    db, manifest, metrics,
    storage::Storage,
    telemetry,
};
use cyanotype_service::http;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Notify;

const SERVICE: &str = "cyanotype-assets";

#[derive(Clone, FromRef)]
pub struct AppState {
    pub db: PgPool,
    pub storage: Storage,
    /// Days in the trash before a photo is deleted for good (`TRASH_RETENTION_DAYS`).
    pub trash_retention_days: i32,
    /// Wakes the janitor to run queued purges now instead of at its next hourly pass.
    pub purge_wake: Arc<Notify>,
}

#[tokio::main]
async fn main() -> Result<()> {
    telemetry::init(SERVICE);
    metrics::init(SERVICE);
    let db_cfg = DatabaseConfig::from_env()?;
    let s3_cfg = S3Config::from_env()?;
    let addr = config::bind_addr("ASSETS_BIND_ADDR", "0.0.0.0:3002");
    let trash_retention_days = config::trash_retention_days()?;
    tracing::info!(trash_retention_days, "trash retention");
    tracing::info!(database = %config::redact_url(&db_cfg.url), s3_bucket = %s3_cfg.bucket, bind_addr = %addr, "starting");
    // Schema is owned by `cyanotype migrate`; services never migrate on boot.
    let state = AppState {
        db: db::connect(&db_cfg.url).await?,
        storage: Storage::new(&s3_cfg),
        trash_retention_days,
        purge_wake: Arc::new(Notify::new()),
    };

    // Daily per-user recovery manifests for `cyanotype reindex` (docs/recovery.md). The first pass waits so that a
    // freshly restored database can be reconciled before anything is written from it.
    manifest::spawn(
        state.db.clone(),
        state.storage.clone(),
        manifest::keep_from_env()?,
        std::time::Duration::from_secs(15 * 60),
    );

    janitor::spawn(
        state.db.clone(),
        state.storage.clone(),
        trash_retention_days,
        state.purge_wake.clone(),
    );

    let app = Router::new()
        .route("/healthz", get(http::healthz))
        .merge(routes::routes());
    http::serve(http::with_http_layers(app).with_state(state), &addr).await
}
