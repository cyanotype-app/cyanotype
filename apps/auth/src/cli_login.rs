//! Signing the command line in through the browser (docs/import.md, "Log in"), so a password is never typed into a
//! terminal. Two flows end in the same place, an ordinary API key named `cyanotype CLI on <device>`:
//!
//!  * Loopback (the default): the CLI listens on `127.0.0.1:<port>`, opens `/cli/authorize` in the browser and the person
//!    presses Allow; the page posts the request here (`/auth/cli/authorize/approve`) and navigates to the CLI's loopback
//!    address with a single-use code, which the CLI trades for the key at `/auth/cli/token` together with its PKCE verifier
//!    (S256), so a code seen by anything else on the machine is useless.
//!  * Device code (RFC 8628, for SSH sessions and machines without a browser): the CLI asks for a short user code
//!    (`/auth/cli/device`), the person types it on `/activate` from any device, and the CLI polls `/auth/cli/device/token`.
//!
//! Codes are random, stored only as their sha256 (like session tokens and API keys), single use and short lived. Approving
//! needs a login session (never an API key) and a CSRF-safe request (a JSON body, which a cross-site form cannot send; the
//! session cookie is SameSite=Lax); every approval and denial lands in the audit log.
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    routing::post,
};
use base64::Engine;
use rand::Rng;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use cyanotype_service::{ApiError, ApiResult, Scope, SessionUser, audit, auth::sha256};

use crate::routes::{
    API_KEY_HINT_LEN, API_KEY_PREFIX, AppState, MAX_API_KEYS_PER_USER, new_api_key, random_token, rfc3339,
};

/// How long a loopback authorization code lives.
const AUTH_CODE_SECS: i32 = 60;
/// How long a device code (and its user code) lives.
const DEVICE_CODE_SECS: i32 = 600;
/// The poll interval a device code starts with; `slow_down` adds [`SLOW_DOWN_STEP`] (RFC 8628, section 3.5).
const DEVICE_INTERVAL_SECS: i32 = 5;
const SLOW_DOWN_STEP: i32 = 5;
const MAX_INTERVAL_SECS: i32 = 60;
/// Consonants only (no vowels, so no words; no easily confused letters): 20^8 codes, typed as `WDJB-MJHT`.
const USER_CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";
const USER_CODE_LEN: usize = 8;
const MAX_DEVICE_NAME: usize = 48;
/// Most device requests that may be open (not yet expired, whatever their state) on the whole server. Starting one needs no
/// account, so this bounds what anonymous callers can pile up; the per-IP rate limiter (10 a minute by default, shared with
/// login) bounds one caller, and a request lives 10 minutes, so one address can hold at most ~100 of these. A household
/// server has a handful open at once; 500 leaves room for a busy one and keeps the table small.
const MAX_OPEN_DEVICE_CODES: i64 = 500;
/// Most logins one account may have approved but not yet collected (device codes waiting for their poll, plus loopback
/// codes not yet traded): each ends in an API key, so a stolen session cannot queue up a pile of keys, and a stuck script
/// cannot approve in a loop. A person approves one at a time; 5 is slack for retries.
const MAX_UNCOLLECTED_PER_USER: i64 = 5;
/// How long the server asks a client to wait after refusing for a full queue.
const BUSY_RETRY_SECS: u64 = 60;
/// Lowest port a loopback redirect may use: below it a different (privileged, possibly shared) service answers.
const MIN_PORT: u16 = 1024;

/// The routes behind the per-IP rate limiter (with login): the ones anybody can call without signing in, and the one that
/// lets a code be guessed.
pub fn limited_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/cli/token", post(token))
        .route("/auth/cli/device", post(device_start))
        .route("/auth/cli/device/lookup", post(device_lookup))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/cli/authorize/check", post(authorize_check))
        .route("/auth/cli/authorize/approve", post(authorize_approve))
        .route("/auth/cli/authorize/deny", post(authorize_deny))
        .route("/auth/cli/device/approve", post(device_approve))
        .route("/auth/cli/device/deny", post(device_deny))
        .route("/auth/cli/device/token", post(device_token))
        .route("/auth/cli/logout", post(logout))
}

// ---- validation

/// The port of a redirect URI that is exactly `http://127.0.0.1:<port>/callback` or `http://[::1]:<port>/callback`, else
/// None. Deliberately narrow: no `localhost` (a name can be pointed elsewhere), no other loopback address, no https, no
/// userinfo, query or fragment, so there is no open redirect and nothing to parse differently from a browser.
pub fn loopback_port(uri: &str) -> Option<u16> {
    let rest = uri
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| uri.strip_prefix("http://[::1]:"))?;
    let (port, path) = rest.split_once('/')?;
    if path != "callback" || port.is_empty() || port.len() > 5 || port.starts_with('0') {
        return None;
    }
    if !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    port.parse::<u16>().ok().filter(|p| *p >= MIN_PORT)
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

/// `state`: 1 to 128 URL-safe characters (so it can be put in the redirect without encoding).
fn valid_state(s: &str) -> bool {
    (1..=128).contains(&s.len()) && s.bytes().all(is_unreserved)
}

/// An S256 challenge: the unpadded base64url of a 32-byte hash, 43 characters.
fn valid_challenge(s: &str) -> bool {
    s.len() == 43 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// A PKCE verifier (RFC 7636): 43 to 128 unreserved characters.
fn valid_verifier(s: &str) -> bool {
    (43..=128).contains(&s.len()) && s.bytes().all(is_unreserved)
}

fn challenge_of(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha256(verifier.as_bytes()))
}

/// The device name as it is shown and used in the key's name: the client chooses it, so it is cut down to plain ASCII
/// (letters, digits, dot, dash, underscore, space), at most 48 characters; nothing usable becomes "unknown device".
pub fn sanitize_device(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .collect();
    let words: Vec<&str> = kept.split_whitespace().collect();
    let name: String = words.join(" ").chars().take(MAX_DEVICE_NAME).collect();
    let name = name.trim().to_string();
    if name.is_empty() { "unknown device".into() } else { name }
}

fn key_name(device: &str) -> String {
    format!("cyanotype CLI on {device}")
}

/// `WDJB-MJHT` with its hyphen and case dropped; None unless it is exactly eight letters of the alphabet.
fn normalize_user_code(raw: &str) -> Option<String> {
    let code: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (code.len() == USER_CODE_LEN && code.bytes().all(|b| USER_CODE_ALPHABET.contains(&b))).then_some(code)
}

fn new_user_code() -> String {
    let mut rng = rand::thread_rng();
    let letters: String = (0..USER_CODE_LEN)
        .map(|_| USER_CODE_ALPHABET[rng.gen_range(0..USER_CODE_ALPHABET.len())] as char)
        .collect();
    format!("{}-{}", &letters[..4], &letters[4..])
}

fn grant_error(code: &'static str, message: &str) -> ApiError {
    ApiError::Limited {
        status: StatusCode::BAD_REQUEST,
        code,
        message: message.into(),
        details: json!({}),
        retry_after_secs: None,
    }
}

fn invalid_grant() -> ApiError {
    grant_error(
        "invalid_grant",
        "That login code is invalid, expired or already used. Run `cyanotype login` again.",
    )
}

fn unknown_code() -> ApiError {
    ApiError::Limited {
        status: StatusCode::NOT_FOUND,
        code: "unknown_code",
        message: "That code was not recognised, or it has expired. Run `cyanotype login` again to get a new one."
            .into(),
        details: json!({}),
        retry_after_secs: None,
    }
}

/// 429 `too_many_login_requests`, with `Retry-After`; the CLI prints the message as it is.
fn too_many(message: &str) -> ApiError {
    ApiError::Limited {
        status: StatusCode::TOO_MANY_REQUESTS,
        code: "too_many_login_requests",
        message: message.into(),
        details: json!({}),
        retry_after_secs: Some(BUSY_RETRY_SECS),
    }
}

/// Drops what has outlived its use: loopback codes at their expiry, device requests five minutes after (so a late poll still
/// hears "expired" rather than "invalid"). Every route that adds a row calls it, so there is no separate job.
async fn purge_expired(db: &mut PgConnection) -> ApiResult<()> {
    sqlx::query("DELETE FROM cli_auth_codes WHERE expires_at < now()")
        .execute(&mut *db)
        .await?;
    sqlx::query("DELETE FROM cli_device_codes WHERE expires_at < now() - interval '5 minutes'")
        .execute(&mut *db)
        .await?;
    Ok(())
}

/// Refuse when the account already has [`MAX_UNCOLLECTED_PER_USER`] approved logins nobody has collected yet.
async fn ensure_approval_room(db: &mut PgConnection, user: Uuid) -> ApiResult<()> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM cli_auth_codes WHERE user_id = $1 AND expires_at > now())
              + (SELECT count(*) FROM cli_device_codes WHERE user_id = $1 AND status = 'approved' AND expires_at > now())",
    )
    .bind(user)
    .fetch_one(db)
    .await?;
    if n >= MAX_UNCOLLECTED_PER_USER {
        return Err(too_many(
            "Several logins for this account are already waiting to be collected. Wait a few minutes (they expire), then try again.",
        ));
    }
    Ok(())
}

fn account_disabled() -> ApiError {
    ApiError::Limited {
        status: StatusCode::FORBIDDEN,
        code: "account_disabled",
        message: "This account has been disabled. Ask the administrator.".into(),
        details: json!({}),
        retry_after_secs: None,
    }
}

/// `import` (the default) or `read`. A `full` key (it can delete photos) is never handed out by an approval page, since a
/// link or code someone sends you could ask for one; it is made on the API keys page.
fn scope_of(raw: Option<&str>) -> ApiResult<Scope> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(Scope::Import),
        Some(s) => match Scope::parse(s) {
            Some(scope @ (Scope::Import | Scope::Read)) => Ok(scope),
            _ => Err(ApiError::BadRequest(
                "scope must be import or read (make a full key on the API keys page)".into(),
            )),
        },
    }
}

/// Refuse early (while the person is looking at the page) when the account has no room for another key.
async fn ensure_key_room(db: &mut PgConnection, user: Uuid) -> ApiResult<()> {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM api_keys WHERE user_id = $1")
        .bind(user)
        .fetch_one(db)
        .await?;
    if n >= MAX_API_KEYS_PER_USER {
        return Err(ApiError::Conflict(format!(
            "this account already has {MAX_API_KEYS_PER_USER} API keys; revoke one in Settings first"
        )));
    }
    Ok(())
}

// ---- loopback flow

#[derive(Deserialize)]
struct AuthorizeReq {
    redirect_uri: String,
    state: String,
    #[serde(default)]
    code_challenge: String,
    #[serde(default)]
    device: String,
    #[serde(default)]
    scope: Option<String>,
}

struct Checked {
    device: String,
    scope: Scope,
}

fn check_authorize(req: &AuthorizeReq, need_challenge: bool) -> ApiResult<Checked> {
    if loopback_port(&req.redirect_uri).is_none() {
        return Err(ApiError::BadRequest(
            "redirect_uri must be http://127.0.0.1:<port>/callback (or http://[::1]:<port>/callback)".into(),
        ));
    }
    if !valid_state(&req.state) {
        return Err(ApiError::BadRequest("state must be 1-128 URL-safe characters".into()));
    }
    if need_challenge && !valid_challenge(&req.code_challenge) {
        return Err(ApiError::BadRequest(
            "code_challenge must be the base64url SHA-256 of the verifier (43 characters)".into(),
        ));
    }
    Ok(Checked {
        device: sanitize_device(&req.device),
        scope: scope_of(req.scope.as_deref())?,
    })
}

/// What the Allow page shows, from the server's own reading of the request: the device name as it will be stored, and the
/// scope. A request the server would refuse is refused here too, before the person is asked about it.
async fn authorize_check(
    user: SessionUser,
    State(st): State<AppState>,
    Json(req): Json<AuthorizeReq>,
) -> ApiResult<Json<Value>> {
    let c = check_authorize(&req, true)?;
    let mut conn = st.db.acquire().await?;
    ensure_key_room(&mut conn, user.id).await?;
    ensure_approval_room(&mut conn, user.id).await?;
    Ok(Json(json!({"device": c.device, "scope": c.scope.as_str()})))
}

async fn authorize_approve(
    user: SessionUser,
    State(st): State<AppState>,
    Json(req): Json<AuthorizeReq>,
) -> ApiResult<Json<Value>> {
    let c = check_authorize(&req, true)?;
    let mut tx = st.db.begin().await?;
    ensure_key_room(&mut tx, user.id).await?;
    purge_expired(&mut tx).await?;
    ensure_approval_room(&mut tx, user.id).await?;
    let code = random_token();
    sqlx::query(
        "INSERT INTO cli_auth_codes (code_hash, user_id, code_challenge, redirect_uri, device_name, scope, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, now() + make_interval(secs => $7))",
    )
    .bind(sha256(code.as_bytes()))
    .bind(user.id)
    .bind(&req.code_challenge)
    .bind(&req.redirect_uri)
    .bind(&c.device)
    .bind(c.scope.as_str())
    .bind(AUTH_CODE_SECS as f64)
    .execute(&mut *tx)
    .await?;
    audit::record(
        &mut *tx,
        Some(user.id),
        "cli_login.approve",
        Some("user"),
        Some(&user.id.to_string()),
        json!({"device": c.device, "scope": c.scope.as_str(), "flow": "browser"}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"redirect": format!("{}?code={code}&state={}", req.redirect_uri, req.state)}),
    ))
}

async fn authorize_deny(
    user: SessionUser,
    State(st): State<AppState>,
    Json(req): Json<AuthorizeReq>,
) -> ApiResult<Json<Value>> {
    let c = check_authorize(&req, false)?;
    audit::record(
        &st.db,
        Some(user.id),
        "cli_login.deny",
        Some("user"),
        Some(&user.id.to_string()),
        json!({"device": c.device, "scope": c.scope.as_str(), "flow": "browser"}),
    )
    .await?;
    Ok(Json(
        json!({"redirect": format!("{}?error=access_denied&state={}", req.redirect_uri, req.state)}),
    ))
}

#[derive(Deserialize)]
struct TokenReq {
    code: String,
    code_verifier: String,
    redirect_uri: String,
}

/// `POST /auth/cli/token`: the code and verifier for the key. The code is deleted as it is read, so one wrong guess (or a
/// replay) spends it; every failure looks the same.
async fn token(State(st): State<AppState>, Json(req): Json<TokenReq>) -> ApiResult<Json<Value>> {
    if !valid_verifier(&req.code_verifier) || req.code.len() > 128 {
        return Err(invalid_grant());
    }
    let mut tx = st.db.begin().await?;
    let row: Option<(Uuid, String, String, String, String, bool)> = sqlx::query_as(
        "DELETE FROM cli_auth_codes WHERE code_hash = $1
         RETURNING user_id, code_challenge, redirect_uri, device_name, scope, expires_at > now()",
    )
    .bind(sha256(req.code.as_bytes()))
    .fetch_optional(&mut *tx)
    .await?;
    let Some((user_id, challenge, redirect_uri, device, scope, live)) = row else {
        return Err(invalid_grant());
    };
    if !live || redirect_uri != req.redirect_uri || challenge_of(&req.code_verifier) != challenge {
        tx.commit().await?;
        return Err(invalid_grant());
    }
    let scope = Scope::parse(&scope).unwrap_or(Scope::Import);
    let issued = issue_key(&mut tx, user_id, &device, scope, "browser").await?;
    tx.commit().await?;
    Ok(Json(issued))
}

/// Makes the key a finished login earned, in the caller's transaction. Refuses a disabled account (it may have been
/// disabled between pressing Allow and the exchange).
async fn issue_key(tx: &mut PgConnection, user_id: Uuid, device: &str, scope: Scope, flow: &str) -> ApiResult<Value> {
    let who: Option<(String,)> = sqlx::query_as("SELECT email FROM users WHERE id = $1 AND disabled_at IS NULL")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some((email,)) = who else {
        return Err(account_disabled());
    };
    ensure_key_room(&mut *tx, user_id).await?;
    let key = new_api_key();
    let hint = format!("{API_KEY_PREFIX}{}", &key[API_KEY_PREFIX.len()..][..API_KEY_HINT_LEN]);
    let name = key_name(device);
    let (id, created_at): (Uuid, time::OffsetDateTime) = sqlx::query_as(
        "INSERT INTO api_keys (user_id, name, key_hash, prefix, scope) VALUES ($1, $2, $3, $4, $5) RETURNING id, created_at",
    )
    .bind(user_id)
    .bind(&name)
    .bind(sha256(key.as_bytes()))
    .bind(&hint)
    .bind(scope.as_str())
    .fetch_one(&mut *tx)
    .await?;
    audit::record(
        &mut *tx,
        Some(user_id),
        "cli_login.issue",
        Some("api_key"),
        Some(&id.to_string()),
        json!({"device": device, "scope": scope.as_str(), "flow": flow}),
    )
    .await?;
    Ok(json!({
        "key": key, "id": id, "name": name, "prefix": hint, "scope": scope.as_str(),
        "email": email, "created_at": rfc3339(created_at),
    }))
}

// ---- device flow

#[derive(Deserialize)]
struct DeviceStartReq {
    #[serde(default)]
    device: String,
    #[serde(default)]
    scope: Option<String>,
}

/// `POST /auth/cli/device`: a device code (the secret the CLI polls with) and the user code to type on `/activate`.
async fn device_start(State(st): State<AppState>, Json(req): Json<DeviceStartReq>) -> ApiResult<Json<Value>> {
    let device = sanitize_device(&req.device);
    let scope = scope_of(req.scope.as_deref())?;
    let mut conn = st.db.acquire().await?;
    purge_expired(&mut conn).await?;
    let (open,): (i64,) = sqlx::query_as("SELECT count(*) FROM cli_device_codes WHERE expires_at > now()")
        .fetch_one(&mut *conn)
        .await?;
    if open >= MAX_OPEN_DEVICE_CODES {
        return Err(too_many(
            "The server has too many command-line logins open right now. Wait a few minutes and run `cyanotype login` again.",
        ));
    }
    drop(conn);
    let device_code = random_token();
    // A user code that collides with a live one (1 in 2.5e10 per pair) is simply drawn again.
    for _ in 0..5 {
        let user_code = new_user_code();
        let normalized = normalize_user_code(&user_code).unwrap_or_default();
        let done = sqlx::query(
            "INSERT INTO cli_device_codes (device_code_hash, user_code_hash, device_name, scope, expires_at, interval_secs)
             VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5), $6) ON CONFLICT DO NOTHING",
        )
        .bind(sha256(device_code.as_bytes()))
        .bind(sha256(normalized.as_bytes()))
        .bind(&device)
        .bind(scope.as_str())
        .bind(DEVICE_CODE_SECS as f64)
        .bind(DEVICE_INTERVAL_SECS)
        .execute(&st.db)
        .await?;
        if done.rows_affected() == 1 {
            return Ok(Json(json!({
                "device_code": device_code, "user_code": user_code, "verification_path": "/activate",
                "expires_in": DEVICE_CODE_SECS, "interval": DEVICE_INTERVAL_SECS,
            })));
        }
    }
    Err(anyhow::anyhow!("could not draw an unused user code").into())
}

#[derive(Deserialize)]
struct UserCodeReq {
    user_code: String,
}

/// A pending, unexpired request for a user code: its row id, device, scope and seconds left.
async fn pending_request(db: &mut PgConnection, user_code: &str) -> ApiResult<(Uuid, String, String, i64)> {
    let normalized = normalize_user_code(user_code).ok_or_else(unknown_code)?;
    sqlx::query_as(
        "SELECT id, device_name, scope, extract(epoch FROM expires_at - now())::bigint FROM cli_device_codes
         WHERE user_code_hash = $1 AND status = 'pending' AND expires_at > now()",
    )
    .bind(sha256(normalized.as_bytes()))
    .fetch_optional(db)
    .await?
    .ok_or_else(unknown_code)
}

/// `POST /auth/cli/device/lookup`: what the activation page shows for the code the person typed (the code travels in the
/// body, never in a URL). Behind the rate limiter, since it answers whether a code exists.
async fn device_lookup(
    user: SessionUser,
    State(st): State<AppState>,
    Json(req): Json<UserCodeReq>,
) -> ApiResult<Json<Value>> {
    let mut conn = st.db.acquire().await?;
    let (_, device, scope, expires_in) = pending_request(&mut conn, &req.user_code).await?;
    ensure_key_room(&mut conn, user.id).await?;
    ensure_approval_room(&mut conn, user.id).await?;
    Ok(Json(
        json!({"device": device, "scope": scope, "expires_in": expires_in}),
    ))
}

async fn device_decide(user: SessionUser, st: AppState, req: UserCodeReq, approve: bool) -> ApiResult<Json<Value>> {
    let mut tx = st.db.begin().await?;
    let (id, device, scope, _) = pending_request(&mut tx, &req.user_code).await?;
    if approve {
        ensure_key_room(&mut tx, user.id).await?;
        ensure_approval_room(&mut tx, user.id).await?;
    }
    let done = sqlx::query(
        "UPDATE cli_device_codes SET status = $2, user_id = $3 WHERE id = $1 AND status = 'pending' AND expires_at > now()",
    )
    .bind(id)
    .bind(if approve { "approved" } else { "denied" })
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    if done.rows_affected() != 1 {
        return Err(unknown_code());
    }
    audit::record(
        &mut *tx,
        Some(user.id),
        if approve { "cli_login.approve" } else { "cli_login.deny" },
        Some("user"),
        Some(&user.id.to_string()),
        json!({"device": device, "scope": scope, "flow": "device"}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"approved": approve, "device": device})))
}

async fn device_approve(
    user: SessionUser,
    State(st): State<AppState>,
    Json(req): Json<UserCodeReq>,
) -> ApiResult<Json<Value>> {
    device_decide(user, st, req, true).await
}

async fn device_deny(
    user: SessionUser,
    State(st): State<AppState>,
    Json(req): Json<UserCodeReq>,
) -> ApiResult<Json<Value>> {
    device_decide(user, st, req, false).await
}

#[derive(Deserialize)]
struct DevicePollReq {
    device_code: String,
}

/// `POST /auth/cli/device/token`: the CLI's poll (RFC 8628, section 3.5). 400 with `code` `authorization_pending`,
/// `slow_down` (carrying the new `interval`), `access_denied` or `expired_token` until the person decides; then the key,
/// once.
async fn device_token(State(st): State<AppState>, Json(req): Json<DevicePollReq>) -> ApiResult<Json<Value>> {
    if req.device_code.len() > 128 {
        return Err(invalid_grant());
    }
    let mut tx = st.db.begin().await?;
    type Row = (Uuid, String, String, String, Option<Uuid>, bool, i32, bool);
    let row: Option<Row> = sqlx::query_as(
        "SELECT id, device_name, scope, status, user_id, expires_at > now(), interval_secs,
                coalesce(now() - last_polled_at < make_interval(secs => interval_secs - 1), false)
         FROM cli_device_codes WHERE device_code_hash = $1 FOR UPDATE",
    )
    .bind(sha256(req.device_code.as_bytes()))
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, device, scope, status, user_id, live, interval, too_fast)) = row else {
        return Err(invalid_grant());
    };
    if !live {
        sqlx::query("DELETE FROM cli_device_codes WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(grant_error(
            "expired_token",
            "The login code expired. Run `cyanotype login` again.",
        ));
    }
    if too_fast {
        let next = (interval + SLOW_DOWN_STEP).min(MAX_INTERVAL_SECS);
        sqlx::query("UPDATE cli_device_codes SET last_polled_at = now(), interval_secs = $2 WHERE id = $1")
            .bind(id)
            .bind(next)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(ApiError::Limited {
            status: StatusCode::BAD_REQUEST,
            code: "slow_down",
            message: "Polling too fast: wait longer between requests.".into(),
            details: json!({"interval": next}),
            retry_after_secs: None,
        });
    }
    match (status.as_str(), user_id) {
        ("approved", Some(user_id)) => {
            sqlx::query("DELETE FROM cli_device_codes WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            let scope = Scope::parse(&scope).unwrap_or(Scope::Import);
            let issued = issue_key(&mut tx, user_id, &device, scope, "device").await;
            // A refused issue (disabled account, key limit) still spends the code.
            tx.commit().await?;
            Ok(Json(issued?))
        }
        ("denied", _) => {
            sqlx::query("DELETE FROM cli_device_codes WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Err(grant_error("access_denied", "The login was denied in the browser."))
        }
        _ => {
            sqlx::query("UPDATE cli_device_codes SET last_polled_at = now() WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Err(grant_error(
                "authorization_pending",
                "Waiting for the login to be approved in the browser.",
            ))
        }
    }
}

// ---- logout

/// `POST /auth/cli/logout` with `Authorization: Bearer <key>`: revokes the key it is called with (any scope may; this is
/// `cyanotype logout`). 401 when the key is already gone.
async fn logout(State(db): State<PgPool>, headers: HeaderMap) -> ApiResult<StatusCode> {
    let key = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;
    let gone = sqlx::query("DELETE FROM api_keys WHERE key_hash = $1")
        .bind(sha256(key.as_bytes()))
        .execute(&db)
        .await?
        .rows_affected();
    if gone == 0 {
        return Err(ApiError::Unauthorized);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_loopback_callbacks_are_redirect_targets() {
        assert_eq!(loopback_port("http://127.0.0.1:53682/callback"), Some(53682));
        assert_eq!(loopback_port("http://[::1]:8123/callback"), Some(8123));
        for bad in [
            "http://localhost:53682/callback",
            "http://127.0.0.2:53682/callback",
            "http://0.0.0.0:53682/callback",
            "https://127.0.0.1:53682/callback",
            "http://127.0.0.1@evil.example:53682/callback",
            "http://127.0.0.1:53682@evil.example/callback",
            "http://evil.example:53682/callback",
            "http://127.0.0.1.evil.example:53682/callback",
            "http://127.0.0.1/callback",
            "http://127.0.0.1:/callback",
            "http://127.0.0.1:0/callback",
            "http://127.0.0.1:80/callback",
            "http://127.0.0.1:1023/callback",
            "http://127.0.0.1:65536/callback",
            "http://127.0.0.1:053682/callback",
            "http://127.0.0.1:+5368/callback",
            "http://127.0.0.1:53682/",
            "http://127.0.0.1:53682/other",
            "http://127.0.0.1:53682/callback/",
            "http://127.0.0.1:53682/callback?x=1",
            "http://127.0.0.1:53682/callback#x",
            "http://127.0.0.1:53682/callback/../x",
            "http://127.0.0.1:53682//evil.example",
            "http://127.0.0.1:53682\\@evil.example/callback",
            "//127.0.0.1:53682/callback",
            "HTTP://127.0.0.1:53682/callback",
            " http://127.0.0.1:53682/callback",
            "",
        ] {
            assert_eq!(loopback_port(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn device_names_are_cut_down_to_plain_text() {
        assert_eq!(sanitize_device("test-laptop"), "test-laptop");
        assert_eq!(sanitize_device("  test   laptop.local "), "test laptop.local");
        assert_eq!(sanitize_device("<script>alert(1)</script>"), "scriptalert1script");
        assert_eq!(sanitize_device("a\u{202e}b\nc\0d"), "abcd");
        assert_eq!(sanitize_device(""), "unknown device");
        assert_eq!(sanitize_device("<>\u{1F600}"), "unknown device");
        assert_eq!(sanitize_device(&"x".repeat(500)).len(), MAX_DEVICE_NAME);
        assert_eq!(key_name("test-laptop"), "cyanotype CLI on test-laptop");
    }

    #[test]
    fn user_codes_normalize_and_draw_from_the_alphabet() {
        assert_eq!(normalize_user_code("wdjb-mjht").as_deref(), Some("WDJBMJHT"));
        assert_eq!(normalize_user_code(" WDJB MJHT ").as_deref(), Some("WDJBMJHT"));
        for bad in ["", "WDJB", "WDJB-MJHTX", "AEIO-UAEI", "WDJB-MJH1", "WDJB_MJHT"] {
            assert_eq!(normalize_user_code(bad), None, "{bad:?}");
        }
        for _ in 0..200 {
            let c = new_user_code();
            assert_eq!(c.len(), 9);
            assert_eq!(c.as_bytes()[4], b'-');
            assert!(normalize_user_code(&c).is_some(), "{c}");
        }
    }

    #[test]
    fn pkce_rules() {
        // RFC 7636, appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(challenge_of(verifier), challenge);
        assert!(valid_verifier(verifier) && valid_challenge(challenge));
        assert!(!valid_verifier("short") && !valid_verifier(&"a".repeat(129)) && !valid_verifier(&"a b".repeat(20)));
        assert!(!valid_challenge("short") && !valid_challenge(&format!("{challenge}=")));
        assert!(
            valid_state("abc-123_x.~") && !valid_state("") && !valid_state("a&b") && !valid_state(&"a".repeat(129))
        );
    }
}
