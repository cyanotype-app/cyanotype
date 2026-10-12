//! Auth service settings read from the environment. Invalid values fail startup loudly:
//! a typo in a security setting must never silently fall back to something looser.
use std::str::FromStr;

use anyhow::{Result, bail};

use crate::ratelimit::{Cidr, RateLimitConfig};

/// Who may create accounts (`REGISTRATION_MODE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationMode {
    /// Anyone may register.
    Open,
    /// Registration works only while the users table is empty (the owner claims the instance).
    FirstUser,
    /// `POST /auth/register` always returns 403.
    Closed,
}

impl FromStr for RegistrationMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "open" => Ok(Self::Open),
            "first_user" => Ok(Self::FirstUser),
            "closed" => Ok(Self::Closed),
            other => bail!("REGISTRATION_MODE must be open, first_user or closed (got {other:?})"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AuthConfig {
    pub registration: RegistrationMode,
    /// Set the `Secure` attribute on the session cookie (`COOKIE_SECURE`, default true).
    pub cookie_secure: bool,
    pub rate_limit: RateLimitConfig,
    /// Peers whose `X-Forwarded-For` is believed (`TRUSTED_PROXIES`).
    pub trusted_proxies: Vec<Cidr>,
}

impl AuthConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let num = |k: &str, default: u64| -> Result<u64> {
            match get(k) {
                Some(v) => v
                    .trim()
                    .parse()
                    .map_err(|_| anyhow::anyhow!("{k} must be a positive integer")),
                None => Ok(default),
            }
        };
        let defaults = RateLimitConfig::default();
        let rate_limit = RateLimitConfig {
            burst: num("AUTH_RATE_LIMIT_BURST", defaults.burst as u64)? as u32,
            per_minute: num("AUTH_RATE_LIMIT_PER_MINUTE", defaults.per_minute as u64)? as u32,
            max_keys: num("AUTH_RATE_LIMIT_MAX_KEYS", defaults.max_keys as u64)? as usize,
        };
        if rate_limit.burst == 0 || rate_limit.per_minute == 0 || rate_limit.max_keys == 0 {
            bail!("AUTH_RATE_LIMIT_* values must be greater than zero");
        }
        Ok(Self {
            registration: get("REGISTRATION_MODE").map_or(Ok(RegistrationMode::FirstUser), |v| v.parse())?,
            cookie_secure: get("COOKIE_SECURE").map_or(Ok(true), |v| parse_bool("COOKIE_SECURE", &v))?,
            rate_limit,
            trusted_proxies: parse_proxies(get("TRUSTED_PROXIES").as_deref())?,
        })
    }
}

fn parse_bool(name: &str, v: &str) -> Result<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => bail!("{name} must be true or false (got {other:?})"),
    }
}

/// Default: loopback and private ranges, which covers the compose network the gateway lives on.
/// `none` trusts no proxy (forwarded headers are ignored).
fn parse_proxies(v: Option<&str>) -> Result<Vec<Cidr>> {
    const DEFAULT: &str = "127.0.0.0/8,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,::1/128,fc00::/7";
    let v = v.unwrap_or(DEFAULT).trim();
    if v.eq_ignore_ascii_case("none") {
        return Ok(vec![]);
    }
    v.split(',').map(|s| s.trim().parse()).collect()
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self::from_lookup(|_| None).expect("defaults are valid")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> Result<AuthConfig> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        AuthConfig::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn defaults_are_the_safe_ones() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.registration, RegistrationMode::FirstUser);
        assert!(c.cookie_secure);
        assert!(!c.trusted_proxies.is_empty());
    }

    #[test]
    fn parses_overrides() {
        let c = cfg(&[
            ("REGISTRATION_MODE", "Closed"),
            ("COOKIE_SECURE", "false"),
            ("AUTH_RATE_LIMIT_BURST", "3"),
            ("TRUSTED_PROXIES", "none"),
        ])
        .unwrap();
        assert_eq!(c.registration, RegistrationMode::Closed);
        assert!(!c.cookie_secure);
        assert_eq!(c.rate_limit.burst, 3);
        assert!(c.trusted_proxies.is_empty());
        assert_eq!(
            cfg(&[("REGISTRATION_MODE", "open")]).unwrap().registration,
            RegistrationMode::Open
        );
    }

    #[test]
    fn bad_values_fail_startup() {
        assert!(cfg(&[("REGISTRATION_MODE", "wide_open")]).is_err());
        assert!(cfg(&[("COOKIE_SECURE", "maybe")]).is_err());
        assert!(cfg(&[("AUTH_RATE_LIMIT_BURST", "0")]).is_err());
        assert!(cfg(&[("TRUSTED_PROXIES", "10.0.0.0/99")]).is_err());
    }
}
