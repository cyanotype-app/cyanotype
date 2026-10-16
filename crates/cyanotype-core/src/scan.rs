//! Read-only bucket scanning helpers for `reindex`, plus the one write it can opt into (`copy_onto_self`).
use std::collections::HashMap;

use anyhow::{Context, Result};
use aws_sdk_s3::types::MetadataDirective;
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
}
