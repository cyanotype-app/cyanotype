use anyhow::Result;
use clap::{Parser, Subcommand};
use cyanotype_core::{config::Config, db};

#[derive(Parser)]
#[command(name = "cyanotype", version, about = "Cyanotype command-line tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Apply database migrations.
    Migrate,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match Cli::parse().command {
        Command::Migrate => {
            let cfg = Config::from_env()?;
            db::migrate(&db::connect(&cfg.database_url).await?).await?;
            println!("migrations applied");
        }
    }
    Ok(())
}
