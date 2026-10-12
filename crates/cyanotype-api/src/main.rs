mod assets;
mod auth;
mod error;

use anyhow::Result;
use axum::{Json, Router, routing::get};
use cyanotype_core::{config::Config, db, storage::Storage};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub storage: Storage,
}

async fn healthz() -> Json<Value> {
    Json(json!({"ok": true}))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cfg = Config::from_env()?;
    let pool = db::connect(&cfg.database_url).await?;
    db::migrate(&pool).await?;
    let state = AppState {
        db: pool,
        storage: Storage::new(&cfg),
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .merge(auth::routes())
        .merge(assets::routes())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    tracing::info!(addr = %cfg.bind_addr, "api listening");
    axum::serve(listener, app).await?;
    Ok(())
}
