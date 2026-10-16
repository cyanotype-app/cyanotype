//! Per-client-IP token-bucket rate limiting for the credential endpoints, in memory.
//!
//! Memory is bounded: at most `max_keys` buckets exist. When the table is full, buckets that
//! have fully refilled (indistinguishable from "never seen") are dropped first, then the least
//! recently seen bucket. IPv6 clients are keyed by their /64.
//!
//! Trust model for the client address: the socket peer is used as-is unless it falls inside
//! `TRUSTED_PROXIES` (the Caddy gateway), in which case the right-most `X-Forwarded-For` entry
//! that is not itself a trusted proxy is the client. Caddy overwrites any client-supplied
//! `X-Forwarded-For`, so a client cannot choose its own address. See docs/deploy.md.
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Clone, Copy, Debug)]
pub struct RateLimitConfig {
    /// Bucket size: requests allowed in a burst.
    pub burst: u32,
    /// Sustained refill rate.
    pub per_minute: u32,
    /// Upper bound on tracked clients.
    pub max_keys: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            burst: 10,
            per_minute: 10,
            max_keys: 10_000,
        }
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

pub struct RateLimiter {
    cfg: RateLimitConfig,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimiter {
    pub fn new(cfg: RateLimitConfig) -> Self {
        Self {
            cfg,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    fn rate_per_sec(&self) -> f64 {
        self.cfg.per_minute as f64 / 60.0
    }

    /// Take one token for `ip`. `Err(wait)` is how long until a token is available.
    pub fn check(&self, ip: IpAddr, now: Instant) -> Result<(), Duration> {
        let key = bucket_key(ip);
        let rate = self.rate_per_sec();
        let burst = self.cfg.burst as f64;
        let mut map = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if !map.contains_key(&key) && map.len() >= self.cfg.max_keys {
            // Full buckets carry no information; drop them first.
            map.retain(|_, b| b.tokens + now.saturating_duration_since(b.last).as_secs_f64() * rate < burst);
            if map.len() >= self.cfg.max_keys
                && let Some(oldest) = map.iter().min_by_key(|(_, b)| b.last).map(|(k, _)| *k)
            {
                map.remove(&oldest);
            }
        }
        let b = map.entry(key).or_insert(Bucket {
            tokens: burst,
            last: now,
        });
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * rate).min(burst);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - b.tokens) / rate))
        }
    }

    #[cfg(test)]
    pub fn tracked(&self) -> usize {
        self.buckets.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// IPv4 as-is; IPv6 truncated to its /64 so one host cannot mint endless addresses.
fn bucket_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let mut o = v6.octets();
            o[8..].fill(0);
            IpAddr::V6(Ipv6Addr::from(o))
        }
        v4 => v4,
    }
}

/// An address range, `a.b.c.d/n`, `::1/128`, or a bare address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    addr: u128,
    mask: u128,
    v6: bool,
}

impl Cidr {
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        let (n, v6) = to_bits(ip);
        v6 == self.v6 && (n & self.mask) == self.addr
    }
}

fn to_bits(ip: IpAddr) -> (u128, bool) {
    match ip {
        IpAddr::V4(v) => (u32::from(v) as u128, false),
        IpAddr::V6(v) => (u128::from(v), true),
    }
}

impl FromStr for Cidr {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        let (ip, len) = match s.split_once('/') {
            Some((ip, len)) => (ip, Some(len)),
            None => (s, None),
        };
        let ip: IpAddr = ip
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid proxy address {s:?}"))?;
        let (bits, v6) = to_bits(ip);
        let width: u32 = if v6 { 128 } else { 32 };
        let len: u32 = match len {
            Some(l) => l
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid prefix length in {s:?}"))?,
            None => width,
        };
        if len > width {
            anyhow::bail!("prefix length too large in {s:?}");
        }
        let host_bits = width - len;
        let mask = match len {
            0 => 0,
            128 => u128::MAX,
            _ => ((1u128 << len) - 1) << host_bits,
        };
        Ok(Self {
            addr: bits & mask,
            mask,
            v6,
        })
    }
}

/// The address to rate-limit: see the module docs for the trust model.
pub fn client_ip(peer: IpAddr, forwarded_for: Option<&str>, trusted: &[Cidr]) -> IpAddr {
    let is_trusted = |ip: IpAddr| trusted.iter().any(|c| c.contains(ip));
    if !is_trusted(peer) {
        return peer;
    }
    let Some(xff) = forwarded_for else { return peer };
    for part in xff.rsplit(',') {
        match part.trim().parse::<IpAddr>() {
            Ok(ip) if is_trusted(ip) => continue,
            Ok(ip) => return ip,
            // Garbage from a trusted proxy: do not guess, fall back to the proxy itself.
            Err(_) => return peer,
        }
    }
    peer
}

/// Parse `TRUSTED_PROXIES`. Default: loopback and private ranges, which covers the compose network the gateway
/// lives on. `none` trusts no proxy (forwarded headers are ignored).
pub fn parse_proxies(v: Option<&str>) -> anyhow::Result<Vec<Cidr>> {
    const DEFAULT: &str = "127.0.0.0/8,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,::1/128,fc00::/7";
    let v = v.unwrap_or(DEFAULT).trim();
    if v.eq_ignore_ascii_case("none") {
        return Ok(vec![]);
    }
    v.split(',').map(|s| s.trim().parse()).collect()
}

/// The client address of a request (see [`client_ip`]); `0.0.0.0` when the peer is unknown (tests).
pub fn request_ip(req: &axum::http::request::Parts, trusted: &[Cidr]) -> IpAddr {
    let peer = req
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
        .unwrap_or(IpAddr::from([0, 0, 0, 0]));
    let xff = req.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
    client_ip(peer, xff, trusted)
}

/// `Retry-After` seconds for a wait.
pub fn retry_after_secs(wait: Duration) -> u64 {
    wait.as_secs().max(1) + u64::from(wait.subsec_nanos() > 0 && wait.as_secs() >= 1)
}

#[derive(Clone)]
pub struct LimitState {
    pub limiter: Arc<RateLimiter>,
    pub trusted: Arc<Vec<Cidr>>,
}

/// axum middleware: 429 with `Retry-After` when the caller's bucket is empty.
pub async fn limit(State(s): State<LimitState>, req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
        .unwrap_or(IpAddr::from([0, 0, 0, 0]));
    let xff = req.headers().get("x-forwarded-for").and_then(|v| v.to_str().ok());
    let ip = client_ip(peer, xff, &s.trusted);
    match s.limiter.check(ip, Instant::now()) {
        Ok(()) => next.run(req).await,
        Err(wait) => {
            let secs = retry_after_secs(wait);
            let mut res = (
                StatusCode::TOO_MANY_REQUESTS,
                axum::Json(json!({"error": "too many requests, slow down"})),
            )
                .into_response();
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                res.headers_mut().insert(header::RETRY_AFTER, v);
            }
            res
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }
    fn cfg(burst: u32, per_minute: u32, max_keys: usize) -> RateLimitConfig {
        RateLimitConfig {
            burst,
            per_minute,
            max_keys,
        }
    }

    #[test]
    fn allows_burst_then_blocks_then_refills() {
        let l = RateLimiter::new(cfg(3, 60, 100)); // 1 token/sec
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(l.check(ip("1.2.3.4"), t0).is_ok());
        }
        let wait = l.check(ip("1.2.3.4"), t0).unwrap_err();
        assert!(wait <= Duration::from_secs(1) && wait > Duration::ZERO);
        // Another client is unaffected.
        assert!(l.check(ip("1.2.3.5"), t0).is_ok());
        // One second later exactly one more request fits.
        let t1 = t0 + Duration::from_millis(1100);
        assert!(l.check(ip("1.2.3.4"), t1).is_ok());
        assert!(l.check(ip("1.2.3.4"), t1).is_err());
    }

    #[test]
    fn memory_is_bounded() {
        let l = RateLimiter::new(cfg(1, 1, 50));
        let t0 = Instant::now();
        for i in 0..1000u32 {
            let _ = l.check(
                IpAddr::from([10, (i >> 8) as u8, i as u8, 1]),
                t0 + Duration::from_millis(i as u64),
            );
            assert!(l.tracked() <= 50);
        }
        assert_eq!(l.tracked(), 50);
    }

    #[test]
    fn ipv6_is_limited_per_slash_64() {
        let l = RateLimiter::new(cfg(1, 1, 100));
        let t0 = Instant::now();
        assert!(l.check(ip("2001:db8:1:2::1"), t0).is_ok());
        assert!(l.check(ip("2001:db8:1:2:ffff::9"), t0).is_err());
        assert!(l.check(ip("2001:db8:1:3::1"), t0).is_ok());
    }

    #[test]
    fn cidr_matching() {
        let c: Cidr = "172.16.0.0/12".parse().unwrap();
        assert!(c.contains(ip("172.18.0.5")));
        assert!(!c.contains(ip("172.32.0.1")));
        assert!(!c.contains(ip("::1")));
        let all: Cidr = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains(ip("8.8.8.8")));
        let one: Cidr = "::1".parse().unwrap();
        assert!(one.contains(ip("::1")) && !one.contains(ip("::2")));
        let v6: Cidr = "fc00::/7".parse().unwrap();
        assert!(v6.contains(ip("fd12::1")) && !v6.contains(ip("2001:db8::1")));
        assert!("10.0.0.0/33".parse::<Cidr>().is_err());
        assert!("nope".parse::<Cidr>().is_err());
    }

    #[test]
    fn forwarded_for_only_honoured_from_trusted_peers() {
        let trusted: Vec<Cidr> = vec!["172.16.0.0/12".parse().unwrap()];
        // Untrusted peer: header is ignored, so a client cannot spoof its address.
        assert_eq!(
            client_ip(ip("203.0.113.9"), Some("1.1.1.1"), &trusted),
            ip("203.0.113.9")
        );
        // Trusted gateway: the forwarded client is used.
        assert_eq!(
            client_ip(ip("172.18.0.2"), Some("198.51.100.7"), &trusted),
            ip("198.51.100.7")
        );
        // A client-prepended entry is ignored: the right-most non-proxy entry wins.
        assert_eq!(
            client_ip(ip("172.18.0.2"), Some("6.6.6.6, 198.51.100.7"), &trusted),
            ip("198.51.100.7")
        );
        // Missing or garbage header from the gateway: fall back to the peer.
        assert_eq!(client_ip(ip("172.18.0.2"), None, &trusted), ip("172.18.0.2"));
        assert_eq!(client_ip(ip("172.18.0.2"), Some("garbage"), &trusted), ip("172.18.0.2"));
        // No trusted proxies configured: never believe the header.
        assert_eq!(client_ip(ip("172.18.0.2"), Some("198.51.100.7"), &[]), ip("172.18.0.2"));
    }
}
