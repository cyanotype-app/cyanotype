//! Share links, visitor side: `/assets/public/s/{token}/...`, no login. (The gateway sends `/assets/*` to this
//! service; the page a visitor opens is the web app's `/s/<token>`.)
//!
//! | Request | Answer |
//! |---|---|
//! | `GET .../` | Info: title, kind, photo count, download policy, expiry. For a password link that is not unlocked yet only `{needs_password: true}`. Counts as one view. |
//! | `POST .../unlock {password}` | `204` and a short-lived `HttpOnly` cookie scoped to this one link, or `401`. |
//! | `GET .../items?limit&cursor` | The photos, newest first, with presigned thumbnail and preview URLs (10 minutes). |
//! | `GET .../items/{id}` | One photo. |
//! | `POST .../items/{id}/prepare` | Ask for the location-free download copy of a JPEG to be made now. |
//! | `GET .../items/{id}/download?file=` | `307` to a presigned attachment URL (one minute). |
//!
//! Privacy rules, all enforced here:
//! * An unknown, revoked and expired token is the same `404` with the same body, so tokens cannot be probed.
//!   Probes cost a per-IP budget (`429`), as do password attempts; a link also locks for ten minutes after ten wrong
//!   passwords in a row.
//! * A visitor gets only: a photo's pixels at thumbnail and preview size (WebP files that carry no metadata at all,
//!   see `worker::imaging`), its size, its time and its asset id. Never the owner's email or id, file names, camera,
//!   EXIF or GPS. Presigned URLs are short-lived, the API never proxies bytes.
//! * Downloads: off if the owner turned them off. Otherwise a JPEG is served as `share.jpg`, the copy the worker
//!   wrote with all metadata removed (see `worker::strip`); anything else (HEIC, RAW, PNG, ...) is only offered if
//!   the owner chose "include originals", which serves the untouched files (and their location).
//! * Everything is read from the database on every request, so revoking, editing or trashing takes effect at once.
use std::{net::IpAddr, sync::Arc, time::Duration};

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use cyanotype_core::{jobs, keys, metrics};
use cyanotype_service::{
    ApiError,
    auth::sha256,
    ratelimit::{Cidr, RateLimitConfig, RateLimiter, parse_proxies, request_ip, retry_after_secs},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{decode_cursor, encode_cursor, paired_files};
use crate::AppState;

const SORT: &str = "coalesce(x.taken_at, x.file_modified_at, x.created_at)";
/// Thumbnails and previews a visitor was handed stop working after this.
pub(super) const IMAGE_TTL: Duration = Duration::from_secs(600);
pub(super) const DOWNLOAD_TTL: Duration = Duration::from_secs(60);
/// How long a correct password keeps a browser unlocked.
const UNLOCK_HOURS: i64 = 6;
/// Wrong passwords in a row (from anyone) before the link locks, and for how long.
const LOCK_AFTER: i32 = 10;
const LOCK_MINUTES: i64 = 10;

/// Limiters and cookie settings of the public side.
#[derive(Clone)]
pub struct ShareState {
    /// Every public request, per client IP.
    lookup: Arc<RateLimiter>,
    /// Requests for a token that does not exist (or is revoked / expired), per client IP.
    miss: Arc<RateLimiter>,
    /// Password attempts, per client IP.
    password: Arc<RateLimiter>,
    trusted: Arc<Vec<Cidr>>,
    cookie_secure: bool,
}

impl ShareState {
    pub fn new(cookie_secure: bool, trusted: Vec<Cidr>) -> Self {
        let limiter = |burst: u32, per_minute: u32| {
            Arc::new(RateLimiter::new(RateLimitConfig {
                burst,
                per_minute,
                max_keys: 10_000,
            }))
        };
        Self {
            lookup: limiter(120, 120),
            miss: limiter(10, 10),
            password: limiter(5, 5),
            trusted: Arc::new(trusted),
            cookie_secure,
        }
    }

    /// `COOKIE_SECURE` (default true, as for the auth service) and `TRUSTED_PROXIES` (same default as auth).
    pub fn from_env() -> anyhow::Result<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let cookie_secure = match get("COOKIE_SECURE").map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("true" | "1" | "yes" | "on") => true,
            Some("false" | "0" | "no" | "off") => false,
            Some(other) => anyhow::bail!("COOKIE_SECURE must be true or false (got {other:?})"),
        };
        if !cookie_secure {
            tracing::warn!(
                "COOKIE_SECURE=false: share unlock cookies are sent without the Secure attribute; use only for local http"
            );
        }
        Ok(Self::new(
            cookie_secure,
            parse_proxies(get("TRUSTED_PROXIES").as_deref())?,
        ))
    }

    #[cfg(test)]
    pub fn with_limits(mut self, lookup: (u32, u32), miss: (u32, u32), password: (u32, u32)) -> Self {
        let l = |(burst, per_minute): (u32, u32)| {
            Arc::new(RateLimiter::new(RateLimitConfig {
                burst,
                per_minute,
                max_keys: 100,
            }))
        };
        self.lookup = l(lookup);
        self.miss = l(miss);
        self.password = l(password);
        self
    }
}

#[derive(Clone, Copy)]
struct ClientIp(IpAddr);

pub fn routes(state: ShareState) -> Router<AppState> {
    Router::new()
        .route("/assets/public/s/{token}", get(info))
        .route("/assets/public/s/{token}/unlock", post(unlock))
        .route("/assets/public/s/{token}/items", get(items))
        .route("/assets/public/s/{token}/items/{id}", get(item))
        .route("/assets/public/s/{token}/items/{id}/prepare", post(prepare))
        .route("/assets/public/s/{token}/items/{id}/download", get(download))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .layer(Extension(state))
}

/// Per-IP budget for every public request, the client address for the handlers, and the headers that keep these
/// responses out of search engines, caches and referrers.
async fn guard(State(st): State<ShareState>, req: Request, next: Next) -> Response {
    let (mut parts, body) = req.into_parts();
    let ip = request_ip(&parts, &st.trusted);
    let mut res = match st.lookup.check(ip, std::time::Instant::now()) {
        Ok(()) => {
            parts.extensions.insert(ClientIp(ip));
            next.run(Request::from_parts(parts, body)).await
        }
        Err(wait) => PubError::TooMany(retry_after_secs(wait)).into_response(),
    };
    let h = res.headers_mut();
    h.insert("x-robots-tag", HeaderValue::from_static("noindex, nofollow, noarchive"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

enum PubError {
    Api(ApiError),
    /// `429` with `Retry-After` seconds.
    TooMany(u64),
}

impl From<ApiError> for PubError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}
impl From<sqlx::Error> for PubError {
    fn from(e: sqlx::Error) -> Self {
        Self::Api(e.into())
    }
}
impl From<anyhow::Error> for PubError {
    fn from(e: anyhow::Error) -> Self {
        Self::Api(e.into())
    }
}

impl IntoResponse for PubError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(e) => e.into_response(),
            Self::TooMany(secs) => {
                let mut res = (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({"error": "too many attempts, try again later"})),
                )
                    .into_response();
                if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                    res.headers_mut().insert(header::RETRY_AFTER, v);
                }
                res
            }
        }
    }
}

type PubResult<T> = Result<T, PubError>;

pub(super) async fn hash_password(p: &str) -> Result<String, ApiError> {
    let p = p.to_string();
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(p.as_bytes(), &SaltString::generate(&mut OsRng))
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!("hash: {e}"))
    })
    .await
    .map_err(|e| ApiError::Internal(e.into()))?
    .map_err(ApiError::Internal)
}

async fn verify_password(hash: String, p: String) -> Result<bool, ApiError> {
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&hash).map_err(|e| anyhow::anyhow!("stored hash: {e}"))?;
        Ok::<_, anyhow::Error>(Argon2::default().verify_password(p.as_bytes(), &parsed).is_ok())
    })
    .await
    .map_err(|e| ApiError::Internal(e.into()))?
    .map_err(ApiError::Internal)
}

#[derive(FromRow)]
struct ShareRow {
    id: Uuid,
    owner_id: Uuid,
    kind: String,
    album_id: Option<Uuid>,
    title: Option<String>,
    album_title: Option<String>,
    password_hash: Option<String>,
    allow_download: bool,
    include_originals: bool,
    expires_at: Option<OffsetDateTime>,
    pw_locked_for_secs: Option<f64>,
}

impl ShareRow {
    fn cookie_name(&self) -> String {
        format!("cy_share_{}", &self.id.simple().to_string()[..16])
    }

    /// FROM and WHERE (parameter `$2` is the album or share id) for the photos this link shows, with `x` the photo.
    /// `$1` is always the owner: photos of anyone else never show, whatever the join tables say.
    fn source(&self) -> (&'static str, &'static str, Uuid) {
        match (self.kind.as_str(), self.album_id) {
            ("album", Some(album)) => (
                "album_assets aa JOIN assets x ON x.id = aa.asset_id",
                "aa.album_id = $2",
                album,
            ),
            _ => (
                "share_assets aa JOIN assets x ON x.id = aa.asset_id",
                "aa.share_id = $2",
                self.id,
            ),
        }
    }
}

/// A hidden photo is in no public link, even when it is a member of the shared album or was picked for the selection before it was
/// hidden (every query of this module goes through this predicate).
const VISIBLE: &str = "x.owner_id = $1 AND x.deleted_at IS NULL AND x.primary_id IS NULL AND NOT x.hidden";

/// The link for `token`, or the one `404` every dead token gets. Each miss spends from the caller's miss budget.
async fn find(s: &AppState, st: &ShareState, ip: IpAddr, token: &str) -> PubResult<ShareRow> {
    let row = if token.len() <= 64
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        sqlx::query_as::<_, ShareRow>(
            "SELECT s.id, s.owner_id, s.kind, s.album_id, s.title, a.title AS album_title, s.password_hash,
                    s.allow_download, s.include_originals, s.expires_at,
                    extract(epoch FROM s.pw_locked_until - now())::float8 AS pw_locked_for_secs
             FROM shares s LEFT JOIN albums a ON a.id = s.album_id
             WHERE s.token = $1 AND (s.expires_at IS NULL OR s.expires_at > now())",
        )
        .bind(token)
        .fetch_optional(&s.db)
        .await?
    } else {
        None
    };
    match row {
        Some(r) => Ok(r),
        None => match st.miss.check(ip, std::time::Instant::now()) {
            Ok(()) => Err(ApiError::NotFound.into()),
            Err(wait) => Err(PubError::TooMany(retry_after_secs(wait))),
        },
    }
}

async fn is_unlocked(s: &AppState, share: &ShareRow, jar: &CookieJar) -> PubResult<bool> {
    if share.password_hash.is_none() {
        return Ok(true);
    }
    let Some(c) = jar.get(&share.cookie_name()) else {
        return Ok(false);
    };
    let ok: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM share_unlocks WHERE token_hash = $1 AND share_id = $2 AND expires_at > now()")
            .bind(sha256(c.value().as_bytes()))
            .bind(share.id)
            .fetch_optional(&s.db)
            .await?;
    Ok(ok.is_some())
}

/// A link a visitor may look into: it exists, is not expired, and the password (if any) was given.
async fn open(s: &AppState, st: &ShareState, ip: IpAddr, token: &str, jar: &CookieJar) -> PubResult<ShareRow> {
    let share = find(s, st, ip, token).await?;
    if !is_unlocked(s, &share, jar).await? {
        return Err(ApiError::Unauthorized.into());
    }
    Ok(share)
}

async fn item_count(s: &AppState, share: &ShareRow) -> sqlx::Result<i64> {
    let (from, wh, r) = share.source();
    sqlx::query_scalar(&format!("SELECT count(*) FROM {from} WHERE {wh} AND {VISIBLE}"))
        .bind(share.owner_id)
        .bind(r)
        .fetch_one(&s.db)
        .await
}

async fn info(
    State(s): State<AppState>,
    Extension(st): Extension<ShareState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Path(token): Path<String>,
    jar: CookieJar,
) -> PubResult<Json<serde_json::Value>> {
    let share = find(&s, &st, ip, &token).await?;
    if !is_unlocked(&s, &share, &jar).await? {
        return Ok(Json(serde_json::json!({"needs_password": true, "unlocked": false})));
    }
    sqlx::query("UPDATE shares SET view_count = view_count + 1, last_viewed_at = now() WHERE id = $1")
        .bind(share.id)
        .execute(&s.db)
        .await?;
    metrics::global().count("shares.views", 1, &[("kind", share.kind.as_str())]);
    tracing::info!(share_id = %share.id, kind = %share.kind, "share viewed");
    let count = item_count(&s, &share).await?;
    Ok(Json(serde_json::json!({
        "needs_password": share.password_hash.is_some(),
        "unlocked": true,
        "kind": share.kind,
        "title": share.title.clone().or(share.album_title.clone()),
        "item_count": count,
        "allow_download": share.allow_download,
        "include_originals": share.include_originals,
        "expires_at": share.expires_at.and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
    })))
}

#[derive(Deserialize)]
struct UnlockBody {
    password: String,
}

/// The cookie that remembers a correct share password. `Secure` is on unless `COOKIE_SECURE=false` was set
/// explicitly (plain-http local dev); the setting defaults to true.
fn unlock_cookie(name: String, value: String, token: &str, secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::build((name, value))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path(format!("/assets/public/s/{token}"))
        .max_age(time::Duration::hours(UNLOCK_HOURS))
        .build();
    if !secure {
        cookie.set_secure(false);
    }
    cookie
}
async fn unlock(
    State(s): State<AppState>,
    Extension(st): Extension<ShareState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Path(token): Path<String>,
    jar: CookieJar,
    Json(body): Json<UnlockBody>,
) -> PubResult<(CookieJar, StatusCode)> {
    let share = find(&s, &st, ip, &token).await?;
    let Some(hash) = share.password_hash.clone() else {
        return Ok((jar, StatusCode::NO_CONTENT));
    };
    if let Some(secs) = share.pw_locked_for_secs.filter(|s| *s > 0.0) {
        return Err(PubError::TooMany(secs.ceil() as u64));
    }
    if let Err(wait) = st.password.check(ip, std::time::Instant::now()) {
        return Err(PubError::TooMany(retry_after_secs(wait)));
    }
    if body.password.len() > 1024 || !verify_password(hash, body.password).await? {
        // After LOCK_AFTER wrong passwords in a row the link refuses all attempts for a while.
        sqlx::query(
            "UPDATE shares SET
                 pw_locked_until = CASE WHEN pw_failures + 1 >= $2 THEN now() + make_interval(mins => $3) ELSE pw_locked_until END,
                 pw_failures = CASE WHEN pw_failures + 1 >= $2 THEN 0 ELSE pw_failures + 1 END
             WHERE id = $1",
        )
        .bind(share.id)
        .bind(LOCK_AFTER)
        .bind(LOCK_MINUTES as i32)
        .execute(&s.db)
        .await?;
        tracing::info!(share_id = %share.id, "share password rejected");
        return Err(ApiError::Unauthorized.into());
    }
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let value = cyanotype_core::keys::hex(&raw);
    sqlx::query("DELETE FROM share_unlocks WHERE expires_at < now()")
        .execute(&s.db)
        .await?;
    sqlx::query("UPDATE shares SET pw_failures = 0 WHERE id = $1")
        .bind(share.id)
        .execute(&s.db)
        .await?;
    sqlx::query("INSERT INTO share_unlocks (token_hash, share_id, expires_at) VALUES ($1, $2, now() + make_interval(hours => $3))")
        .bind(sha256(value.as_bytes()))
        .bind(share.id)
        .bind(UNLOCK_HOURS as i32)
        .execute(&s.db)
        .await?;
    // Scoped to this one link: the cookie is only sent to its own API prefix.
    let cookie = unlock_cookie(share.cookie_name(), value, &token, st.cookie_secure);
    tracing::info!(share_id = %share.id, "share unlocked");
    Ok((jar.add(cookie), StatusCode::NO_CONTENT))
}

#[derive(FromRow)]
struct ItemRow {
    id: Uuid,
    width: Option<i32>,
    height: Option<i32>,
    sort_time: OffsetDateTime,
    thumbhash: Option<Vec<u8>>,
    content_type: String,
    original_name: String,
    size_bytes: i64,
    share_copy_at: Option<OffsetDateTime>,
}

const ITEM_COLUMNS: &str = "x.id, x.width, x.height, {SORT} AS sort_time, x.thumbhash, x.content_type, x.original_name, x.size_bytes, x.share_copy_at";

fn item_columns() -> String {
    ITEM_COLUMNS.replace("{SORT}", SORT)
}

#[derive(Serialize)]
struct Download {
    /// Pass as `?file=` to the download route.
    id: Uuid,
    label: String,
    /// Known for originals; the stripped copy's size is not recorded.
    size_bytes: Option<i64>,
}

#[derive(Serialize)]
struct PublicItem {
    id: Uuid,
    width: Option<i32>,
    height: Option<i32>,
    #[serde(with = "time::serde::rfc3339")]
    sort_time: OffsetDateTime,
    thumbhash: Option<String>,
    thumb_url: Option<String>,
    preview_url: Option<String>,
    downloads: Vec<Download>,
    /// A download would be offered once the worker has made the location-free copy (`POST .../prepare` asks for it).
    download_pending: bool,
}

fn is_jpeg(r: &ItemRow) -> bool {
    let ct = r.content_type.to_ascii_lowercase();
    ct == "image/jpeg" || ct == "image/jpg" || matches!(keys::sanitize_ext(&r.original_name).as_str(), "jpg" | "jpeg")
}

fn format_label(r: &ItemRow) -> String {
    match keys::sanitize_ext(&r.original_name).as_str() {
        "jpg" | "jpeg" => "JPEG".into(),
        "bin" => "Original".into(),
        e => e.to_ascii_uppercase(),
    }
}

async fn build_items(s: &AppState, share: &ShareRow, rows: Vec<ItemRow>) -> PubResult<Vec<PublicItem>> {
    let raws = if share.allow_download && share.include_originals {
        let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
        paired_files(&s.db, &ids).await.map_err(PubError::Api)?
    } else {
        Default::default()
    };
    let mut raws = raws;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let (thumbhash, thumb_url, preview_url) = match &r.thumbhash {
            Some(h) => (
                Some(B64.encode(h)),
                Some(
                    s.storage
                        .presign_get_ttl(&keys::thumb(share.owner_id, r.id), IMAGE_TTL)
                        .await?,
                ),
                Some(
                    s.storage
                        .presign_get_ttl(&keys::preview(share.owner_id, r.id), IMAGE_TTL)
                        .await?,
                ),
            ),
            None => (None, None, None),
        };
        let mut downloads = Vec::new();
        let mut pending = false;
        if share.allow_download {
            if share.include_originals {
                downloads.push(Download {
                    id: r.id,
                    label: format_label(&r),
                    size_bytes: Some(r.size_bytes),
                });
                for p in raws.remove(&r.id).unwrap_or_default() {
                    downloads.push(Download {
                        id: p.id,
                        label: "RAW".into(),
                        size_bytes: Some(p.size_bytes),
                    });
                }
            } else if is_jpeg(&r) {
                if r.share_copy_at.is_some() {
                    downloads.push(Download {
                        id: r.id,
                        label: "JPEG".into(),
                        size_bytes: None,
                    });
                } else {
                    pending = true;
                }
            }
        }
        out.push(PublicItem {
            id: r.id,
            width: r.width,
            height: r.height,
            sort_time: r.sort_time,
            thumbhash,
            thumb_url,
            preview_url,
            downloads,
            download_pending: pending,
        });
    }
    Ok(out)
}

#[derive(Deserialize)]
struct ItemsQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

async fn items(
    State(s): State<AppState>,
    Extension(st): Extension<ShareState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Path(token): Path<String>,
    Query(q): Query<ItemsQuery>,
    jar: CookieJar,
) -> PubResult<Json<serde_json::Value>> {
    let share = open(&s, &st, ip, &token, &jar).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 200);
    let after = match &q.cursor {
        Some(c) => Some(decode_cursor(c).ok_or_else(|| ApiError::BadRequest("invalid cursor".into()))?),
        None => None,
    };
    let (from, wh, r) = share.source();
    let cols = item_columns();
    let select = format!("SELECT {cols} FROM {from} WHERE {wh} AND {VISIBLE}");
    let rows = match after {
        Some((ts, id)) => {
            sqlx::query_as::<_, ItemRow>(&format!(
                "{select} AND ({SORT}, x.id) < ($3, $4) ORDER BY {SORT} DESC, x.id DESC LIMIT $5"
            ))
            .bind(share.owner_id)
            .bind(r)
            .bind(ts)
            .bind(id)
            .bind(limit)
            .fetch_all(&s.db)
            .await?
        }
        None => {
            sqlx::query_as::<_, ItemRow>(&format!("{select} ORDER BY {SORT} DESC, x.id DESC LIMIT $3"))
                .bind(share.owner_id)
                .bind(r)
                .bind(limit)
                .fetch_all(&s.db)
                .await?
        }
    };
    let next_cursor = if rows.len() as i64 == limit {
        rows.last().map(|l| encode_cursor(l.sort_time, l.id))
    } else {
        None
    };
    let items = build_items(&s, &share, rows).await?;
    Ok(Json(serde_json::json!({"items": items, "next_cursor": next_cursor})))
}

/// One photo of the link, or the `404` of a photo that is not (or no longer) in it.
async fn one(s: &AppState, share: &ShareRow, id: Uuid) -> PubResult<ItemRow> {
    let (from, wh, r) = share.source();
    let cols = item_columns();
    let row = sqlx::query_as::<_, ItemRow>(&format!(
        "SELECT {cols} FROM {from} WHERE {wh} AND {VISIBLE} AND x.id = $3"
    ))
    .bind(share.owner_id)
    .bind(r)
    .bind(id)
    .fetch_optional(&s.db)
    .await?;
    row.ok_or_else(|| ApiError::NotFound.into())
}

async fn item(
    State(s): State<AppState>,
    Extension(st): Extension<ShareState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Path((token, id)): Path<(String, Uuid)>,
    jar: CookieJar,
) -> PubResult<Json<PublicItem>> {
    let share = open(&s, &st, ip, &token, &jar).await?;
    let row = one(&s, &share, id).await?;
    let mut items = build_items(&s, &share, vec![row]).await?;
    Ok(Json(items.remove(0)))
}

async fn prepare(
    State(s): State<AppState>,
    Extension(st): Extension<ShareState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Path((token, id)): Path<(String, Uuid)>,
    jar: CookieJar,
) -> PubResult<(StatusCode, Json<serde_json::Value>)> {
    let share = open(&s, &st, ip, &token, &jar).await?;
    let row = one(&s, &share, id).await?;
    let needed = share.allow_download && !share.include_originals && is_jpeg(&row) && row.share_copy_at.is_none();
    if needed {
        // Idempotent (one job per photo); the worker is woken by the queue's NOTIFY.
        jobs::enqueue(&s.db, jobs::KIND_SHARE_COPY, id, 1).await?;
    }
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({"pending": needed}))))
}

#[derive(Deserialize)]
struct DownloadQuery {
    file: Option<Uuid>,
}

async fn download(
    State(s): State<AppState>,
    Extension(st): Extension<ShareState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Path((token, id)): Path<(String, Uuid)>,
    Query(q): Query<DownloadQuery>,
    jar: CookieJar,
) -> PubResult<Response> {
    let share = open(&s, &st, ip, &token, &jar).await?;
    let row = one(&s, &share, id).await?;
    if !share.allow_download {
        return Err(ApiError::Forbidden("downloads are turned off for this link".into()).into());
    }
    let file = q.file.unwrap_or(id);
    let url = if share.include_originals {
        // The photo's own file or the RAW behind it, untouched.
        let found: Option<(String, String)> = sqlx::query_as(
            "SELECT original_key, original_name FROM assets
             WHERE id = $1 AND owner_id = $2 AND (id = $3 OR primary_id = $3)",
        )
        .bind(file)
        .bind(share.owner_id)
        .bind(id)
        .fetch_optional(&s.db)
        .await?;
        let (key, name) = found.ok_or(ApiError::NotFound)?;
        s.storage.presign_get_download(&key, &name, DOWNLOAD_TTL).await?
    } else {
        // Only the location-free JPEG copy. Never the original, never a RAW, never HEIC.
        if file != id || !is_jpeg(&row) {
            return Err(ApiError::NotFound.into());
        }
        if row.share_copy_at.is_none() {
            return Err(
                ApiError::Conflict("this download is still being prepared, try again in a moment".into()).into(),
            );
        }
        s.storage
            .presign_get_download(&keys::share_copy(share.owner_id, id), &row.original_name, DOWNLOAD_TTL)
            .await?
    };
    metrics::global().count("shares.downloads", 1, &[("kind", share.kind.as_str())]);
    tracing::info!(share_id = %share.id, asset_id = %id, originals = share.include_originals, "share download");
    Ok((
        StatusCode::TEMPORARY_REDIRECT,
        [(header::LOCATION, url), (header::CACHE_CONTROL, "no-store".to_string())],
    )
        .into_response())
}

#[cfg(test)]
mod cookie_tests {
    use super::unlock_cookie;

    #[test]
    fn unlock_cookie_is_secure_unless_switched_off() {
        let on = unlock_cookie("n".into(), "v".into(), "tok", true).to_string();
        assert!(
            on.contains("Secure") && on.contains("HttpOnly") && on.contains("Path=/assets/public/s/tok"),
            "{on}"
        );
        let off = unlock_cookie("n".into(), "v".into(), "tok", false).to_string();
        assert!(!off.contains("Secure") && off.contains("HttpOnly"), "{off}");
    }
}
