use anyhow::Result;
use clap::{Parser, Subcommand};
use cyanotype_core::{
    config::{self, DatabaseConfig},
    db, telemetry,
};

#[derive(Parser)]
#[command(name = "cyanotype", version, about = "Cyanotype command-line tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Apply database migrations. The only place migrations run; services never do it on boot.
    Migrate,
}

#[tokio::main]
async fn main() -> Result<()> {
    telemetry::init("cyanotype-cli");
    match Cli::parse().command {
        Command::Migrate => {
            let cfg = DatabaseConfig::from_env()?;
            tracing::info!(database = %config::redact_url(&cfg.url), "applying migrations");
            db::migrate(&db::connect(&cfg.url).await?).await?;
            tracing::info!("migrations applied");
        }
    }
    Ok(())
}
