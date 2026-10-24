mod api;
mod cache;
mod config;
#[cfg(feature = "server-tools")]
mod faces;
mod import;
#[cfg(feature = "server-tools")]
mod places;
#[cfg(feature = "server-tools")]
mod reindex;
mod release;
mod takeout;
mod update;
mod walk;

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
#[cfg(feature = "server-tools")]
use cyanotype_core::{
    config::{DatabaseConfig, redact_url},
    db, telemetry,
};

#[derive(Parser)]
#[command(name = "cyanotype", version = release::LONG_VERSION, about = "Cyanotype command-line tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Apply database migrations and load the bundled places dataset. The only place migrations run; services never
    /// do it on boot.
    #[cfg(feature = "server-tools")]
    Migrate,
    /// Rebuild or repair the asset index from the bucket (disaster recovery). Dry run unless --apply.
    #[cfg(feature = "server-tools")]
    Reindex(reindex::ReindexArgs),
    /// Write today's per-user recovery manifests to the bucket now (the assets service also does this daily).
    #[cfg(feature = "server-tools")]
    Manifest,
    /// Print a one-time password reset link for an account (after a bucket-only rebuild the restored accounts have locked
    /// passwords, and nobody can sign in to ask an admin for a link). Runs where `migrate` runs.
    #[cfg(feature = "server-tools")]
    ResetLink(reindex::ResetLinkArgs),
    /// Manage the data of the face-finding feature (off unless FACES_ENABLED is set). Runs where `migrate` runs.
    #[cfg(feature = "server-tools")]
    #[command(subcommand)]
    Faces(faces::FacesCommand),
    /// Log in to a Cyanotype server and store an API key for the other commands.
    Login(LoginArgs),
    /// Import photos and videos from files and directories (resumable; safe to re-run).
    Import(import::ImportArgs),
    /// Show the server, who you are logged in as, and how many assets it holds.
    Status,
    /// Update this program to the version your Cyanotype server offers (checksum and signature verified).
    Update(update::UpdateArgs),
}

#[derive(clap::Args)]
struct LoginArgs {
    /// Server URL, e.g. https://photos.example.com
    #[arg(long, env = config::ENV_SERVER)]
    server: String,
    /// Store this existing API key instead of creating one with email + password.
    #[arg(long)]
    api_key: Option<String>,
    /// Account email (prompted when omitted).
    #[arg(long)]
    email: Option<String>,
    /// Read the password from the first line of stdin instead of prompting (for scripts).
    #[arg(long)]
    password_stdin: bool,
    /// Name for the new API key (default: "cli on <hostname>").
    #[arg(long)]
    name: Option<String>,
    /// What the new key may do: `import` (upload and status only; the default), `read` or `full`.
    #[arg(long, value_parser = ["import", "read", "full"], default_value = "import")]
    scope: String,
    /// Two-factor code (or a recovery code), when the account has two-factor authentication (prompted when omitted).
    #[arg(long)]
    code: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<ExitCode> {
    let command = Cli::parse().command;
    // The once-a-day "a newer version exists" hint: started alongside the commands a person runs by hand, printed
    // after them, and never allowed to slow or fail them.
    let hint = match command {
        Command::Login(_) | Command::Import(_) | Command::Status => update::BackgroundCheck::start(),
        _ => update::BackgroundCheck::none(),
    };
    let code = dispatch(command).await;
    hint.finish().await;
    code
}

async fn dispatch(command: Command) -> Result<ExitCode> {
    match command {
        #[cfg(feature = "server-tools")]
        Command::Migrate => {
            telemetry::init("cyanotype-cli");
            let cfg = DatabaseConfig::from_env()?;
            tracing::info!(database = %redact_url(&cfg.url), "applying migrations");
            let pool = db::connect(&cfg.url).await?;
            db::migrate(&pool).await?;
            tracing::info!("migrations applied");
            places::load_bundled(&pool).await?;
        }
        #[cfg(feature = "server-tools")]
        Command::Reindex(a) => return reindex::run(a).await,
        #[cfg(feature = "server-tools")]
        Command::Manifest => reindex::write_manifests().await?,
        #[cfg(feature = "server-tools")]
        Command::ResetLink(a) => reindex::reset_link(a).await?,
        #[cfg(feature = "server-tools")]
        Command::Faces(c) => return faces::run(c).await,
        Command::Update(a) => update::run(a).await?,
        Command::Login(a) => login(a).await?,
        Command::Status => status().await?,
        Command::Import(a) => {
            return Ok(if import::run(a).await? {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn prompt(label: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        bail!("{label} is required (stdin is not a terminal; pass it as a flag)");
    }
    eprint!("{label}: ");
    std::io::stderr().flush()?;
    let mut s = String::new();
    std::io::stdin().lock().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn default_key_name() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown host".into());
    format!("cli on {host}")
}

async fn login(a: LoginArgs) -> Result<()> {
    let server = config::normalize_server(&a.server)?;
    // Kept apart from the key so that what is printed below (who) is never derived from the secret.
    let key: String;
    let who: String;
    match a.api_key {
        Some(given) => {
            key = given.trim().to_string();
            let me = api::Api::new(&server, Some(&key))?
                .me()
                .await
                .map_err(|e| anyhow::anyhow!("that API key was not accepted by {server}: {e}"))?;
            who = me.email;
        }
        None => {
            let email = match a.email {
                Some(e) => e,
                None => prompt("Email")?,
            };
            let password = if a.password_stdin {
                let mut p = String::new();
                std::io::stdin().lock().read_line(&mut p)?;
                p.trim_end_matches(['\r', '\n']).to_string()
            } else if let Ok(p) = std::env::var("CYANOTYPE_PASSWORD") {
                p
            } else {
                rpassword::prompt_password("Password: ").context("reading password")?
            };
            let api = api::Api::new(&server, None)?;
            let cookie = match api.login(&email, &password).await.map_err(|e| anyhow::anyhow!("{e}"))? {
                api::LoginOutcome::Session(c) => c,
                api::LoginOutcome::Challenge(ch) => {
                    let code = match a.code {
                        Some(c) => c,
                        None => prompt("Two-factor code")?,
                    };
                    api.login_code(&ch, &code).await.map_err(|e| anyhow::anyhow!("{e}"))?
                }
            };
            let name = a.name.unwrap_or_else(default_key_name);
            let created = api.create_api_key(&cookie, &name, &a.scope).await;
            // The session was only a means to mint the key; do not leave it around.
            api.logout(&cookie).await;
            key = created.map_err(|e| anyhow::anyhow!("creating API key: {e}"))?;
            who = email;
        }
    }
    let path = config::config_path()?;
    config::save_to(
        &path,
        &config::Config {
            server: Some(server.clone()),
            api_key: Some(key),
        },
    )?;
    println!("Logged in to {server} as {who}.");
    println!("Credentials saved to {} (mode 600).", path.display());
    Ok(())
}

async fn status() -> Result<()> {
    let c = config::credentials()?;
    let api = api::Api::new(&c.server, Some(&c.api_key))?;
    let src = |env: bool| if env { "environment" } else { "config file" };
    println!("Server:  {} ({})", c.server, src(c.server_from_env));
    println!(
        "API key: {}… ({})",
        c.api_key.chars().take(10).collect::<String>(),
        src(c.key_from_env)
    );
    let me = api.me().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("User:    {} ({})", me.email, me.id);
    if let Some(scope) = &me.scope {
        println!("Scope:   {scope}");
    }
    let n = api.asset_count().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("Assets:  {n}");
    Ok(())
}
