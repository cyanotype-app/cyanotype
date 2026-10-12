//! Environment-driven configuration. Each service loads only the pieces it needs
//! (`DatabaseConfig`, `S3Config`, and a bind address); there is no catch-all config.
//!
//! Types holding secrets implement `Debug` by hand so that `{:?}` can never leak them.
use std::fmt;

use anyhow::{Context, Result};

pub struct DatabaseConfig {
    pub url: String,
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("url", &redact_url(&self.url))
            .finish()
    }
}

#[derive(Clone)]
pub struct S3Config {
    pub endpoint: Option<String>,
    /// Endpoint embedded in presigned URLs; must be reachable by clients.
    pub public_endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub force_path_style: bool,
    pub presign_ttl_secs: u64,
}

impl fmt::Debug for S3Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint.as_deref().map(redact_url))
            .field("public_endpoint", &self.public_endpoint.as_deref().map(redact_url))
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("force_path_style", &self.force_path_style)
            .field("presign_ttl_secs", &self.presign_ttl_secs)
            .finish()
    }
}

fn var(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing env var {name}"))
}

fn opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Address to bind, from `env_name`, falling back to `default`.
pub fn bind_addr(env_name: &str, default: &str) -> String {
    opt(env_name).unwrap_or_else(|| default.into())
}

impl DatabaseConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            url: var("DATABASE_URL")?,
        })
    }
}

impl S3Config {
    pub fn from_env() -> Result<Self> {
        let endpoint = opt("S3_ENDPOINT");
        Ok(Self {
            public_endpoint: opt("S3_PUBLIC_ENDPOINT").or_else(|| endpoint.clone()),
            endpoint,
            region: opt("S3_REGION").unwrap_or_else(|| "us-east-1".into()),
            bucket: var("S3_BUCKET")?,
            access_key: var("S3_ACCESS_KEY")?,
            secret_key: var("S3_SECRET_KEY")?,
            force_path_style: opt("S3_FORCE_PATH_STYLE").is_some_and(|v| v == "true" || v == "1"),
            presign_ttl_secs: opt("PRESIGN_TTL_SECS")
                .map(|v| v.parse())
                .transpose()
                .context("PRESIGN_TTL_SECS")?
                .unwrap_or(900),
        })
    }
}

/// Replace the userinfo password (and any query string) of a URL so it is safe to log.
pub fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return "<redacted>".into();
    };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let authority = match authority.rsplit_once('@') {
        Some((userinfo, host)) => match userinfo.split_once(':') {
            Some((user, _)) => format!("{user}:***@{host}"),
            None => format!("{userinfo}@{host}"),
        },
        None => authority.to_string(),
    };
    format!("{scheme}://{authority}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_database_password() {
        let c = DatabaseConfig {
            url: "postgres://cyanotype:hunter2@db:5432/cyanotype?sslmode=require".into(),
        };
        let d = format!("{c:?}");
        assert!(!d.contains("hunter2"), "{d}");
        assert!(d.contains("cyanotype:***@db:5432/cyanotype"), "{d}");
    }

    #[test]
    fn s3_debug_hides_keys() {
        let c = S3Config {
            endpoint: Some("http://s3:9000".into()),
            public_endpoint: None,
            region: "r".into(),
            bucket: "b".into(),
            access_key: "AKIASECRETKEY".into(),
            secret_key: "sekrit-value".into(),
            force_path_style: true,
            presign_ttl_secs: 1,
        };
        let d = format!("{c:?}");
        assert!(!d.contains("AKIASECRETKEY") && !d.contains("sekrit-value"), "{d}");
    }
}
