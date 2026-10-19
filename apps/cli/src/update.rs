//! `cyanotype update`, the once-a-day "a newer version exists" hint, and the server's minimum-version gate.
//!
//! The CLI is served by the Cyanotype server it talks to (`/cli/*`, docs/releases.md). Everything an update
//! installs is checked against the signed `SHA256SUMS` first; nothing unsigned or badly signed is ever installed.
use std::{
    io::{IsTerminal, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::{
    config,
    release::{self, Manifest, Version},
};

/// Largest download or unpacked binary we accept, so a hostile or broken server cannot fill the disk.
const MAX_BYTES: u64 = 256 * 1024 * 1024;
/// The hint runs at most this often.
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
/// How long a finished command waits for the background check before giving up on it.
const CHECK_GRACE: Duration = Duration::from_millis(1500);

#[derive(Args)]
pub struct UpdateArgs {
    /// Only report whether a newer version exists; install nothing.
    #[arg(long)]
    check: bool,
    /// Install this exact version (also a downgrade). The server keeps only its latest build, so this works only
    /// for the version it currently offers.
    #[arg(long, value_name = "X.Y.Z")]
    version: Option<String>,
    /// Server to update from (default: the one you are logged in to).
    #[arg(long, env = config::ENV_SERVER)]
    server: Option<String>,
}

// ---- URLs and downloads -----------------------------------------------------------------------------------------

fn client(timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(format!("cyanotype-cli/{}", release::VERSION))
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .build()?)
}

fn cli_url(server: &str, file: &str) -> String {
    format!("{}/cli/{file}", server.trim_end_matches('/'))
}

async fn fetch(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let res = http.get(url).send().await.with_context(|| format!("GET {url}"))?;
    let status = res.status();
    if !status.is_success() {
        bail!("GET {url}: {status}");
    }
    if res.content_length().is_some_and(|n| n > MAX_BYTES) {
        bail!("GET {url}: too large");
    }
    let bytes = res.bytes().await.with_context(|| format!("GET {url}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        bail!("GET {url}: too large");
    }
    Ok(bytes.to_vec())
}

async fn fetch_manifest(http: &reqwest::Client, server: &str) -> Result<(Manifest, Vec<u8>)> {
    let body = fetch(http, &cli_url(server, "latest.json")).await?;
    let m = Manifest::parse(std::str::from_utf8(&body).context("latest.json is not text")?)?;
    Ok((m, body))
}

/// The server to talk to without needing an API key: the flag/env, else the config file's.
fn resolve_server(arg: Option<&str>) -> Result<String> {
    if let Some(s) = arg.filter(|s| !s.trim().is_empty()) {
        return config::normalize_server(s);
    }
    let file = config::load_from(&config::config_path()?)?;
    match file.server {
        Some(s) => config::normalize_server(&s),
        None => bail!("no server configured: run `cyanotype login <server URL>` or pass --server URL"),
    }
}

// ---- refusing to touch what a package manager owns --------------------------------------------------------------

/// Advice if `exe` lives somewhere another tool manages (so `update` must not overwrite it), else None.
pub fn managed_by(exe: &Path) -> Option<&'static str> {
    let p = exe.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    let has = |s: &str| p.contains(s);
    if has("/cellar/") || has("/homebrew/") || has("/linuxbrew/") {
        return Some("it was installed by Homebrew: run `brew upgrade cyanotype`");
    }
    if has("/nix/store/") {
        return Some("it lives in the Nix store: update it through Nix");
    }
    if has("/snap/") {
        return Some("it was installed as a snap: run `snap refresh`");
    }
    if has("/scoop/") {
        return Some("it was installed by Scoop: run `scoop update cyanotype`");
    }
    if has("/chocolatey/") {
        return Some("it was installed by Chocolatey: run `choco upgrade cyanotype`");
    }
    if has("/winget/") || has("/windowsapps/") {
        return Some("it was installed by winget: run `winget upgrade cyanotype`");
    }
    if has("/.cargo/bin/") {
        return Some("it was installed with `cargo install`: run `cargo install --force` again");
    }
    for dir in ["/usr/bin/", "/bin/", "/usr/sbin/", "/sbin/", "/usr/lib/", "/opt/"] {
        if p.starts_with(dir) {
            return Some(
                "it is in a system directory owned by your package manager: update it with apt, dnf or similar",
            );
        }
    }
    None
}

// ---- replacing the running executable ---------------------------------------------------------------------------

/// Put `contents` in place of the executable at `target`, atomically: write a new file beside it, then rename it
/// over the old one. A running Unix process keeps its old inode, so replacing the file under it is safe. Windows
/// cannot overwrite a running exe but can rename it, so the old one is moved aside first (and put back on failure).
pub fn replace_executable(target: &Path, contents: &[u8]) -> Result<()> {
    let dir = target.parent().context("executable has no directory")?;
    let tmp = dir.join(format!(".cyanotype-update-{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let write = || -> Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(target)
                .map(|m| m.permissions().mode() & 0o7777)
                .unwrap_or(0o755);
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode | 0o111))?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        let hint = if e
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
        {
            format!(
                " (no permission to write to {}; run the update as the user who owns it, or with sudo)",
                dir.display()
            )
        } else {
            String::new()
        };
        return Err(e.context(format!("writing the new executable next to {}{hint}", target.display())));
    }
    if let Err(e) = swap_into_place(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.context(format!("replacing {}", target.display())));
    }
    Ok(())
}

#[cfg(unix)]
fn swap_into_place(new: &Path, target: &Path) -> Result<()> {
    std::fs::rename(new, target)?;
    Ok(())
}

#[cfg(windows)]
fn swap_into_place(new: &Path, target: &Path) -> Result<()> {
    let old = old_exe_path(target);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(target, &old).context("moving the running executable aside")?;
    if let Err(e) = std::fs::rename(new, target) {
        let _ = std::fs::rename(&old, target);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(windows)]
fn old_exe_path(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

/// Windows leaves the previous exe behind as `cyanotype.exe.old` (it was running); remove it next time.
fn cleanup_leftovers(_exe: &Path) {
    #[cfg(windows)]
    let _ = std::fs::remove_file(old_exe_path(_exe));
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut out)
        .context("unpacking the download")?;
    if out.len() as u64 > MAX_BYTES {
        bail!("the unpacked download is too large");
    }
    Ok(out)
}

// ---- the update itself ------------------------------------------------------------------------------------------

pub struct Plan {
    pub server: String,
    /// The release public key (`ssh-ed25519 ...`); None when this build has none compiled in.
    pub public_key: Option<String>,
    pub current: Version,
    pub release_build: bool,
    pub target: String,
    /// The executable to replace.
    pub exe: PathBuf,
    pub want: Option<String>,
    pub check_only: bool,
    /// Run the new binary with `--version` before swapping it in (off in tests, which download stand-in files).
    pub sanity_check: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate {
        current: Version,
        latest: Version,
    },
    /// The server offers an older build than the one running (only `--version` installs it).
    ServerBehind {
        current: Version,
        latest: Version,
    },
    Available {
        current: Version,
        latest: Version,
    },
    Installed {
        from: Version,
        to: Version,
    },
}

pub async fn perform(plan: &Plan) -> Result<Outcome> {
    let http = client(Duration::from_secs(300))?;
    let (manifest, manifest_body) = fetch_manifest(&http, &plan.server)
        .await
        .with_context(|| format!("asking {} for its CLI release", plan.server))?;
    let latest = manifest.version();
    let current = plan.current.clone();

    let wanted = match &plan.want {
        Some(w) => {
            let w = Version::parse(w)?;
            if w != latest {
                bail!(
                    "{} has version {latest} only (it keeps just the latest build, so {w} cannot be installed from it)",
                    plan.server
                );
            }
            Some(w)
        }
        None => None,
    };
    if wanted.as_ref() == Some(&current) || (wanted.is_none() && latest == current) {
        return Ok(Outcome::UpToDate { current, latest });
    }
    if wanted.is_none() && latest < current {
        return Ok(Outcome::ServerBehind { current, latest });
    }
    if plan.check_only {
        return Ok(Outcome::Available { current, latest });
    }

    if !plan.release_build {
        bail!("this is a development build ({current}); update only replaces release builds");
    }
    if let Some(advice) = managed_by(&plan.exe) {
        bail!("{} is not managed by cyanotype: {advice}", plan.exe.display());
    }
    if !manifest.signed {
        bail!(
            "{} offers an unsigned build (it was built without the release key); refusing to install it",
            plan.server
        );
    }
    let key = plan.public_key.as_deref().context(
        "this cyanotype has no release signing key built in, so it cannot verify an update; \
         install a release build with the install script",
    )?;
    let entry = manifest
        .targets
        .get(&plan.target)
        .with_context(|| format!("{} has no build for {}", plan.server, plan.target))?;

    let sums = fetch(&http, &cli_url(&plan.server, "SHA256SUMS")).await?;
    let sig = fetch(&http, &cli_url(&plan.server, "SHA256SUMS.sig"))
        .await
        .context("this server's build is not signed")?;
    let (sums, sig) = (String::from_utf8(sums)?, String::from_utf8(sig)?);
    // latest.json is listed in the signed sums too, so what we just read (version, file names) is authentic.
    release::verify_download(key, &sums, &sig, "latest.json", &manifest_body)
        .context("latest.json failed verification")?;
    let packed = fetch(&http, &cli_url(&plan.server, &entry.file)).await?;
    release::verify_download(key, &sums, &sig, &entry.file, &packed)
        .with_context(|| format!("{} failed verification; nothing was installed", entry.file))?;
    let binary = gunzip(&packed)?;
    if binary.len() < 1024 {
        bail!("the downloaded program is implausibly small; nothing was installed");
    }

    cleanup_leftovers(&plan.exe);
    if plan.sanity_check {
        sanity_check(&plan.exe, &binary, &latest)?;
    }
    replace_executable(&plan.exe, &binary)?;
    Ok(Outcome::Installed {
        from: current,
        to: latest,
    })
}

/// Run the new program once (`--version`) from a scratch file beside the target before swapping it in.
fn sanity_check(exe: &Path, binary: &[u8], expect: &Version) -> Result<()> {
    let dir = exe.parent().context("executable has no directory")?;
    let probe = dir.join(format!(".cyanotype-probe-{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut f = std::fs::File::create(&probe)?;
        f.write_all(binary)?;
        f.sync_all()?;
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755))?;
        }
        let out = std::process::Command::new(&probe)
            .arg("--version")
            .output()
            .context("the new program does not start on this machine")?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || !text.contains(&expect.to_string()) {
            bail!(
                "the new program did not report version {expect} (said {:?})",
                text.trim()
            );
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&probe);
    result.context("checking the downloaded program; nothing was installed")
}

pub async fn run(args: UpdateArgs) -> Result<()> {
    let server = resolve_server(args.server.as_deref())?;
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .context("locating this executable")?;
    let plan = Plan {
        server: server.clone(),
        public_key: release::compiled_in_key().map(str::to_string),
        current: Version::parse(release::VERSION)?,
        release_build: release::is_release_build(),
        target: release::TARGET.to_string(),
        exe,
        want: args.version,
        check_only: args.check,
        sanity_check: true,
    };
    match perform(&plan).await? {
        Outcome::UpToDate { current, .. } => println!("cyanotype {current} is up to date ({server})."),
        Outcome::ServerBehind { current, latest } => println!(
            "You have cyanotype {current}; {server} offers {latest}, which is older. \
             Run `cyanotype update --version {latest}` to go back to it."
        ),
        Outcome::Available { current, latest } => {
            println!("cyanotype {latest} is available (you have {current}). Run `cyanotype update`.")
        }
        Outcome::Installed { from, to } => println!("Updated cyanotype {from} -> {to} ({}).", plan.exe.display()),
    }
    Ok(())
}

// ---- the server's minimum version -------------------------------------------------------------------------------

/// Refuse (with how to fix it) when this CLI is older than the server's declared minimum. Any trouble reading the
/// server's `latest.json` (an older server without one, a network blip) lets the command go ahead: the gate guards
/// against known incompatibility, it must not make the CLI unusable.
pub async fn enforce_minimum(server: &str, current: &Version) -> Result<()> {
    let Ok(http) = client(Duration::from_secs(8)) else {
        return Ok(());
    };
    let Ok((m, _)) = fetch_manifest(&http, server).await else {
        return Ok(());
    };
    if release::meets_minimum(current, &m.min_cli()) {
        return Ok(());
    }
    bail!(
        "this cyanotype ({current}) is too old for {server}, which needs {} or newer. \
         Run `cyanotype update` and try again.",
        m.min_cli()
    )
}

// ---- the once-a-day hint ----------------------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckState {
    /// Unix seconds of the last check (successful or not, so an unreachable server is not retried every command).
    pub checked_at: u64,
    pub latest: Option<String>,
}

/// What the environment says about whether a hint may be shown.
#[derive(Debug, Clone, Copy, Default)]
pub struct CheckEnv {
    pub ci: bool,
    pub opted_out: bool,
    pub interactive: bool,
    pub release_build: bool,
}

fn truthy(v: Option<&str>) -> bool {
    matches!(v.map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some(s) if !s.is_empty() && s != "0" && s != "false" && s != "no")
}

impl CheckEnv {
    pub fn from_process() -> CheckEnv {
        let var = |k: &str| std::env::var(k).ok();
        CheckEnv {
            ci: truthy(var("CI").as_deref())
                || [
                    "GITHUB_ACTIONS",
                    "GITLAB_CI",
                    "BUILDKITE",
                    "JENKINS_URL",
                    "TF_BUILD",
                    "CIRCLECI",
                ]
                .iter()
                .any(|k| truthy(var(k).as_deref())),
            opted_out: truthy(var("CYANOTYPE_NO_UPDATE_CHECK").as_deref()),
            // The hint goes to stderr, so it is only for a person watching stderr.
            interactive: std::io::stderr().is_terminal() && std::io::stdin().is_terminal(),
            release_build: release::is_release_build(),
        }
    }
}

/// Whether to ask the server now.
pub fn should_check(env: &CheckEnv, state: Option<&CheckState>, now: u64) -> bool {
    if env.ci || env.opted_out || !env.interactive || !env.release_build {
        return false;
    }
    match state {
        Some(s) => now.saturating_sub(s.checked_at) >= CHECK_INTERVAL_SECS || s.checked_at > now,
        None => true,
    }
}

pub fn hint(current: &Version, latest: &Version) -> Option<String> {
    (latest > current).then(|| format!("cyanotype {latest} is available (you have {current}). Run `cyanotype update`."))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn state_path() -> Result<PathBuf> {
    config::cache_path("update-check.json")
}

fn load_state(path: &Path) -> Option<CheckState> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn save_state(path: &Path, s: &CheckState) -> Result<()> {
    std::fs::create_dir_all(path.parent().context("no cache dir")?)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(s)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// One check: ask the server, remember the time, return the hint if there is one. Errors are the caller's to ignore.
pub async fn check_once(server: &str, state_file: &Path, current: &Version, now: u64) -> Result<Option<String>> {
    // Record the attempt first: if the request hangs or fails, tomorrow is the next try, not the next command.
    let mut state = CheckState {
        checked_at: now,
        latest: None,
    };
    let _ = save_state(state_file, &state);
    let http = client(Duration::from_secs(5))?;
    let (m, _) = fetch_manifest(&http, server).await?;
    state.latest = Some(m.version.clone());
    let _ = save_state(state_file, &state);
    Ok(hint(current, &m.version()))
}

/// A background check started with the command and collected after it. Never fails and never waits long.
pub struct BackgroundCheck(Option<tokio::task::JoinHandle<Option<String>>>);

impl BackgroundCheck {
    pub fn none() -> BackgroundCheck {
        BackgroundCheck(None)
    }

    pub fn start() -> BackgroundCheck {
        BackgroundCheck(start_inner(CheckEnv::from_process()))
    }

    /// Print the hint to stderr if the check found one and finished promptly.
    pub async fn finish(self) {
        let Some(handle) = self.0 else { return };
        if let Ok(Ok(Some(msg))) = tokio::time::timeout(CHECK_GRACE, handle).await {
            eprintln!("{msg}");
        }
    }
}

fn start_inner(env: CheckEnv) -> Option<tokio::task::JoinHandle<Option<String>>> {
    // Cheap, local gates first: no task, no file read when the environment says no.
    if env.ci || env.opted_out || !env.interactive || !env.release_build {
        return None;
    }
    let state_file = state_path().ok()?;
    if !should_check(&env, load_state(&state_file).as_ref(), now_secs()) {
        return None;
    }
    // Only a logged-in server is asked; a CLI that was never pointed anywhere phones nobody.
    let server = config::load_from(&config::config_path().ok()?)
        .ok()
        .and_then(|c| c.server)
        .or_else(|| std::env::var(config::ENV_SERVER).ok())
        .and_then(|s| config::normalize_server(&s).ok())?;
    let current = Version::parse(release::VERSION).ok()?;
    Some(tokio::spawn(async move {
        check_once(&server, &state_file, &current, now_secs())
            .await
            .ok()
            .flatten()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::tests::TestKey;
    use axum::{Router, http::StatusCode, routing::get};
    use flate2::{Compression, write::GzEncoder};
    use std::{collections::HashMap, sync::Arc};

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    // -- throttle and switches

    fn env() -> CheckEnv {
        CheckEnv {
            ci: false,
            opted_out: false,
            interactive: true,
            release_build: true,
        }
    }

    #[test]
    fn check_runs_at_most_daily() {
        let now = 1_000_000;
        assert!(should_check(&env(), None, now));
        let fresh = CheckState {
            checked_at: now - 3600,
            latest: None,
        };
        assert!(!should_check(&env(), Some(&fresh), now));
        let almost = CheckState {
            checked_at: now - (CHECK_INTERVAL_SECS - 1),
            latest: None,
        };
        assert!(!should_check(&env(), Some(&almost), now));
        let stale = CheckState {
            checked_at: now - CHECK_INTERVAL_SECS,
            latest: None,
        };
        assert!(should_check(&env(), Some(&stale), now));
        // A clock that went backwards must not silence the check forever.
        let future = CheckState {
            checked_at: now + 10,
            latest: None,
        };
        assert!(should_check(&env(), Some(&future), now));
    }

    #[test]
    fn check_switches() {
        let now = 1_000_000;
        for off in [
            CheckEnv { ci: true, ..env() },
            CheckEnv {
                opted_out: true,
                ..env()
            },
            CheckEnv {
                interactive: false,
                ..env()
            },
            CheckEnv {
                release_build: false,
                ..env()
            },
        ] {
            assert!(!should_check(&off, None, now), "{off:?}");
            assert!(start_inner(off).is_none(), "{off:?}");
        }
        assert!(truthy(Some("1")) && truthy(Some("true")) && truthy(Some("yes")));
        assert!(!truthy(Some("0")) && !truthy(Some("false")) && !truthy(Some("")) && !truthy(None));
    }

    #[test]
    fn hint_text() {
        assert_eq!(
            hint(&v("0.2.0"), &v("0.3.0")).unwrap(),
            "cyanotype 0.3.0 is available (you have 0.2.0). Run `cyanotype update`."
        );
        assert!(hint(&v("0.3.0"), &v("0.3.0")).is_none());
        assert!(hint(&v("0.4.0"), &v("0.3.0")).is_none());
    }

    #[test]
    fn state_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join("update-check.json");
        assert!(load_state(&p).is_none());
        let s = CheckState {
            checked_at: 42,
            latest: Some("1.2.3".into()),
        };
        save_state(&p, &s).unwrap();
        assert_eq!(load_state(&p).unwrap(), s);
        std::fs::write(&p, b"garbage").unwrap();
        assert!(load_state(&p).is_none());
    }

    // -- a stub server that plays the part of /cli/*

    struct Dist {
        files: HashMap<String, Vec<u8>>,
    }

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    const TARGET: &str = "x86_64-unknown-linux-musl";
    const FILE: &str = "cyanotype-x86_64-unknown-linux-musl.gz";

    fn new_binary() -> Vec<u8> {
        vec![0x7f; 4096]
    }

    /// A signed (or unsigned) dist for `version`.
    fn dist(key: Option<&TestKey>, version: &str, min: &str, binary: &[u8], signed_flag: bool) -> Dist {
        let packed = gz(binary);
        let latest = serde_json::json!({
            "schema": 1, "version": version, "git_sha": "abc1234", "signed": signed_flag,
            "min_cli_version": min, "min_server_api": 1,
            "targets": { TARGET: { "file": FILE, "sha256": release::sha256_hex(&packed), "size": packed.len() } }
        })
        .to_string();
        let sums = format!(
            "{}  {FILE}\n{}  latest.json\n",
            release::sha256_hex(&packed),
            release::sha256_hex(latest.as_bytes())
        );
        let mut files = HashMap::new();
        if let Some(k) = key {
            files.insert(
                "SHA256SUMS.sig".to_string(),
                k.sign(sums.as_bytes(), release::NAMESPACE).into_bytes(),
            );
        }
        files.insert("SHA256SUMS".into(), sums.into_bytes());
        files.insert("latest.json".into(), latest.into_bytes());
        files.insert(FILE.into(), packed);
        Dist { files }
    }

    async fn serve(d: Dist) -> String {
        let files = Arc::new(d.files);
        let app = Router::new().route(
            "/cli/{name}",
            get(move |axum::extract::Path(name): axum::extract::Path<String>| {
                let files = files.clone();
                async move {
                    match files.get(&name) {
                        Some(b) => (StatusCode::OK, b.clone()),
                        None => (StatusCode::NOT_FOUND, Vec::new()),
                    }
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn plan(server: &str, key: Option<&TestKey>, exe: &Path) -> Plan {
        Plan {
            server: server.to_string(),
            public_key: key.map(TestKey::public_line),
            current: v("0.2.0"),
            release_build: true,
            target: TARGET.into(),
            exe: exe.to_path_buf(),
            want: None,
            check_only: false,
            sanity_check: false,
        }
    }

    fn fake_exe(dir: &Path) -> PathBuf {
        let p = dir.join("cyanotype");
        std::fs::write(&p, b"old binary").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o750)).unwrap();
        }
        p
    }

    #[tokio::test]
    async fn update_installs_a_signed_build() {
        let key = TestKey::generate();
        let base = serve(dist(Some(&key), "0.3.0", "0.1.0", &new_binary(), true)).await;
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());
        let out = perform(&plan(&base, Some(&key), &exe)).await.unwrap();
        assert_eq!(
            out,
            Outcome::Installed {
                from: v("0.2.0"),
                to: v("0.3.0")
            }
        );
        assert_eq!(std::fs::read(&exe).unwrap(), new_binary());
        // No temp files left behind, and the mode of the old file is kept.
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
                0o750 | 0o111
            );
        }
    }

    #[tokio::test]
    async fn update_refuses_unsigned_and_badly_signed() {
        let key = TestKey::generate();
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());

        // Built without the release key: marked unsigned, no .sig file.
        let base = serve(dist(None, "0.3.0", "0.1.0", &new_binary(), false)).await;
        let e = perform(&plan(&base, Some(&key), &exe)).await.unwrap_err();
        assert!(format!("{e:#}").contains("unsigned"), "{e:#}");

        // Claims to be signed but the signature is missing.
        let base = serve(dist(None, "0.3.0", "0.1.0", &new_binary(), true)).await;
        assert!(perform(&plan(&base, Some(&key), &exe)).await.is_err());

        // Signed by somebody else.
        let stranger = TestKey::generate();
        let base = serve(dist(Some(&stranger), "0.3.0", "0.1.0", &new_binary(), true)).await;
        assert!(perform(&plan(&base, Some(&key), &exe)).await.is_err());

        // Right signature, tampered binary.
        let mut d2 = dist(Some(&key), "0.3.0", "0.1.0", &new_binary(), true);
        d2.files.insert(FILE.into(), gz(&[1u8; 4096]));
        let base = serve(d2).await;
        let e = perform(&plan(&base, Some(&key), &exe)).await.unwrap_err();
        assert!(format!("{e:#}").contains("mismatch"), "{e:#}");

        // A build with no key compiled in cannot verify anything.
        let base = serve(dist(Some(&key), "0.3.0", "0.1.0", &new_binary(), true)).await;
        assert!(perform(&plan(&base, None, &exe)).await.is_err());

        assert_eq!(
            std::fs::read(&exe).unwrap(),
            b"old binary",
            "nothing may have been installed"
        );
    }

    #[tokio::test]
    async fn update_check_version_and_package_manager_rules() {
        let key = TestKey::generate();
        let base = serve(dist(Some(&key), "0.3.0", "0.1.0", &new_binary(), true)).await;
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());

        let mut p = plan(&base, Some(&key), &exe);
        p.check_only = true;
        assert_eq!(
            perform(&p).await.unwrap(),
            Outcome::Available {
                current: v("0.2.0"),
                latest: v("0.3.0")
            }
        );
        assert_eq!(std::fs::read(&exe).unwrap(), b"old binary");

        let mut p = plan(&base, Some(&key), &exe);
        p.current = v("0.3.0");
        assert!(matches!(perform(&p).await.unwrap(), Outcome::UpToDate { .. }));
        p.current = v("0.4.0");
        assert!(matches!(perform(&p).await.unwrap(), Outcome::ServerBehind { .. }));

        // --version: only what the server has. A downgrade to the server's (older) version works.
        let mut p = plan(&base, Some(&key), &exe);
        p.current = v("0.4.0");
        p.want = Some("0.3.0".into());
        assert!(matches!(perform(&p).await.unwrap(), Outcome::Installed { .. }));
        let mut p = plan(&base, Some(&key), &exe);
        p.want = Some("0.1.0".into());
        let e = perform(&p).await.unwrap_err();
        assert!(format!("{e:#}").contains("only"), "{e:#}");

        // Development builds and package-managed paths are refused.
        let mut p = plan(&base, Some(&key), &exe);
        p.release_build = false;
        assert!(perform(&p).await.is_err());
        let p = plan(&base, Some(&key), Path::new("/usr/bin/cyanotype"));
        assert!(format!("{:#}", perform(&p).await.unwrap_err()).contains("package manager"));
    }

    #[test]
    fn package_manager_paths() {
        for p in [
            "/usr/bin/cyanotype",
            "/opt/homebrew/Cellar/cyanotype/1.0/bin/cyanotype",
            "/usr/local/Cellar/cyanotype/1.0/bin/cyanotype",
            "/home/linuxbrew/.linuxbrew/bin/cyanotype",
            "/nix/store/abc-cyanotype/bin/cyanotype",
            "/snap/cyanotype/1/bin/cyanotype",
            "/home/u/.cargo/bin/cyanotype",
            "C:\\Users\\u\\scoop\\apps\\cyanotype\\current\\cyanotype.exe",
            "C:\\ProgramData\\chocolatey\\lib\\cyanotype\\cyanotype.exe",
        ] {
            assert!(managed_by(Path::new(p)).is_some(), "{p}");
        }
        for p in [
            "/home/u/.local/bin/cyanotype",
            "/usr/local/bin/cyanotype",
            "/tmp/x/cyanotype",
            "C:\\Users\\u\\AppData\\Local\\Programs\\cyanotype\\cyanotype.exe",
        ] {
            assert!(managed_by(Path::new(p)).is_none(), "{p}");
        }
    }

    #[test]
    fn replace_is_atomic_and_cleans_up() {
        let d = tempfile::tempdir().unwrap();
        let exe = fake_exe(d.path());
        replace_executable(&exe, b"new contents").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new contents");
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1, "no temp file left");

        // The old file stays readable through an open handle (as a running binary would be).
        let held = std::fs::File::open(&exe).unwrap();
        replace_executable(&exe, b"newer").unwrap();
        let mut old = String::new();
        let mut held = held;
        held.read_to_string(&mut old).unwrap();
        assert_eq!(old, "new contents");
        assert_eq!(std::fs::read(&exe).unwrap(), b"newer");

        // A directory that cannot be written leaves the executable alone.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let ro = tempfile::tempdir().unwrap();
            let target = fake_exe(ro.path());
            std::fs::set_permissions(ro.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
            // Root ignores directory permissions; only assert when the write really is refused.
            if replace_executable(&target, b"x").is_err() {
                assert_eq!(std::fs::read(&target).unwrap(), b"old binary");
            }
            std::fs::set_permissions(ro.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[tokio::test]
    async fn minimum_version_gate_against_a_server() {
        let key = TestKey::generate();
        let base = serve(dist(Some(&key), "0.9.0", "0.5.0", &new_binary(), true)).await;
        assert!(enforce_minimum(&base, &v("0.5.0")).await.is_ok());
        assert!(enforce_minimum(&base, &v("0.9.0")).await.is_ok());
        let e = enforce_minimum(&base, &v("0.4.9")).await.unwrap_err().to_string();
        assert!(
            e.contains("too old") && e.contains("cyanotype update") && e.contains("0.5.0"),
            "{e}"
        );
        // Development builds pass.
        assert!(enforce_minimum(&base, &v("0.0.1-dev")).await.is_ok());
        // A server without /cli/latest.json (older, or not Cyanotype's web image) does not block anything.
        let none = serve(Dist { files: HashMap::new() }).await;
        assert!(enforce_minimum(&none, &v("0.0.1")).await.is_ok());
        assert!(enforce_minimum("http://127.0.0.1:1", &v("0.0.1")).await.is_ok());
    }

    #[tokio::test]
    async fn background_check_records_time_and_hints_once() {
        let key = TestKey::generate();
        let base = serve(dist(Some(&key), "0.9.0", "0.1.0", &new_binary(), true)).await;
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("update-check.json");
        let msg = check_once(&base, &f, &v("0.2.0"), 5000).await.unwrap().unwrap();
        assert!(msg.starts_with("cyanotype 0.9.0 is available (you have 0.2.0)"));
        assert_eq!(
            load_state(&f).unwrap(),
            CheckState {
                checked_at: 5000,
                latest: Some("0.9.0".into())
            }
        );
        // An unreachable server still records the attempt, and reports an error the caller ignores.
        let f2 = d.path().join("two.json");
        assert!(check_once("http://127.0.0.1:1", &f2, &v("0.2.0"), 7000).await.is_err());
        assert_eq!(load_state(&f2).unwrap().checked_at, 7000);
        assert!(!should_check(&env(), load_state(&f2).as_ref(), 7000 + 60));
    }
}
