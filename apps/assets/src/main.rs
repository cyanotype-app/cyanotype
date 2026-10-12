//! Assets service: presigned multipart upload flow and asset reads. Routes live under `/assets/*`.
mod routes;

use anyhow::Result;
use axum::{Router, extract::FromRef, routing::get};
use cyanotype_core::{
    config::{self, DatabaseConfig, S3Config},
    db,
    storage::Storage,
    telemetry,
};
use cyanotype_service::http;
use sqlx::PgPool;

const SERVICE: &str = "cyanotype-assets";

#[derive(Clone, FromRef)]
pub struct AppState {
    pub db: PgPool,
    pub storage: Storage,
}

#[tokio::main]
async fn main() -> Result<()> {
    telemetry::init(SERVICE);
    let db_cfg = DatabaseConfig::from_env()?;
    let s3_cfg = S3Config::from_env()?;
    let addr = config::bind_addr("ASSETS_BIND_ADDR", "0.0.0.0:3002");
    tracing::info!(database = %config::redact_url(&db_cfg.url), s3_bucket = %s3_cfg.bucket, bind_addr = %addr, "starting");
    // Schema is owned by `cyanotype migrate`; services never migrate on boot.
    let state = AppState {
        db: db::connect(&db_cfg.url).await?,
        storage: Storage::new(&s3_cfg),
    };

    let app = Router::new()
        .route("/healthz", get(http::healthz))
        .merge(routes::routes());
    http::serve(http::with_http_layers(app).with_state(state), &addr).await
}
