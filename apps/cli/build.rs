//! Stamps the build with its version, git sha and target triple.
//!
//! Release builds (the Docker `cli-dist` stage) pass `CYANOTYPE_CLI_VERSION` (from the `cli-vX.Y.Z` tag) and
//! `CYANOTYPE_GIT_SHA`. Without a version the build is a development build: it reports `<Cargo version>-dev`,
//! never checks for updates and never refuses to import for being old.
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=CYANOTYPE_CLI_VERSION");
    println!("cargo:rerun-if-env-changed=CYANOTYPE_GIT_SHA");
    println!("cargo:rerun-if-changed=build.rs");

    let release = std::env::var("CYANOTYPE_CLI_VERSION")
        .ok()
        .map(|v| v.trim().trim_start_matches("cli-v").trim_start_matches('v').to_string())
        .filter(|v| !v.is_empty());
    let version = release
        .clone()
        .unwrap_or_else(|| format!("{}-dev", std::env::var("CARGO_PKG_VERSION").unwrap()));
    let sha = std::env::var("CYANOTYPE_GIT_SHA")
        .ok()
        .map(|s| s.trim().chars().take(7).collect::<String>())
        .filter(|s| !s.is_empty())
        .or_else(local_git_sha)
        .unwrap_or_else(|| "unknown".into());

    println!("cargo:rustc-env=CYANOTYPE_VERSION={version}");
    println!("cargo:rustc-env=CYANOTYPE_SHA={sha}");
    println!("cargo:rustc-env=CYANOTYPE_IS_RELEASE={}", release.is_some());
    println!("cargo:rustc-env=CYANOTYPE_TARGET={}", std::env::var("TARGET").unwrap());
}

/// For a local `cargo build`: the checkout's short sha. There is no .git inside the Docker build context.
fn local_git_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}
