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

    pub async fn presign_parts(&self, key: &str, upload_id: &str, parts: i32) -> Result<Vec<PresignedPart>> {
        let mut out = Vec::with_capacity(parts as usize);
        for number in 1..=parts {
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

    pub async fn abort_multipart(&self, key: &str, upload_id: &str) -> Result<()> {
        self.client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .send()
            .await
            .context("abort_multipart_upload")?;
        Ok(())
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
