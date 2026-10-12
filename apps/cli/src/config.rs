//! Where the CLI keeps its server URL and API key, and where it caches file hashes.
//!
//! `$XDG_CONFIG_HOME/cyanotype/config.toml` (mode 600, directory mode 700), falling back to
//! `~/.config`. `CYANOTYPE_SERVER` and `CYANOTYPE_API_KEY` override the file.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const ENV_SERVER: &str = "CYANOTYPE_SERVER";
pub const ENV_API_KEY: &str = "CYANOTYPE_API_KEY";

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub server: Option<String>,
    pub api_key: Option<String>,
}

/// A usable server + key, and where they came from (for `status`).
#[derive(Debug, Clone)]
pub struct Credentials {
    pub server: String,
    pub api_key: String,
    pub server_from_env: bool,
    pub key_from_env: bool,
}

/// `<xdg>/cyanotype/<file>` when `xdg` is set and non-empty, else `<home>/<fallback>/cyanotype/<file>`.
pub fn xdg_path(xdg: Option<&str>, home: Option<&str>, fallback: &str, file: &str) -> Result<PathBuf> {
    let base = match xdg.filter(|v| !v.is_empty()) {
        Some(x) => PathBuf::from(x),
        None => match home.filter(|v| !v.is_empty()) {
            Some(h) => Path::new(h).join(fallback),
            None => bail!("cannot locate a config/cache directory: set HOME or XDG_CONFIG_HOME / XDG_CACHE_HOME"),
        },
    };
    Ok(base.join("cyanotype").join(file))
}

pub fn config_path() -> Result<PathBuf> {
    xdg_path(
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        ".config",
        "config.toml",
    )
}

pub fn cache_path(file: &str) -> Result<PathBuf> {
    xdg_path(
        std::env::var("XDG_CACHE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        ".cache",
        file,
    )
}

pub fn load_from(path: &Path) -> Result<Config> {
    match fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write the config with mode 600 (the file holds a credential). Replaced atomically via a temp file.
pub fn save_to(path: &Path, cfg: &Config) -> Result<()> {
    let dir = path.parent().context("config path has no parent")?;
    create_private_dir(dir)?;
    let tmp = path.with_extension("toml.tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = private_file(&tmp)?;
    f.write_all(toml::to_string_pretty(cfg)?.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("creating {}", dir.display()))
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))
}

#[cfg(unix)]
fn private_file(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))
}

#[cfg(not(unix))]
fn private_file(path: &Path) -> Result<fs::File> {
    fs::File::create(path).with_context(|| format!("creating {}", path.display()))
}

/// Trim, drop trailing slashes, require http(s).
pub fn normalize_server(s: &str) -> Result<String> {
    let s = s.trim().trim_end_matches('/');
    if !(s.starts_with("http://") || s.starts_with("https://")) || s.len() < 9 {
        bail!("server must be a URL like https://photos.example.com (got {s:?})");
    }
    Ok(s.to_string())
}

/// Environment beats file. Pure so it can be tested without touching the process environment.
pub fn resolve(file: &Config, env_server: Option<&str>, env_key: Option<&str>) -> Result<Credentials> {
    let env_server = env_server.filter(|v| !v.trim().is_empty());
    let env_key = env_key.filter(|v| !v.trim().is_empty());
    let server = env_server
        .map(str::to_string)
        .or_else(|| file.server.clone())
        .context("no server configured: run `cyanotype login --server URL` or set CYANOTYPE_SERVER")?;
    let api_key = env_key
        .map(|k| k.trim().to_string())
        .or_else(|| file.api_key.clone())
        .context("no API key configured: run `cyanotype login` or set CYANOTYPE_API_KEY")?;
    Ok(Credentials {
        server: normalize_server(&server)?,
        api_key,
        server_from_env: env_server.is_some(),
        key_from_env: env_key.is_some(),
    })
}

pub fn credentials() -> Result<Credentials> {
    let file = load_from(&config_path()?)?;
    resolve(
        &file,
        std::env::var(ENV_SERVER).ok().as_deref(),
        std::env::var(ENV_API_KEY).ok().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_paths() {
        let p = xdg_path(Some("/x"), Some("/h"), ".config", "config.toml").unwrap();
        assert_eq!(p, Path::new("/x/cyanotype/config.toml"));
        let p = xdg_path(Some(""), Some("/h"), ".config", "config.toml").unwrap();
        assert_eq!(p, Path::new("/h/.config/cyanotype/config.toml"));
        let p = xdg_path(None, Some("/h"), ".cache", "hashes.jsonl").unwrap();
        assert_eq!(p, Path::new("/h/.cache/cyanotype/hashes.jsonl"));
        assert!(xdg_path(None, None, ".config", "c").is_err());
    }

    #[test]
    fn env_overrides_file() {
        let file = Config {
            server: Some("https://file.example/".into()),
            api_key: Some("cyk_file".into()),
        };
        let c = resolve(&file, None, None).unwrap();
        assert_eq!(
            (c.server.as_str(), c.api_key.as_str()),
            ("https://file.example", "cyk_file")
        );
        assert!(!c.server_from_env && !c.key_from_env);
        let c = resolve(&file, Some("http://localhost:3000"), Some(" cyk_env\n")).unwrap();
        assert_eq!(
            (c.server.as_str(), c.api_key.as_str()),
            ("http://localhost:3000", "cyk_env")
        );
        assert!(c.server_from_env && c.key_from_env);
        // Empty env vars do not mask the file.
        let c = resolve(&file, Some(""), Some("  ")).unwrap();
        assert_eq!(c.api_key, "cyk_file");
        assert!(resolve(&Config::default(), None, Some("k")).is_err());
        assert!(resolve(&Config::default(), Some("https://a.example"), None).is_err());
        assert!(resolve(&file, Some("ftp://nope"), None).is_err());
    }

    #[test]
    fn save_and_load_roundtrip_with_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conf").join("cyanotype").join("config.toml");
        assert_eq!(load_from(&path).unwrap(), Config::default());
        let cfg = Config {
            server: Some("https://x.example".into()),
            api_key: Some("cyk_secret".into()),
        };
        save_to(&path, &cfg).unwrap();
        assert_eq!(load_from(&path).unwrap(), cfg);
        // Overwriting keeps it private and leaves no temp file.
        save_to(&path, &cfg).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(
                fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert!(!path.with_extension("toml.tmp").exists());
    }
}
