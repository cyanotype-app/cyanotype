//! Per-user recovery manifests: `manifests/{user}/originals-YYYYMMDD.jsonl`.
//!
//! Object metadata (`objmeta`) makes each original self-describing from its upload on, but it is write-once: it
//! cannot say that a photo was later trashed, favorited or put in an album, and objects uploaded before it existed
//! have none. The manifest carries that state. Once a day the assets service writes, per user, one JSON object per
//! line: a `meta` header, every `asset` (not mid-purge) with its id, hash, name, timestamps, trash state, favorite
//! flag and RAW/JPEG partner, and every `album` with its photos by hash. `reindex` reads them to restore what object
//! keys and metadata alone cannot.
//!
//! Dated files, not one rolling file, on purpose: if the database is restored from an old dump and the assets
//! service then writes a (stale) manifest, only that day's file is replaced and the earlier ones are still there.
//! `reindex` merges all of a user's manifests, newest first, and the first entry for a photo wins. The newest
//! `MANIFEST_KEEP` files are kept; older ones are deleted (they are tiny and regenerable, unlike originals).
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{keys, storage::Storage};

pub const FORMAT_VERSION: u32 = 1;
pub const DEFAULT_KEEP: usize = 14;
const MAX_MANIFEST_BYTES: u64 = 1 << 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Line {
    Meta(MetaLine),
    Asset(AssetLine),
    Album(AlbumLine),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaLine {
    pub v: u32,
    pub user: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
    pub assets: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetLine {
    pub id: Uuid,
    /// Lowercase hex.
    pub sha256: String,
    pub key: String,
    pub name: String,
    pub content_type: String,
    pub size_bytes: i64,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub created_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub file_modified_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub taken_at: Option<OffsetDateTime>,
    /// In the trash since this time.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub deleted_at: Option<OffsetDateTime>,
    /// Absent when the database has no favorites column (older schema).
    #[serde(default)]
    pub favorite: Option<bool>,
    /// Hash of the JPEG/HEIC this RAW is stacked behind.
    #[serde(default)]
    pub pair_primary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumLine {
    pub id: Uuid,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Hash of the cover photo.
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub created_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
    /// Member photos by hash.
    #[serde(default)]
    pub items: Vec<String>,
}

/// A user's manifests merged: newest manifest first, the first entry for a photo (or album) wins.
#[derive(Debug, Default)]
pub struct UserManifest {
    pub assets: HashMap<[u8; 32], AssetLine>,
    pub albums: BTreeMap<Uuid, AlbumLine>,
    pub files: usize,
    pub bad_lines: usize,
}

pub fn today_utc() -> String {
    let d = OffsetDateTime::now_utc().date();
    format!("{:04}{:02}{:02}", d.year(), d.month() as u8, d.day())
}

/// `YYYYMMDD` of a manifest key, if it is one.
pub fn key_date(key: &str) -> Option<&str> {
    let d = key
        .rsplit('/')
        .next()?
        .strip_prefix("originals-")?
        .strip_suffix(".jsonl")?;
    (d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit())).then_some(d)
}

/// Build the manifest of one user from the database. Returns the JSONL bytes and the number of assets.
pub async fn build(db: &PgPool, user: Uuid) -> Result<(Vec<u8>, u64)> {
    let mut body: Vec<Line> = Vec::new();
    let rows = sqlx::query(
        "SELECT a.id, a.sha256, a.original_key, a.original_name, a.content_type, a.size_bytes, a.created_at,
                a.file_modified_at, a.taken_at, a.deleted_at,
                (to_jsonb(a) ->> 'favorite')::boolean AS favorite, p.sha256 AS pair_primary
         FROM assets a LEFT JOIN assets p ON p.id = a.primary_id
         WHERE a.owner_id = $1 AND a.purging_at IS NULL ORDER BY a.created_at, a.id",
    )
    .bind(user)
    .fetch_all(db)
    .await
    .context("read assets")?;
    for r in &rows {
        let sha: Vec<u8> = r.try_get("sha256")?;
        let pair: Option<Vec<u8>> = r.try_get("pair_primary")?;
        body.push(Line::Asset(AssetLine {
            id: r.try_get("id")?,
            sha256: keys::hex(&sha),
            key: r.try_get("original_key")?,
            name: r.try_get("original_name")?,
            content_type: r.try_get("content_type")?,
            size_bytes: r.try_get("size_bytes")?,
            created_at: r.try_get("created_at")?,
            file_modified_at: r.try_get("file_modified_at")?,
            taken_at: r.try_get("taken_at")?,
            deleted_at: r.try_get("deleted_at")?,
            favorite: r.try_get("favorite")?,
            pair_primary: pair.map(|p| keys::hex(&p)),
        }));
    }
    let n_assets = rows.len() as u64;

    let mut items: HashMap<Uuid, Vec<String>> = HashMap::new();
    for r in sqlx::query(
        "SELECT aa.album_id, a.sha256 FROM album_assets aa
         JOIN albums al ON al.id = aa.album_id JOIN assets a ON a.id = aa.asset_id
         WHERE al.owner_id = $1 AND a.purging_at IS NULL ORDER BY aa.album_id, aa.added_at, a.id",
    )
    .bind(user)
    .fetch_all(db)
    .await
    .context("read album items")?
    {
        let sha: Vec<u8> = r.try_get("sha256")?;
        items.entry(r.try_get("album_id")?).or_default().push(keys::hex(&sha));
    }
    for r in sqlx::query(
        "SELECT al.id, al.title, al.description, al.created_at, al.updated_at, c.sha256 AS cover
         FROM albums al LEFT JOIN assets c ON c.id = al.cover_asset_id AND c.purging_at IS NULL
         WHERE al.owner_id = $1 ORDER BY al.created_at, al.id",
    )
    .bind(user)
    .fetch_all(db)
    .await
    .context("read albums")?
    {
        let id: Uuid = r.try_get("id")?;
        let cover: Option<Vec<u8>> = r.try_get("cover")?;
        body.push(Line::Album(AlbumLine {
            id,
            title: r.try_get("title")?,
            description: r.try_get("description")?,
            cover: cover.map(|c| keys::hex(&c)),
            created_at: r.try_get("created_at")?,
            updated_at: r.try_get("updated_at")?,
            items: items.remove(&id).unwrap_or_default(),
        }));
    }

    let mut out = Vec::new();
    let header = Line::Meta(MetaLine {
        v: FORMAT_VERSION,
        user,
        generated_at: OffsetDateTime::now_utc(),
        assets: n_assets,
    });
    for line in std::iter::once(&header).chain(body.iter()) {
        serde_json::to_writer(&mut out, line)?;
        out.push(b'\n');
    }
    Ok((out, n_assets))
}

/// Parse manifest JSONL. Unparseable lines (and unknown future line types) are counted, not fatal.
pub fn parse(bytes: &[u8]) -> (Vec<Line>, usize) {
    let (mut lines, mut bad) = (Vec::new(), 0);
    for raw in bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
    {
        match serde_json::from_slice::<Line>(raw) {
            Ok(l) => lines.push(l),
            Err(_) => bad += 1,
        }
    }
    (lines, bad)
}

/// All manifest keys under `manifests/`, grouped by user, each list newest first.
pub async fn list_all(storage: &Storage) -> Result<BTreeMap<Uuid, Vec<String>>> {
    let mut by_user: BTreeMap<Uuid, Vec<String>> = BTreeMap::new();
    let mut token = None;
    loop {
        let page = storage.list_page(keys::MANIFESTS_PREFIX, token).await?;
        for o in page.objects {
            let user = o
                .key
                .strip_prefix(keys::MANIFESTS_PREFIX)
                .and_then(|r| r.split('/').next())
                .and_then(|u| Uuid::parse_str(u).ok());
            if let (Some(u), Some(_)) = (user, key_date(&o.key)) {
                by_user.entry(u).or_default().push(o.key);
            }
        }
        match page.next {
            Some(t) => token = Some(t),
            None => break,
        }
    }
    for v in by_user.values_mut() {
        v.sort_by(|a, b| b.cmp(a)); // YYYYMMDD sorts lexically
    }
    Ok(by_user)
}

/// Fetch and merge the given manifest keys (newest first, as returned by [`list_all`]).
pub async fn load(storage: &Storage, user: Uuid, manifest_keys: &[String]) -> Result<UserManifest> {
    let mut m = UserManifest::default();
    for key in manifest_keys {
        let bytes = storage.get_bytes_limited(key, MAX_MANIFEST_BYTES).await?;
        let (lines, bad) = parse(&bytes);
        m.files += 1;
        m.bad_lines += bad;
        for line in lines {
            match line {
                Line::Meta(h) if h.user != user => {
                    tracing::warn!(key, "manifest header names another user; skipping the file's data");
                    break;
                }
                Line::Asset(a) => {
                    if let Some(sha) = keys::parse_sha256(&a.sha256) {
                        m.assets.entry(sha).or_insert(a);
                    } else {
                        m.bad_lines += 1;
                    }
                }
                Line::Album(a) => {
                    m.albums.entry(a.id).or_insert(a);
                }
                Line::Meta(_) => {}
            }
        }
    }
    Ok(m)
}

/// Write today's manifest for one user and prune to the newest `keep`. Returns the number of assets written.
pub async fn write_user(db: &PgPool, storage: &Storage, user: Uuid, keep: usize) -> Result<u64> {
    let (bytes, n) = build(db, user).await?;
    let key = keys::manifest(user, &today_utc());
    storage.put_bytes(&key, bytes, "application/x-ndjson").await?;
    let mut existing: Vec<String> = Vec::new();
    let mut token = None;
    loop {
        let page = storage.list_page(&keys::manifest_prefix(user), token).await?;
        existing.extend(
            page.objects
                .into_iter()
                .map(|o| o.key)
                .filter(|k| key_date(k).is_some()),
        );
        match page.next {
            Some(t) => token = Some(t),
            None => break,
        }
    }
    existing.sort_by(|a, b| b.cmp(a));
    if existing.len() > keep {
        storage.delete_all_versions(&existing[keep..]).await?;
    }
    Ok(n)
}

/// Write today's manifest for every user that has photos and does not have one yet. Returns users written.
pub async fn write_missing(db: &PgPool, storage: &Storage, keep: usize) -> Result<usize> {
    let have = list_all(storage).await?;
    let today = today_utc();
    let users: Vec<(Uuid,)> = sqlx::query_as("SELECT DISTINCT owner_id FROM assets WHERE purging_at IS NULL")
        .fetch_all(db)
        .await?;
    let mut written = 0;
    for (user,) in users {
        if have
            .get(&user)
            .is_some_and(|ks| ks.iter().any(|k| key_date(k) == Some(today.as_str())))
        {
            continue;
        }
        match write_user(db, storage, user, keep).await {
            Ok(n) => {
                tracing::info!(user_id = %user, assets = n, "wrote recovery manifest");
                written += 1;
            }
            Err(e) => tracing::warn!(user_id = %user, error = ?e, "could not write recovery manifest"),
        }
    }
    Ok(written)
}

/// Background task: daily manifests. The first pass waits `startup_delay` so a freshly restored database can be
/// reconciled with `reindex` before anything is written from it; `keep` of 0 disables the task.
pub fn spawn(db: PgPool, storage: Storage, keep: usize, startup_delay: Duration) {
    if keep == 0 {
        tracing::info!("recovery manifests disabled (MANIFEST_KEEP=0)");
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(startup_delay).await;
        loop {
            if let Err(e) = write_missing(&db, &storage, keep).await {
                tracing::warn!(error = ?e, "recovery manifest pass failed");
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

/// `MANIFEST_KEEP`: how many daily manifests to keep per user (default 14; 0 turns the daily job off).
pub fn keep_from_env() -> Result<usize> {
    match std::env::var("MANIFEST_KEEP").ok().filter(|v| !v.is_empty()) {
        None => Ok(DEFAULT_KEEP),
        Some(v) => v
            .trim()
            .parse()
            .with_context(|| format!("MANIFEST_KEEP must be a whole number, got {v:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_dates() {
        let u = Uuid::nil();
        assert_eq!(key_date(&keys::manifest(u, "20260131")), Some("20260131"));
        assert_eq!(key_date("manifests/x/originals-2026013.jsonl"), None);
        assert_eq!(key_date("manifests/x/other.jsonl"), None);
    }

    #[test]
    fn lines_round_trip_and_tolerate_noise() {
        let a = AssetLine {
            id: Uuid::new_v4(),
            sha256: "ab".repeat(32),
            key: "k".into(),
            name: "Süd \"x\".jpg".into(),
            content_type: "image/jpeg".into(),
            size_bytes: 3,
            created_at: Some(time::macros::datetime!(2026-01-02 03:04:05 UTC)),
            file_modified_at: None,
            taken_at: None,
            deleted_at: None,
            favorite: Some(true),
            pair_primary: None,
        };
        let mut buf = serde_json::to_vec(&Line::Asset(a.clone())).unwrap();
        buf.extend_from_slice(b"\n\nnot json\n{\"t\":\"from_the_future\"}\n");
        let (lines, bad) = parse(&buf);
        assert_eq!(bad, 2);
        assert!(matches!(&lines[..], [Line::Asset(b)] if *b == a));
    }
}
