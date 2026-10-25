//! HTTP client for the Cyanotype API. The wire types are written by hand to mirror the services
//! (`apps/assets/src/routes.rs`, `apps/auth/src/routes.rs`).
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::TryStreamExt;
use reqwest::{Method, RequestBuilder, Response, StatusCode, header};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

const API_ATTEMPTS: u32 = 6;
const PUT_ATTEMPTS: u32 = 5;
/// Read buffer for streaming a part from disk: the whole per-request memory cost of an upload.
const STREAM_CHUNK: usize = 256 * 1024;

#[derive(Debug, Serialize)]
pub struct CheckRequest {
    pub sha256: String,
    pub size_bytes: i64,
    pub filename: String,
    pub content_type: String,
    /// RFC 3339 mtime hint; omitted when it looks bogus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_modified_at: Option<String>,
    /// Google Takeout metadata for this file (`import --takeout`), held by the server until the photo is processed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takeout: Option<TakeoutField>,
}

/// The `takeout` field of a check call: the sidecar's text and the album folder.
#[derive(Debug, Clone, Serialize)]
pub struct TakeoutField {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidecar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    pub favorites: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PartUrl {
    pub number: i32,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DonePart {
    pub number: i32,
    pub etag: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CheckResponse {
    Exists {
        #[allow(dead_code)]
        asset_id: String,
    },
    Upload {
        upload_id: String,
        part_size: i64,
        parts: Vec<PartUrl>,
        #[serde(default)]
        uploaded: Vec<DonePart>,
    },
}

#[derive(Debug, Serialize)]
struct CompleteRequest<'a> {
    upload_id: &'a str,
    parts: &'a [DonePart],
}

#[derive(Debug, Deserialize)]
pub struct Me {
    pub id: String,
    pub email: String,
    /// What this key may do (`import`, `read`, `full`); absent from servers older than key scopes.
    #[serde(default)]
    pub scope: Option<String>,
}

/// What a finished login through the browser returns (`POST /auth/cli/token`, and the device poll once approved).
#[derive(Debug, Clone, Deserialize)]
pub struct KeyGrant {
    pub key: String,
    pub email: String,
}

/// `POST /auth/cli/device`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    /// Where on the server the code is entered (`/activate`).
    pub verification_path: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// The answer to one poll of the device flow.
#[derive(Debug)]
pub enum PollOutcome {
    Key(KeyGrant),
    Pending,
    /// Poll less often: the new interval in seconds.
    SlowDown(u64),
    Denied,
    Expired,
}

/// What a correct password earned: a session, or (the account has two-factor authentication) a challenge to answer with a code.
pub enum LoginOutcome {
    Session(String),
    Challenge(String),
}

/// How a request failed, as far as the caller's control flow cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 404: e.g. `/assets/complete` for an upload that no longer exists.
    NotFound,
    /// A presigned URL was refused (expired, or signature problem). Ask `check` for fresh ones.
    Expired(String),
    /// The account's limits refused the upload (see [`LimitInfo`]). Retrying right away will not help.
    Limit(LimitInfo),
    /// Retrying will not help.
    Fatal(String),
}

/// A refusal by the account's limits, as the assets service words it (HTTP 413 or 429 with a `code`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitInfo {
    /// `quota_exceeded`, `daily_cap_exceeded`, `file_too_large`, `file_exceeds_daily_cap` or `upload_larger_than_declared`.
    pub code: String,
    /// The server's own words, which say what to do.
    pub message: String,
    /// RFC 3339: when uploads can resume (the daily cap only).
    pub retry_at: Option<String>,
}

impl LimitInfo {
    /// Reads a limit refusal out of a response body; None for any other error.
    pub fn from_body(text: &str) -> Option<LimitInfo> {
        let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
        let code = v.get("code")?.as_str()?;
        if !matches!(
            code,
            "quota_exceeded"
                | "daily_cap_exceeded"
                | "file_too_large"
                | "file_exceeds_daily_cap"
                | "upload_larger_than_declared"
        ) {
            return None;
        }
        Some(LimitInfo {
            code: code.to_string(),
            message: v.get("error")?.as_str()?.to_string(),
            retry_at: v.get("retry_at").and_then(|t| t.as_str()).map(str::to_string),
        })
    }

    /// Whether the whole import should stop (every further upload would be refused too) rather than skip this one file.
    pub fn stops_import(&self) -> bool {
        matches!(self.code.as_str(), "quota_exceeded" | "daily_cap_exceeded")
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::NotFound => write!(f, "not found"),
            ApiError::Expired(m) | ApiError::Fatal(m) => write!(f, "{m}"),
            ApiError::Limit(l) => write!(f, "{}", l.message),
        }
    }
}

impl std::error::Error for ApiError {}

pub type ApiResult<T> = Result<T, ApiError>;

/// Byte-progress callback: positive when bytes go out, negative when a failed attempt is rolled back.
pub type ByteCb = Arc<dyn Fn(i64) + Send + Sync>;

/// Backoff before retry number `attempt` (1-based): 0.5s, 1s, 2s, ... capped at 30s, with up to 25% jitter.
pub fn backoff(attempt: u32) -> Duration {
    let base = 500u64.saturating_mul(1 << (attempt - 1).min(6)).min(30_000);
    let jitter = rand_u64() % (base / 4 + 1);
    Duration::from_millis(base + jitter)
}

fn rand_u64() -> u64 {
    // Not security sensitive: spreads retries. Subsecond clock noise is plenty.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as u64)
        .wrapping_mul(6364136223846793005)
        >> 16
}

fn retry_after(res: &Response) -> Option<Duration> {
    res.headers()
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()
        .map(|s| Duration::from_secs(s.min(120)))
}

async fn snippet(res: Response) -> String {
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    snippet_of(status, &body)
}

fn snippet_of(status: StatusCode, body: &str) -> String {
    let body = body.trim();
    // The services answer {"error": "..."}; show just the message when it is that.
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| body.chars().take(200).collect());
    if msg.is_empty() {
        status.to_string()
    } else {
        format!("{status}: {msg}")
    }
}

pub fn is_transient(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::REQUEST_TIMEOUT
}

#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    /// No overall timeout: parts can be large and links slow. Per-request timeouts are set by size.
    put_http: reqwest::Client,
    base: String,
    key: Option<String>,
}

impl Api {
    pub fn new(server: &str, api_key: Option<&str>) -> anyhow::Result<Self> {
        let ua = concat!("cyanotype-cli/", env!("CARGO_PKG_VERSION"));
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(ua)
                .connect_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(60))
                .build()?,
            put_http: reqwest::Client::builder()
                .user_agent(ua)
                .connect_timeout(Duration::from_secs(20))
                .build()?,
            base: server.trim_end_matches('/').to_string(),
            key: api_key.map(str::to_string),
        })
    }

    fn req(&self, m: Method, path: &str) -> RequestBuilder {
        let r = self.http.request(m, format!("{}{path}", self.base));
        match &self.key {
            Some(k) => r.bearer_auth(k),
            None => r,
        }
    }

    /// Send with retry on network errors, 5xx and 429. Returns the first non-transient response.
    async fn send(&self, build: impl Fn() -> RequestBuilder) -> ApiResult<Response> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let last = attempt >= API_ATTEMPTS;
            match build().send().await {
                // A 429 from the daily upload cap is an answer, not congestion: waiting a few seconds will not change it.
                Ok(res) if res.status() == StatusCode::TOO_MANY_REQUESTS => {
                    let status = res.status();
                    let wait = retry_after(&res).unwrap_or_else(|| backoff(attempt));
                    let text = res.text().await.unwrap_or_default();
                    if let Some(limit) = LimitInfo::from_body(&text) {
                        return Err(ApiError::Limit(limit));
                    }
                    if last {
                        return Err(ApiError::Fatal(format!(
                            "server kept failing: {}",
                            snippet_of(status, &text)
                        )));
                    }
                    tokio::time::sleep(wait).await;
                }
                Ok(res) if is_transient(res.status()) && !last => {
                    let wait = retry_after(&res).unwrap_or_else(|| backoff(attempt));
                    tokio::time::sleep(wait).await;
                }
                Ok(res) if is_transient(res.status()) => {
                    return Err(ApiError::Fatal(format!("server kept failing: {}", snippet(res).await)));
                }
                Ok(res) => return Ok(res),
                Err(e) if !last && (e.is_connect() || e.is_timeout() || e.is_request() || e.is_body()) => {
                    tokio::time::sleep(backoff(attempt)).await;
                }
                Err(e) => return Err(ApiError::Fatal(format!("network error: {}", describe(e)))),
            }
        }
    }

    async fn json<T: for<'de> Deserialize<'de>>(&self, res: Response) -> ApiResult<T> {
        let status = res.status();
        if status == StatusCode::NOT_FOUND {
            return Err(ApiError::NotFound);
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Fatal(
                "unauthorized: the API key is missing, revoked or wrong (run `cyanotype login`)".into(),
            ));
        }
        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            if let Some(limit) = LimitInfo::from_body(&text) {
                return Err(ApiError::Limit(limit));
            }
            return Err(ApiError::Fatal(snippet_of(status, &text)));
        }
        res.json::<T>()
            .await
            .map_err(|e| ApiError::Fatal(format!("unexpected response: {e}")))
    }

    pub async fn me(&self) -> ApiResult<Me> {
        let res = self.send(|| self.req(Method::GET, "/auth/me")).await?;
        self.json(res).await
    }

    pub async fn asset_count(&self) -> ApiResult<i64> {
        #[derive(Deserialize)]
        struct Count {
            count: i64,
        }
        let res = self.send(|| self.req(Method::GET, "/assets/count")).await?;
        Ok(self.json::<Count>(res).await?.count)
    }

    pub async fn check(&self, body: &CheckRequest) -> ApiResult<CheckResponse> {
        let res = self.send(|| self.req(Method::POST, "/assets/check").json(body)).await?;
        self.json(res).await
    }

    /// `Err(NotFound)` means the upload is gone (already completed, or aborted): run `check` again.
    pub async fn complete(&self, upload_id: &str, parts: &[DonePart]) -> ApiResult<String> {
        #[derive(Deserialize)]
        struct Done {
            asset_id: String,
        }
        let body = CompleteRequest { upload_id, parts };
        let res = self
            .send(|| self.req(Method::POST, "/assets/complete").json(&body))
            .await?;
        Ok(self.json::<Done>(res).await?.asset_id)
    }

    /// PUT `len` bytes of `path` starting at `offset` to a presigned URL, streaming from disk. Returns the ETag.
    /// `progress` gets +n as bytes are handed to the network and -n when a failed attempt is taken back, so
    /// the running total never over-counts.
    pub async fn put_part(
        &self,
        url: &str,
        path: &Path,
        offset: u64,
        len: u64,
        progress: &ByteCb,
    ) -> ApiResult<String> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let sent = Arc::new(AtomicU64::new(0));
            let failure = match self.put_once(url, path, offset, len, &sent, progress.clone()).await {
                Ok(etag) => return Ok(etag),
                Err(f) => f,
            };
            progress(-(sent.load(Ordering::Relaxed) as i64));
            match failure {
                PutFailure::Final(e) => return Err(e),
                PutFailure::Transient(m) if attempt >= PUT_ATTEMPTS => {
                    return Err(ApiError::Fatal(format!("{m} (after {attempt} attempts)")));
                }
                PutFailure::Transient(_) => tokio::time::sleep(backoff(attempt)).await,
            }
        }
    }

    async fn put_once(
        &self,
        url: &str,
        path: &Path,
        offset: u64,
        len: u64,
        sent: &Arc<AtomicU64>,
        progress: ByteCb,
    ) -> Result<String, PutFailure> {
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|e| PutFailure::Final(ApiError::Fatal(format!("open: {e}"))))?;
        let on_disk = file
            .metadata()
            .await
            .map_err(|e| PutFailure::Final(ApiError::Fatal(format!("stat: {e}"))))?
            .len();
        if on_disk < offset + len {
            return Err(PutFailure::Final(ApiError::Fatal(
                "file changed while importing (it shrank)".into(),
            )));
        }
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| PutFailure::Final(ApiError::Fatal(format!("seek: {e}"))))?;
        let counter = sent.clone();
        let stream = ReaderStream::with_capacity(file.take(len), STREAM_CHUNK).inspect_ok(move |b| {
            counter.fetch_add(b.len() as u64, Ordering::Relaxed);
            progress(b.len() as i64);
        });
        // Time allowance: a minute plus enough for ~50 KiB/s, so a stalled link is cut off and retried.
        let timeout = Duration::from_secs(60 + len / (50 * 1024));
        let res = self
            .put_http
            .put(url)
            .header(header::CONTENT_LENGTH, len)
            .timeout(timeout)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .map_err(|e| PutFailure::Transient(format!("network error: {}", describe(e))))?;
        let status = res.status();
        if status.is_success() {
            return res
                .headers()
                .get(header::ETAG)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .ok_or_else(|| PutFailure::Final(ApiError::Fatal("storage returned no ETag".into())));
        }
        if is_transient(status) {
            return Err(PutFailure::Transient(format!("storage answered {status}")));
        }
        let msg = snippet(res).await;
        // 403: presigned URL expired (or was rejected). 404: the multipart upload no longer exists, e.g. it was
        // completed by another process meanwhile. Either way `check` knows what to do next.
        if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
            return Err(PutFailure::Final(ApiError::Expired(msg)));
        }
        Err(PutFailure::Final(ApiError::Fatal(format!(
            "storage refused the part: {msg}"
        ))))
    }

    // ---- login helpers (cookie session, used once to mint an API key)

    /// Log in with email + password; returns the session cookie (`name=value`).
    pub async fn login(&self, email: &str, password: &str) -> ApiResult<LoginOutcome> {
        let res = self
            .send(|| {
                self.req(Method::POST, "/auth/login")
                    .json(&serde_json::json!({"email": email, "password": password}))
            })
            .await?;
        let status = res.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Fatal("wrong email or password".into()));
        }
        if !status.is_success() {
            return Err(ApiError::Fatal(snippet(res).await));
        }
        let cookie = res
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|c| c.split(';').next().filter(|kv| kv.starts_with("cyanotype_session=")))
            .map(str::to_string);
        if let Some(c) = cookie {
            return Ok(LoginOutcome::Session(c));
        }
        #[derive(Deserialize)]
        struct Mfa {
            challenge: Option<String>,
        }
        match self.json::<Mfa>(res).await.ok().and_then(|m| m.challenge) {
            Some(ch) => Ok(LoginOutcome::Challenge(ch)),
            None => Err(ApiError::Fatal("server did not start a session".into())),
        }
    }

    /// The second step of a login with two-factor authentication: a 6-digit code or a recovery code. Returns the session cookie.
    pub async fn login_code(&self, challenge: &str, code: &str) -> ApiResult<String> {
        let res = self
            .send(|| {
                self.req(Method::POST, "/auth/login/2fa")
                    .json(&serde_json::json!({"challenge": challenge, "code": code}))
            })
            .await?;
        let status = res.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Fatal("that code was not accepted".into()));
        }
        if !status.is_success() {
            return Err(ApiError::Fatal(snippet(res).await));
        }
        res.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|c| c.split(';').next().filter(|kv| kv.starts_with("cyanotype_session=")))
            .map(str::to_string)
            .ok_or_else(|| ApiError::Fatal("server did not start a session".into()))
    }

    /// Mint an API key with a session cookie. Returns the plaintext key (shown once by the server).
    pub async fn create_api_key(&self, cookie: &str, name: &str, scope: &str) -> ApiResult<String> {
        #[derive(Deserialize)]
        struct Created {
            key: String,
        }
        let res = self
            .send(|| {
                self.req(Method::POST, "/auth/api-keys")
                    .header(header::COOKIE, cookie)
                    .json(&serde_json::json!({"name": name, "scope": scope}))
            })
            .await?;
        Ok(self.json::<Created>(res).await?.key)
    }

    // ---- login through the browser (src/login.rs)

    /// The loopback flow's last step: the single-use code and the PKCE verifier for an API key.
    pub async fn cli_token(&self, code: &str, verifier: &str, redirect_uri: &str) -> ApiResult<KeyGrant> {
        let res = self
            .send(|| {
                self.req(Method::POST, "/auth/cli/token")
                    .json(&serde_json::json!({"code": code, "code_verifier": verifier, "redirect_uri": redirect_uri}))
            })
            .await?;
        self.json(res).await
    }

    /// The device flow's first step: a user code to show and a device code to poll with.
    pub async fn device_start(&self, device: &str, scope: &str) -> ApiResult<DeviceStart> {
        let res = self
            .send(|| {
                self.req(Method::POST, "/auth/cli/device")
                    .json(&serde_json::json!({"device": device, "scope": scope}))
            })
            .await?;
        self.json(res).await
    }

    /// One poll of the device flow (RFC 8628, section 3.5).
    pub async fn device_poll(&self, device_code: &str) -> ApiResult<PollOutcome> {
        let res = self
            .send(|| {
                self.req(Method::POST, "/auth/cli/device/token")
                    .json(&serde_json::json!({"device_code": device_code}))
            })
            .await?;
        if res.status().is_success() {
            return Ok(PollOutcome::Key(self.json(res).await?));
        }
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        let body: serde_json::Value = serde_json::from_str(text.trim()).unwrap_or_default();
        match body.get("code").and_then(|c| c.as_str()) {
            Some("authorization_pending") => Ok(PollOutcome::Pending),
            Some("slow_down") => Ok(PollOutcome::SlowDown(
                body.get("interval").and_then(|i| i.as_u64()).unwrap_or(10),
            )),
            Some("access_denied") => Ok(PollOutcome::Denied),
            Some("expired_token") | Some("invalid_grant") => Ok(PollOutcome::Expired),
            _ => Err(ApiError::Fatal(snippet_of(status, &text))),
        }
    }

    /// `cyanotype logout`: revoke the key this client is configured with. False when the server no longer knows it.
    pub async fn cli_logout(&self) -> ApiResult<bool> {
        let res = self.send(|| self.req(Method::POST, "/auth/cli/logout")).await?;
        match res.status() {
            StatusCode::NO_CONTENT | StatusCode::OK => Ok(true),
            StatusCode::UNAUTHORIZED => Ok(false),
            status => Err(ApiError::Fatal(snippet_of(
                status,
                &res.text().await.unwrap_or_default(),
            ))),
        }
    }

    pub async fn logout(&self, cookie: &str) {
        let _ = self
            .req(Method::POST, "/auth/logout")
            .header(header::COOKIE, cookie)
            .send()
            .await;
    }
}

enum PutFailure {
    Transient(String),
    Final(ApiError),
}

/// reqwest errors nest the interesting part (DNS failure, TLS error, ...) in the source chain.
/// The URL is dropped: presigned URLs carry a signature and must not end up in logs or the failure list.
fn describe(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut s = e.to_string();
    let mut src = std::error::Error::source(&e);
    while let Some(c) = src {
        s.push_str(": ");
        s.push_str(&c.to_string());
        src = c.source();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_is_capped() {
        let ms = |a| backoff(a).as_millis() as u64;
        assert!((500..=625).contains(&ms(1)));
        assert!((1000..=1250).contains(&ms(2)));
        assert!((2000..=2500).contains(&ms(3)));
        assert!((30_000..=37_500).contains(&ms(20)));
    }

    #[test]
    fn transient_statuses() {
        assert!(is_transient(StatusCode::BAD_GATEWAY));
        assert!(is_transient(StatusCode::from_u16(522).unwrap()));
        assert!(is_transient(StatusCode::TOO_MANY_REQUESTS));
        assert!(!is_transient(StatusCode::FORBIDDEN));
        assert!(!is_transient(StatusCode::NOT_FOUND));
        assert!(!is_transient(StatusCode::OK));
    }

    #[test]
    fn check_response_parses_both_shapes_and_old_servers() {
        let r: CheckResponse = serde_json::from_str(r#"{"status":"exists","asset_id":"abc"}"#).unwrap();
        assert!(matches!(r, CheckResponse::Exists { .. }));
        let r: CheckResponse = serde_json::from_str(
            r#"{"status":"upload","upload_id":"u","part_size":16,"parts":[{"number":2,"url":"http://x"}],
                "uploaded":[{"number":1,"etag":"\"e\""}]}"#,
        )
        .unwrap();
        let CheckResponse::Upload { parts, uploaded, .. } = r else {
            panic!()
        };
        assert_eq!((parts[0].number, uploaded[0].number), (2, 1));
        // `uploaded` is optional so the client keeps working against a server without resume support.
        let r: CheckResponse =
            serde_json::from_str(r#"{"status":"upload","upload_id":"u","part_size":16,"parts":[]}"#).unwrap();
        assert!(matches!(r, CheckResponse::Upload { uploaded, .. } if uploaded.is_empty()));
    }
}
