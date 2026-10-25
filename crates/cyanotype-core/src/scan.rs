//! Read-only bucket scanning helpers for `reindex` and the integrity check, plus the two writes they can make
//! (`copy_onto_self`, `restore_version`).
use std::collections::HashMap;

use anyhow::{Context, Result};
use aws_sdk_s3::types::MetadataDirective;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::storage::Storage;

/// One current object from a listing.
#[derive(Debug, Clone)]
pub struct Listed {
    pub key: String,
    pub size: i64,
    pub last_modified: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct ListPage {
    pub objects: Vec<Listed>,
    /// Pass back to continue; None at the end.
    pub next: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Head {
    pub size: i64,
    pub content_type: Option<String>,
    pub metadata: HashMap<String, String>,
    pub last_modified: Option<OffsetDateTime>,
}

/// One stored version of one key (or its delete marker), newest first in [`Storage::key_versions`].
#[derive(Debug, Clone)]
pub struct KeyVersion {
    pub version_id: Option<String>,
    pub size: i64,
    pub is_latest: bool,
    pub delete_marker: bool,
    pub last_modified: Option<OffsetDateTime>,
}

/// The outcome of [`Storage::hash_object`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hashed {
    pub sha256: [u8; 32],
    /// Bytes read; at most `limit + 1` (reading stops one past the limit, so a longer object is seen as longer without
    /// being read to its end).
    pub bytes: u64,
}

fn to_time(t: &aws_sdk_s3::primitives::DateTime) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(t.secs()).ok()
}

/// `CopyObject` cannot copy sources above 5 GiB.
pub const MAX_COPY_BYTES: i64 = 5 * 1024 * 1024 * 1024;

impl Storage {
    /// One page (up to 1000 keys) of `ListObjectsV2` under `prefix`. On a versioned bucket this lists current
    /// versions only: objects whose newest version is a delete marker do not appear.
    pub async fn list_page(&self, prefix: &str, token: Option<String>) -> Result<ListPage> {
        let page = self
            .client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(prefix)
            .set_continuation_token(token)
            .send()
            .await
            .context("list_objects_v2")?;
        let objects = page
            .contents()
            .iter()
            .filter_map(|o| {
                Some(Listed {
                    key: o.key()?.to_string(),
                    size: o.size().unwrap_or(0),
                    last_modified: o.last_modified().and_then(to_time),
                })
            })
            .collect();
        let next = match (page.is_truncated, page.next_continuation_token) {
            (Some(true), Some(t)) => Some(t),
            _ => None,
        };
        Ok(ListPage { objects, next })
    }

    /// HEAD an object: None if it does not exist (or its newest version is a delete marker).
    pub async fn head(&self, key: &str) -> Result<Option<Head>> {
        match self.client.head_object().bucket(&self.bucket).key(key).send().await {
            Ok(o) => Ok(Some(Head {
                size: o.content_length.unwrap_or(0),
                content_type: o.content_type,
                metadata: o.metadata.unwrap_or_default(),
                last_modified: o.last_modified.as_ref().and_then(to_time),
            })),
            Err(e) if e.as_service_error().is_some_and(|s| s.is_not_found()) => Ok(None),
            Err(e) => Err(e).context("head_object"),
        }
    }

    /// Replace an object's user metadata by copying it onto itself (`MetadataDirective=REPLACE`).
    ///
    /// On a versioned bucket this writes a NEW VERSION holding a full copy of the bytes: the object's storage is
    /// doubled until the previous version is removed. The caller must pass the content type to keep, since REPLACE
    /// also replaces it. Keys are ASCII path characters (see `keys`); anything else is refused.
    pub async fn copy_onto_self(&self, key: &str, content_type: &str, metadata: HashMap<String, String>) -> Result<()> {
        anyhow::ensure!(
            key.bytes().all(|b| b.is_ascii_alphanumeric() || b"/-._".contains(&b)),
            "refusing to copy unexpected key {key:?}"
        );
        self.client
            .copy_object()
            .bucket(&self.bucket)
            .key(key)
            .copy_source(format!("{}/{key}", self.bucket))
            .metadata_directive(MetadataDirective::Replace)
            .content_type(content_type)
            .set_metadata(Some(metadata))
            .send()
            .await
            .context("copy_object")?;
        Ok(())
    }

    /// Every version and delete marker of exactly `key`, newest first. A store that cannot list versions answers with
    /// the current object alone (no version id).
    pub async fn key_versions(&self, key: &str) -> Result<Vec<KeyVersion>> {
        let mut out = Vec::new();
        let mut key_marker: Option<String> = None;
        let mut version_marker: Option<String> = None;
        loop {
            let res = self
                .client
                .list_object_versions()
                .bucket(&self.bucket)
                .prefix(key)
                .set_key_marker(key_marker.clone())
                .set_version_id_marker(version_marker.clone())
                .send()
                .await;
            let page = match res {
                Ok(p) => p,
                Err(e) => {
                    let status = e.raw_response().map(|r| r.status().as_u16());
                    if out.is_empty() && status == Some(501) {
                        return Ok(match self.head(key).await? {
                            Some(h) => vec![KeyVersion {
                                version_id: None,
                                size: h.size,
                                is_latest: true,
                                delete_marker: false,
                                last_modified: h.last_modified,
                            }],
                            None => Vec::new(),
                        });
                    }
                    return Err(e).context("list_object_versions");
                }
            };
            for v in page.versions().iter().filter(|v| v.key() == Some(key)) {
                out.push(KeyVersion {
                    version_id: v.version_id().map(str::to_string),
                    size: v.size().unwrap_or(0),
                    is_latest: v.is_latest().unwrap_or(false),
                    delete_marker: false,
                    last_modified: v.last_modified().and_then(to_time),
                });
            }
            for m in page.delete_markers().iter().filter(|m| m.key() == Some(key)) {
                out.push(KeyVersion {
                    version_id: m.version_id().map(str::to_string),
                    size: 0,
                    is_latest: m.is_latest().unwrap_or(false),
                    delete_marker: true,
                    last_modified: m.last_modified().and_then(to_time),
                });
            }
            if page.is_truncated == Some(true)
                && (page.next_key_marker.is_some() || page.next_version_id_marker.is_some())
            {
                key_marker = page.next_key_marker;
                version_marker = page.next_version_id_marker;
            } else {
                break;
            }
        }
        // Newest first, whatever order the store listed them in.
        out.sort_by_key(|a| std::cmp::Reverse(a.last_modified));
        Ok(out)
    }

    /// SHA-256 of an object (the latest version, or `version_id`), streamed: one chunk in memory at a time, never the file.
    /// Stops after `limit + 1` bytes. None if the object (or version) does not exist.
    pub async fn hash_object(&self, key: &str, version_id: Option<&str>, limit: u64) -> Result<Option<Hashed>> {
        let res = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .set_version_id(version_id.map(str::to_string))
            .send()
            .await;
        let mut out = match res {
            Ok(o) => o,
            Err(e)
                if e.as_service_error().is_some_and(|s| s.is_no_such_key())
                    || matches!(e.raw_response().map(|r| r.status().as_u16()), Some(404)) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e).context("get_object"),
        };
        let mut hasher = Sha256::new();
        let mut bytes = 0u64;
        while let Some(chunk) = out.body.try_next().await.context("read object body")? {
            bytes += chunk.len() as u64;
            hasher.update(&chunk);
            if bytes > limit {
                break;
            }
        }
        Ok(Some(Hashed {
            sha256: hasher.finalize().into(),
            bytes,
        }))
    }

    /// Make an older version the current one again: copy it onto the key (a new version, the old one stays). Keeps its
    /// content type and user metadata. `CopyObject` cannot copy sources above 5 GiB. Keys are ASCII path characters.
    pub async fn restore_version(&self, key: &str, version_id: &str) -> Result<()> {
        anyhow::ensure!(
            key.bytes().all(|b| b.is_ascii_alphanumeric() || b"/-._".contains(&b)),
            "refusing to copy unexpected key {key:?}"
        );
        let mut encoded = String::new();
        for b in version_id.bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                encoded.push(b as char);
            } else {
                encoded.push_str(&format!("%{b:02X}"));
            }
        }
        self.client
            .copy_object()
            .bucket(&self.bucket)
            .key(key)
            .copy_source(format!("{}/{key}?versionId={encoded}", self.bucket))
            .send()
            .await
            .context("copy_object (restore a version)")?;
        Ok(())
    }
}
