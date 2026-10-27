//! `cyanotype pets delete`: remove all pet data (docs/pets.md, "Operating it"). Runs where `migrate` runs (it needs DATABASE_URL).
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use cyanotype_core::{
    config::{DatabaseConfig, redact_url},
    db, pets, telemetry,
};

#[derive(Subcommand)]
pub enum PetsCommand {
    /// Delete the dogs and cats found in photos, the record of which photos were scanned and the queued scan jobs (for one account or
    /// for everyone). The photos and everything else are untouched. Dry run unless --apply.
    Delete(DeleteArgs),
}

#[derive(Args)]
pub struct DeleteArgs {
    /// Only this account's pet data (its email address). Without it, everyone's.
    #[arg(long, value_name = "EMAIL")]
    email: Option<String>,
    /// Delete. Without it (the default) only the numbers are reported.
    #[arg(long)]
    apply: bool,
}

pub async fn run(cmd: PetsCommand) -> Result<ExitCode> {
    match cmd {
        PetsCommand::Delete(a) => delete(a).await,
    }
}

async fn delete(a: DeleteArgs) -> Result<ExitCode> {
    telemetry::init("cyanotype-pets");
    let db_cfg = DatabaseConfig::from_env()?;
    tracing::info!(database = %redact_url(&db_cfg.url), apply = a.apply, "delete pet data");
    let pool = db::connect(&db_cfg.url).await?;
    let owner = match &a.email {
        Some(email) => {
            let id: Option<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE lower(email) = lower($1)")
                .bind(email.trim())
                .fetch_optional(&pool)
                .await?;
            match id {
                Some((id,)) => Some(id),
                None => bail!("--email: no account with that address"),
            }
        }
        None => None,
    };
    let d = pets::delete_all(&pool, owner, !a.apply).await?;
    let scope = if owner.is_some() {
        "that account"
    } else {
        "every account"
    };
    if a.apply {
        println!(
            "Deleted {} detections, {} scan records and {} queued or finished scan jobs of {scope}.",
            d.detections, d.scans, d.jobs
        );
        if pets::enabled() {
            println!(
                "PETS_ENABLED is on in this environment (it is on unless set to 0): the worker will scan the photos again. Set PETS_ENABLED=0 and restart the worker to keep it off."
            );
        }
    } else {
        println!(
            "Would delete {} detections, {} scan records and {} queued or finished scan jobs of {scope}. Run again with --apply to delete them.",
            d.detections, d.scans, d.jobs
        );
    }
    Ok(ExitCode::SUCCESS)
}
