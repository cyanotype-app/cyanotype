mod api;
mod cache;
mod config;
#[cfg(feature = "server-tools")]
mod faces;
mod import;
mod login;
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
    /// Print who you are logged in as.
    Whoami,
    /// Revoke this machine's API key on the server and remove it from the config file.
    Logout,
    /// Update this program to the version your Cyanotype server offers (checksum and signature verified).
    Update(update::UpdateArgs),
}

#[derive(clap::Args)]
struct LoginArgs {
    /// Server URL, e.g. https://photos.example.com (default: the one from the last login).
    #[arg(value_name = "SERVER")]
    server_arg: Option<String>,
    /// Same as the SERVER argument.
    #[arg(long, env = config::ENV_SERVER)]
    server: Option<String>,
    /// Use the device-code login: print a short code to enter at <server>/activate on any device. Chosen automatically
    /// over SSH and on a machine without a display.
    #[arg(long)]
    device: bool,
    /// Store this existing API key instead of signing in through the browser.
    #[arg(long)]
    api_key: Option<String>,
    /// What the new key may do: `import` (upload and status only; the default) or `read`. For a `full` key, make one on
    /// the API keys page and pass it with --api-key.
    #[arg(long, value_parser = ["import", "read"], default_value = "import")]
    scope: String,
    /// Deprecated: sign in with an email and a password read from stdin (for old scripts; use the browser login, or
    /// `--api-key`).
    #[arg(long)]
    password_stdin: bool,
    /// With --password-stdin: the account email (prompted when omitted).
    #[arg(long)]
    email: Option<String>,
    /// With --password-stdin: name for the new API key (default: "cli on <hostname>").
    #[arg(long)]
    name: Option<String>,
    /// With --password-stdin: two-factor code (or a recovery code), when the account has two-factor authentication.
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
        Command::Whoami => whoami().await?,
        Command::Logout => logout().await?,
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
    format!("cli on {}", login::device_name())
}

/// `cyanotype login`: the browser (loopback, or the device code), or with `--api-key` / `--password-stdin` the older ways.
async fn login(a: LoginArgs) -> Result<()> {
    let legacy = a.password_stdin || std::env::var("CYANOTYPE_PASSWORD").is_ok_and(|p| !p.is_empty());
    if !legacy && (a.email.is_some() || a.name.is_some() || a.code.is_some()) {
        bail!(
            "--email, --name and --code belong to the deprecated password login (--password-stdin); the browser login needs none of them"
        );
    }
    let saved = config::load_from(&config::config_path()?)?;
    let server = a
        .server_arg
        .or(a.server)
        .or(saved.server)
        .context("which server? run `cyanotype login https://photos.example.com`")?;
    let server = config::normalize_server(&server)?;
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
        None if legacy => {
            eprintln!(
                "note: logging in with a password is deprecated; run `cyanotype login` without --password-stdin to sign in through the browser"
            );
            let email = match a.email {
                Some(e) => e,
                None => prompt("Email")?,
            };
            let password = match std::env::var("CYANOTYPE_PASSWORD").ok().filter(|p| !p.is_empty()) {
                Some(p) if !a.password_stdin => p,
                _ => {
                    let mut p = String::new();
                    std::io::stdin().lock().read_line(&mut p)?;
                    p.trim_end_matches(['\r', '\n']).to_string()
                }
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
        None => {
            let grant = login::login(&api::Api::new(&server, None)?, &server, &a.scope, a.device).await?;
            key = grant.key;
            who = grant.email;
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

async fn whoami() -> Result<()> {
    let c = config::credentials()?;
    let me = api::Api::new(&c.server, Some(&c.api_key))?
        .me()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{} on {}", me.email, c.server);
    Ok(())
}

/// Revokes the key in the config file on the server, then forgets it locally (the server address stays, so `login` needs no
/// argument). A key from `CYANOTYPE_API_KEY` is left alone: it is not ours to remove.
async fn logout() -> Result<()> {
    let path = config::config_path()?;
    let mut cfg = config::load_from(&path)?;
    let (Some(server), Some(key)) = (cfg.server.clone(), cfg.api_key.clone()) else {
        println!("Not logged in.");
        return Ok(());
    };
    match api::Api::new(&server, Some(&key))?.cli_logout().await {
        Ok(true) => println!("Revoked this machine's API key on {server}."),
        Ok(false) => println!("The server no longer knew this key (already revoked)."),
        Err(e) => eprintln!(
            "warning: could not revoke the key on {server} ({e}); remove it under Settings > API keys when the server is reachable"
        ),
    }
    cfg.api_key = None;
    config::save_to(&path, &cfg)?;
    println!("Removed the key from {}.", path.display());
    if std::env::var(config::ENV_API_KEY).is_ok_and(|k| !k.trim().is_empty()) {
        eprintln!(
            "note: {} is set in the environment and was not touched",
            config::ENV_API_KEY
        );
    }
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
