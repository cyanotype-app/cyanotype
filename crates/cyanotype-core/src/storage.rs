use std::time::Duration;

use anyhow::{Context, Result};
use aws_sdk_s3::{
    Client,
    config::{Credentials, Region},
    presigning::PresigningConfig,
    types::{CompletedMultipartUpload, CompletedPart, Delete, ObjectIdentifier},
};

use crate::config::S3Config;

#[derive(Clone)]
pub struct Storage {
    /// Used for server-side calls (HEAD, multipart create/complete).
    pub(crate) client: Client,
    /// Used only to presign, so URLs carry the client-reachable host.
    presign_client: Client,
    pub(crate) bucket: String,
    ttl: Duration,
}

fn build_client(c: &S3Config, endpoint: Option<&str>) -> Client {
    let mut b = aws_sdk_s3::Config::builder()
        .behavior_version_latest()
        .region(Region::new(c.region.clone()))
        .credentials_provider(Credentials::new(&c.access_key, &c.secret_key, None, None, "static"))
        .force_path_style(c.force_path_style);
    if let Some(e) = endpoint {
        b = b.endpoint_url(e);
    }
    Client::from_conf(b.build())
}

#[derive(Debug, Clone)]
pub struct PresignedPart {
    pub number: i32,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct UploadedPart {
    pub number: i32,
    pub etag: String,
    pub size: i64,
}

/// The outcome of [`Storage::get_range`].
#[derive(Debug)]
pub enum RangeRead {
    Found {
        body: bytes::Bytes,
        /// The store's `Content-Range` (`bytes 0-99/12345`).
        content_range: Option<String>,
        etag: Option<String>,
    },
    NotFound,
    /// The range starts past the end of the object.
    Unsatisfiable,
    /// `if_match` no longer matches.
    PreconditionFailed,
}

/// One stored version of a key, or a delete marker. `version_id` is None on stores without versioning
/// support (see [`Storage::list_versions`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectVersion {
    pub key: String,
    pub version_id: Option<String>,
}

/// `DeleteObjects` accepts at most this many keys per request.
pub const DELETE_BATCH: usize = 1000;

/// Whether a failed `ListObjectVersions` means "this store does not do versions" (as opposed to a real error).
fn is_unsupported(status: Option<u16>, code: Option<&str>) -> bool {
    status == Some(501) || matches!(code, Some("NotImplemented" | "XNotImplemented"))
}

impl Storage {
    pub fn new(cfg: &S3Config) -> Self {
        Self {
            client: build_client(cfg, cfg.endpoint.as_deref()),
            presign_client: build_client(cfg, cfg.public_endpoint.as_deref()),
            bucket: cfg.bucket.clone(),
            ttl: Duration::from_secs(cfg.presign_ttl_secs),
        }
    }

    /// Start a multipart upload. `metadata` becomes the object's user metadata (`x-amz-meta-*`) once the upload
    /// completes; see `objmeta`. Clients never send it: presigned part URLs carry no metadata.
    pub async fn create_multipart(
        &self,
        key: &str,
        content_type: &str,
        metadata: std::collections::HashMap<String, String>,
    ) -> Result<String> {
        let out = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .set_metadata(Some(metadata))
            .send()
            .await
            .context("create_multipart_upload")?;
        out.upload_id.context("S3 returned no upload id")
    }

    /// Presign `PUT` URLs for the given part numbers (1-based).
    pub async fn presign_parts(
        &self,
        key: &str,
        upload_id: &str,
        numbers: impl IntoIterator<Item = i32>,
    ) -> Result<Vec<PresignedPart>> {
        let mut out = Vec::new();
        for number in numbers {
            let req = self
                .presign_client
                .upload_part()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(upload_id)
                .part_number(number)
                .presigned(PresigningConfig::expires_in(self.ttl)?)
                .await
                .context("presign upload_part")?;
            out.push(PresignedPart {
                number,
                url: req.uri().to_string(),
            });
        }
        Ok(out)
    }

    /// Upload one part from memory (used by the backend-proxied upload path). Returns the part's ETag.
    /// `Bytes` is replayable, so the SDK can sign it and retry transient failures.
    pub async fn upload_part(&self, key: &str, upload_id: &str, number: i32, body: bytes::Bytes) -> Result<String> {
        let out = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(number)
            .content_length(body.len() as i64)
            .body(body.into())
            .send()
            .await
            .context("upload_part")?;
        out.e_tag.context("S3 returned no ETag for the part")
    }

    pub async fn complete_multipart(&self, key: &str, upload_id: &str, parts: &[(i32, String)]) -> Result<()> {
        let parts = parts
            .iter()
            .map(|(n, etag)| CompletedPart::builder().part_number(*n).e_tag(etag).build())
            .collect::<Vec<_>>();
        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(parts)).build())
            .send()
            .await
            .context("complete_multipart_upload")?;
        Ok(())
    }

    /// Parts already stored for a multipart upload as `(number, etag, size)`, or `None` if S3 no longer knows
    /// the upload (aborted, completed or expired).
    pub async fn list_parts(&self, key: &str, upload_id: &str) -> Result<Option<Vec<UploadedPart>>> {
        let mut out = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let res = self
                .client
                .list_parts()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(upload_id)
                .set_part_number_marker(marker.clone())
                .send()
                .await;
            let page = match res {
                Ok(p) => p,
                Err(e)
                    if e.as_service_error()
                        .is_some_and(|s| s.meta().code() == Some("NoSuchUpload")) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e).context("list_parts"),
            };
            for p in page.parts() {
                if let (Some(number), Some(etag), Some(size)) = (p.part_number, p.e_tag(), p.size) {
                    out.push(UploadedPart {
                        number,
                        etag: etag.to_string(),
                        size,
                    });
                }
            }
            match (page.is_truncated, page.next_part_number_marker) {
                (Some(true), Some(m)) => marker = Some(m),
                _ => return Ok(Some(out)),
            }
        }
    }

    /// Abort a multipart upload. Already-gone uploads are not an error.
    pub async fn abort_multipart(&self, key: &str, upload_id: &str) -> Result<()> {
        match self
            .client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(e)
                if e.as_service_error()
                    .is_some_and(|s| s.meta().code() == Some("NoSuchUpload")) =>
            {
                Ok(())
            }
            Err(e) => Err(e).context("abort_multipart_upload"),
        }
    }

    /// Size of the object, or None if it does not exist.
    pub async fn head_size(&self, key: &str) -> Result<Option<i64>> {
        match self.client.head_object().bucket(&self.bucket).key(key).send().await {
            Ok(o) => Ok(o.content_length),
            Err(e) if e.as_service_error().is_some_and(|s| s.is_not_found()) => Ok(None),
            Err(e) => Err(e).context("head_object"),
        }
    }

    /// Download an object fully into memory, refusing anything larger than `max_bytes`
    /// (checked against the response's Content-Length before reading the body, and again
    /// against the bytes actually received).
    pub async fn get_bytes_limited(&self, key: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let out = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .context("get_object")?;
        if let Some(len) = out.content_length
            && len as u64 > max_bytes
        {
            anyhow::bail!("object {key} is {len} bytes, over the {max_bytes} byte limit");
        }
        let bytes = out.body.collect().await.context("read object body")?.into_bytes();
        if bytes.len() as u64 > max_bytes {
            anyhow::bail!("object {key} exceeded the {max_bytes} byte limit");
        }
        Ok(bytes.to_vec())
    }

    /// One byte range of an object (`range` is an HTTP range such as `bytes=0-16383`), for serving map tiles out of
    /// a PMTiles archive. The caller bounds the length; the body is buffered. `if_match` makes the store answer 412
    /// when the object changed since the client first read it.
    pub async fn get_range(&self, key: &str, range: &str, if_match: Option<&str>) -> Result<RangeRead> {
        let mut req = self.client.get_object().bucket(&self.bucket).key(key).range(range);
        if let Some(e) = if_match {
            req = req.if_match(e);
        }
        match req.send().await {
            Ok(o) => {
                let content_range = o.content_range.clone();
                let etag = o.e_tag.clone();
                let body = o.body.collect().await.context("read object range")?.into_bytes();
                Ok(RangeRead::Found {
                    body,
                    content_range,
                    etag,
                })
            }
            Err(e) => match (
                e.raw_response().map(|r| r.status().as_u16()),
                e.as_service_error().is_some_and(|s| s.is_no_such_key()),
            ) {
                (_, true) | (Some(404), _) => Ok(RangeRead::NotFound),
                (Some(416), _) => Ok(RangeRead::Unsatisfiable),
                (Some(412), _) => Ok(RangeRead::PreconditionFailed),
                _ => Err(e).context("get_object (range)"),
            },
        }
    }

    /// Download an object fully into memory (no size cap; prefer `get_bytes_limited`).
    pub async fn get_bytes(&self, key: &str) -> Result<Vec<u8>> {
        self.get_bytes_limited(key, u64::MAX).await
    }

    /// Upload (or overwrite) an object from memory.
    pub async fn put_bytes(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(bytes.into())
            .send()
            .await
            .context("put_object")?;
        Ok(())
    }

    pub async fn presign_get(&self, key: &str) -> Result<String> {
        let req = self
            .presign_client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(PresigningConfig::expires_in(self.ttl)?)
            .await
            .context("presign get_object")?;
        Ok(req.uri().to_string())
    }

    /// Like [`Storage::presign_get`] with a shorter life (`ttl`, clamped to the configured presign TTL): for URLs that
    /// go to people who are not signed in (share links).
    pub async fn presign_get_ttl(&self, key: &str, ttl: Duration) -> Result<String> {
        let req = self
            .presign_client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(PresigningConfig::expires_in(ttl.min(self.ttl))?)
            .await
            .context("presign get_object")?;
        Ok(req.uri().to_string())
    }

    /// Presigned GET that makes the browser save the object instead of showing it: the signed
    /// `response-content-disposition` is `attachment` with the given file name, and the URL lives only `ttl`
    /// (clamped to the configured presign TTL). A top-level navigation or `<a>` click follows it, so the bucket
    /// needs no CORS policy and the bytes never pass through the API.
    pub async fn presign_get_download(&self, key: &str, filename: &str, ttl: Duration) -> Result<String> {
        let req = self
            .presign_client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .response_content_disposition(content_disposition_attachment(filename))
            .presigned(PresigningConfig::expires_in(ttl.min(self.ttl))?)
            .await
            .context("presign get_object (download)")?;
        Ok(req.uri().to_string())
    }

    /// Every version and delete marker of every key that starts with `prefix`, across all pages. On a store
    /// that cannot list versions (HTTP 501 / `NotImplemented`) this degrades to a plain listing with
    /// `version_id: None`, which [`Storage::delete_versions`] then deletes without a version id.
    pub async fn list_versions(&self, prefix: &str) -> Result<Vec<ObjectVersion>> {
        let mut out = Vec::new();
        let mut key_marker: Option<String> = None;
        let mut version_marker: Option<String> = None;
        loop {
            let res = self
                .client
                .list_object_versions()
                .bucket(&self.bucket)
                .prefix(prefix)
                .set_key_marker(key_marker.clone())
                .set_version_id_marker(version_marker.clone())
                .send()
                .await;
            let page = match res {
                Ok(p) => p,
                Err(e) => {
                    let status = e.raw_response().map(|r| r.status().as_u16());
                    let code = e.as_service_error().and_then(|s| s.meta().code());
                    if out.is_empty() && is_unsupported(status, code) {
                        tracing::warn!(
                            prefix,
                            "ListObjectVersions unsupported; falling back to a plain listing"
                        );
                        return self.list_unversioned(prefix).await;
                    }
                    return Err(e).context("list_object_versions");
                }
            };
            for v in page.versions() {
                if let Some(key) = v.key() {
                    out.push(ObjectVersion {
                        key: key.to_string(),
                        version_id: v.version_id().map(str::to_string),
                    });
                }
            }
            for m in page.delete_markers() {
                if let Some(key) = m.key() {
                    out.push(ObjectVersion {
                        key: key.to_string(),
                        version_id: m.version_id().map(str::to_string),
                    });
                }
            }
            if page.is_truncated == Some(true) {
                key_marker = page.next_key_marker;
                version_marker = page.next_version_id_marker;
                if key_marker.is_none() && version_marker.is_none() {
                    return Ok(out);
                }
            } else {
                return Ok(out);
            }
        }
    }

    async fn list_unversioned(&self, prefix: &str) -> Result<Vec<ObjectVersion>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .set_continuation_token(token.clone())
                .send()
                .await
                .context("list_objects_v2")?;
            out.extend(page.contents().iter().filter_map(|o| {
                o.key().map(|k| ObjectVersion {
                    key: k.to_string(),
                    version_id: None,
                })
            }));
            match (page.is_truncated, page.next_continuation_token) {
                (Some(true), Some(t)) => token = Some(t),
                _ => return Ok(out),
            }
        }
    }

    /// Permanently delete exactly these versions / markers, at most [`DELETE_BATCH`] per request. A version
    /// id of None deletes the key without one (the unversioned fallback). Any per-object failure is an error,
    /// and the caller retries the whole set (deleting something already gone succeeds). Returns how many were
    /// deleted.
    pub async fn delete_versions(&self, versions: &[ObjectVersion]) -> Result<usize> {
        for chunk in versions.chunks(DELETE_BATCH) {
            let ids = chunk
                .iter()
                .map(|v| {
                    ObjectIdentifier::builder()
                        .key(&v.key)
                        .set_version_id(v.version_id.clone())
                        .build()
                        .context("object identifier")
                })
                .collect::<Result<Vec<_>>>()?;
            let delete = Delete::builder().set_objects(Some(ids)).quiet(true).build()?;
            match self
                .client
                .delete_objects()
                .bucket(&self.bucket)
                .delete(delete)
                .send()
                .await
            {
                Ok(out) => {
                    if let Some(e) = out.errors().first() {
                        anyhow::bail!(
                            "delete_objects failed for {} object(s), first: {} {}",
                            out.errors().len(),
                            e.code().unwrap_or("?"),
                            e.message().unwrap_or("")
                        );
                    }
                }
                Err(e) => {
                    // Some S3-compatible stores reject the bulk call (checksum headers, 501). Fall back to
                    // one DeleteObject per version so permanent deletion still works there.
                    tracing::warn!(error = ?e, "delete_objects rejected; deleting one by one");
                    for v in chunk {
                        self.client
                            .delete_object()
                            .bucket(&self.bucket)
                            .key(&v.key)
                            .set_version_id(v.version_id.clone())
                            .send()
                            .await
                            .context("delete_object")?;
                    }
                }
            }
        }
        Ok(versions.len())
    }

    /// Permanently delete every version and delete marker of each exact key in `keys`. Nothing else that merely
    /// shares a prefix with a key is touched. Returns the number of versions/markers removed.
    pub async fn delete_all_versions(&self, keys: &[String]) -> Result<usize> {
        let mut all = Vec::new();
        for key in keys {
            all.extend(self.list_versions(key).await?.into_iter().filter(|v| &v.key == key));
        }
        self.delete_versions(&all).await
    }

    /// Permanently delete every version and delete marker of every key under `prefix`.
    pub async fn delete_prefix_all_versions(&self, prefix: &str) -> Result<usize> {
        let all = self.list_versions(prefix).await?;
        self.delete_versions(&all).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_listing_is_detected() {
        assert!(is_unsupported(Some(501), None));
        assert!(is_unsupported(None, Some("NotImplemented")));
        assert!(!is_unsupported(Some(403), Some("AccessDenied")));
        assert!(!is_unsupported(None, None));
    }
}

/// `Content-Disposition: attachment` for `filename`. A plain `filename="..."` for ASCII names; otherwise an ASCII
/// fallback plus the RFC 5987 `filename*` so non-Latin names survive. Quotes, backslashes and control characters
/// are never emitted raw (no header injection through a crafted upload name).
pub fn content_disposition_attachment(filename: &str) -> String {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let clean: String = base.chars().filter(|c| !c.is_control()).collect();
    let clean = if clean.trim().is_empty() {
        "download".to_string()
    } else {
        clean
    };
    let ascii: String = clean
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' && c != '%' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if ascii == clean {
        return format!("attachment; filename=\"{ascii}\"");
    }
    let mut encoded = String::new();
    for b in clean.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod disposition_tests {
    use super::content_disposition_attachment as cd;

    #[test]
    fn plain_names_are_quoted_as_is() {
        assert_eq!(cd("IMG_1234.CR3"), "attachment; filename=\"IMG_1234.CR3\"");
        assert_eq!(cd("my photo (1).jpg"), "attachment; filename=\"my photo (1).jpg\"");
    }

    #[test]
    fn quotes_paths_and_controls_are_neutralised() {
        assert_eq!(
            cd("a\"b.jpg"),
            "attachment; filename=\"a_b.jpg\"; filename*=UTF-8''a%22b.jpg"
        );
        assert_eq!(cd("../../etc/passwd"), "attachment; filename=\"passwd\"");
        assert_eq!(
            cd("x\r\nSet-Cookie: a=b.jpg"),
            "attachment; filename=\"xSet-Cookie: a=b.jpg\""
        );
        assert_eq!(cd(""), "attachment; filename=\"download\"");
    }

    #[test]
    fn non_ascii_names_get_an_rfc5987_form() {
        assert_eq!(
            cd("Zürich 100%.jpg"),
            "attachment; filename=\"Z_rich 100_.jpg\"; filename*=UTF-8''Z%C3%BCrich%20100%25.jpg"
        );
    }
}
