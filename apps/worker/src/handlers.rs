//! Job handlers. Both are idempotent: they overwrite the same S3 keys / asset columns.
use std::sync::Arc;

use anyhow::{Context, Result};
use cyanotype_core::{jobs, keys, storage::Storage};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{exif, imaging};

/// Errors that retrying cannot fix; the job is failed immediately.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Permanent(pub String);

pub enum Outcome {
    Done,
    /// Finished without doing work (logged); the job is marked done, not failed.
    Skipped(&'static str),
}

pub struct Ctx {
    pub db: PgPool,
    pub storage: Storage,
    pub max_original_bytes: u64,
}

#[derive(sqlx::FromRow)]
struct AssetRow {
    owner_id: Uuid,
    sha256: Vec<u8>,
    original_key: String,
    original_name: String,
    content_type: String,
    size_bytes: i64,
}

pub fn is_supported(content_type: &str, filename: &str) -> bool {
    let ct = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match ct.as_str() {
        "image/jpeg" | "image/jpg" | "image/png" | "image/webp" => true,
        // Generic types (clients that don't set one): trust the extension.
        "" | "application/octet-stream" => {
            matches!(keys::sanitize_ext(filename).as_str(), "jpg" | "jpeg" | "png" | "webp")
        }
        _ => false,
    }
}

pub async fn run(ctx: &Arc<Ctx>, job: &jobs::Job) -> Result<Outcome> {
    let asset: AssetRow = sqlx::query_as(
        "SELECT owner_id, sha256, original_key, original_name, content_type, size_bytes FROM assets WHERE id = $1",
    )
    .bind(job.asset_id)
    .fetch_one(&ctx.db)
    .await
    .context("load asset")?;

    if !is_supported(&asset.content_type, &asset.original_name) {
        tracing::info!(content_type = %asset.content_type, "unsupported content type; skipping");
        return Ok(Outcome::Skipped("unsupported content type"));
    }
    if asset.size_bytes < 0 || asset.size_bytes as u64 > ctx.max_original_bytes {
        return Err(Permanent(format!(
            "original is {} bytes, over the {} byte worker limit",
            asset.size_bytes, ctx.max_original_bytes
        ))
        .into());
    }

    let bytes = ctx
        .storage
        .get_bytes_limited(&asset.original_key, ctx.max_original_bytes)
        .await
        .context("download original")?;

    match job.kind.as_str() {
        jobs::KIND_METADATA => metadata(ctx, job, &asset, bytes).await?,
        jobs::KIND_THUMBNAIL => thumbnail(ctx, job, &asset, bytes).await?,
        other => anyhow::bail!("no handler for job kind {other}"),
    }
    Ok(Outcome::Done)
}

struct Metadata {
    width: u32,
    height: u32,
    exif: exif::ExifInfo,
}

fn compute_metadata(bytes: &[u8], expected_sha: &[u8]) -> Result<Metadata> {
    let actual = Sha256::digest(bytes);
    if actual.as_slice() != expected_sha {
        return Err(Permanent(format!(
            "sha256 mismatch: expected {}, object has {}",
            keys::hex(expected_sha),
            keys::hex(&actual)
        ))
        .into());
    }
    let exif = exif::parse(bytes);
    let (w, h) = imaging::raw_dimensions(bytes).map_err(|e| Permanent(format!("{e:#}")))?;
    let (width, height) = imaging::oriented_dims(w, h, exif.orientation);
    Ok(Metadata { width, height, exif })
}

async fn metadata(ctx: &Arc<Ctx>, job: &jobs::Job, asset: &AssetRow, bytes: Vec<u8>) -> Result<()> {
    let sha = asset.sha256.clone();
    let m = tokio::task::spawn_blocking(move || compute_metadata(&bytes, &sha))
        .await
        .context("metadata task panicked")?
        .inspect_err(|e| {
            if e.is::<Permanent>() {
                tracing::error!(key = %asset.original_key, error = %e, "metadata job failed permanently (integrity or decode error)");
            }
        })?;

    let e = &m.exif;
    sqlx::query(
        "UPDATE assets SET width = $2, height = $3, taken_at = $4, latitude = $5, longitude = $6,
             camera_make = $7, camera_model = $8, lens_model = $9, exif = $10
         WHERE id = $1",
    )
    .bind(job.asset_id)
    .bind(m.width as i32)
    .bind(m.height as i32)
    .bind(e.taken_at)
    .bind(e.latitude)
    .bind(e.longitude)
    .bind(&e.camera_make)
    .bind(&e.camera_model)
    .bind(&e.lens_model)
    .bind(e.json.as_ref().map(sqlx::types::Json))
    .execute(&ctx.db)
    .await
    .context("update asset metadata")?;
    Ok(())
}

async fn thumbnail(ctx: &Arc<Ctx>, job: &jobs::Job, asset: &AssetRow, bytes: Vec<u8>) -> Result<()> {
    let d = tokio::task::spawn_blocking(move || {
        let orientation = exif::parse(&bytes).orientation;
        imaging::derive(&bytes, orientation).map_err(|e| anyhow::Error::from(Permanent(format!("{e:#}"))))
    })
    .await
    .context("thumbnail task panicked")??;
    tracing::debug!(width = d.width, height = d.height, "derived images ready");

    ctx.storage
        .put_bytes(&keys::thumb(asset.owner_id, job.asset_id), d.thumb_webp, "image/webp")
        .await?;
    ctx.storage
        .put_bytes(
            &keys::preview(asset.owner_id, job.asset_id),
            d.preview_webp,
            "image/webp",
        )
        .await?;
    sqlx::query("UPDATE assets SET thumbhash = $2 WHERE id = $1")
        .bind(job.asset_id)
        .bind(&d.thumbhash)
        .execute(&ctx.db)
        .await
        .context("update thumbhash")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_types() {
        assert!(is_supported("image/jpeg", "a.jpg"));
        assert!(is_supported("IMAGE/PNG; charset=x", "a"));
        assert!(is_supported("application/octet-stream", "IMG.WEBP"));
        assert!(!is_supported("application/octet-stream", "IMG.HEIC"));
        assert!(!is_supported("image/heic", "a.jpg"));
        assert!(!is_supported("video/mp4", "a.mp4"));
    }

    #[test]
    fn sha_mismatch_is_permanent() {
        let err = compute_metadata(b"hello", &[0u8; 32]).err().unwrap();
        assert!(err.is::<Permanent>());
        assert!(err.to_string().contains("sha256 mismatch"));
    }

    #[test]
    fn metadata_ok_with_matching_sha() {
        let img = image::RgbImage::new(20, 10);
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        let sha = Sha256::digest(&b);
        let m = compute_metadata(&b, &sha).unwrap();
        assert_eq!((m.width, m.height), (20, 10));
    }
}
