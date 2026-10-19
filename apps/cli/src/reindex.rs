//! `cyanotype reindex` and `cyanotype manifest`: thin wrappers over `cyanotype_core::reindex` / `manifest`.
//! Runs where `migrate` runs (it needs DATABASE_URL and the S3_* variables). See docs/recovery.md.
use std::{process::ExitCode, time::Duration};

use anyhow::{Context, Result, bail};
use clap::Args;
use cyanotype_core::{
    config::{DatabaseConfig, S3Config, redact_url},
    db, identity, manifest,
    reindex::{self, Options},
    storage::Storage,
    telemetry,
};

#[derive(Args)]
pub struct ReindexArgs {
    /// Write the changes. Without it (the default) reindex only reports what it would do.
    #[arg(long)]
    apply: bool,
    /// Report only; this is the default and the flag exists to say so explicitly.
    #[arg(long, conflicts_with = "apply")]
    dry_run: bool,
    /// Give objects whose owner no longer exists to this existing account instead of skipping them.
    #[arg(long, value_name = "EMAIL")]
    assign_to: Option<String>,
    /// Move rows whose original is missing from the bucket to the trash (they are never deleted).
    #[arg(long)]
    mark_missing: bool,
    /// Re-queue thumbnail jobs for rows whose thumbnail or preview is missing.
    #[arg(long)]
    rethumb: bool,
    /// Also write the self-describing metadata onto existing originals that lack it. A server-side self-copy:
    /// on a versioned bucket every such object is stored twice until the old version is removed. Needs --apply.
    #[arg(long)]
    backfill_metadata: bool,
    /// Ignore the per-user manifests (names, trash state, favorites, albums come only from object metadata).
    #[arg(long)]
    no_manifest: bool,
    /// Parallel S3 HEAD requests.
    #[arg(long, default_value_t = 16)]
    concurrency: usize,
    /// Leave alone objects newer than this many seconds (an upload may still be registering). 0 disables.
    #[arg(long, default_value_t = 600, value_name = "SECONDS")]
    min_age_secs: u64,
    /// Print the report as one JSON line at the end (the last line of stdout) instead of text.
    #[arg(long)]
    json: bool,
}

pub async fn run(a: ReindexArgs) -> Result<ExitCode> {
    telemetry::init("cyanotype-reindex");
    if a.backfill_metadata && !a.apply {
        tracing::info!("--backfill-metadata without --apply: reporting only");
    }
    let db_cfg = DatabaseConfig::from_env()?;
    let s3_cfg = S3Config::from_env()?;
    tracing::info!(database = %redact_url(&db_cfg.url), s3_bucket = %s3_cfg.bucket, apply = a.apply, "reindex");
    let pool = db::connect(&db_cfg.url).await?;
    let storage = Storage::new(&s3_cfg);

    let assign_to = match &a.assign_to {
        Some(email) => {
            let id: Option<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE lower(email) = lower($1)")
                .bind(email.trim())
                .fetch_optional(&pool)
                .await?;
            match id {
                Some((id,)) => Some(id),
                None => bail!("--assign-to: no user with that email (register the account first)"),
            }
        }
        None => None,
    };
    if a.concurrency == 0 || a.concurrency > 256 {
        bail!("--concurrency must be between 1 and 256");
    }
    let report = reindex::run(
        &pool,
        &storage,
        &Options {
            apply: a.apply,
            assign_to,
            mark_missing: a.mark_missing,
            rethumb: a.rethumb,
            backfill_metadata: a.backfill_metadata,
            use_manifests: !a.no_manifest,
            concurrency: a.concurrency,
            min_age: Duration::from_secs(a.min_age_secs),
        },
    )
    .await?;
    if a.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print!("{}", report.text());
    }
    Ok(if report.errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[derive(Args)]
pub struct ResetLinkArgs {
    /// The account's email address.
    #[arg(long, value_name = "EMAIL")]
    email: String,
    /// How long the link works.
    #[arg(long, default_value_t = 24, value_name = "HOURS")]
    hours: i32,
}

/// `cyanotype reset-link`: the same one-time link the admin portal makes, for an owner who cannot sign in.
pub async fn reset_link(a: ResetLinkArgs) -> Result<()> {
    telemetry::init("cyanotype-cli");
    let pool = db::connect(&DatabaseConfig::from_env()?.url).await?;
    let link = identity::create_reset_link(&pool, &a.email, a.hours).await?;
    println!("Reset link for {} (works once, until {}):", link.email, link.expires_at);
    println!("/reset-password?token={}", link.token);
    println!("Put your site's address in front of it and open it in a browser; the page asks for the new password.");
    Ok(())
}

/// `cyanotype manifest`: write today's manifests (accounts, and one per user with photos), replacing today's files.
pub async fn write_manifests() -> Result<()> {
    telemetry::init("cyanotype-cli");
    let pool = db::connect(&DatabaseConfig::from_env()?.url).await?;
    let storage = Storage::new(&S3Config::from_env()?);
    let keep = manifest::keep_from_env()?.max(1);
    let accounts = identity::write(&pool, &storage, keep)
        .await
        .context("account manifest")?;
    tracing::info!(accounts, "wrote account manifest");
    let users: Vec<(uuid::Uuid,)> = sqlx::query_as("SELECT DISTINCT owner_id FROM assets WHERE purging_at IS NULL")
        .fetch_all(&pool)
        .await?;
    for (user,) in &users {
        let n = manifest::write_user(&pool, &storage, *user, keep)
            .await
            .with_context(|| format!("manifest for {user}"))?;
        tracing::info!(user_id = %user, assets = n, "wrote recovery manifest");
    }
    println!("wrote manifests for {} user(s)", users.len());
    Ok(())
}
