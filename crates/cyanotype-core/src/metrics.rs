//! DogStatsD metrics, hand-rolled (a UDP line protocol is ~100 lines; no extra dependency).
//!
//! Design rules:
//! - **Never block or fail the caller.** Calls format one line and `try_send` it into a bounded channel; a
//!   background thread batches lines into datagrams, resolves the Agent's address (again every minute, because
//!   the Agent container's IP changes when it is recreated) and sends them. A full channel, an unresolvable
//!   host or a failed send silently drops the metric.
//! - **No-op when unconfigured.** Without `DOGSTATSD_URL` / `DD_AGENT_HOST` nothing is sent and no thread starts.
//! - Names get the `cyanotype.` prefix; every line carries `env`, `service` and `version` (from `DD_ENV`,
//!   `DD_SERVICE`, `DD_VERSION`, the same values the logs carry) plus the per-call tags.
//!
//! Configuration (first match wins):
//! - `DOGSTATSD_URL` (or `DD_DOGSTATSD_URL`): `udp://host:port` (a bare `host:port` also works). `unix://` is not
//!   supported (it would need the socket mounted into every container).
//! - `DD_AGENT_HOST` (+ `DD_DOGSTATSD_PORT`, default 8125).
use std::{
    net::{ToSocketAddrs, UdpSocket},
    sync::{
        OnceLock,
        mpsc::{self, RecvTimeoutError, SyncSender},
    },
    time::{Duration, Instant},
};

use crate::{keys, pairing::RAW_EXTS, telemetry::Meta};

pub const PREFIX: &str = "cyanotype.";
/// Datagrams stay under a typical MTU (the Agent accepts up to 8 KiB by default; 1400 is safe everywhere).
const MAX_DATAGRAM: usize = 1400;
const QUEUE_LEN: usize = 8192;
const FLUSH_EVERY: Duration = Duration::from_millis(100);
const RESOLVE_EVERY: Duration = Duration::from_secs(60);
const RETRY_UNRESOLVED_EVERY: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct Metrics {
    sender: Option<SyncSender<String>>,
    global_tags: String,
}

impl Metrics {
    /// A sender that does nothing.
    pub fn disabled() -> Self {
        Self {
            sender: None,
            global_tags: String::new(),
        }
    }

    /// Configure from the environment; disabled (and silent) when no Agent address is configured.
    pub fn from_env(meta: &Meta) -> Self {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let url = get("DOGSTATSD_URL").or_else(|| get("DD_DOGSTATSD_URL"));
        let target = match target_from(
            url.as_deref(),
            get("DD_AGENT_HOST").as_deref(),
            get("DD_DOGSTATSD_PORT").as_deref(),
        ) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("metrics disabled: {e}");
                return Self::disabled();
            }
        };
        let Some(target) = target else { return Self::disabled() };
        tracing::info!(target = %target, "sending DogStatsD metrics");
        let mut global = vec![("service", meta.service.as_str()), ("version", meta.version.as_str())];
        if let Some(env) = &meta.env {
            global.push(("env", env));
        }
        Self::connect(&target, &global)
    }

    /// Send to `host:port` over UDP with `global_tags` on every line.
    pub fn connect(target: &str, global_tags: &[(&str, &str)]) -> Self {
        let (tx, rx) = mpsc::sync_channel::<String>(QUEUE_LEN);
        let target = target.to_string();
        let spawned = std::thread::Builder::new()
            .name("dogstatsd".into())
            .spawn(move || run_sender(&target, &rx));
        if spawned.is_err() {
            return Self::disabled();
        }
        Self {
            sender: Some(tx),
            global_tags: join_tags(global_tags),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.sender.is_some()
    }

    /// Add `value` to a counter.
    pub fn count(&self, name: &str, value: u64, tags: &[(&str, &str)]) {
        self.send(name, &value.to_string(), 'c', tags);
    }

    /// Set a gauge.
    pub fn gauge(&self, name: &str, value: f64, tags: &[(&str, &str)]) {
        if value.is_finite() {
            self.send(name, &value.to_string(), 'g', tags);
        }
    }

    /// Record one sample of a histogram: the Agent publishes `.avg`, `.median`, `.95percentile`, `.max` and
    /// `.count` series for it with no setup in Datadog.
    pub fn histogram(&self, name: &str, value: f64, tags: &[(&str, &str)]) {
        if value.is_finite() {
            self.send(name, &value.to_string(), 'h', tags);
        }
    }

    fn send(&self, name: &str, value: &str, kind: char, tags: &[(&str, &str)]) {
        if let Some(tx) = &self.sender {
            // Full queue or dead thread: drop the sample.
            let _ = tx.try_send(format_line(name, value, kind, &self.global_tags, tags));
        }
    }
}

/// `cyanotype.<name>:<value>|<type>|#tag:value,...`
pub fn format_line(name: &str, value: &str, kind: char, global_tags: &str, tags: &[(&str, &str)]) -> String {
    let mut line = format!("{PREFIX}{name}:{value}|{kind}");
    let per_call = join_tags(tags);
    if !global_tags.is_empty() || !per_call.is_empty() {
        line.push_str("|#");
        line.push_str(global_tags);
        if !global_tags.is_empty() && !per_call.is_empty() {
            line.push(',');
        }
        line.push_str(&per_call);
    }
    line
}

fn join_tags(tags: &[(&str, &str)]) -> String {
    tags.iter()
        .map(|(k, v)| format!("{}:{}", clean_tag(k), clean_tag(v)))
        .collect::<Vec<_>>()
        .join(",")
}

/// Characters that would break the line protocol or the tag list become `_`.
fn clean_tag(s: &str) -> String {
    s.chars()
        .map(|c| {
            if matches!(c, '|' | ',' | '#' | '@') || c.is_whitespace() || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// The Agent address from the configuration values, `Ok(None)` when none is set.
pub fn target_from(url: Option<&str>, host: Option<&str>, port: Option<&str>) -> Result<Option<String>, String> {
    if let Some(url) = url {
        let url = url.trim();
        if url.starts_with("unix://") || url.starts_with("unixgram://") {
            return Err("unix sockets are not supported; use udp://host:port".into());
        }
        let rest = url.strip_prefix("udp://").unwrap_or(url);
        let rest = rest.trim_end_matches('/');
        if rest.contains("://") || rest.is_empty() {
            return Err(format!("unsupported DOGSTATSD_URL {url:?}; use udp://host:port"));
        }
        return Ok(Some(if rest.contains(':') {
            rest.to_string()
        } else {
            format!("{rest}:8125")
        }));
    }
    Ok(host.map(|h| format!("{}:{}", h.trim(), port.map_or("8125", str::trim))))
}

fn resolve(target: &str) -> Option<std::net::SocketAddr> {
    target.to_socket_addrs().ok()?.next()
}

/// The sender thread: batch lines into datagrams and ship them.
fn run_sender(target: &str, rx: &mpsc::Receiver<String>) {
    let socket = UdpSocket::bind("0.0.0.0:0").or_else(|_| UdpSocket::bind("[::]:0"));
    let Ok(socket) = socket else { return };
    let mut addr = None;
    let mut next_resolve = Instant::now();
    let mut warned = false;
    let mut buf = String::new();
    let mut flush_at = Instant::now() + FLUSH_EVERY;
    loop {
        let wait = flush_at.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(line) => {
                if !buf.is_empty() && buf.len() + 1 + line.len() > MAX_DATAGRAM {
                    flush(&socket, addr, &mut buf);
                }
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&line);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                flush(&socket, addr, &mut buf);
                return;
            }
        }
        if Instant::now() >= next_resolve {
            addr = resolve(target);
            next_resolve = Instant::now()
                + if addr.is_some() {
                    RESOLVE_EVERY
                } else {
                    RETRY_UNRESOLVED_EVERY
                };
            if addr.is_none() && !warned {
                warned = true;
                tracing::warn!(
                    target,
                    "DogStatsD host does not resolve (is the Datadog Agent running?); metrics are dropped until it does"
                );
            }
        }
        if Instant::now() >= flush_at {
            flush(&socket, addr, &mut buf);
            flush_at = Instant::now() + FLUSH_EVERY;
        }
    }
}

fn flush(socket: &UdpSocket, addr: Option<std::net::SocketAddr>, buf: &mut String) {
    if let (Some(addr), false) = (addr, buf.is_empty()) {
        let _ = socket.send_to(buf.as_bytes(), addr);
    }
    buf.clear();
}

static GLOBAL: OnceLock<Metrics> = OnceLock::new();

/// Install the process-wide sender from the environment. Call once, first thing in `main` after `telemetry::init`.
pub fn init(service_name: &str) -> &'static Metrics {
    GLOBAL.get_or_init(|| Metrics::from_env(&Meta::from_env(service_name)))
}

static DISABLED: OnceLock<Metrics> = OnceLock::new();

/// The process-wide sender; a no-op one when `init` was not called (tests, the CLI).
pub fn global() -> &'static Metrics {
    GLOBAL.get().unwrap_or_else(|| DISABLED.get_or_init(Metrics::disabled))
}

/// Coarse file family for tags: `jpeg`, `heic`, `raw`, `png` or `other`. Uses the content type, then the
/// extension (uploaders often send `application/octet-stream` for RAW files).
pub fn family(content_type: &str, filename: &str) -> &'static str {
    let ct = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let ext = keys::sanitize_ext(filename);
    let ext = ext.as_str();
    if RAW_EXTS.contains(&ext)
        || ct.contains("-raw")
        || ct.contains("x-canon")
        || ct.contains("x-nikon")
        || ct.contains("x-adobe-dng")
        || ct.contains("x-sony")
        || ct.contains("x-fuji")
    {
        return "raw";
    }
    match ct.as_str() {
        "image/jpeg" | "image/jpg" => return "jpeg",
        "image/png" => return "png",
        "image/heic" | "image/heif" | "image/heic-sequence" | "image/heif-sequence" => return "heic",
        _ => {}
    }
    match ext {
        "jpg" | "jpeg" | "jpe" => "jpeg",
        "png" => "png",
        "heic" | "heif" | "hif" => "heic",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;

    #[test]
    fn lines_have_prefix_type_and_tags() {
        assert_eq!(
            format_line(
                "jobs.completed",
                "1",
                'c',
                "service:w,env:dev",
                &[("kind", "thumbnail"), ("family", "raw")]
            ),
            "cyanotype.jobs.completed:1|c|#service:w,env:dev,kind:thumbnail,family:raw"
        );
        assert_eq!(format_line("x", "2.5", 'g', "", &[]), "cyanotype.x:2.5|g");
        assert_eq!(format_line("x", "1", 'c', "", &[("a", "b")]), "cyanotype.x:1|c|#a:b");
        assert_eq!(format_line("x", "1", 'c', "s:w", &[]), "cyanotype.x:1|c|#s:w");
    }

    #[test]
    fn tag_values_cannot_break_the_protocol() {
        assert_eq!(join_tags(&[("k", "a b,c|d#e\nf")]), "k:a_b_c_d_e_f");
    }

    #[test]
    fn targets_from_configuration() {
        assert_eq!(target_from(None, None, None), Ok(None));
        assert_eq!(
            target_from(Some("udp://datadog-agent:8125"), None, None),
            Ok(Some("datadog-agent:8125".into()))
        );
        assert_eq!(
            target_from(Some("datadog-agent"), None, None),
            Ok(Some("datadog-agent:8125".into()))
        );
        assert_eq!(
            target_from(None, Some("10.0.0.5"), None),
            Ok(Some("10.0.0.5:8125".into()))
        );
        assert_eq!(
            target_from(None, Some("agent"), Some("9125")),
            Ok(Some("agent:9125".into()))
        );
        // The URL wins over the host.
        assert_eq!(target_from(Some("udp://a:1"), Some("b"), None), Ok(Some("a:1".into())));
        assert!(target_from(Some("unix:///var/run/datadog/dsd.socket"), None, None).is_err());
        assert!(target_from(Some("http://agent:8125"), None, None).is_err());
    }

    #[test]
    fn disabled_is_a_silent_no_op() {
        let m = Metrics::disabled();
        assert!(!m.is_enabled());
        m.count("a", 1, &[("k", "v")]);
        m.gauge("a", 1.0, &[]);
        m.histogram("a", 1.0, &[]);
        // Nothing configured in the environment of the test process: the global one is disabled too.
        assert!(!global().is_enabled());
    }

    #[test]
    fn content_type_families() {
        assert_eq!(family("image/jpeg", "a.jpg"), "jpeg");
        assert_eq!(family("image/jpeg; charset=x", "noext"), "jpeg");
        assert_eq!(family("image/heic", "a.heic"), "heic");
        assert_eq!(family("application/octet-stream", "IMG.HEIC"), "heic");
        assert_eq!(family("application/octet-stream", "DSC_1.NEF"), "raw");
        assert_eq!(family("image/x-canon-cr2", "x"), "raw");
        assert_eq!(family("image/x-adobe-dng", "x.dng"), "raw");
        assert_eq!(family("image/png", "a.png"), "png");
        assert_eq!(family("", "a.jpeg"), "jpeg");
        assert_eq!(family("video/mp4", "a.mp4"), "other");
        assert_eq!(family("image/tiff", "a.tif"), "other");
        // The extension beats a generic image type for RAW files.
        assert_eq!(family("image/tiff", "a.dng"), "raw");
    }

    /// A fake Agent: a local UDP socket receives exactly the lines the sender emits.
    #[test]
    fn sends_dogstatsd_lines_to_a_udp_listener() {
        let agent = UdpSocket::bind("127.0.0.1:0").unwrap();
        agent.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let m = Metrics::connect(
            &agent.local_addr().unwrap().to_string(),
            &[("service", "t"), ("env", "test")],
        );
        assert!(m.is_enabled());
        m.count("jobs.completed", 1, &[("kind", "metadata"), ("outcome", "done")]);
        m.gauge("jobs.queued", 12.0, &[("kind", "thumbnail")]);
        m.histogram("jobs.duration", 153.5, &[("kind", "thumbnail"), ("family", "raw")]);
        let mut got = String::new();
        let mut b = [0u8; 2048];
        while got.lines().count() < 3 {
            let n = agent.recv(&mut b).expect("datagram within 5 s");
            got.push_str(std::str::from_utf8(&b[..n]).unwrap());
            got.push('\n');
        }
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(
            lines[0],
            "cyanotype.jobs.completed:1|c|#service:t,env:test,kind:metadata,outcome:done"
        );
        assert_eq!(
            lines[1],
            "cyanotype.jobs.queued:12|g|#service:t,env:test,kind:thumbnail"
        );
        assert_eq!(
            lines[2],
            "cyanotype.jobs.duration:153.5|h|#service:t,env:test,kind:thumbnail,family:raw"
        );
    }

    #[test]
    fn many_lines_are_split_into_datagrams_within_the_limit() {
        let agent = UdpSocket::bind("127.0.0.1:0").unwrap();
        agent.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let m = Metrics::connect(&agent.local_addr().unwrap().to_string(), &[]);
        for i in 0..200 {
            m.count("c", i, &[("kind", "metadata")]);
        }
        let (mut lines, mut b) = (0, [0u8; 4096]);
        while lines < 200 {
            let n = agent.recv(&mut b).expect("datagram");
            assert!(n <= MAX_DATAGRAM);
            lines += b[..n].split(|c| *c == b'\n').count();
        }
        assert_eq!(lines, 200);
    }

    #[test]
    fn an_unresolvable_host_never_blocks_or_fails_the_caller() {
        let m = Metrics::connect("nonexistent-agent.invalid:8125", &[]);
        let start = Instant::now();
        for i in 0..20_000 {
            m.count("c", i, &[]);
        }
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "enqueueing must not wait for DNS or the network"
        );
    }
}
