//! Auth service: registration, login, logout, `/auth/me`. Routes live under `/auth/*`.
mod routes;

use anyhow::Result;
use axum::{Router, routing::get};
use cyanotype_core::{
    config::{self, DatabaseConfig},
    db, telemetry,
};
use cyanotype_service::http;

const SERVICE: &str = "cyanotype-auth";

#[tokio::main]
async fn main() -> Result<()> {
    telemetry::init(SERVICE);
    let db_cfg = DatabaseConfig::from_env()?;
    let addr = config::bind_addr("AUTH_BIND_ADDR", "0.0.0.0:3001");
    tracing::info!(database = %config::redact_url(&db_cfg.url), bind_addr = %addr, "starting");
    // Schema is owned by `cyanotype migrate`; services never migrate on boot.
    let pool = db::connect(&db_cfg.url).await?;

    let app = Router::new()
        .route("/healthz", get(http::healthz))
        .merge(routes::routes());
    http::serve(http::with_http_layers(app).with_state(pool), &addr).await
}
