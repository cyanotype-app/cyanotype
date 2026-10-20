//! The `ocr` job: the text (and QR codes) on each photo, read from its `preview.webp` (never the original, so there is no big
//! decode), stored in `asset_text` for search, copy and the viewer's text overlay (`docs/library.md`, "Text in photos").
//!
//! Like embedding it is background work in a loop of its own, and the *least* urgent kind: it takes a job only when no
//! thumbnail, metadata, embed or other foreground job is runnable, and it shares the embed loop's one-at-a-time permit, so
//! the two ONNX models are never in memory working at once. A photo's job is queued when its embedding is stored; the sweep
//! queues the rest (a library that existed before, a model switch). The models load on the first job and are dropped
//! again after a quiet spell (`IDLE_UNLOAD`), so a library that is fully read does not keep them resident. While a job runs it
//! holds `TRANSIENT_BYTES` of the shared byte budget (`WORKER_MEMORY_BUDGET_MB`): the decoded preview and the networks'
//! activations, measured in docs/build.md.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use cyanotype_core::{jobs, keys, metrics, ocr as store};
use cyanotype_ocr::{Config, DetParams, Engine, Files, Line, qr::QrCode};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    finish,
    handlers::{Ctx, Outcome, Permanent},
};

/// A preview is at most 1440 px on the long edge; anything bigger than this is not ours.
const MAX_PREVIEW_BYTES: u64 = 16 << 20;
/// Budget held while a photo is read: the decoded preview (8 MB at the largest), the detection network's activations at
/// 960 px and the recognition batches: the highest process peak measured over the 38 bake-off images was 268 MB with the models
/// loaded (78 MB), so 190 MB above them; 224 MB leaves margin (docs/build.md).
pub const TRANSIENT_BYTES: u64 = 224 << 20;
/// How long the models stay loaded with nothing to read.
const IDLE_UNLOAD: Duration = Duration::from_secs(300);
/// How often the sweep that queues unread photos runs.
const SWEEP_EVERY: Duration = Duration::from_secs(900);
const FALLBACK_POLL: Duration = Duration::from_secs(30);
const FOREGROUND_RECHECK: Duration = Duration::from_secs(1);

/// Where the model files are: `OCR_MODEL_DIR`, else `<SEARCH_MODEL_DIR>/ocr` (the images install them there).
pub fn model_dir() -> PathBuf {
    std::env::var_os("OCR_MODEL_DIR").map_or_else(|| cyanotype_clip::models_root().join("ocr"), PathBuf::from)
}

fn files() -> Files {
    let d = model_dir();
    Files {
        det: d.join("det.onnx"),
        rec: d.join("rec.onnx"),
        dict: d.join("rec.yml"),
    }
}

/// Are the model files installed? A dev checkout that skipped `scripts/fetch-models.sh` runs without reading text, and its `ocr`
/// jobs stay queued.
pub fn model_installed() -> bool {
    let f = files();
    f.det.exists() && f.rec.exists() && f.dict.exists()
}

/// The settings PP-OCRv6 small's own `inference.yml` gives its postprocessing (`models/ocr.lock`).
pub fn config(threads: usize) -> Config {
    Config {
        det: DetParams {
            thresh: 0.2,
            box_thresh: 0.45,
            unclip: 1.4,
            ..DetParams::default()
        },
        threads,
        ..Config::default()
    }
}

/// The (lazily loaded) networks.
pub struct Reader {
    threads: usize,
    engine: Mutex<Option<(Arc<Engine>, Instant)>>,
}

impl Reader {
    pub fn new(threads: usize) -> Self {
        Self {
            threads,
            engine: Mutex::new(None),
        }
    }

    async fn engine(&self) -> Result<Arc<Engine>> {
        if let Some((e, used)) = self.engine.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            *used = Instant::now();
            return Ok(e.clone());
        }
        let threads = self.threads;
        let started = Instant::now();
        let e = tokio::task::spawn_blocking(move || Engine::load(&files(), config(threads)))
            .await
            .context("OCR model load task panicked")??;
        tracing::info!(
            model = store::MODEL,
            threads,
            load_ms = started.elapsed().as_millis() as u64,
            "text model loaded"
        );
        let e = Arc::new(e);
        *self.engine.lock().unwrap_or_else(|p| p.into_inner()) = Some((e.clone(), Instant::now()));
        Ok(e)
    }

    fn unload_if_idle(&self) {
        let mut guard = self.engine.lock().unwrap_or_else(|p| p.into_inner());
        if guard.as_ref().is_some_and(|(_, used)| used.elapsed() >= IDLE_UNLOAD) {
            *guard = None;
            tracing::info!("text model unloaded after a quiet spell");
        }
    }

    fn loaded(&self) -> bool {
        self.engine.lock().unwrap_or_else(|p| p.into_inner()).is_some()
    }
}

/// Kinds that must not wait behind reading text: everything else, embedding included.
fn foreground_kinds() -> impl Iterator<Item = &'static str> {
    jobs::DEFAULT_PIPELINE.iter().copied().chain([
        jobs::KIND_EMBED,
        jobs::KIND_PAIR,
        jobs::KIND_GEOCODE,
        jobs::KIND_SHARE_COPY,
        jobs::KIND_TAKEOUT,
    ])
}

/// The ocr loop. `busy` is the embed loop's permit: held for each job, so shutdown can wait for it and the two models never
/// work at the same time.
pub async fn run_loop(ctx: Arc<Ctx>, reader: Arc<Reader>, waker: jobs::JobWaker, busy: Arc<Semaphore>) {
    let foreground: Vec<&str> = foreground_kinds().collect();
    let mut last_sweep: Option<Instant> = None;
    loop {
        metrics::global().gauge("ocr.model_loaded", if reader.loaded() { 1.0 } else { 0.0 }, &[]);
        if last_sweep.is_none_or(|t| t.elapsed() >= SWEEP_EVERY) {
            last_sweep = Some(Instant::now());
            sweep(&ctx).await;
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
        let job = match jobs::claim(&ctx.db, jobs::KIND_OCR).await {
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
            if reader.loaded() {
                reader.unload_if_idle();
            }
            jobs::wait_for_work(&ctx.db, &waker, &[jobs::KIND_OCR], SWEEP_EVERY.min(FALLBACK_POLL)).await;
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
        let result = process(&ctx, &reader, &job).instrument(span.clone()).await;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if matches!(result, Ok(Outcome::Done)) {
            metrics::global().histogram("ocr.duration", elapsed, &[("model", store::MODEL)]);
        }
        finish::record(&ctx, &job, "image", result, elapsed)
            .instrument(span)
            .await;
        drop(permit);
    }
}

/// Queue what has not been read by the current model (a library that existed before, a model switch, lost jobs).
async fn sweep(ctx: &Ctx) {
    match store::enqueue_missing(&ctx.db, store::MODEL, store::VERSION).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            queued = n,
            model = store::MODEL,
            "queued photos whose text has not been read"
        ),
        Err(e) => tracing::warn!(error = %format!("{e:#}"), "text sweep failed"),
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    owner_id: Uuid,
    hidden: bool,
    purging: bool,
    has_preview: bool,
}

async fn process(ctx: &Arc<Ctx>, reader: &Arc<Reader>, job: &jobs::Job) -> Result<Outcome> {
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
        return Ok(Outcome::Skipped("hidden RAW: the visible photo is the one read"));
    }
    if !r.has_preview {
        return Ok(Outcome::Skipped("no preview yet"));
    }
    let bytes = ctx
        .storage
        .get_bytes_limited(&keys::preview(r.owner_id, job.asset_id), MAX_PREVIEW_BYTES)
        .await
        .context("download preview")?;
    let _held = ctx.budget.reserve(TRANSIENT_BYTES).await?;
    let engine = reader.engine().await?;
    let found = tokio::task::spawn_blocking(move || read(&engine, &bytes))
        .await
        .context("ocr task panicked")??;
    store::store(&ctx.db, job.asset_id, store::MODEL, store::VERSION, &found)
        .await
        .context("store text")?;
    Ok(Outcome::Done)
}

/// Decode a preview and read it. A preview that cannot be decoded is a permanent failure of that photo alone.
pub fn read(engine: &Engine, webp: &[u8]) -> Result<store::Found> {
    let img = webp::Decoder::new(webp)
        .decode()
        .ok_or_else(|| anyhow::Error::from(Permanent("preview is not a decodable WebP".into())))?;
    let (w, h) = (img.width(), img.height());
    let px: &[u8] = &img;
    let rgb: std::borrow::Cow<[u8]> = if img.is_alpha() {
        // Over white, so transparent graphics (a logo, a sticker) are not read as black on black.
        std::borrow::Cow::Owned(
            px.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    let a = u32::from(p[3]);
                    [0, 1, 2].map(|c| ((u32::from(p[c]) * a + 255 * (255 - a)) / 255) as u8)
                })
                .collect(),
        )
    } else {
        std::borrow::Cow::Borrowed(px)
    };
    let lines = engine.read(&rgb, w, h)?;
    let qr = cyanotype_ocr::qr::detect(&rgb, w, h);
    Ok(found(&lines, &qr))
}

fn q(quad: &[[f32; 2]; 4]) -> Value {
    let r = |v: f32| (f64::from(v) * 10_000.0).round() / 10_000.0;
    json!(quad.iter().flatten().map(|v| r(*v)).collect::<Vec<_>>())
}

/// The stored shape of what was read (`cyanotype_core::ocr::Found`).
pub fn found(lines: &[Line], qr: &[QrCode]) -> store::Found {
    store::Found {
        text: cyanotype_ocr::plain_text(lines),
        lines: Value::Array(
            lines
                .iter()
                .map(|l| {
                    json!({
                        "t": l.text,
                        "s": (f64::from(l.score) * 1000.0).round() / 1000.0,
                        "q": q(&l.quad),
                        "w": l.words.iter().map(|w| json!({"t": w.text, "q": q(&w.quad)})).collect::<Vec<_>>(),
                    })
                })
                .collect(),
        ),
        qr: Value::Array(qr.iter().map(|c| json!({"t": c.text, "q": q(&c.quad)})).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_shape_has_text_lines_words_and_codes() {
        let line = Line {
            text: "Total 4.20".into(),
            score: 0.93456,
            quad: [[0.1, 0.1], [0.5, 0.1], [0.5, 0.15], [0.1, 0.15]],
            words: vec![cyanotype_ocr::Word {
                text: "Total".into(),
                quad: [[0.1, 0.1], [0.3, 0.1], [0.3, 0.15], [0.1, 0.15]],
            }],
        };
        let code = QrCode {
            text: "https://example.org".into(),
            quad: [[0.6, 0.6], [0.8, 0.6], [0.8, 0.8], [0.6, 0.8]],
        };
        let f = found(&[line], &[code]);
        assert_eq!(f.text, "Total 4.20");
        assert_eq!(f.lines[0]["t"], "Total 4.20");
        assert_eq!(f.lines[0]["s"], 0.935);
        assert_eq!(f.lines[0]["q"].as_array().unwrap().len(), 8);
        assert_eq!(f.lines[0]["w"][0]["t"], "Total");
        assert_eq!(f.qr[0]["t"], "https://example.org");
        assert_eq!(found(&[], &[]).lines, json!([]));
    }

    #[test]
    fn a_preview_that_is_not_a_webp_is_not_decodable() {
        assert!(webp::Decoder::new(b"GIF89a....").decode().is_none());
    }

    /// The whole path on a real photo with the real model, when both are there (`scripts/fetch-models.sh`, and a picture in
    /// `OCR_TEST_IMAGE`, e.g. one of the bake-off set): a preview WebP in, the text and the stored shape out. Skipped otherwise.
    #[test]
    fn reads_a_real_preview_end_to_end_when_the_model_and_a_picture_are_installed() {
        let (Some(img), true) = (std::env::var_os("OCR_TEST_IMAGE"), model_installed()) else {
            return;
        };
        let rgb = image::open(img).unwrap().to_rgb8();
        let (w, h) = rgb.dimensions();
        let webp = ::webp::Encoder::from_rgb(rgb.as_raw(), w, h).encode(85.0).to_vec();
        let engine = Engine::load(&files(), config(1)).unwrap();
        let found = read(&engine, &webp).unwrap();
        let t = found.text.to_lowercase();
        assert!(t.contains("abbey") && t.contains("westminster"), "{:?}", found.text);
        assert!(
            found
                .lines
                .as_array()
                .unwrap()
                .iter()
                .all(|l| l["q"].as_array().unwrap().len() == 8)
        );
    }
}
