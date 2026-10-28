//! The `pets` job: the dogs and cats in each photo, stored in `pet_detections` for the owner's Pets view, filter and smart albums to read
//! (`docs/pets.md`). On unless `PETS_ENABLED` turns it off; off, none of this runs and no job is queued.
//!
//! It works in two steps. **The check** reads the photo's search embedding (which the `embed` job stored; no photo is read) against a few
//! prompts and says whether a dog, a cat or neither is in it (`cyanotype_clip::zeroshot`): microseconds, and for most photos the whole job.
//! **The boxes** are found only for a photo the check confirmed: D-FINE-N reads the photo's `preview.webp` (never the original, so there is
//! no big decode) and the boxes of the confirmed kinds are kept. A photo the check confirmed but the detector found no box in is stored as
//! having no pets (measured in docs/pets.md: 3 of 129).
//!
//! **The fingerprints** (named pets, docs/pets.md): the crop of each box is embedded with DINOv2-small (the class token of its 8-bit build, which
//! told individual animals apart far better than the search model's tower in the bake-off) and stored with the animal, for "Is this Biscuit?" to
//! compare. They are made in the same job, from the same decoded preview, and the model is loaded and dropped with the detector. Animals found before named pets (or before another fingerprint model) have none: the sweep queues their photos again,
//! and the job then only fingerprints them, without finding them again, so their ids and what the owner decided about them stay.
//!
//! Like text reading and faces it is background work in a loop of its own, and the *least* urgent kind: it takes a job only when no
//! thumbnail, metadata, embed, text, faces or other foreground job is runnable, and it shares the embed loop's one-at-a-time permit, so
//! the ONNX models of all of them never work at the same time. A photo's job is queued when its embedding is stored; the sweep queues
//! the rest (a library that existed before, a model switch, a rotate or crop). The detector loads on the first photo that needs it and is
//! dropped again after a quiet spell (`IDLE_UNLOAD`). While it runs a job holds `TRANSIENT_BYTES` of the shared byte budget
//! (`WORKER_MEMORY_BUDGET_MB`): the decoded preview, the detector's input and the network's activations, measured in docs/pets.md.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use cyanotype_clip::zeroshot::{PetCheck, PetShares};
use cyanotype_core::{
    embeddings, jobs, keys, metrics,
    pets::{self as store, Basis, NewDetection, Scan},
};
use cyanotype_pets::{
    Detection, Detector, DetectorConfig, Kind, crop,
    dino::{self, DinoEncoder, Pooling},
};
use tokio::sync::Semaphore;
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    finish,
    handlers::{Ctx, Outcome, Permanent},
};

/// A preview is at most 1440 px on the long edge; anything bigger than this is not ours.
const MAX_PREVIEW_BYTES: u64 = 16 << 20;
/// Budget held while a photo is scanned: the decoded preview (RGBA, then RGB), the detector's 640 x 640 input and the network's
/// activations, and the crops and activations of the fingerprint model (named pets: 64 MB before they, 96 MB with). The highest process peak measured over the bake-off photos with the model loaded (docs/pets.md) is what the figure is
/// read from.
pub const TRANSIENT_BYTES: u64 = 96 << 20;
/// A crop whose shorter side is under this many pixels of the preview is not fingerprinted: at 64 px the bake-off's retrieval is still 0.89
/// mean average precision (0.93 at full size), at 48 px 0.85 and at 32 px 0.76 (docs/pets.md, "Telling individuals apart").
pub const MIN_CROP_PX: u32 = 64;
/// How long the model stays loaded with nothing to scan.
const IDLE_UNLOAD: Duration = Duration::from_secs(300);
/// How often the sweep that queues unscanned photos runs.
const SWEEP_EVERY: Duration = Duration::from_secs(900);
const FALLBACK_POLL: Duration = Duration::from_secs(30);
const FOREGROUND_RECHECK: Duration = Duration::from_secs(1);

/// Where the model file is: `PETS_MODEL_DIR`, else `<SEARCH_MODEL_DIR>/pets` (the images install it there).
pub fn model_dir() -> PathBuf {
    cyanotype_pets::model_dir()
}

/// Are the model files installed (the detector and the fingerprint model)? A dev checkout that skipped `scripts/fetch-models.sh` runs without
/// scanning pets, and its `pets` jobs stay queued.
pub fn model_installed() -> bool {
    cyanotype_pets::models_installed(&model_dir())
}

/// The check (its prompts are a few kilobytes, always at hand) and the (lazily loaded) detector and fingerprint model.
pub struct Finder {
    threads: usize,
    /// The search model whose embeddings are read, and its prompts.
    model: cyanotype_clip::Model,
    check: PetCheck,
    detector: Mutex<Option<(Arc<Detector>, Instant)>>,
    dino: Mutex<Option<(Arc<DinoEncoder>, Instant)>>,
}

impl Finder {
    /// `None` when `model` has no prompts: only SigLIP 2 does, and the check's thresholds are its own.
    pub fn new(threads: usize, model: cyanotype_clip::Model) -> Option<Self> {
        Some(Self {
            threads,
            model,
            check: PetCheck::for_model(model)?,
            detector: Mutex::new(None),
            dino: Mutex::new(None),
        })
    }

    async fn encoder(&self) -> Result<Arc<DinoEncoder>> {
        if let Some((e, used)) = self.dino.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            *used = Instant::now();
            return Ok(e.clone());
        }
        let threads = self.threads;
        let started = Instant::now();
        let e = tokio::task::spawn_blocking(move || {
            DinoEncoder::load(&model_dir().join(cyanotype_pets::DINO_FILE), threads)
        })
        .await
        .context("fingerprint model load task panicked")??;
        let load_ms = started.elapsed().as_millis() as u64;
        tracing::info!(
            model = store::EMBED_MODEL,
            threads,
            load_ms,
            "pet fingerprint model loaded"
        );
        metrics::global().histogram("pets.model_load_ms", load_ms as f64, &[("model", store::EMBED_MODEL)]);
        let e = Arc::new(e);
        *self.dino.lock().unwrap_or_else(|p| p.into_inner()) = Some((e.clone(), Instant::now()));
        Ok(e)
    }

    async fn detector(&self) -> Result<Arc<Detector>> {
        if let Some((d, used)) = self.detector.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            *used = Instant::now();
            return Ok(d.clone());
        }
        let threads = self.threads;
        let started = Instant::now();
        let d = tokio::task::spawn_blocking(move || Detector::load(&model_dir(), threads, DetectorConfig::default()))
            .await
            .context("pet model load task panicked")??;
        let load_ms = started.elapsed().as_millis() as u64;
        tracing::info!(model = store::MODEL, threads, load_ms, "pet model loaded");
        metrics::global().histogram("pets.model_load_ms", load_ms as f64, &[("model", store::MODEL)]);
        let d = Arc::new(d);
        *self.detector.lock().unwrap_or_else(|p| p.into_inner()) = Some((d.clone(), Instant::now()));
        Ok(d)
    }

    fn unload_if_idle(&self) {
        let mut guard = self.detector.lock().unwrap_or_else(|p| p.into_inner());
        if guard.as_ref().is_some_and(|(_, used)| used.elapsed() >= IDLE_UNLOAD) {
            *guard = None;
            tracing::info!("pet model unloaded after a quiet spell");
        }
        drop(guard);
        let mut guard = self.dino.lock().unwrap_or_else(|p| p.into_inner());
        if guard.as_ref().is_some_and(|(_, used)| used.elapsed() >= IDLE_UNLOAD) {
            *guard = None;
            tracing::info!("pet fingerprint model unloaded after a quiet spell");
        }
    }

    fn loaded(&self) -> bool {
        self.detector.lock().unwrap_or_else(|p| p.into_inner()).is_some()
            || self.dino.lock().unwrap_or_else(|p| p.into_inner()).is_some()
    }
}

/// Kinds that must not wait behind finding pets: everything else, embedding, text and faces included.
pub(crate) fn foreground_kinds() -> impl Iterator<Item = &'static str> {
    crate::faces::foreground_kinds().chain([jobs::KIND_FACES])
}

/// The vectors the check reads: the search model's, at the version the `embed` job writes.
fn basis(model: cyanotype_clip::Model) -> Basis<'static> {
    Basis {
        model: model.id(),
        version: model.version(),
    }
}

/// The pets loop. `busy` is the embed loop's permit: held for each job, so shutdown can wait for it and the models never work at the
/// same time.
pub async fn run_loop(ctx: Arc<Ctx>, finder: Arc<Finder>, waker: jobs::JobWaker, busy: Arc<Semaphore>) {
    let foreground: Vec<&str> = foreground_kinds().collect();
    let mut last_sweep: Option<Instant> = None;
    loop {
        metrics::global().gauge("pets.model_loaded", if finder.loaded() { 1.0 } else { 0.0 }, &[]);
        if last_sweep.is_none_or(|t| t.elapsed() >= SWEEP_EVERY) {
            last_sweep = Some(Instant::now());
            sweep(&ctx, finder.model).await;
        }
        match jobs::runnable_waiting(&ctx.db, &foreground).await {
            Ok(true) => {
                tokio::time::sleep(FOREGROUND_RECHECK).await;
                continue;
            }
            Ok(false) => {}
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "could not check for foreground jobs");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }
        let permit = busy.clone().acquire_owned().await;
        let job = match jobs::claim(&ctx.db, jobs::KIND_PETS).await {
            Ok(j) => j,
            Err(e) => {
                drop(permit);
                tracing::error!(error = %format!("{e:#}"), "claim failed");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let Some(job) = job else {
            drop(permit);
            if finder.loaded() {
                finder.unload_if_idle();
            }
            jobs::wait_for_work(&ctx.db, &waker, &[jobs::KIND_PETS], SWEEP_EVERY.min(FALLBACK_POLL)).await;
            continue;
        };
        let started = Instant::now();
        let span = tracing::info_span!(
            "job",
            job.id = job.id,
            job.kind = %job.kind,
            job.attempts = job.attempts,
            asset.id = %job.asset_id
        );
        let result = process(&ctx, &finder, &job).instrument(span.clone()).await;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if let Ok(Outcome::Done) = result {
            metrics::global().histogram("pets.duration", elapsed, &[("model", store::MODEL)]);
        }
        finish::record(&ctx, &job, "image", result, elapsed)
            .instrument(span)
            .await;
        drop(permit);
    }
}

/// Queue what has not been scanned by the current model (a library that existed before, a model switch, lost jobs); once nothing is
/// missing, drop the rows of the models it replaced.
async fn sweep(ctx: &Ctx, model: cyanotype_clip::Model) {
    match store::enqueue_missing(&ctx.db, store::MODEL, store::VERSION, basis(model)).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            queued = n,
            model = store::MODEL,
            "queued photos whose pets have not been looked for"
        ),
        Err(e) => tracing::warn!(error = %format!("{e:#}"), "pets sweep failed"),
    }
    match store::missing_count(&ctx.db, store::MODEL, store::VERSION, basis(model)).await {
        Ok(0) => match store::prune_superseded(&ctx.db, store::MODEL, store::VERSION).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                removed = n,
                model = store::MODEL,
                "removed pets found by replaced models"
            ),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "pruning replaced pets failed"),
        },
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %format!("{e:#}"), "counting unscanned photos failed"),
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    owner_id: Uuid,
    hidden: bool,
    purging: bool,
    has_preview: bool,
}

async fn process(ctx: &Arc<Ctx>, finder: &Arc<Finder>, job: &jobs::Job) -> Result<Outcome> {
    let Some(r): Option<Row> = sqlx::query_as(
        "SELECT owner_id, primary_id IS NOT NULL AS hidden, purging_at IS NOT NULL AS purging,
                thumbhash IS NOT NULL AS has_preview
         FROM assets WHERE id = $1",
    )
    .bind(job.asset_id)
    .fetch_optional(&ctx.db)
    .await
    .context("load asset")?
    else {
        return Ok(Outcome::Skipped("asset deleted"));
    };
    if r.purging {
        return Ok(Outcome::Skipped("asset is being deleted"));
    }
    if r.hidden {
        return Ok(Outcome::Skipped("hidden RAW: the visible photo is the one scanned"));
    }
    if !r.has_preview {
        return Ok(Outcome::Skipped("no preview yet"));
    }
    // The check reads the search embedding, which is of the picture as it was until the embed job that follows an edit has replaced it:
    // not before that (the job queued then will find the new one).
    let embedding_pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM jobs WHERE asset_id = $1 AND kind = 'embed' AND state IN ('queued', 'running'))",
    )
    .bind(job.asset_id)
    .fetch_one(&ctx.db)
    .await
    .context("look for a pending embedding")?;
    if embedding_pending {
        return Ok(Outcome::Skipped("the photo is being embedded again"));
    }
    // The check: the search embedding says which kinds are in the photo. No embedding (it was pruned or the model changed since the job
    // was queued) is not a failure and not a scan: the sweep queues the photo again once it has one.
    let Some(embedding) = embeddings::get(&ctx.db, job.asset_id, finder.model.id(), finder.model.version())
        .await
        .context("load the search embedding")?
    else {
        return Ok(Outcome::Skipped("not indexed for search yet"));
    };
    // Scanned already (by this model and version): the animals stay as they are, and only what has no fingerprint yet gets one.
    let scanned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pet_scans WHERE asset_id = $1 AND model = $2 AND version = $3)",
    )
    .bind(job.asset_id)
    .bind(store::MODEL)
    .bind(store::VERSION)
    .fetch_one(&ctx.db)
    .await
    .context("look for the scan")?;
    if scanned {
        return fingerprint_existing(ctx, finder, job, r.owner_id).await;
    }
    let shares = finder
        .check
        .shares(&embedding)
        .ok_or_else(|| anyhow::Error::from(Permanent("the search embedding has the wrong size".into())))?;
    let confirmed = confirmed_kinds(shares);
    let metrics = metrics::global();
    let found = if confirmed.is_empty() {
        metrics.count("pets.checked", 1, &[("verdict", "none")]);
        Vec::new()
    } else {
        // The boxes: a pet the check confirmed gets its box from the detector, which reads the preview.
        metrics.count("pets.checked", 1, &[("verdict", "pet")]);
        let bytes = ctx
            .storage
            .get_bytes_limited(&keys::preview(r.owner_id, job.asset_id), MAX_PREVIEW_BYTES)
            .await
            .context("download preview")?;
        let _held = ctx.budget.reserve(TRANSIENT_BYTES).await?;
        let detector = finder.detector().await?;
        let encoder = finder.encoder().await?;
        let started = Instant::now();
        let (found, detector_ms) = tokio::task::spawn_blocking(move || -> Result<(Vec<NewDetection>, f64)> {
            let (rgb, w, h) = decode(&bytes)?;
            drop(bytes);
            let mut found = detect(&detector, &rgb, w, h, &confirmed)?;
            let detector_ms = started.elapsed().as_secs_f64() * 1000.0;
            let boxes: Vec<[f32; 4]> = found.iter().map(|d| d.bbox).collect();
            for (d, print) in found.iter_mut().zip(fingerprints(&encoder, &rgb, w, h, &boxes)?) {
                d.embedding = print;
            }
            Ok((found, detector_ms))
        })
        .await
        .context("pets task panicked")??;
        metrics.histogram("pets.detector_ms", detector_ms, &[("model", store::MODEL)]);
        metrics.histogram(
            "pets.fingerprint_ms",
            (started.elapsed().as_secs_f64() * 1000.0 - detector_ms).max(0.0),
            &[("model", store::EMBED_MODEL)],
        );
        if found.is_empty() {
            // Said to be in the photo, found nowhere in it: counted, so a detector that drifts from the check shows.
            metrics.count("pets.unlocated", 1, &[("model", store::MODEL)]);
        }
        found
    };
    metrics.histogram("pets.per_photo", found.len() as f64, &[("model", store::MODEL)]);
    for kind in cyanotype_core::pets::KINDS {
        let n = found.iter().filter(|d| d.kind == kind).count();
        if n > 0 {
            metrics.count("pets.found", n as u64, &[("kind", kind), ("model", store::MODEL)]);
        }
    }
    let scan = Scan {
        dog_share: shares.dog,
        cat_share: shares.cat,
        found,
        // Nothing to fingerprint is as good as done.
        fingerprinted: true,
    };
    let stored = store::store(&ctx.db, job.asset_id, store::MODEL, store::VERSION, &scan)
        .await
        .context("store pets")?;
    if !stored {
        return Ok(Outcome::Skipped("asset deleted while scanning"));
    }
    Ok(Outcome::Done)
}

/// The kinds the check confirmed (a share of at least `zeroshot::MIN_SHARE`).
pub fn confirmed_kinds(shares: PetShares) -> Vec<Kind> {
    let (dog, cat) = shares.confirmed();
    [(dog, Kind::Dog), (cat, Kind::Cat)]
        .into_iter()
        .filter_map(|(yes, kind)| yes.then_some(kind))
        .collect()
}

/// A preview as RGB8 (over white where it has transparency, like the other models read it). One that cannot be decoded is a permanent
/// failure of that photo alone.
fn decode(webp: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let img = webp::Decoder::new(webp)
        .decode()
        .ok_or_else(|| anyhow::Error::from(Permanent("preview is not a decodable WebP".into())))?;
    let (w, h) = (img.width(), img.height());
    let px: &[u8] = &img;
    let rgb: Vec<u8> = if img.is_alpha() {
        px.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let a = u32::from(p[3]);
                [0, 1, 2].map(|c| ((u32::from(p[c]) * a + 255 * (255 - a)) / 255) as u8)
            })
            .collect()
    } else {
        px.to_vec()
    };
    Ok((rgb, w, h))
}

/// The boxes of the `confirmed` kinds in a decoded preview.
pub fn detect(detector: &Detector, rgb: &[u8], w: u32, h: u32, confirmed: &[Kind]) -> Result<Vec<NewDetection>> {
    let found: Vec<Detection> = detector
        .detect(rgb, w, h)?
        .into_iter()
        .filter(|d| confirmed.contains(&d.kind))
        .collect();
    Ok(to_new_detections(&found, w, h))
}

/// The fingerprint of each box (fractions of the preview) in a decoded preview: the crop around the box (a square, [`crop::PAD`] bigger),
/// DINOv2's class token. `None` for a crop too small to be worth one ([`MIN_CROP_PX`]).
pub fn fingerprints(
    encoder: &DinoEncoder,
    rgb: &[u8],
    w: u32,
    h: u32,
    boxes: &[[f32; 4]],
) -> Result<Vec<Option<Vec<f32>>>> {
    let mut out = Vec::with_capacity(boxes.len());
    for b in boxes {
        let crop = crop::crop(rgb, w, h, *b, crop::PAD, true).filter(|c| c.width.min(c.height) >= MIN_CROP_PX);
        out.push(match crop {
            Some(c) => encoder.embed(&dino::preprocess(&c)?, &[Pooling::Cls])?.pop(),
            None => None,
        });
    }
    Ok(out)
}

/// A photo that was scanned already: fingerprint the animals that have none by [`store::EMBED_MODEL`] (found before named pets, or before
/// another fingerprint model) without finding them again, so their ids, and with them what the owner decided, stay.
async fn fingerprint_existing(ctx: &Arc<Ctx>, finder: &Arc<Finder>, job: &jobs::Job, owner: Uuid) -> Result<Outcome> {
    let todo = store::unfingerprinted(&ctx.db, job.asset_id, store::MODEL)
        .await
        .context("look for animals without a fingerprint")?;
    if todo.is_empty() {
        return Ok(Outcome::Done);
    }
    let bytes = ctx
        .storage
        .get_bytes_limited(&keys::preview(owner, job.asset_id), MAX_PREVIEW_BYTES)
        .await
        .context("download preview")?;
    let _held = ctx.budget.reserve(TRANSIENT_BYTES).await?;
    let encoder = finder.encoder().await?;
    let started = Instant::now();
    let boxes: Vec<[f32; 4]> = todo.iter().map(|d| d.bbox).collect();
    let prints = tokio::task::spawn_blocking(move || {
        let (rgb, w, h) = decode(&bytes)?;
        drop(bytes);
        fingerprints(&encoder, &rgb, w, h, &boxes)
    })
    .await
    .context("pets task panicked")??;
    metrics::global().histogram(
        "pets.fingerprint_ms",
        started.elapsed().as_secs_f64() * 1000.0,
        &[("model", store::EMBED_MODEL)],
    );
    let rows: Vec<(Uuid, Option<Vec<f32>>)> = todo.iter().map(|d| d.id).zip(prints).collect();
    store::set_fingerprints(&ctx.db, &rows)
        .await
        .context("store fingerprints")?;
    Ok(Outcome::Done)
}

/// The stored shape of what was found: coordinates as fractions of the preview, strongest first.
pub fn to_new_detections(found: &[Detection], w: u32, h: u32) -> Vec<NewDetection> {
    let (fw, fh) = (w.max(1) as f32, h.max(1) as f32);
    let unit = |v: f32| v.clamp(0.0, 1.0);
    found
        .iter()
        .filter_map(|d| {
            let (x, y) = (unit(d.bbox[0] / fw), unit(d.bbox[1] / fh));
            let (bw, bh) = (
                unit((d.bbox[0] + d.bbox[2]) / fw) - x,
                unit((d.bbox[1] + d.bbox[3]) / fh) - y,
            );
            // A box that is nothing once it is inside the picture is not an animal.
            (bw > 0.0 && bh > 0.0).then(|| NewDetection {
                kind: d.kind.name().to_string(),
                bbox: [x, y, bw, bh],
                score: d.score,
                embedding: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use cyanotype_clip::{Model, zeroshot};

    use super::*;

    fn bbox(d: &NewDetection) -> [f32; 4] {
        d.bbox
    }

    #[test]
    fn boxes_are_stored_as_fractions_of_the_preview() {
        let d = Detection {
            kind: Kind::Cat,
            score: 0.37,
            bbox: [144.0, 108.0, 288.0, 216.0],
        };
        let out = to_new_detections(&[d], 1440, 1080);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].kind.as_str(), out[0].score), ("cat", 0.37));
        assert!(
            bbox(&out[0])
                .iter()
                .zip([0.1, 0.1, 0.2, 0.2])
                .all(|(a, b)| (a - b).abs() < 1e-6),
            "{:?}",
            out[0].bbox
        );
    }

    #[test]
    fn coordinates_stay_in_range_and_a_box_with_no_area_is_dropped() {
        let hang = Detection {
            kind: Kind::Dog,
            score: 0.5,
            bbox: [-3.0, 0.0, 2000.0, 10.0],
        };
        let nothing = Detection {
            kind: Kind::Dog,
            score: 0.5,
            bbox: [200.0, 20.0, 5.0, 5.0],
        };
        let out = to_new_detections(&[hang, nothing], 100, 100);
        assert_eq!(out.len(), 1, "a box wholly outside the picture is not an animal");
        assert!(out[0].bbox.iter().all(|v| (0.0..=1.0).contains(v)), "{:?}", out[0].bbox);
        assert_eq!(out[0].bbox[2], 1.0);
    }

    #[test]
    fn the_kinds_the_detector_names_are_the_kinds_the_database_and_api_know() {
        let ours: Vec<&str> = Kind::ALL.iter().map(|k| k.name()).collect();
        assert_eq!(ours, cyanotype_core::pets::KINDS.to_vec());
    }

    #[test]
    fn a_share_at_the_threshold_confirms_its_kind_and_one_below_does_not() {
        let min = zeroshot::MIN_SHARE;
        let kinds = |dog, cat| confirmed_kinds(PetShares { dog, cat });
        assert_eq!(kinds(min, 0.0), vec![Kind::Dog]);
        assert_eq!(kinds(0.0, min), vec![Kind::Cat]);
        assert_eq!(kinds(0.9, 0.9), vec![Kind::Dog, Kind::Cat]);
        assert!(kinds(min - 0.001, min - 0.001).is_empty());
    }

    #[test]
    fn pets_wait_behind_every_other_kind_of_job_faces_included() {
        let kinds: Vec<&str> = foreground_kinds().collect();
        for k in [
            jobs::KIND_THUMBNAIL,
            jobs::KIND_EMBED,
            jobs::KIND_OCR,
            jobs::KIND_FACES,
            jobs::KIND_TAKEOUT,
        ] {
            assert!(kinds.contains(&k), "{k}");
        }
        assert!(!kinds.contains(&jobs::KIND_PETS));
    }

    #[test]
    fn only_the_search_model_with_prompts_can_run_the_job() {
        assert!(Finder::new(1, Model::Siglip2B16).is_some());
        assert!(Finder::new(1, Model::ClipB32).is_none());
    }

    #[test]
    fn a_preview_that_is_not_a_webp_is_not_decodable() {
        assert!(webp::Decoder::new(b"GIF89a....").decode().is_none());
    }

    // ---- the job against a real database and bucket ---------------------------------------------------------------------------

    use cyanotype_core::{config::S3Config, db, storage::Storage};
    use sqlx::{Connection, Executor, PgConnection, PgPool};

    use crate::budget::MemoryBudget;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/cyanotype-clip/tests/fixtures/images")
    }

    /// Does the detector's model file exist? Without it the tests that run it are skipped, unless `CYANOTYPE_REQUIRE_MODEL` is set (CI).
    fn detector_installed() -> bool {
        if model_installed() {
            return true;
        }
        assert!(
            std::env::var_os("CYANOTYPE_REQUIRE_MODEL").is_none(),
            "CYANOTYPE_REQUIRE_MODEL is set but {} has no pet model",
            model_dir().display()
        );
        false
    }

    /// A migrated scratch database and a real bucket (the S3_* variables CI sets): or None, and the test skips.
    struct Env {
        pool: PgPool,
        ctx: Arc<Ctx>,
        admin_url: String,
        name: String,
    }

    impl Env {
        async fn new() -> Option<Self> {
            let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
            std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty())?;
            let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
            let mut admin = PgConnection::connect(&admin_url).await.unwrap();
            admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
            let (base, _) = admin_url.rsplit_once('/').unwrap();
            let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
            db::migrate(&pool).await.unwrap();
            let ctx = Arc::new(Ctx {
                db: pool.clone(),
                storage: Storage::new(&S3Config::from_env().expect("S3_* env")),
                max_original_bytes: 64 << 20,
                budget: MemoryBudget::new(512 << 20),
                raw_develop: false,
                search_model: Model::Siglip2B16,
            });
            Some(Self {
                pool,
                ctx,
                admin_url,
                name,
            })
        }

        async fn teardown(self) {
            self.pool.close().await;
            let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
            admin
                .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str())
                .await
                .unwrap();
        }

        /// A photo with a preview in the bucket (None: no object) and a search vector (None: not embedded yet), and its claimed job.
        async fn asset(&self, owner: Uuid, preview: Option<Vec<u8>>, embedding: Option<&[f32]>) -> (Uuid, jobs::Job) {
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash)
                 VALUES ($1, sha256($2::bytea), 'k', 'p.jpg', 'image/jpeg', 1, '\\x01') RETURNING id",
            )
            .bind(owner)
            .bind(Uuid::new_v4().as_bytes().to_vec())
            .fetch_one(&self.pool)
            .await
            .unwrap();
            if let Some(bytes) = preview {
                self.ctx
                    .storage
                    .put_bytes(&keys::preview(owner, id), bytes, "image/webp")
                    .await
                    .unwrap();
            }
            if let Some(v) = embedding {
                let m = Model::Siglip2B16;
                embeddings::store(&self.pool, id, m.id(), m.version(), v).await.unwrap();
            }
            store::enqueue_for_asset(&self.pool, id, store::VERSION).await.unwrap();
            let job = jobs::claim(&self.pool, jobs::KIND_PETS).await.unwrap().unwrap();
            assert_eq!(job.asset_id, id);
            (id, job)
        }
    }

    async fn owner(pool: &PgPool) -> Uuid {
        sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ($1, 'x') RETURNING id")
            .bind(format!("{}@example.com", Uuid::new_v4()))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// A fixture photo as the worker's own preview encoder writes it (1440 px at most, WebP quality 80).
    fn preview_of(name: &str) -> Vec<u8> {
        let img = image::open(fixtures().join(format!("{name}.jpg"))).unwrap();
        crate::imaging::encode_webp(&img, 80.0).unwrap()
    }

    /// What the search model's embedding of a photo of the thing named by prompt number `i` would be (the prompt's own vector: the check
    /// reads it as that thing, exactly).
    fn embedding_of_prompt(text: &str) -> Vec<f32> {
        let i = zeroshot::all_prompts().position(|p| p == text).unwrap();
        PetCheck::for_model(Model::Siglip2B16).unwrap().prompt_vectors()[i].clone()
    }

    fn dog_embedding() -> Vec<f32> {
        embedding_of_prompt("a photo of a dog")
    }

    fn cat_embedding() -> Vec<f32> {
        embedding_of_prompt("a photo of a cat")
    }

    fn person_embedding() -> Vec<f32> {
        embedding_of_prompt("a photo of a person")
    }

    async fn detections(pool: &PgPool, asset: Uuid) -> Vec<(String, f32, f32, f32, f32, f32)> {
        sqlx::query_as(
            "SELECT kind, box_x, box_y, box_w, box_h, score FROM pet_detections WHERE asset_id = $1 ORDER BY idx",
        )
        .bind(asset)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn scans(pool: &PgPool) -> Vec<(i32, i32, f32, f32)> {
        sqlx::query_as("SELECT version, detections, dog_share, cat_share FROM pet_scans ORDER BY scanned_at")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    fn finder() -> Arc<Finder> {
        Arc::new(Finder::new(1, Model::Siglip2B16).unwrap())
    }

    #[tokio::test]
    async fn a_dog_the_check_confirms_gets_its_box_and_running_again_replaces_it() {
        if !detector_installed() {
            return;
        }
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        let (id, job) = env.asset(owner, Some(preview_of("dog")), Some(&dog_embedding())).await;
        assert!(!finder.loaded());
        for _ in 0..2 {
            assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
            assert!(finder.loaded(), "the detector loads when a photo needs it");
            let rows = detections(&env.pool, id).await;
            assert!(!rows.is_empty() && rows.iter().all(|r| r.0 == "dog"), "{rows:?}");
            let (_, x, y, w, h, score) = rows[0].clone();
            assert!(
                x >= 0.0 && y >= 0.0 && x + w <= 1.0 + 1e-6 && y + h <= 1.0 + 1e-6 && w > 0.2 && h > 0.2,
                "a dog that fills a good part of the picture: {x} {y} {w} {h}"
            );
            assert!(score >= 0.1);
            let owner_of: Uuid = sqlx::query_scalar("SELECT owner_id FROM pet_detections WHERE asset_id = $1 LIMIT 1")
                .bind(id)
                .fetch_one(&env.pool)
                .await
                .unwrap();
            assert_eq!(owner_of, owner);
            // One scan row, however often the job runs, with what the check said.
            let s = scans(&env.pool).await;
            assert_eq!(s.len(), 1);
            assert_eq!((s[0].0, s[0].1 as usize), (store::VERSION, rows.len()));
            assert!(s[0].2 > 0.9 && s[0].3 < 0.1, "{s:?}");
        }
        env.teardown().await;
    }

    #[tokio::test]
    async fn a_cat_the_check_confirms_gets_its_box() {
        if !detector_installed() {
            return;
        }
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        let (id, job) = env.asset(owner, Some(preview_of("cat")), Some(&cat_embedding())).await;
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        let rows = detections(&env.pool, id).await;
        assert!(!rows.is_empty() && rows.iter().all(|r| r.0 == "cat"), "{rows:?}");
        env.teardown().await;
    }

    #[tokio::test]
    async fn the_check_decides_which_kinds_are_kept_whatever_the_detector_sees() {
        if !detector_installed() {
            return;
        }
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        // The picture is a dog. The check says: a cat only. The detector's dog is not kept, and no cat is found: no pets.
        let (a, job) = env.asset(owner, Some(preview_of("dog")), Some(&cat_embedding())).await;
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        assert!(detections(&env.pool, a).await.is_empty());
        let s = scans(&env.pool).await;
        assert_eq!(
            (s[0].1, s[0].2 < 0.1, s[0].3 > 0.9),
            (0, true, true),
            "the scan says what the check saw: {s:?}"
        );
        jobs::complete(&env.pool, job.id).await.unwrap();
        // The check says: a dog and a cat both. The detector finds a dog only.
        let mixed: Vec<f32> = dog_embedding()
            .iter()
            .zip(cat_embedding())
            .map(|(d, c)| d + c)
            .collect();
        let (b, job) = env.asset(owner, Some(preview_of("dog")), Some(&mixed)).await;
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        let kinds: Vec<String> = detections(&env.pool, b).await.into_iter().map(|r| r.0).collect();
        assert!(!kinds.is_empty() && kinds.iter().all(|k| k == "dog"), "{kinds:?}");
        env.teardown().await;
    }

    #[tokio::test]
    async fn a_pet_the_check_confirms_and_the_detector_cannot_find_is_a_photo_without_pets() {
        if !detector_installed() {
            return;
        }
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        // A horse that the check (wrongly) calls a dog: the boxes are the proof, and there are none.
        let (id, job) = env
            .asset(owner, Some(preview_of("horse")), Some(&dog_embedding()))
            .await;
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        assert!(detections(&env.pool, id).await.is_empty());
        let s = scans(&env.pool).await;
        assert_eq!(
            (s.len(), s[0].1, s[0].2 > 0.9),
            (1, 0, true),
            "scanned, no pets, and what the check said is kept"
        );
        env.teardown().await;
    }

    #[tokio::test]
    async fn a_photo_the_check_says_has_no_pet_is_scanned_without_its_preview_being_read() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        // No object in the bucket at all: reading the preview would fail. The check never asks for it, and the detector never loads.
        let (id, job) = env.asset(owner, None, Some(&person_embedding())).await;
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        assert!(!finder.loaded(), "no detector was needed");
        assert!(detections(&env.pool, id).await.is_empty());
        let s = scans(&env.pool).await;
        assert_eq!(
            (s.len(), s[0].1, s[0].2 < 0.1, s[0].3 < 0.1),
            (1, 0, true, true),
            "scanned, no pets: not the same as not scanned"
        );
        // Running the job again changes nothing.
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        assert_eq!(scans(&env.pool).await.len(), 1);
        env.teardown().await;
    }

    #[tokio::test]
    async fn a_photo_without_its_search_vector_is_skipped_and_recorded_as_nothing() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        let (id, job) = env.asset(owner, Some(preview_of("dog")), None).await;
        assert!(matches!(
            process(&env.ctx, &finder, &job).await.unwrap(),
            Outcome::Skipped(_)
        ));
        assert_eq!(
            scans(&env.pool).await,
            vec![],
            "not scanned: the sweep will queue it once it has a vector"
        );
        assert!(detections(&env.pool, id).await.is_empty());
        // ... and it does, as the vector arrives.
        jobs::complete(&env.pool, job.id).await.unwrap();
        sqlx::query("UPDATE jobs SET state = 'done' WHERE kind = 'pets'")
            .execute(&env.pool)
            .await
            .unwrap();
        let m = Model::Siglip2B16;
        assert_eq!(
            store::enqueue_missing(&env.pool, store::MODEL, store::VERSION, basis(m))
                .await
                .unwrap(),
            0
        );
        embeddings::store(&env.pool, id, m.id(), m.version(), &person_embedding())
            .await
            .unwrap();
        assert_eq!(
            store::enqueue_missing(&env.pool, store::MODEL, store::VERSION, basis(m))
                .await
                .unwrap(),
            1
        );
        env.teardown().await;
    }

    #[tokio::test]
    async fn a_photo_whose_embedding_is_being_made_again_waits_for_the_new_one() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        // The photo was rotated: its old embedding is still stored, and the embed job that replaces it has not run.
        let (id, job) = env.asset(owner, None, Some(&person_embedding())).await;
        let m = Model::Siglip2B16;
        jobs::enqueue(&env.pool, jobs::KIND_EMBED, id, m.version())
            .await
            .unwrap();
        assert!(matches!(
            process(&env.ctx, &finder, &job).await.unwrap(),
            Outcome::Skipped(_)
        ));
        assert_eq!(
            scans(&env.pool).await,
            vec![],
            "nothing was read from the old embedding"
        );
        // Once the embedding job is done, the same job reads the new one.
        sqlx::query("UPDATE jobs SET state = 'done' WHERE kind = 'embed'")
            .execute(&env.pool)
            .await
            .unwrap();
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        assert_eq!(scans(&env.pool).await.len(), 1);
        env.teardown().await;
    }

    #[tokio::test]
    async fn photos_that_cannot_be_scanned_are_skipped_and_failures_are_sorted() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        let dog = dog_embedding();
        let (purging, j_purging) = env.asset(owner, Some(vec![1]), Some(&dog)).await;
        let (hidden, j_hidden) = env.asset(owner, Some(vec![1]), Some(&dog)).await;
        let (gone, j_gone) = env.asset(owner, Some(vec![1]), Some(&dog)).await;
        let (no_preview, j_no_preview) = env.asset(owner, Some(vec![1]), Some(&dog)).await;
        let (corrupt, j_corrupt) = env.asset(owner, Some(b"not a webp at all".to_vec()), Some(&dog)).await;
        let (_, j_missing) = env.asset(owner, None, Some(&dog)).await;
        sqlx::query("UPDATE assets SET deleted_at = now(), purging_at = now() WHERE id = $1")
            .bind(purging)
            .execute(&env.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
            .bind(hidden)
            .bind(corrupt)
            .execute(&env.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE assets SET thumbhash = NULL WHERE id = $1")
            .bind(no_preview)
            .execute(&env.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM assets WHERE id = $1")
            .bind(gone)
            .execute(&env.pool)
            .await
            .unwrap();
        for j in [&j_purging, &j_hidden, &j_gone, &j_no_preview] {
            assert!(matches!(
                process(&env.ctx, &finder, j).await.unwrap(),
                Outcome::Skipped(_)
            ));
        }
        // A preview that is not a WebP will not become one by retrying; a preview that cannot be fetched is retried. (Both only matter to
        // a photo the check confirmed: the others never read it.)
        if detector_installed() {
            assert!(
                process(&env.ctx, &finder, &j_corrupt)
                    .await
                    .err()
                    .unwrap()
                    .is::<Permanent>()
            );
        }
        let err = process(&env.ctx, &finder, &j_missing).await.err().unwrap();
        assert!(!err.is::<Permanent>() && format!("{err:#}").contains("download preview"));
        assert_eq!(scans(&env.pool).await, vec![], "nothing was recorded for any of them");
        env.teardown().await;
    }

    #[tokio::test]
    async fn the_sweep_queues_what_is_missing_and_only_then_prunes_older_scans() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let m = Model::Siglip2B16;
        // Two photos the check will say have no pet (so the job needs no preview and no detector).
        let a = env.asset(owner, None, Some(&person_embedding())).await.0;
        let b = env.asset(owner, None, Some(&person_embedding())).await.0;
        sqlx::query("DELETE FROM jobs").execute(&env.pool).await.unwrap();
        // An older model's records for both photos.
        for id in [a, b] {
            let old = Scan {
                dog_share: 0.5,
                cat_share: 0.0,
                found: vec![],
                fingerprinted: false,
            };
            store::store(&env.pool, id, "an-older-model", 1, &old).await.unwrap();
        }
        sweep(&env.ctx, m).await;
        let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'pets' AND state = 'queued'")
            .fetch_one(&env.pool)
            .await
            .unwrap();
        assert_eq!(queued, 2);
        let old = || async {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pet_scans WHERE model = 'an-older-model'")
                .fetch_one(&env.pool)
                .await
                .unwrap()
        };
        assert_eq!(
            old().await,
            2,
            "the older model's records stay while the new one has not covered the library"
        );
        // Scan both; the next sweep finds nothing missing and drops the older records.
        let finder = finder();
        while let Some(job) = jobs::claim(&env.pool, jobs::KIND_PETS).await.unwrap() {
            assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
            jobs::complete(&env.pool, job.id).await.unwrap();
        }
        assert_eq!(
            scans(&env.pool).await.len(),
            2,
            "a scan by the new model replaced each older one"
        );
        sweep(&env.ctx, m).await;
        assert_eq!(old().await, 0);
        env.teardown().await;
    }

    async fn prints(pool: &PgPool, asset: Uuid) -> Vec<(Uuid, Option<Vec<f32>>, Option<String>)> {
        let rows: Vec<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, embedding::text, embed_model FROM pet_detections WHERE asset_id = $1 ORDER BY idx",
        )
        .bind(asset)
        .fetch_all(pool)
        .await
        .unwrap();
        rows.into_iter()
            .map(|(id, e, m)| {
                (
                    id,
                    e.map(|t| cyanotype_core::people_math::parse_literal(&t).unwrap()),
                    m,
                )
            })
            .collect()
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        cyanotype_core::people_math::cosine(a, b)
    }

    fn real_embedding(name: &str) -> Vec<f32> {
        let m = Model::Siglip2B16;
        let enc = cyanotype_clip::ImageEncoder::load(m, &m.dir(), 1).unwrap();
        let img = image::open(fixtures().join(format!("{name}.jpg"))).unwrap().to_rgb8();
        let pre = cyanotype_clip::preprocess_rgb8(m, img.width(), img.height(), img.as_raw()).unwrap();
        enc.embed(&[pre]).unwrap().remove(0)
    }

    /// Both models' files, or a skip (unless CI says they must be there).
    fn models_installed() -> bool {
        let m = Model::Siglip2B16;
        let vision = m.dir().join(m.vision_file());
        if !vision.exists() {
            assert!(
                std::env::var_os("CYANOTYPE_REQUIRE_MODEL").is_none(),
                "CYANOTYPE_REQUIRE_MODEL is set but {} is missing",
                vision.display()
            );
            return false;
        }
        detector_installed()
    }

    #[tokio::test]
    async fn an_animal_is_fingerprinted_in_the_job_that_finds_it_and_the_same_dog_twice_is_alike() {
        if !models_installed() {
            return;
        }
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        let dog = real_embedding("dog");
        let mut seen = Vec::new();
        for name in ["dog", "dog", "cat"] {
            let embedding = if name == "dog" {
                dog.clone()
            } else {
                real_embedding("cat")
            };
            let (id, job) = env.asset(owner, Some(preview_of(name)), Some(&embedding)).await;
            assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
            jobs::complete(&env.pool, job.id).await.unwrap();
            seen.push(prints(&env.pool, id).await);
        }
        for p in seen.iter().flatten() {
            let v = p.1.as_ref().expect("a fingerprint");
            assert_eq!(v.len(), store::EMBED_DIM);
            assert!((v.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-2, "unit length");
            assert_eq!(p.2.as_deref(), Some(store::EMBED_MODEL));
        }
        let (d1, d2, c) = (
            seen[0][0].1.as_ref().unwrap(),
            seen[1][0].1.as_ref().unwrap(),
            seen[2][0].1.as_ref().unwrap(),
        );
        assert!(cosine(d1, d2) > 0.99, "the same picture twice: {}", cosine(d1, d2));
        assert!(cosine(d1, c) < 0.9, "a dog and a cat: {}", cosine(d1, c));
        // The same dog twice is a proposal for a pet that has the first one: end to end through the stored numbers.
        let first: Uuid = seen[0][0].0;
        let pet: Uuid =
            sqlx::query_scalar("INSERT INTO pets (owner_id, name, kind) VALUES ($1, 'Biscuit', 'dog') RETURNING id")
                .bind(owner)
                .fetch_one(&env.pool)
                .await
                .unwrap();
        sqlx::query("UPDATE pet_detections SET pet_id = $2 WHERE id = $1")
            .bind(first)
            .bind(pet)
            .execute(&env.pool)
            .await
            .unwrap();
        let got = cyanotype_core::named_pets::suggestions(&env.pool, owner, pet, false, 10)
            .await
            .unwrap();
        assert_eq!(
            got.iter().map(|g| g.detection_id).collect::<Vec<_>>(),
            vec![seen[1][0].0],
            "{got:?}"
        );
        assert_eq!(got[0].tier, cyanotype_core::named_pets::Tier::Likely);
        env.teardown().await;
    }

    #[tokio::test]
    async fn animals_found_before_fingerprints_get_them_without_being_found_again_and_keep_their_pet() {
        if !models_installed() {
            return;
        }
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        let (id, job) = env
            .asset(owner, Some(preview_of("dog")), Some(&real_embedding("dog")))
            .await;
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        let before = prints(&env.pool, id).await;
        assert!(before.iter().all(|p| p.1.is_some()));
        // As the library was before named pets: the animals are there, without a fingerprint, and the owner has put one in a pet.
        let pet: Uuid =
            sqlx::query_scalar("INSERT INTO pets (owner_id, name, kind) VALUES ($1, 'Biscuit', 'dog') RETURNING id")
                .bind(owner)
                .fetch_one(&env.pool)
                .await
                .unwrap();
        sqlx::query("UPDATE pet_detections SET embedding = NULL, embed_model = NULL, pet_id = $2 WHERE id = $1")
            .bind(before[0].0)
            .bind(pet)
            .execute(&env.pool)
            .await
            .unwrap();
        assert_eq!(
            store::unfingerprinted(&env.pool, id, store::MODEL).await.unwrap().len(),
            1
        );
        jobs::complete(&env.pool, job.id).await.unwrap();
        // The sweep queues the photo; the job fingerprints without finding again.
        assert_eq!(
            store::enqueue_missing(&env.pool, store::MODEL, store::VERSION, basis(Model::Siglip2B16))
                .await
                .unwrap(),
            1
        );
        let job = jobs::claim(&env.pool, jobs::KIND_PETS).await.unwrap().unwrap();
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        let after = prints(&env.pool, id).await;
        assert_eq!(after[0].0, before[0].0, "the same animal, the same row");
        assert!(cosine(after[0].1.as_ref().unwrap(), before[0].1.as_ref().unwrap()) > 0.999);
        let pet_of: Option<Uuid> = sqlx::query_scalar("SELECT pet_id FROM pet_detections WHERE id = $1")
            .bind(after[0].0)
            .fetch_one(&env.pool)
            .await
            .unwrap();
        assert_eq!(pet_of, Some(pet));
        assert!(
            store::unfingerprinted(&env.pool, id, store::MODEL)
                .await
                .unwrap()
                .is_empty()
        );
        env.teardown().await;
    }

    #[test]
    fn a_crop_that_is_too_small_has_no_fingerprint_and_a_missing_box_none() {
        if !model_installed() {
            return;
        }
        let enc = DinoEncoder::load(&model_dir().join(cyanotype_pets::DINO_FILE), 1).unwrap();
        let (w, h) = (800u32, 600u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
        // 3 % of the width is 24 px, with the padding 29: under the minimum. 20 % is 160 px: in.
        let out = fingerprints(
            &enc,
            &rgb,
            w,
            h,
            &[[0.1, 0.1, 0.03, 0.04], [0.1, 0.1, 0.2, 0.3], [0.5, 0.5, 0.0, 0.0]],
        )
        .unwrap();
        assert!(out[0].is_none() && out[2].is_none());
        let v = out[1].as_ref().expect("a big enough crop");
        assert_eq!(v.len(), store::EMBED_DIM);
    }

    /// The real image tower and the real check on four of the search tests' fixture photos: the whole job as the worker runs it.
    #[tokio::test]
    async fn the_real_search_embedding_of_a_dog_a_cat_a_horse_and_a_car_through_the_whole_job() {
        let m = Model::Siglip2B16;
        let vision = m.dir().join(m.vision_file());
        if !vision.exists() || !detector_installed() {
            assert!(
                std::env::var_os("CYANOTYPE_REQUIRE_MODEL").is_none(),
                "CYANOTYPE_REQUIRE_MODEL is set but {} is missing",
                vision.display()
            );
            return;
        }
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = finder();
        let enc = cyanotype_clip::ImageEncoder::load(m, &m.dir(), 1).unwrap();
        let embed = |name: &str| -> Vec<f32> {
            let img = image::open(fixtures().join(format!("{name}.jpg"))).unwrap().to_rgb8();
            let pre = cyanotype_clip::preprocess_rgb8(m, img.width(), img.height(), img.as_raw()).unwrap();
            enc.embed(&[pre]).unwrap().remove(0)
        };
        let mut seen = Vec::new();
        for name in ["dog", "cat", "horse", "car"] {
            let (id, job) = env.asset(owner, Some(preview_of(name)), Some(&embed(name))).await;
            assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
            jobs::complete(&env.pool, job.id).await.unwrap();
            let kinds: Vec<String> = detections(&env.pool, id).await.into_iter().map(|r| r.0).collect();
            seen.push((name, kinds));
        }
        let only = |kind: &str, n: usize| vec![kind.to_string(); n];
        assert!(
            !seen[0].1.is_empty() && seen[0].1 == only("dog", seen[0].1.len()),
            "{seen:?}"
        );
        assert!(
            !seen[1].1.is_empty() && seen[1].1 == only("cat", seen[1].1.len()),
            "{seen:?}"
        );
        assert!(
            seen[2].1.is_empty() && seen[3].1.is_empty(),
            "a horse and a car are no pets: {seen:?}"
        );
        env.teardown().await;
    }
}
