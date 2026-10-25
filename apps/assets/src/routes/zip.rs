//! Download as a ZIP: a whole album, or the photos you selected, as one archive that is streamed.
//!
//! | Request | Answer |
//! |---|---|
//! | `POST /assets/zip/plan` | `{files, bytes, name}`: what the archive would hold (so a button can say "12 photos, 48 MB", or fail before a download starts). |
//! | `POST /assets/zip` | The archive. `Content-Type: application/zip`, an attachment, with an exact `Content-Length`. |
//! | `GET /assets/albums/{id}/zip` | The archive of one album (a plain link; works with a `read` API key). |
//!
//! The POST body is a bulk selector (`scope.rs`: `ids`, or a `scope` with `except`, so "select all 4,000" ships no ids) and
//! an optional `name` for the file. It is JSON, or a form with the JSON in a `selection` field: the page submits a hidden
//! form so the *browser* streams the response to disk (a `fetch` would hold the archive in the page's memory).
//!
//! What goes in: the originals of the caller's own live photos, exactly as the library lists them (not trashed or being
//! purged; hidden ones only for explicit ids with the Hidden view unlocked, never through a scope or an album, as the
//! album's own listing). A RAW + JPEG pair is both files, the photo's and the RAW behind it. Edits (rotate, crop) are not
//! applied: these are the files as uploaded. Entries keep the original file names (made safe: no folders, no `..`),
//! repeated names become `IMG_0001 (2).jpg`, and each entry's time is the photo's capture time (else the file's own time).
//!
//! The server reads each original from the bucket and relays it, one chunk at a time: memory stays flat whatever the size.
//! The response starts at once (the first entry's header is sent before its bytes are fetched). If an original cannot be
//! read half way, the connection is aborted rather than finished, so a broken archive is never mistaken for a whole one.
//! The ZIP is store-only (photos do not compress) and uses ZIP64 where a file or the archive passes 4 GiB.
//!
//! Limits (owner): at most [`MAX_FILES`] files per archive (`413` says to split it), [`SLOTS`] archives streaming at once
//! per process (`429` with `Retry-After`). Share links have their own, tighter ones (`share_public.rs`).
use std::io;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use bytes::Bytes;
use cyanotype_core::{
    metrics,
    storage::{ObjectStream, Storage, content_disposition_attachment},
};
use serde::Deserialize;
use sqlx::FromRow;
use time::OffsetDateTime;
use tokio::sync::SemaphorePermit;
use uuid::Uuid;

use super::scope::{Pick, Scope, Selector};
use crate::{
    AppState,
    zip::{Entry, NameSet, ZipWriter, archive_len, archive_name},
};
use cyanotype_service::{ApiError, ApiResult, AuthUser, Unlock};

/// Most files in one archive of the owner's own photos.
pub(super) const MAX_FILES: usize = 50_000;
/// Archives of the owner's streaming at once, per process (each holds one chunk in memory and one bucket connection).
const SLOTS: usize = 16;
static OWNER_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(SLOTS);

/// One file to fetch and where it goes in the archive.
pub(super) struct Planned {
    pub key: String,
    pub entry: Entry,
}

/// What an archive holds, before a byte is read.
pub(super) struct Archive {
    /// The download's file name (`<album>.zip`).
    pub file_name: String,
    pub files: Vec<Planned>,
}

impl Archive {
    pub fn new(title: Option<&str>, files: Vec<Planned>) -> Self {
        Self {
            file_name: archive_name(title),
            files,
        }
    }

    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.entry.size).sum()
    }
}

/// One original (or RAW) to include.
#[derive(FromRow)]
pub(super) struct FileRow {
    pub original_key: String,
    pub original_name: String,
    pub size_bytes: i64,
    /// The photo's capture time, else the file's own modification time.
    pub mtime: Option<OffsetDateTime>,
}

/// The files with unique, safe names, in the order given.
pub(super) fn plan_files(rows: Vec<FileRow>) -> Vec<Planned> {
    let mut names = NameSet::new();
    rows.into_iter()
        .map(|r| Planned {
            entry: Entry {
                name: names.unique(&r.original_name),
                mtime: r.mtime.map(|t| t.unix_timestamp()),
                size: u64::try_from(r.size_bytes).unwrap_or(0),
            },
            key: r.original_key,
        })
        .collect()
}

/// The bulk selector, from JSON or from the `selection` field of a form; plus the optional file name.
fn parse(headers: &HeaderMap, body: &[u8]) -> ApiResult<(Selector, Option<String>)> {
    #[derive(Deserialize)]
    struct Form {
        selection: String,
    }
    #[derive(Deserialize)]
    struct Named {
        name: Option<String>,
    }
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("json"));
    let json: Vec<u8> = if is_json {
        body.to_vec()
    } else {
        serde_urlencoded::from_bytes::<Form>(body)
            .map_err(|_| ApiError::BadRequest("send JSON, or a form with a selection field".into()))?
            .selection
            .into_bytes()
    };
    let bad = |_| ApiError::BadRequest("the selection is not valid JSON".into());
    let sel: Selector = serde_json::from_slice(&json).map_err(bad)?;
    let named: Named = serde_json::from_slice(&json).map_err(bad)?;
    Ok((sel, named.name))
}

/// The archive of what `pick` names, for `owner`.
async fn plan(s: &AppState, owner: Uuid, unlocked: bool, pick: Pick, name: Option<String>) -> ApiResult<Archive> {
    let title = match (&name, &pick) {
        (Some(n), _) => Some(n.clone()),
        (None, Pick::Scope(Scope::Album(id), ..)) => {
            sqlx::query_scalar("SELECT title FROM albums WHERE id = $1 AND owner_id = $2")
                .bind(id)
                .bind(owner)
                .fetch_optional(&s.db)
                .await?
        }
        _ => None,
    };
    let ids = {
        let mut conn = s.db.acquire().await?;
        pick.resolve(&mut conn, owner).await?.unwrap_or_default()
    };
    // Whatever the ids came from, the live-photo rules are applied again here, in the same query that reads the files. The
    // photo's own file and the RAW behind it are two index lookups (`UNION ALL`), not one join on `id = ... OR primary_id = ...`.
    const PICKED: &str = "p.id = ANY($1) AND p.owner_id = $2 AND p.deleted_at IS NULL AND p.purging_at IS NULL
           AND p.primary_id IS NULL AND (NOT p.hidden OR $3)";
    let rows: Vec<FileRow> = sqlx::query_as(&format!(
        "SELECT original_key, original_name, size_bytes, mtime FROM (
           SELECT f.original_key, f.original_name, f.size_bytes, coalesce(p.taken_at, p.file_modified_at) AS mtime,
                  coalesce(p.taken_at, p.file_modified_at, p.created_at) AS sort_t, p.id AS pid, 0 AS part, f.id AS fid
           FROM assets p JOIN assets f ON f.id = p.id WHERE {PICKED}
           UNION ALL
           SELECT f.original_key, f.original_name, f.size_bytes, coalesce(p.taken_at, p.file_modified_at),
                  coalesce(p.taken_at, p.file_modified_at, p.created_at), p.id, 1, f.id
           FROM assets p JOIN assets f ON f.primary_id = p.id AND f.owner_id = p.owner_id
           WHERE {PICKED} AND f.purging_at IS NULL
         ) u ORDER BY sort_t, pid, part, fid LIMIT $4"
    ))
    .bind(&ids)
    .bind(owner)
    .bind(unlocked)
    .bind(MAX_FILES as i64 + 1)
    .fetch_all(&s.db)
    .await?;
    if rows.len() > MAX_FILES {
        return Err(ApiError::PayloadTooLarge(format!(
            "an archive holds at most {MAX_FILES} files; download it in parts"
        )));
    }
    if rows.is_empty() {
        return Err(ApiError::NotFound);
    }
    Ok(Archive::new(title.as_deref(), plan_files(rows)))
}

/// `POST /assets/zip/plan`: how many files and bytes the archive would hold, and its name.
pub(super) async fn plan_selection(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<axum::Json<serde_json::Value>> {
    let (sel, name) = parse(&headers, &body)?;
    let pick = sel.validated(unlock, s.faces_enabled)?;
    let a = plan(&s, user.id, unlock.0, pick, name).await?;
    Ok(axum::Json(serde_json::json!({
        "files": a.files.len(),
        "bytes": a.bytes(),
        "name": a.file_name,
    })))
}

/// `POST /assets/zip`: the archive of a selection.
pub(super) async fn selection(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let (sel, name) = parse(&headers, &body)?;
    let pick = sel.validated(unlock, s.faces_enabled)?;
    let source = if pick.is_scope() { "scope" } else { "ids" };
    let a = plan(&s, user.id, unlock.0, pick, name).await?;
    stream_owner(&s, user.id, a, source)
}

/// `GET /assets/albums/{id}/zip`: the archive of an album (a smart album: what its rules match now).
pub(super) async fn album(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    let pick = Pick::Scope(Scope::Album(id), Vec::new(), unlock.0, s.faces_enabled);
    let a = plan(&s, user.id, unlock.0, pick, None).await?;
    stream_owner(&s, user.id, a, "album")
}

fn stream_owner(s: &AppState, owner: Uuid, a: Archive, source: &'static str) -> ApiResult<Response> {
    let Ok(permit) = OWNER_SLOTS.try_acquire() else {
        return Err(busy());
    };
    metrics::global().count("assets.zip_downloads", 1, &[("source", source)]);
    tracing::info!(owner = %owner, files = a.files.len(), bytes = a.bytes(), source, "zip download");
    Ok(respond(&s.storage, a, permit))
}

pub(super) fn busy() -> ApiError {
    ApiError::Limited {
        status: StatusCode::TOO_MANY_REQUESTS,
        code: "zip_busy",
        message: "too many downloads are running right now, try again in a minute".into(),
        details: serde_json::Value::Null,
        retry_after_secs: Some(30),
    }
}

/// The streaming response of `archive`. The permit is held until the body is dropped (finished, failed or abandoned).
pub(super) fn respond(storage: &Storage, archive: Archive, permit: SemaphorePermit<'static>) -> Response {
    let entries: Vec<Entry> = archive.files.iter().map(|f| f.entry.clone()).collect();
    let len = archive_len(&entries);
    let run = Run {
        storage: storage.clone(),
        files: archive.files.into_iter(),
        w: ZipWriter::new(),
        cur: None,
        step: Step::Next,
        sent: 0,
        _permit: permit,
    };
    let body = Body::from_stream(futures_util::stream::try_unfold(run, |mut r| async move {
        Ok::<_, io::Error>(r.next().await?.map(|b| (b, r)))
    }));
    let mut res = Response::new(body);
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    if let Ok(v) = HeaderValue::from_str(&content_disposition_attachment(&archive.file_name)) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

enum Step {
    /// Open the next entry (or write the directory, after the last).
    Next,
    /// Relay the open entry's bytes.
    Data,
    Done,
}

/// The state of one streaming archive: which file is open, how far it is, the writer's running totals.
struct Run {
    storage: Storage,
    files: std::vec::IntoIter<Planned>,
    w: ZipWriter,
    cur: Option<ObjectStream>,
    step: Step,
    sent: u64,
    _permit: SemaphorePermit<'static>,
}

fn fail(msg: String) -> io::Error {
    tracing::warn!(error = %msg, "zip download aborted");
    io::Error::other(msg)
}

impl Run {
    /// The next piece of the archive, or `None` when it is complete.
    async fn next(&mut self) -> io::Result<Option<Bytes>> {
        loop {
            match self.step {
                Step::Done => return Ok(None),
                Step::Next => {
                    let Some(f) = self.files.next() else {
                        self.step = Step::Done;
                        let dir = self.w.finish();
                        self.sent += dir.len() as u64;
                        tracing::info!(bytes = self.sent, "zip download finished");
                        return Ok(Some(dir.into()));
                    };
                    let obj = match self.storage.get_stream(&f.key).await {
                        Ok(Some(o)) => o,
                        Ok(None) => return Err(fail(format!("{} is missing from storage", f.entry.name))),
                        Err(e) => return Err(fail(format!("{}: {e:#}", f.entry.name))),
                    };
                    if obj.len != Some(f.entry.size) {
                        return Err(fail(format!(
                            "{} is {:?} bytes in storage, expected {}",
                            f.entry.name, obj.len, f.entry.size
                        )));
                    }
                    let header = self.w.begin(&f.entry);
                    self.cur = Some(obj);
                    self.step = Step::Data;
                    self.sent += header.len() as u64;
                    return Ok(Some(header.into()));
                }
                Step::Data => {
                    let obj = self.cur.as_mut().expect("an entry is open");
                    match obj.next_chunk().await {
                        Ok(Some(chunk)) => {
                            if chunk.is_empty() {
                                continue;
                            }
                            self.w.update(&chunk);
                            self.sent += chunk.len() as u64;
                            return Ok(Some(chunk));
                        }
                        Ok(None) => {
                            let desc = self.w.end().map_err(fail)?;
                            self.cur = None;
                            self.step = Step::Next;
                            self.sent += desc.len() as u64;
                            return Ok(Some(desc.into()));
                        }
                        Err(e) => return Err(fail(format!("{e:#}"))),
                    }
                }
            }
        }
    }
}

/// The slot for one public archive (`share_public.rs`), apart from the owner's so a link cannot starve the owner.
pub(super) fn public_slot() -> Result<SemaphorePermit<'static>, ApiError> {
    static PUBLIC_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    PUBLIC_SLOTS.try_acquire().map_err(|_| busy())
}
