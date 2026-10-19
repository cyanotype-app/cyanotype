//! Auth service: registration, login, logout, `/auth/me`. Routes live under `/auth/*`.
mod invites;
mod routes;
mod settings;
mod totp;
mod twofactor;
mod users_admin;

use std::sync::Arc;

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

    let cfg = settings::AuthConfig::from_env()?;
    tracing::info!(
        registration = ?cfg.registration,
        cookie_secure = cfg.cookie_secure,
        rate_limit = ?cfg.rate_limit,
        two_factor = cfg.totp_key.is_some(),
        trusted_proxies = cfg.trusted_proxies.len(),
        "auth settings"
    );
    if cfg.totp_key.is_none() {
        tracing::warn!(
            "AUTH_TOTP_KEY is not set: two-factor authentication cannot be set up, and accounts that already have it cannot sign in. See docs/deploy.md"
        );
    }
    if !cfg.cookie_secure {
        tracing::warn!(
            "COOKIE_SECURE=false: session cookies are sent without the Secure attribute; use only for local http"
        );
    }
    let state = routes::AppState {
        db: pool,
        cfg: Arc::new(cfg),
    };
    let app = Router::new()
        .route("/healthz", get(http::healthz))
        .merge(routes::routes(state));
    http::serve(http::with_http_layers(app), &addr).await
}
