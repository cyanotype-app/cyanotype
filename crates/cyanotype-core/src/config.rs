use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    pub s3: S3Config,
    pub presign_ttl_secs: u64,
}

#[derive(Debug, Clone)]
pub struct S3Config {
    pub endpoint: Option<String>,
    /// Endpoint embedded in presigned URLs; must be reachable by clients.
    pub public_endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub force_path_style: bool,
}

fn var(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing env var {name}"))
}

fn opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let endpoint = opt("S3_ENDPOINT");
        Ok(Self {
            database_url: var("DATABASE_URL")?,
            bind_addr: opt("BIND_ADDR").unwrap_or_else(|| "0.0.0.0:3000".into()),
            presign_ttl_secs: opt("PRESIGN_TTL_SECS")
                .map(|v| v.parse())
                .transpose()
                .context("PRESIGN_TTL_SECS")?
                .unwrap_or(900),
            s3: S3Config {
                public_endpoint: opt("S3_PUBLIC_ENDPOINT").or_else(|| endpoint.clone()),
                endpoint,
                region: opt("S3_REGION").unwrap_or_else(|| "us-east-1".into()),
                bucket: var("S3_BUCKET")?,
                access_key: var("S3_ACCESS_KEY")?,
                secret_key: var("S3_SECRET_KEY")?,
                force_path_style: opt("S3_FORCE_PATH_STYLE").is_some_and(|v| v == "true" || v == "1"),
            },
        })
    }
}
