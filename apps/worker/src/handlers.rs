//! Job handlers. Both are idempotent: they overwrite the same S3 keys / asset columns.
use std::sync::Arc;

use anyhow::{Context, Result};
use cyanotype_core::{jobs, keys, storage::Storage};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    budget::{MemoryBudget, Reservation},
    exif,
    formats::{self, Kind},
    media,
};

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
    /// Total memory all in-flight jobs may need together; see `budget`.
    pub budget: MemoryBudget,
    /// Allow the heavy, pixel-capped full RAW develop when a RAW has no embedded preview.
    pub raw_develop: bool,
}

impl Ctx {
    fn limits(&self, max_decode_bytes: u64) -> media::Limits {
        media::Limits {
            max_decode_bytes,
            allow_raw_develop: self.raw_develop,
        }
    }
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

/// Fixed per-job allowance on top of the original and the decode estimate: S3 client buffers,
/// libwebp encode buffers, allocator slack.
const JOB_OVERHEAD_BYTES: u64 = 48 << 20;
/// Reading a RAW header may transiently hold a decompressed copy of the sensor samples when a rawler
/// decoder ignores its "dummy" mode; budget for that.
const RAW_INSPECT_FACTOR: u64 = 3;

pub async fn run(ctx: &Arc<Ctx>, job: &jobs::Job) -> Result<Outcome> {
    let asset: AssetRow = sqlx::query_as(
        "SELECT owner_id, sha256, original_key, original_name, content_type, size_bytes FROM assets WHERE id = $1",
    )
    .bind(job.asset_id)
    .fetch_one(&ctx.db)
    .await
    .context("load asset")?;

    if !formats::is_candidate(&asset.content_type, &asset.original_name) {
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
    let size = asset.size_bytes as u64;
    let total = ctx.budget.total_bytes();
    let base = size + JOB_OVERHEAD_BYTES;
    if base > total {
        return Err(Permanent(format!(
            "original is {} MB, which does not fit the worker memory budget of {} MB",
            size >> 20,
            total >> 20
        ))
        .into());
    }

    // The S3 body is buffered and then copied into one Vec, so the download itself peaks near 2x the
    // object; hold that much until the original is in memory, then keep just the original.
    let mut held = ctx.budget.reserve((size * 2 + JOB_OVERHEAD_BYTES).min(total)).await?;
    let bytes = Arc::new(
        ctx.storage
            .get_bytes_limited(&asset.original_key, ctx.max_original_bytes)
            .await
            .context("download original")?,
    );

    held.shrink_to(base);

    // Route by content, not by the stored type: uploaders may send application/octet-stream.
    let Some(kind) = formats::sniff(&bytes, &asset.original_name) else {
        tracing::info!(content_type = %asset.content_type, name = %asset.original_name, "not a supported image format; skipping");
        return Ok(Outcome::Skipped("unsupported image format"));
    };
    tracing::debug!(?kind, bytes = bytes.len(), "routed");
    if kind == Kind::Raw {
        // Release then re-reserve (never top up in place) so two jobs can't deadlock each waiting
        // for the other's memory.
        drop(held);
        held = ctx
            .budget
            .reserve((size * RAW_INSPECT_FACTOR + JOB_OVERHEAD_BYTES).min(total))
            .await?;
    }

    match job.kind.as_str() {
        jobs::KIND_METADATA => metadata(ctx, job, &asset, kind, bytes).await?,
        jobs::KIND_THUMBNAIL => thumbnail(ctx, job, &asset, kind, bytes, held, base).await?,
        other => anyhow::bail!("no handler for job kind {other}"),
    }
    Ok(Outcome::Done)
}

struct Metadata {
    width: u32,
    height: u32,
    exif: exif::ExifInfo,
}

fn compute_metadata(kind: Kind, bytes: &Arc<Vec<u8>>, expected_sha: &[u8], limits: media::Limits) -> Result<Metadata> {
    let actual = Sha256::digest(bytes.as_slice());
    if actual.as_slice() != expected_sha {
        return Err(Permanent(format!(
            "sha256 mismatch: expected {}, object has {}",
            keys::hex(expected_sha),
            keys::hex(&actual)
        ))
        .into());
    }
    // A decode refusal (too big to thumbnail) does not stop us recording metadata: that only needs headers.
    let p = media::prepare(kind, bytes, limits).map_err(|e| Permanent(format!("{e:#}")))?;
    Ok(Metadata {
        width: p.width,
        height: p.height,
        exif: p.exif,
    })
}

async fn metadata(ctx: &Arc<Ctx>, job: &jobs::Job, asset: &AssetRow, kind: Kind, bytes: Arc<Vec<u8>>) -> Result<()> {
    let sha = asset.sha256.clone();
    let limits = ctx.limits(ctx.budget.total_bytes());
    let m = tokio::task::spawn_blocking(move || compute_metadata(kind, &bytes, &sha, limits))
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

async fn thumbnail(
    ctx: &Arc<Ctx>,
    job: &jobs::Job,
    asset: &AssetRow,
    kind: Kind,
    bytes: Arc<Vec<u8>>,
    held: Reservation,
    base: u64,
) -> Result<()> {
    let total = ctx.budget.total_bytes();
    // Header pass: sizes, EXIF and the decode estimate, with no pixel decoding.
    let limits = ctx.limits(total.saturating_sub(base));
    let prepared = {
        let bytes = bytes.clone();
        tokio::task::spawn_blocking(move || media::prepare(kind, &bytes, limits))
            .await
            .context("thumbnail prepare task panicked")?
            .map_err(|e| Permanent(format!("{e:#}")))?
    };
    if let Some(reason) = &prepared.refusal {
        tracing::error!(key = %asset.original_key, reason, "refusing to decode");
        return Err(Permanent(reason.clone()).into());
    }
    // The decode size is now known: swap the provisional reservation for exactly what is needed.
    // (Release first, then reserve; see `run`.)
    drop(held);
    let _held = ctx.budget.reserve((base + prepared.cost_bytes).min(total)).await?;

    let d = tokio::task::spawn_blocking(move || media::render(&prepared, &bytes))
        .await
        .context("thumbnail task panicked")?
        .map_err(|e| anyhow::Error::from(Permanent(format!("{e:#}"))))?;
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

    const ROOMY: media::Limits = media::Limits {
        max_decode_bytes: 1 << 30,
        allow_raw_develop: false,
    };

    #[test]
    fn sha_mismatch_is_permanent() {
        let err = compute_metadata(Kind::Png, &Arc::new(b"hello".to_vec()), &[0u8; 32], ROOMY)
            .err()
            .unwrap();
        assert!(err.is::<Permanent>());
        assert!(err.to_string().contains("sha256 mismatch"));
    }

    #[test]
    fn garbage_with_matching_sha_is_permanent() {
        let b = Arc::new(b"not an image".to_vec());
        let sha = Sha256::digest(b.as_slice());
        let err = compute_metadata(Kind::Jpeg, &b, &sha, ROOMY).err().unwrap();
        assert!(err.is::<Permanent>());
    }

    #[test]
    fn metadata_ok_with_matching_sha() {
        let img = image::RgbImage::new(20, 10);
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        let sha = Sha256::digest(&b);
        let m = compute_metadata(Kind::Png, &Arc::new(b), &sha, ROOMY).unwrap();
        assert_eq!((m.width, m.height), (20, 10));
    }
}
