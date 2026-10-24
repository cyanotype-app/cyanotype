//! Job handlers. Both are idempotent: they overwrite the same S3 keys / asset columns.
use std::sync::Arc;

use anyhow::{Context, Result};
use cyanotype_core::{
    jobs, keys, metrics,
    pairing::{self, Paired},
    places::{self, Geocoded},
    storage::Storage,
};
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

/// Facts a job learns about its asset that the caller wants for tags and logs.
#[derive(Default)]
pub struct JobInfo {
    /// `metrics::family` of the asset (set once the asset row is loaded).
    pub family: std::sync::OnceLock<&'static str>,
}

impl JobInfo {
    pub fn family(&self) -> &'static str {
        self.family.get().copied().unwrap_or("unknown")
    }
}

pub struct Ctx {
    pub db: PgPool,
    pub storage: Storage,
    pub max_original_bytes: u64,
    /// Total memory all in-flight jobs may need together; see `budget`.
    pub budget: MemoryBudget,
    /// Allow the heavy, pixel-capped full RAW develop when a RAW has no embedded preview.
    pub raw_develop: bool,
    /// The model photos are embedded with (`SEARCH_MODEL`); its version is the `embed` job's pipeline version.
    pub search_model: cyanotype_clip::Model,
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
    /// Queued for permanent deletion: derived objects must not be (re)created.
    purging: bool,
}

/// Fixed per-job allowance on top of the original and the decode estimate: S3 client buffers,
/// libwebp encode buffers, allocator slack.
const JOB_OVERHEAD_BYTES: u64 = 48 << 20;
/// Reading a RAW header may transiently hold a decompressed copy of the sensor samples when a rawler
/// decoder ignores its "dummy" mode; budget for that.
const RAW_INSPECT_FACTOR: u64 = 3;

pub async fn run(ctx: &Arc<Ctx>, job: &jobs::Job, info: &JobInfo) -> Result<Outcome> {
    // Trashed assets are still processed: the trash view shows their thumbnails and restoring must not leave
    // them blank. Only assets queued for permanent deletion (or already gone) are skipped.
    let Some(asset): Option<AssetRow> = sqlx::query_as(
        "SELECT owner_id, sha256, original_key, original_name, content_type, size_bytes, (purging_at IS NOT NULL) AS purging
         FROM assets WHERE id = $1",
    )
    .bind(job.asset_id)
    .fetch_optional(&ctx.db)
    .await
    .context("load asset")?
    else {
        tracing::info!("asset no longer exists; skipping");
        return Ok(Outcome::Skipped("asset deleted"));
    };
    let _ = info
        .family
        .set(metrics::family(&asset.content_type, &asset.original_name));
    if asset.purging {
        tracing::info!("asset is being permanently deleted; skipping");
        return Ok(Outcome::Skipped("asset is being deleted"));
    }

    // Burst grouping reads the vectors the embed jobs stored: database-only. (Runs for a trashed photo too: that is how its
    // neighbours get regrouped without it.)
    if job.kind == jobs::KIND_BURST {
        return crate::burst::run(ctx, job).await;
    }

    // Backfill of RAW + JPEG pairs: a database-only job, nothing to download.
    if job.kind == jobs::KIND_PAIR {
        return pair(ctx, job).await;
    }

    // Reverse geocoding reads the coordinates the metadata job stored: database-only too.
    if job.kind == jobs::KIND_GEOCODE {
        return geocode(ctx, job).await;
    }

    // Google Takeout metadata held for this photo: database plus one small bucket write (the edit), no download.
    if job.kind == jobs::KIND_TAKEOUT {
        return takeout(ctx, job).await;
    }

    if !formats::is_candidate(&asset.content_type, &asset.original_name) {
        tracing::info!(content_type = %asset.content_type, "unsupported content type; skipping");
        return Ok(Outcome::Skipped("unsupported content type"));
    }
    if asset.size_bytes < 0 || asset.size_bytes as u64 > ctx.max_original_bytes {
        refused("over_size_limit", info);
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
        refused("over_budget", info);
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
        jobs::KIND_THUMBNAIL => return thumbnail(ctx, job, info, &asset, kind, bytes, held, base).await,
        jobs::KIND_SHARE_COPY => return share_copy(ctx, job, &asset, kind, bytes).await,
        other => anyhow::bail!("no handler for job kind {other}"),
    }
    Ok(Outcome::Done)
}

/// A job turned away because the file cannot fit the worker's limits; the reason tells size limit from memory budget.
fn refused(reason: &str, info: &JobInfo) {
    metrics::global().count(
        "worker.memory_budget.refusals",
        1,
        &[("reason", reason), ("family", info.family())],
    );
}

async fn takeout(ctx: &Arc<Ctx>, job: &jobs::Job) -> Result<Outcome> {
    let done = cyanotype_core::takeout::apply(&ctx.db, &ctx.storage, job.asset_id)
        .await
        .context("apply takeout metadata")?;
    if !done.found {
        return Ok(Outcome::Skipped("no unapplied takeout metadata"));
    }
    tracing::info!(
        date = done.date,
        location = done.location,
        caption = done.caption,
        favorite = done.favorite,
        archive = done.archive,
        album = done.album,
        "applied takeout metadata"
    );
    Ok(Outcome::Done)
}

async fn pair(ctx: &Arc<Ctx>, job: &jobs::Job) -> Result<Outcome> {
    match pairing::pair_asset(&ctx.db, job.asset_id).await.context("pair asset")? {
        Paired::Linked { raw, primary } => {
            tracing::info!(%raw, %primary, "paired RAW with its JPEG/HEIC");
            Ok(Outcome::Done)
        }
        Paired::Already => Ok(Outcome::Skipped("already paired")),
        Paired::NotApplicable => Ok(Outcome::Skipped("not a RAW or JPEG/HEIC, or not live")),
        Paired::NoMatch => Ok(Outcome::Skipped("no counterpart")),
    }
}

async fn geocode(ctx: &Arc<Ctx>, job: &jobs::Job) -> Result<Outcome> {
    match places::geocode_asset(&ctx.db, job.asset_id)
        .await
        .context("geocode asset")?
    {
        Geocoded::Placed => Ok(Outcome::Done),
        Geocoded::NoPlace => Ok(Outcome::Skipped("no populated place within range")),
        Geocoded::NoCoordinates => Ok(Outcome::Skipped("no coordinates")),
        Geocoded::Gone => Ok(Outcome::Skipped("asset deleted")),
    }
}

/// Write `derived/{owner}/{asset}/share.jpg`: the JPEG original minus all metadata but orientation, which is what share
/// links let visitors download (see `strip`). Other formats have no such copy (nothing here can strip them), so the
/// job finishes without one and shares only offer their original when the owner opted in to that.
async fn share_copy(
    ctx: &Arc<Ctx>,
    job: &jobs::Job,
    asset: &AssetRow,
    kind: Kind,
    bytes: Arc<Vec<u8>>,
) -> Result<Outcome> {
    if kind != Kind::Jpeg {
        return Ok(Outcome::Skipped("not a JPEG"));
    }
    let stripped = tokio::task::spawn_blocking(move || {
        let orientation = exif::parse(&bytes).orientation;
        crate::strip::strip_jpeg(&bytes, orientation)
    })
    .await
    .context("share copy task panicked")?
    .map_err(|e| anyhow::Error::from(Permanent(format!("{e:#}"))))?;
    let share_bytes = stripped.len() as i64;
    ctx.storage
        .put_bytes(&keys::share_copy(asset.owner_id, job.asset_id), stripped, "image/jpeg")
        .await?;
    // Same publish rule as thumbnails: a purge that started while this ran has removed (or will remove) derived
    // objects, so take back what was just written.
    let published = sqlx::query(
        "UPDATE assets SET share_copy_at = now(), share_copy_bytes = $2 WHERE id = $1 AND purging_at IS NULL",
    )
    .bind(job.asset_id)
    .bind(share_bytes)
    .execute(&ctx.db)
    .await
    .context("mark share copy")?
    .rows_affected();
    if published == 0 {
        ctx.storage
            .delete_prefix_all_versions(&keys::derived_prefix(asset.owner_id, job.asset_id))
            .await
            .context("remove derived objects of a deleted asset")?;
        return Ok(Outcome::Skipped("asset deleted while writing the share copy"));
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
    // What the file says. While the photo's date or location is edited (`cyanotype_core::edits`) the file's value goes
    // next to the edit (`*_exif`) instead of over it, so a re-run of this job never undoes a correction.
    let (effective_lat, effective_lon): (Option<f64>, Option<f64>) = sqlx::query_as(
        "UPDATE assets SET width = $2, height = $3,
             taken_at = CASE WHEN date_edited THEN taken_at ELSE $4 END,
             taken_at_exif = CASE WHEN date_edited THEN $4 ELSE NULL END,
             latitude = CASE WHEN location_edited THEN latitude ELSE $5 END,
             longitude = CASE WHEN location_edited THEN longitude ELSE $6 END,
             latitude_exif = CASE WHEN location_edited THEN $5 ELSE NULL END,
             longitude_exif = CASE WHEN location_edited THEN $6 ELSE NULL END,
             camera_make = $7, camera_model = $8, lens_model = $9, exif = $10
         WHERE id = $1
         RETURNING latitude, longitude",
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
    .fetch_one(&ctx.db)
    .await
    .context("update asset metadata")?;
    // The coordinates may be new or changed (a re-run after an EXIF fix): look the place up again, or forget it.
    if effective_lat.is_some() && effective_lon.is_some() {
        places::requeue(&ctx.db, job.asset_id).await.context("queue geocode")?;
    } else {
        places::geocode_asset(&ctx.db, job.asset_id)
            .await
            .context("clear place")?;
    }
    // The capture time and camera are known now: pair with the RAW/JPEG of the same shot if its other half is
    // already here (or, when it is not, the other half's own metadata job pairs them). A failure retries this job.
    if let Paired::Linked { raw, primary } = pairing::pair_asset(&ctx.db, job.asset_id).await.context("pair asset")? {
        tracing::info!(%raw, %primary, "paired RAW with its JPEG/HEIC");
    }
    // Google Takeout metadata held for this photo waits for the file's own values (the edit is made over them).
    cyanotype_core::takeout::enqueue_for_asset(&ctx.db, job.asset_id)
        .await
        .context("queue takeout metadata")?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn thumbnail(
    ctx: &Arc<Ctx>,
    job: &jobs::Job,
    info: &JobInfo,
    asset: &AssetRow,
    kind: Kind,
    bytes: Arc<Vec<u8>>,
    held: Reservation,
    base: u64,
) -> Result<Outcome> {
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
    metrics::global().histogram(
        "jobs.decoded_megapixels",
        f64::from(prepared.width) * f64::from(prepared.height) / 1e6,
        &[("kind", &job.kind), ("family", info.family())],
    );
    if let Some(reason) = &prepared.refusal {
        refused("decode_refused", info);
        tracing::error!(key = %asset.original_key, reason, "refusing to decode");
        return Err(Permanent(reason.clone()).into());
    }
    // The decode size is now known: swap the provisional reservation for exactly what is needed.
    // (Release first, then reserve; see `run`.)
    drop(held);
    let mut _held = ctx.budget.reserve((base + prepared.cost_bytes).min(total)).await?;
    metrics::global().count(
        "jobs.decode_path",
        1,
        &[("path", prepared.path()), ("family", info.family())],
    );

    let prepared = Arc::new(prepared);
    let rendered = {
        let (p, b) = (prepared.clone(), bytes.clone());
        tokio::task::spawn_blocking(move || media::render(&p, &b))
            .await
            .context("thumbnail task panicked")?
    };
    let d = match rendered {
        Ok(d) => d,
        // The reduced-size decoders (DCT-scaled JPEG, streamed PNG) cover the common files; for an odd one they
        // cannot read, fall back to the plain full decode if that fits the budget.
        Err(e) => match prepared.full_decode_fallback(&bytes, limits) {
            Some(full) => {
                tracing::warn!(key = %asset.original_key, error = %format!("{e:#}"), "reduced-size decode failed; trying a full decode");
                metrics::global().count("jobs.decode_fallbacks", 1, &[("family", info.family())]);
                drop(_held);
                _held = ctx.budget.reserve((base + full.cost_bytes).min(total)).await?;
                tokio::task::spawn_blocking(move || media::render(&full, &bytes))
                    .await
                    .context("thumbnail task panicked")?
                    .map_err(|e| anyhow::Error::from(Permanent(format!("{e:#}"))))?
            }
            None => return Err(Permanent(format!("{e:#}")).into()),
        },
    };
    tracing::debug!(width = d.width, height = d.height, "derived images ready");

    // Their sizes are recorded with the thumbhash below (the usage counters, docs/admin.md).
    let derived_bytes = (d.thumb_webp.len() + d.preview_webp.len()) as i64;
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
    // Publish only if the asset still exists and is not being purged. A purge that started while this job was
    // rendering has already removed (or is about to remove) the derived objects; whatever we just wrote would be
    // orphaned, so take it back out. (`purging_at` is set before the purge deletes anything, so either this
    // check sees it, or the purge's listing sees our objects.)
    let published =
        sqlx::query("UPDATE assets SET thumbhash = $2, derived_bytes = $3 WHERE id = $1 AND purging_at IS NULL")
            .bind(job.asset_id)
            .bind(&d.thumbhash)
            .bind(derived_bytes)
            .execute(&ctx.db)
            .await
            .context("update thumbhash")?
            .rows_affected();
    if published == 0 {
        ctx.storage
            .delete_prefix_all_versions(&keys::derived_prefix(asset.owner_id, job.asset_id))
            .await
            .context("remove derived objects of a deleted asset")?;
        return Ok(Outcome::Skipped("asset deleted while rendering"));
    }
    // The preview exists now: queue the embedding for search (background priority work, see `embed`). The pipeline
    // version is the model version, so a model swap re-embeds through the same job.
    jobs::enqueue(&ctx.db, jobs::KIND_EMBED, job.asset_id, ctx.search_model.version())
        .await
        .context("queue embedding")?;
    Ok(Outcome::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyanotype_core::{config::S3Config, db};
    use sqlx::{Connection, Executor, PgConnection};

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

    /// The metadata job never undoes a corrected date or location (`cyanotype_core::edits`): the file's values go next to
    /// the edit, and a photo that is not edited still takes them. Needs Postgres and the S3 variables.
    #[tokio::test]
    async fn the_metadata_job_keeps_edited_dates_and_locations_and_still_records_the_files_own() {
        use crate::exif::testutil::{ascii, exif_app1, rationals, with_app1};
        let Ok(admin_url) = std::env::var("DATABASE_URL") else {
            return;
        };
        if std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty()).is_none() {
            return;
        }
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        let (owner,): (Uuid,) =
            sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ('w@example.com','x') RETURNING id")
                .fetch_one(&pool)
                .await
                .unwrap();
        let storage = Storage::new(&S3Config::from_env().expect("S3_* env"));
        let ctx = Arc::new(Ctx {
            db: pool.clone(),
            storage: storage.clone(),
            max_original_bytes: 1 << 24,
            budget: MemoryBudget::new(512 << 20),
            raw_develop: false,
            search_model: cyanotype_clip::DEFAULT_MODEL,
        });
        // A JPEG taken 2021-12-31 23:59:58 UTC at 33.85 S, 151.21 W... (the file's own values).
        let sub = vec![(0x9003, 2, ascii("2021:12:31 23:59:58"))];
        let gps = vec![
            (0x0001, 2, ascii("S")),
            (0x0002, 5, rationals(&[(33, 1), (51, 1), (0, 1)])),
            (0x0003, 2, ascii("W")),
            (0x0004, 5, rationals(&[(151, 1), (12, 1), (36, 1)])),
        ];
        let jpeg = with_app1(
            &crate::exif::tests_support_jpeg(),
            &exif_app1(&[(0x010F, 2, ascii("ACME"))], &sub, &gps),
        );
        let sha: [u8; 32] = Sha256::digest(&jpeg).into();
        let key = keys::original(owner, &sha, "jpg");
        ctx.storage.put_bytes(&key, jpeg.clone(), "image/jpeg").await.unwrap();
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes,
                                 date_edited, taken_at, location_edited, latitude, longitude)
             VALUES ($1, $2, $3, 'a.jpg', 'image/jpeg', $4, true, '2000-01-01T00:00:00Z', true, 48.0, 2.0) RETURNING id",
        )
        .bind(owner)
        .bind(&sha[..])
        .bind(&key)
        .bind(jpeg.len() as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
        let job = jobs::Job {
            id: 1,
            kind: jobs::KIND_METADATA.into(),
            asset_id: id,
            version: 1,
            attempts: 1,
        };
        assert!(matches!(run(&ctx, &job, &JobInfo::default()).await, Ok(Outcome::Done)));
        let (taken, exif_taken, lat, lat_exif, camera): (String, Option<String>, f64, Option<f64>, Option<String>) = sqlx::query_as(
            "SELECT taken_at::text, taken_at_exif::text, latitude, latitude_exif, camera_make FROM assets WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(taken, "2000-01-01 00:00:00+00", "the edited date stays");
        assert_eq!(
            exif_taken.as_deref(),
            Some("2021-12-31 23:59:58+00"),
            "the file's own is kept next to it"
        );
        assert_eq!(lat, 48.0, "the edited location stays");
        assert!((lat_exif.unwrap() + 33.85).abs() < 1e-9);
        assert_eq!(camera.as_deref(), Some("ACME"), "everything else is still recorded");
        // The geocode job is for the coordinates that are in effect.
        // Revert (as `edits::apply` does) and run again: the file's values are the effective ones.
        sqlx::query("UPDATE assets SET date_edited = false, location_edited = false WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(run(&ctx, &job, &JobInfo::default()).await, Ok(Outcome::Done)));
        let (taken, exif_taken, lat): (String, Option<String>, f64) =
            sqlx::query_as("SELECT taken_at::text, taken_at_exif::text, latitude FROM assets WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (taken.as_str(), exif_taken, lat < 0.0),
            ("2021-12-31 23:59:58+00", None, true)
        );

        pool.close().await;
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin
            .execute(format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)").as_str())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn jobs_skip_assets_that_are_purging_or_gone_but_not_trashed_ones() {
        let Ok(admin_url) = std::env::var("DATABASE_URL") else {
            return;
        };
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
        db::migrate(&pool).await.unwrap();

        let (owner,): (Uuid,) =
            sqlx::query_as("INSERT INTO users (email, password_hash) VALUES ('w@example.com','x') RETURNING id")
                .fetch_one(&pool)
                .await
                .unwrap();
        let mut ids = Vec::new();
        for (n, state) in ["trashed", "purging"].into_iter().enumerate() {
            let (id,): (Uuid,) = sqlx::query_as(
                "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, deleted_at, purging_at)
                 VALUES ($1, sha256($2::bytea), 'k', 'a.txt', 'text/plain', 1, now(), CASE WHEN $3 THEN now() END) RETURNING id",
            )
            .bind(owner)
            .bind(n.to_string().as_bytes())
            .bind(state == "purging")
            .fetch_one(&pool)
            .await
            .unwrap();
            ids.push(id);
        }
        let ctx = Arc::new(Ctx {
            db: pool.clone(),
            storage: Storage::new(&S3Config {
                endpoint: Some("http://127.0.0.1:1".into()),
                public_endpoint: None,
                region: "us-east-1".into(),
                bucket: "b".into(),
                access_key: "k".into(),
                secret_key: "s".into(),
                force_path_style: true,
                presign_ttl_secs: 60,
            }),
            max_original_bytes: 1 << 20,
            budget: MemoryBudget::new(256 << 20),
            raw_develop: false,
            search_model: cyanotype_clip::DEFAULT_MODEL,
        });
        let job = |asset_id| jobs::Job {
            id: 1,
            kind: jobs::KIND_THUMBNAIL.into(),
            asset_id,
            version: 1,
            attempts: 1,
        };
        // Purging and nonexistent assets are skipped without touching storage.
        for id in [ids[1], Uuid::new_v4()] {
            assert!(matches!(
                run(&ctx, &job(id), &JobInfo::default()).await,
                Ok(Outcome::Skipped(_))
            ));
        }
        // A trashed asset is processed as usual: here it gets as far as the (unsupported) content type check.
        assert!(matches!(
            run(&ctx, &job(ids[0]), &JobInfo::default()).await,
            Ok(Outcome::Skipped("unsupported content type"))
        ));

        pool.close().await;
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin
            .execute(format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)").as_str())
            .await
            .unwrap();
    }
}
