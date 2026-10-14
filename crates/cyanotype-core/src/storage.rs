use std::time::Duration;

use anyhow::{Context, Result};
use aws_sdk_s3::{
    Client,
    config::{Credentials, Region},
    presigning::PresigningConfig,
    types::{CompletedMultipartUpload, CompletedPart},
};

use crate::config::S3Config;

#[derive(Clone)]
pub struct Storage {
    /// Used for server-side calls (HEAD, multipart create/complete).
    client: Client,
    /// Used only to presign, so URLs carry the client-reachable host.
    presign_client: Client,
    bucket: String,
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

impl Storage {
    pub fn new(cfg: &S3Config) -> Self {
        Self {
            client: build_client(cfg, cfg.endpoint.as_deref()),
            presign_client: build_client(cfg, cfg.public_endpoint.as_deref()),
            bucket: cfg.bucket.clone(),
            ttl: Duration::from_secs(cfg.presign_ttl_secs),
        }
    }

    pub async fn create_multipart(&self, key: &str, content_type: &str) -> Result<String> {
        let out = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
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
}
