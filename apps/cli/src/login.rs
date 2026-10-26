//! `cyanotype login` through the browser, so no password is typed into a terminal (docs/import.md, "Log in").
//!
//! Two flows, both ending in an API key the server mints after the person pressed Allow in a signed-in browser:
//!  * loopback (the default, like `gh auth login`): listen on `127.0.0.1:<random port>`, open `<server>/cli/authorize` with
//!    a PKCE challenge, receive a single-use code on `/callback` and trade it plus the verifier for the key;
//!  * device code (RFC 8628), for an SSH session or a machine without a display: print a short code to enter on
//!    `<server>/activate` from any device, and poll until it is approved.
use std::{
    io::IsTerminal,
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::api::{Api, DeviceStart, KeyGrant, PollOutcome};

/// How long the loopback listener waits for the person to press Allow.
const LOOPBACK_WAIT: Duration = Duration::from_secs(300);
/// How long one connection to the listener may take to send its request line.
const REQUEST_WAIT: Duration = Duration::from_secs(5);
const MAX_REQUEST: usize = 8 * 1024;

/// Which flow to use: `--device`, or automatically when there is no browser to be had on this machine (an SSH session, or
/// Linux without a display server). `env` looks a variable up; a variable that is set but empty counts as unset.
pub fn wants_device_flow(forced: bool, linux: bool, env: impl Fn(&str) -> Option<String>) -> bool {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    if forced || ["SSH_CONNECTION", "SSH_TTY", "SSH_CLIENT"].iter().any(|k| set(k)) {
        return true;
    }
    linux && !set("DISPLAY") && !set("WAYLAND_DISPLAY")
}

/// This machine's name, shown to the person approving and used in the key's name. The server cuts it down further.
pub fn device_name() -> String {
    let from_env = ["HOSTNAME", "COMPUTERNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok())
        .filter(|h| !h.trim().is_empty());
    let from_file = || {
        ["/etc/hostname", "/proc/sys/kernel/hostname"]
            .iter()
            .find_map(|p| std::fs::read_to_string(p).ok())
            .filter(|h| !h.trim().is_empty())
    };
    let from_command = || {
        let out = std::process::Command::new("hostname")
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
            .filter(|h| !h.trim().is_empty())
    };
    from_env
        .or_else(from_file)
        .or_else(from_command)
        .map(|h| h.trim().to_string())
        .unwrap_or_else(|| "unknown device".into())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    SystemRandom::new()
        .fill(&mut b)
        .expect("the operating system's random source failed");
    b
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A PKCE pair (RFC 7636, method S256).
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Self {
        Self::from_verifier(b64url(&random_bytes::<32>()))
    }

    pub fn from_verifier(verifier: String) -> Self {
        let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
        Self { verifier, challenge }
    }
}

/// The page the browser opens: `<server>/cli/authorize?redirect_uri=…&state=…&code_challenge=…&device=…&scope=…`.
pub fn authorize_url(
    server: &str,
    port: u16,
    state: &str,
    challenge: &str,
    device: &str,
    scope: &str,
) -> Result<String> {
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let url = reqwest::Url::parse_with_params(
        &format!("{server}/cli/authorize"),
        [
            ("redirect_uri", redirect.as_str()),
            ("state", state),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("device", device),
            ("scope", scope),
        ],
    )
    .with_context(|| format!("building the login address from {server}"))?;
    Ok(url.into())
}

/// Asks the system to open `url` in the default browser; false when that did not work (the caller prints the address).
pub fn open_browser(url: &str) -> bool {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        // `cmd /c start` would hand the `&` in the address to the shell; this opens it without one.
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler").arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// What arrived on the loopback address.
#[derive(Debug, PartialEq, Eq)]
pub enum Callback {
    Code(String),
    Denied,
}

/// What a request line on the listener means: `Some(result)` for `/callback` with the right state, `None` for anything else
/// (a favicon fetch, a probe, a wrong state), which is answered and ignored.
fn parse_callback(request_line: &str, state: &str) -> Option<Callback> {
    let target = request_line.strip_prefix("GET ")?.split(' ').next()?;
    let (path, query) = target.split_once('?')?;
    if path != "/callback" {
        return None;
    }
    let url = reqwest::Url::parse(&format!("http://127.0.0.1/?{query}")).ok()?;
    let get = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    // The state ties the callback to this login attempt: anything else on the machine that finds the port cannot answer it.
    if get("state").as_deref() != Some(state) {
        return None;
    }
    if get("error").is_some() {
        return Some(Callback::Denied);
    }
    get("code").filter(|c| !c.is_empty()).map(Callback::Code)
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Cyanotype CLI</title>\
<body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem\">\
<h1>You can close this tab</h1><p>The cyanotype command line is signed in. Go back to your terminal.</p></body>";
const DENIED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Cyanotype CLI</title>\
<body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem\">\
<h1>Login cancelled</h1><p>Nothing was allowed. You can close this tab.</p></body>";

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let msg = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Reads one request's first line (bounded in size and time).
async fn request_line(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let read = async {
        while !buf.contains(&b'\n') && buf.len() < MAX_REQUEST {
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Some(())
    };
    tokio::time::timeout(REQUEST_WAIT, read).await.ok()??;
    let line = String::from_utf8_lossy(&buf);
    Some(line.lines().next()?.trim_end().to_string())
}

/// Serves the listener until the callback with the right state arrives (or `wait` runs out).
pub async fn wait_for_callback(listener: &TcpListener, state: &str, wait: Duration) -> Result<Callback> {
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let (mut stream, _) = match tokio::time::timeout(left, listener.accept()).await {
            Err(_) => bail!("timed out waiting for the browser; run `cyanotype login` again"),
            Ok(r) => r.context("accepting the browser's connection")?,
        };
        let Some(line) = request_line(&mut stream).await else {
            continue;
        };
        match parse_callback(&line, state) {
            Some(cb) => {
                let page = if cb == Callback::Denied { DENIED_PAGE } else { DONE_PAGE };
                respond(&mut stream, "200 OK", page).await;
                return Ok(cb);
            }
            None => respond(&mut stream, "404 Not Found", "not found").await,
        }
    }
}

/// The default flow. `open` shows the address to the browser (false: it could not be opened, and the person is told to
/// open the printed address by hand); a parameter so tests can play the browser.
pub async fn login_loopback(
    api: &Api,
    server: &str,
    scope: &str,
    device: &str,
    open: impl FnOnce(&str) -> bool + Send + 'static,
    wait: Duration,
) -> Result<KeyGrant> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("starting the local listener for the browser")?;
    let port = listener.local_addr()?.port();
    let pkce = Pkce::new();
    let state = b64url(&random_bytes::<16>());
    let url = authorize_url(server, port, &state, &pkce.challenge, device, scope)?;
    eprintln!(
        "Opening your browser to sign in. If it does not open, go to this address in one, signed in to your account:"
    );
    eprintln!("\n  {url}\n");
    eprintln!("Waiting for you to allow the login in the browser… (Ctrl-C to cancel)");
    // Off the async threads and never waited for: some openers stay running until the browser exits, and the page the
    // browser loads needs this task free to answer it.
    let shown = url.clone();
    tokio::spawn(async move {
        if !tokio::task::spawn_blocking(move || open(&shown)).await.unwrap_or(false) {
            eprintln!("(Could not start a browser here: open the address above by hand.)");
        }
    });
    match wait_for_callback(&listener, &state, wait).await? {
        Callback::Denied => bail!("the login was denied in the browser"),
        Callback::Code(code) => {
            let redirect = format!("http://127.0.0.1:{port}/callback");
            api.cli_token(&code, &pkce.verifier, &redirect)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        }
    }
}

fn times(unit: Duration, n: u64) -> Duration {
    unit.saturating_mul(u32::try_from(n).unwrap_or(u32::MAX))
}

/// The device flow: print the code, poll until it is approved, denied or expires. `unit` is how long one "second" of the
/// server's interval lasts (a second; tests make it a millisecond).
pub async fn login_device(
    api: &Api,
    server: &str,
    scope: &str,
    device: &str,
    unit: Duration,
    qr: bool,
) -> Result<KeyGrant> {
    let start: DeviceStart = api
        .device_start(device, scope)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    eprintln!(
        "To sign in, open {server}{} on any device and enter this code:",
        start.verification_path
    );
    eprintln!("\n  {}\n", start.user_code);
    if qr {
        // Or scan this with a phone: the address carries the code in its fragment, which a browser never sends anywhere;
        // the page fills the code in and still asks the person to confirm.
        if let Some(art) = crate::qr::render(&activation_url(server, &start.verification_path, &start.user_code)) {
            eprintln!("Or scan this with your phone:\n\n{art}");
        }
    }
    eprintln!(
        "Check that the page names \"{}\". The code works for {} minutes. Waiting…",
        device_label(device),
        (start.expires_in / 60).max(1)
    );
    let mut interval = start.interval.max(1);
    let give_up = Instant::now() + times(unit, start.expires_in.saturating_add(30));
    loop {
        tokio::time::sleep(times(unit, interval)).await;
        match api
            .device_poll(&start.device_code)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
        {
            PollOutcome::Key(grant) => return Ok(grant),
            PollOutcome::Pending => {}
            PollOutcome::SlowDown(next) => interval = next.max(interval + 5),
            PollOutcome::Denied => bail!("the login was denied in the browser"),
            PollOutcome::Expired => bail!("the code expired before it was approved; run `cyanotype login` again"),
        }
        if Instant::now() > give_up {
            bail!("gave up waiting for approval; run `cyanotype login` again");
        }
    }
}

/// The activation page with the user code pre-filled, for the QR code: `<server>/activate#code=WDJB-MJHT`. The code is in the
/// fragment so that it never reaches a server log, a proxy or a referrer.
pub fn activation_url(server: &str, path: &str, user_code: &str) -> String {
    format!("{}{path}#code={user_code}", server.trim_end_matches('/'))
}

/// The name as the server will show it (letters, digits and a few separators), for the "check that the page names" line.
pub fn device_label(device: &str) -> String {
    let kept: String = device
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .take(48)
        .collect();
    let kept = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    if kept.is_empty() { "unknown device".into() } else { kept }
}

/// Picks the flow and runs it. `device_flag`: `--device`.
pub async fn login(api: &Api, server: &str, scope: &str, device_flag: bool, want_qr: bool) -> Result<KeyGrant> {
    let device = device_name();
    let linux = cfg!(target_os = "linux");
    if wants_device_flow(device_flag, linux, |k| std::env::var(k).ok()) {
        let qr = want_qr && std::io::stdout().is_terminal() && std::io::stderr().is_terminal();
        login_device(api, server, scope, &device, Duration::from_secs(1), qr).await
    } else {
        if !std::io::stdout().is_terminal() {
            eprintln!("(not a terminal: if nobody can press Allow here, use `cyanotype login --device`)");
        }
        login_loopback(api, server, scope, &device, open_browser, LOOPBACK_WAIT).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_device_flow_is_chosen_for_ssh_and_for_a_linux_box_without_a_display() {
        let none: &[(&str, &str)] = &[];
        assert!(wants_device_flow(true, false, env(none)));
        // A desktop: Linux with a display, or any other system.
        assert!(!wants_device_flow(false, true, env(&[("DISPLAY", ":0")])));
        assert!(!wants_device_flow(
            false,
            true,
            env(&[("WAYLAND_DISPLAY", "wayland-0")])
        ));
        assert!(!wants_device_flow(false, false, env(none)));
        // No display on Linux, and SSH on any system.
        assert!(wants_device_flow(false, true, env(none)));
        assert!(wants_device_flow(false, true, env(&[("DISPLAY", "")])));
        for v in ["SSH_CONNECTION", "SSH_TTY", "SSH_CLIENT"] {
            assert!(wants_device_flow(false, false, env(&[(v, "x")])));
            assert!(wants_device_flow(false, true, env(&[(v, "x"), ("DISPLAY", ":0")])));
        }
        assert!(!wants_device_flow(false, false, env(&[("SSH_TTY", "")])));
    }

    #[test]
    fn pkce_matches_the_rfc_example_and_fresh_pairs_are_valid() {
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let a = Pkce::new();
        let b = Pkce::new();
        assert_eq!((a.verifier.len(), a.challenge.len()), (43, 43));
        assert_ne!(a.verifier, b.verifier);
        assert!(
            a.verifier
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
    }

    #[test]
    fn the_authorize_address_carries_everything_encoded() {
        let u = authorize_url(
            "https://photos.example.com",
            53682,
            "st",
            "chal",
            "test laptop&x",
            "import",
        )
        .unwrap();
        assert!(u.starts_with("https://photos.example.com/cli/authorize?redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2Fcallback&state=st&code_challenge=chal&code_challenge_method=S256&device=test+laptop%26x&scope=import"), "{u}");
        assert!(authorize_url("not a url", 1, "s", "c", "d", "import").is_err());
    }

    #[test]
    fn only_the_callback_with_this_logins_state_counts() {
        let line = |q: &str| format!("GET /callback?{q} HTTP/1.1");
        assert_eq!(
            parse_callback(&line("code=abc&state=s1"), "s1"),
            Some(Callback::Code("abc".into()))
        );
        assert_eq!(
            parse_callback(&line("state=s1&code=abc"), "s1"),
            Some(Callback::Code("abc".into()))
        );
        assert_eq!(
            parse_callback(&line("error=access_denied&state=s1"), "s1"),
            Some(Callback::Denied)
        );
        assert_eq!(parse_callback(&line("code=abc&state=other"), "s1"), None);
        assert_eq!(parse_callback(&line("code=abc"), "s1"), None);
        assert_eq!(parse_callback(&line("code=&state=s1"), "s1"), None);
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1", "s1"), None);
        assert_eq!(parse_callback("GET /other?code=abc&state=s1 HTTP/1.1", "s1"), None);
        assert_eq!(parse_callback("POST /callback?code=abc&state=s1 HTTP/1.1", "s1"), None);
        assert_eq!(parse_callback("", "s1"), None);
    }

    #[test]
    fn the_activation_address_carries_the_code_in_the_fragment() {
        assert_eq!(
            activation_url("https://photos.example.com/", "/activate", "WDJB-MJHT"),
            "https://photos.example.com/activate#code=WDJB-MJHT"
        );
    }

    #[test]
    fn labels_match_what_the_server_shows() {
        assert_eq!(device_label("test-laptop"), "test-laptop");
        assert_eq!(device_label("  a   b<c> "), "a bc");
        assert_eq!(device_label("<>"), "unknown device");
    }
}

#[cfg(test)]
#[path = "login_flows_tests.rs"]
mod flows;
