//! Auth service settings read from the environment. Invalid values fail startup loudly:
//! a typo in a security setting must never silently fall back to something looser.
use std::str::FromStr;

use anyhow::{Result, bail};

use crate::totp::TotpKey;
use cyanotype_service::ratelimit::{Cidr, RateLimitConfig, parse_proxies};

/// Who may create accounts (`REGISTRATION_MODE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationMode {
    /// Anyone may register.
    Open,
    /// Registration works only while the users table is empty (the owner claims the instance).
    FirstUser,
    /// The first account registers freely (it becomes the admin); every later one needs a valid, unused invite token
    /// made by an admin. The default.
    Invite,
    /// `POST /auth/register` always returns 403.
    Closed,
}

impl FromStr for RegistrationMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "open" => Ok(Self::Open),
            "first_user" => Ok(Self::FirstUser),
            "invite" => Ok(Self::Invite),
            "closed" => Ok(Self::Closed),
            other => bail!("REGISTRATION_MODE must be open, first_user, invite or closed (got {other:?})"),
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
    /// The server key that seals TOTP secrets (`AUTH_TOTP_KEY`). Unset: two-factor enrolment is off.
    pub totp_key: Option<TotpKey>,
    /// Seconds since the epoch; a field so tests can fix the time of a code.
    pub clock: fn() -> u64,
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
            registration: get("REGISTRATION_MODE").map_or(Ok(RegistrationMode::Invite), |v| v.parse())?,
            cookie_secure: get("COOKIE_SECURE").map_or(Ok(true), |v| parse_bool("COOKIE_SECURE", &v))?,
            rate_limit,
            trusted_proxies: parse_proxies(get("TRUSTED_PROXIES").as_deref())?,
            totp_key: get("AUTH_TOTP_KEY").map(|v| TotpKey::parse(&v)).transpose()?,
            clock: crate::twofactor::unix_now,
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
        assert_eq!(c.registration, RegistrationMode::Invite);
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
        // Every value an existing deployment may already set keeps its meaning.
        for (v, m) in [
            ("first_user", RegistrationMode::FirstUser),
            ("invite", RegistrationMode::Invite),
            (" INVITE ", RegistrationMode::Invite),
            ("closed", RegistrationMode::Closed),
        ] {
            assert_eq!(cfg(&[("REGISTRATION_MODE", v)]).unwrap().registration, m);
        }
    }

    #[test]
    fn bad_values_fail_startup() {
        assert!(cfg(&[("REGISTRATION_MODE", "wide_open")]).is_err());
        assert!(cfg(&[("COOKIE_SECURE", "maybe")]).is_err());
        assert!(cfg(&[("AUTH_RATE_LIMIT_BURST", "0")]).is_err());
        assert!(cfg(&[("TRUSTED_PROXIES", "10.0.0.0/99")]).is_err());
        // A malformed 2FA key fails startup; it never silently turns 2FA off.
        assert!(cfg(&[("AUTH_TOTP_KEY", "not a key")]).is_err());
    }

    #[test]
    fn totp_key_is_optional_and_parsed() {
        assert!(cfg(&[]).unwrap().totp_key.is_none());
        assert!(cfg(&[("AUTH_TOTP_KEY", &"ab".repeat(32))]).unwrap().totp_key.is_some());
    }
}
