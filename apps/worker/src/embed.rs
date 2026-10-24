//! The `embed` job: an image embedding (with the model `SEARCH_MODEL` names) of each photo, computed from the photo's `preview.webp` (never the original,
//! so there is no big decode and nothing to refuse for memory), stored in `asset_embeddings`.
//!
//! Embedding is background work. It runs in its own loop, apart from the thumbnail/metadata loop, and takes a batch
//! only when no thumbnail, metadata or other foreground job is runnable, so uploads are always finished first and the
//! 50,000-photo backfill only uses what is left. The ONNX session is loaded on the first job, runs with few threads
//! (`WORKER_EMBED_THREADS`, default 1) and is dropped again after a quiet spell, so a library that is fully indexed
//! does not keep ~150 MB resident.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use cyanotype_clip::{ImageEncoder, Model, preprocess_rgb8};
use cyanotype_core::{bursts, embeddings, jobs, keys, metrics};
use tokio::{sync::Semaphore, task::JoinSet};
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    finish,
    handlers::{Ctx, Outcome, Permanent},
};

/// Jobs taken per round. Their previews download concurrently (so S3 latency overlaps with inference) and run as one
/// ONNX batch; more than this only adds latency and memory.
const BATCH: i64 = 4;
/// A preview is at most 1440 px on the long edge; anything bigger than this is not ours.
const MAX_PREVIEW_BYTES: u64 = 16 << 20;
/// Budget held per photo of a batch while it is decoded and preprocessed: the RGBA decode (about 8 MB at the largest
/// preview), the resized copy and the float tensor (0.6 MB).
const PER_IMAGE_BYTES: u64 = 24 << 20;
/// How long the session stays loaded with nothing to embed.
const IDLE_UNLOAD: Duration = Duration::from_secs(300);
/// How often the sweep that catches stragglers (a model swap, an unpaired RAW) runs.
const SWEEP_EVERY: Duration = Duration::from_secs(900);
/// Fallback wake-up of an idle embed loop; notifications normally wake it at once.
const FALLBACK_POLL: Duration = Duration::from_secs(30);
/// While foreground jobs are runnable the embed loop looks again this often.
const FOREGROUND_RECHECK: Duration = Duration::from_secs(1);

/// The (lazily loaded) image tower.
pub struct Embedder {
    model: Model,
    threads: usize,
    session: Mutex<Option<(Arc<ImageEncoder>, Instant)>>,
}

impl Embedder {
    /// The session loads on first use.
    pub fn new(model: Model, threads: usize) -> Self {
        Self {
            model,
            threads,
            session: Mutex::new(None),
        }
    }

    /// Are the model files installed? A dev checkout that skipped `scripts/fetch-models.sh` runs without embedding, and
    /// its embed jobs stay queued.
    pub fn model_installed(model: Model) -> bool {
        model.dir().join(model.vision_file()).exists()
    }

    async fn encoder(&self) -> Result<Arc<ImageEncoder>> {
        if let Some((enc, used)) = self.session.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            *used = Instant::now();
            return Ok(enc.clone());
        }
        let (model, threads) = (self.model, self.threads);
        let started = Instant::now();
        let enc = tokio::task::spawn_blocking(move || ImageEncoder::load(model, &model.dir(), threads))
            .await
            .context("model load task panicked")??;
        tracing::info!(
            model = model.id(),
            threads,
            load_ms = started.elapsed().as_millis() as u64,
            "image model loaded"
        );
        let enc = Arc::new(enc);
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some((enc.clone(), Instant::now()));
        Ok(enc)
    }

    /// Drop the session if it has been idle for [`IDLE_UNLOAD`].
    fn unload_if_idle(&self) {
        let mut guard = self.session.lock().unwrap_or_else(|p| p.into_inner());
        if guard.as_ref().is_some_and(|(_, used)| used.elapsed() >= IDLE_UNLOAD) {
            *guard = None;
            tracing::info!("image model unloaded after a quiet spell");
        }
    }

    fn loaded(&self) -> bool {
        self.session.lock().unwrap_or_else(|p| p.into_inner()).is_some()
    }
}

/// Kinds that must not wait behind embedding.
fn foreground_kinds() -> impl Iterator<Item = &'static str> {
    jobs::DEFAULT_PIPELINE.iter().copied().chain([
        jobs::KIND_PAIR,
        jobs::KIND_GEOCODE,
        jobs::KIND_SHARE_COPY,
        jobs::KIND_EDITED_COPY,
        jobs::KIND_TAKEOUT,
    ])
}

/// The embed loop. `busy` is held for the length of each batch, so shutdown can wait for it.
pub async fn run_loop(ctx: Arc<Ctx>, embedder: Arc<Embedder>, waker: jobs::JobWaker, busy: Arc<Semaphore>) {
    let model = embedder.model;
    let foreground: Vec<&str> = foreground_kinds().collect();
    let mut last_sweep: Option<Instant> = None;
    loop {
        metrics::global().gauge("embed.model_loaded", if embedder.loaded() { 1.0 } else { 0.0 }, &[]);
        if last_sweep.is_none_or(|t| t.elapsed() >= SWEEP_EVERY) {
            last_sweep = Some(Instant::now());
            sweep(&ctx, model).await;
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
        let batch = match jobs::claim_batch(&ctx.db, jobs::KIND_EMBED, BATCH).await {
            Ok(b) => b,
            Err(e) => {
                drop(permit);
                tracing::error!(error = %format!("{e:#}"), "claim failed");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        if batch.is_empty() {
            drop(permit);
            if embedder.loaded() {
                embedder.unload_if_idle();
            }
            let nap = SWEEP_EVERY.min(FALLBACK_POLL);
            jobs::wait_for_work(&ctx.db, &waker, &[jobs::KIND_EMBED], nap).await;
            continue;
        }
        process(&ctx, &embedder, batch).await;
        drop(permit);
    }
}

/// Queue what is missing for `model`; once nothing is, drop the vectors of the models it replaced (search kept answering
/// from them until now, `docs/library.md`, "Changing the search model").
async fn sweep(ctx: &Ctx, model: Model) {
    burst_sweep(ctx).await;
    let (id, version) = (model.id(), model.version());
    match embeddings::enqueue_missing(&ctx.db, id, version).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(queued = n, model = id, version, "queued photos missing an embedding"),
        Err(e) => tracing::warn!(error = %format!("{e:#}"), "embedding sweep failed"),
    }
    match embeddings::missing_count(&ctx.db, id, version).await {
        Ok(0) => match embeddings::prune_superseded(&ctx.db, id, version).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(removed = n, model = id, "removed vectors of replaced models"),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "pruning replaced embeddings failed"),
        },
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %format!("{e:#}"), "counting missing embeddings failed"),
    }
}

/// The burst sweep: photos never grouped, or grouped under older rules, and the finished jobs of the last days. Runs with
/// the embedding sweep (a model change re-embeds the library, and every vector queues its own regroup besides).
async fn burst_sweep(ctx: &Ctx) {
    match bursts::enqueue_missing(&ctx.db).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(queued = n, "queued photos to be grouped into bursts"),
        Err(e) => tracing::warn!(error = %format!("{e:#}"), "burst sweep failed"),
    }
    if let Err(e) = bursts::prune_jobs(&ctx.db).await {
        tracing::warn!(error = %format!("{e:#}"), "pruning burst jobs failed");
    }
}

/// A job settled before any inference: skipped, or failed.
enum Prepared {
    Skip(&'static str),
    Failed(anyhow::Error),
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    owner_id: Uuid,
    hidden: bool,
    purging: bool,
    has_preview: bool,
}

async fn process(ctx: &Arc<Ctx>, embedder: &Arc<Embedder>, batch: Vec<jobs::Job>) {
    let started = Instant::now();
    let ids: Vec<Uuid> = batch.iter().map(|j| j.asset_id).collect();
    let rows: Result<Vec<Row>> = sqlx::query_as(
        "SELECT id, owner_id, primary_id IS NOT NULL AS hidden, purging_at IS NOT NULL AS purging,
                thumbhash IS NOT NULL AS has_preview
         FROM assets WHERE id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&ctx.db)
    .await
    .map_err(Into::into);
    let rows: HashMap<Uuid, Row> = match rows {
        Ok(r) => r.into_iter().map(|r| (r.id, r)).collect(),
        Err(e) => {
            // Could not even read the assets: every job retries.
            let msg = format!("load assets: {e:#}");
            for job in &batch {
                finish_job(ctx, job, Err(anyhow::anyhow!(msg.clone())), started).await;
            }
            return;
        }
    };

    // Decide per job, then download the previews of the ones that need a vector, concurrently.
    let mut results: Vec<Option<Prepared>> = Vec::with_capacity(batch.len());
    let mut downloads: JoinSet<(usize, Result<Vec<u8>>)> = JoinSet::new();
    for (i, job) in batch.iter().enumerate() {
        results.push(match rows.get(&job.asset_id) {
            None => Some(Prepared::Skip("asset deleted")),
            Some(r) if r.purging => Some(Prepared::Skip("asset is being deleted")),
            Some(r) if r.hidden => Some(Prepared::Skip("hidden RAW: the visible photo is the one searched")),
            Some(r) if !r.has_preview => Some(Prepared::Skip("no preview yet")),
            Some(r) => {
                let (storage, key) = (ctx.storage.clone(), keys::preview(r.owner_id, r.id));
                downloads.spawn(async move {
                    let bytes = storage
                        .get_bytes_limited(&key, MAX_PREVIEW_BYTES)
                        .await
                        .context("download preview");
                    (i, bytes)
                });
                None
            }
        });
    }
    let wanted = results.iter().filter(|r| r.is_none()).count() as u64;
    let mut previews: Vec<(usize, Vec<u8>)> = Vec::new();
    while let Some(done) = downloads.join_next().await {
        match done {
            Ok((i, Ok(bytes))) => previews.push((i, bytes)),
            Ok((i, Err(e))) => results[i] = Some(Prepared::Failed(e)),
            Err(e) => tracing::error!(error = %e, "preview download task failed"),
        }
    }
    previews.sort_by_key(|(i, _)| *i);

    // Decode + preprocess + infer off the async threads, holding budget for the pixels.
    let mut vectors: HashMap<usize, Result<Vec<f32>>> = HashMap::new();
    if !previews.is_empty() {
        match embed_previews(ctx, embedder, previews).await {
            Ok(out) => vectors.extend(out),
            Err(e) => {
                // The model could not load or run: nothing in this batch is the photo's fault, retry all of it.
                let msg = format!("{e:#}");
                for (i, slot) in results.iter_mut().enumerate() {
                    if slot.is_none() {
                        *slot = Some(Prepared::Failed(anyhow::anyhow!(msg.clone())));
                        vectors.remove(&i);
                    }
                }
            }
        }
    }

    let per_image_ms = started.elapsed().as_secs_f64() * 1000.0 / wanted.max(1) as f64;
    let m = metrics::global();
    if wanted > 0 {
        m.histogram("embed.batch_size", wanted as f64, &[("model", embedder.model.id())]);
    }
    for (i, job) in batch.iter().enumerate() {
        let result: Result<Outcome> = match results[i].take() {
            Some(Prepared::Skip(reason)) => Ok(Outcome::Skipped(reason)),
            Some(Prepared::Failed(e)) => Err(e),
            None => match vectors.remove(&i) {
                Some(Ok(v)) => store(ctx, embedder.model, job, &v).await,
                Some(Err(e)) => Err(e),
                None => Err(anyhow::anyhow!("no embedding was produced")),
            },
        };
        if result.is_ok() && matches!(result, Ok(Outcome::Done)) {
            m.histogram("embed.duration", per_image_ms, &[("model", embedder.model.id())]);
        }
        finish_job(ctx, job, result, started).await;
    }
}

async fn finish_job(ctx: &Arc<Ctx>, job: &jobs::Job, result: Result<Outcome>, started: Instant) {
    let span = tracing::info_span!(
        "job",
        job.id = job.id,
        job.kind = %job.kind,
        job.attempts = job.attempts,
        asset.id = %job.asset_id
    );
    finish::record(ctx, job, "image", result, started.elapsed().as_secs_f64() * 1000.0)
        .instrument(span)
        .await;
}

async fn store(ctx: &Arc<Ctx>, model: Model, job: &jobs::Job, v: &[f32]) -> Result<Outcome> {
    embeddings::store(&ctx.db, job.asset_id, model.id(), model.version(), v)
        .await
        .context("store embedding")?;
    // Its text is read next, after everything else (a failure here is not the embedding's either).
    if let Err(e) = cyanotype_core::ocr::enqueue_for_asset(&ctx.db, job.asset_id, cyanotype_core::ocr::VERSION).await {
        tracing::warn!(error = %format!("{e:#}"), "could not queue the text job");
    }
    // ... and its faces, when the feature is on (it is off by default).
    if cyanotype_core::faces::enabled()
        && let Err(e) =
            cyanotype_core::faces::enqueue_for_asset(&ctx.db, job.asset_id, cyanotype_core::faces::VERSION).await
    {
        tracing::warn!(error = %format!("{e:#}"), "could not queue the faces job");
    }
    // The photo can be compared with its neighbours now: regroup its burst (a failure here is not the embedding's).
    if let Err(e) = bursts::requeue_after_embed(&ctx.db, job.asset_id).await {
        tracing::warn!(error = %format!("{e:#}"), "could not queue the burst regroup");
    }
    // ... and a near-duplicate scan of the library is due once the import goes quiet (one scan per import, not per photo).
    if let Err(e) = cyanotype_core::duplicates::request_for_asset(&ctx.db, job.asset_id).await {
        tracing::warn!(error = %format!("{e:#}"), "could not request a near-duplicate scan");
    }
    Ok(Outcome::Done)
}

/// Decode, preprocess and embed `previews` (keyed by their position in the batch). A preview that cannot be decoded is a
/// permanent failure of that photo alone; an error from the model fails the whole call.
async fn embed_previews(
    ctx: &Arc<Ctx>,
    embedder: &Arc<Embedder>,
    previews: Vec<(usize, Vec<u8>)>,
) -> Result<Vec<(usize, Result<Vec<f32>>)>> {
    let _held = ctx.budget.reserve(PER_IMAGE_BYTES * previews.len() as u64).await?;
    let encoder = embedder.encoder().await?;
    let model = embedder.model;
    tokio::task::spawn_blocking(move || {
        let mut out: Vec<(usize, Result<Vec<f32>>)> = Vec::new();
        let mut ready: Vec<(usize, Vec<f32>)> = Vec::new();
        for (i, bytes) in previews {
            match decode_and_preprocess(model, &bytes) {
                Ok(t) => ready.push((i, t)),
                Err(e) => out.push((i, Err(anyhow::Error::from(Permanent(format!("{e:#}")))))),
            }
        }
        if !ready.is_empty() {
            let (idx, tensors): (Vec<usize>, Vec<Vec<f32>>) = ready.into_iter().unzip();
            let vectors = encoder.embed(&tensors)?;
            out.extend(idx.into_iter().zip(vectors.into_iter().map(Ok)));
        }
        Ok(out)
    })
    .await
    .context("embed task panicked")?
}

/// preview.webp bytes to the model's input tensor.
pub fn decode_and_preprocess(model: Model, webp: &[u8]) -> Result<Vec<f32>> {
    let img = webp::Decoder::new(webp)
        .decode()
        .context("preview is not a decodable WebP")?;
    let (w, h) = (img.width(), img.height());
    let px: &[u8] = &img;
    let rgb: std::borrow::Cow<[u8]> = if img.is_alpha() {
        // Over white, so a transparent graphic is not read as black.
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
    preprocess_rgb8(model, w, h, &rgb)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cyanotype_clip::TextEncoder;
    use cyanotype_core::{config::S3Config, db, storage::Storage};
    use sha2::{Digest, Sha256};
    use sqlx::{Connection, Executor, PgConnection, PgPool};

    use super::*;
    use crate::budget::MemoryBudget;

    #[test]
    fn transparent_previews_are_read_over_white_not_black() {
        // A 64x48 fully transparent WebP with a black RGB underneath: a naive drop of the alpha channel would be all black.
        let rgba = vec![0u8; 64 * 48 * 4];
        let webp = webp::Encoder::from_rgba(&rgba, 64, 48).encode_lossless().to_vec();
        for model in Model::ALL {
            let got = decode_and_preprocess(model, &webp).unwrap();
            let white = preprocess_rgb8(model, 64, 48, &vec![255u8; 64 * 48 * 3]).unwrap();
            assert_eq!(got.len(), white.len());
            let worst = got
                .iter()
                .zip(&white)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 0.05, "{model:?}: {worst}");
        }
    }

    #[test]
    fn a_preview_that_is_not_a_webp_is_an_error() {
        assert!(decode_and_preprocess(Model::ClipB32, b"GIF89a....").is_err());
    }

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/cyanotype-clip/tests/fixtures/images")
    }

    /// The model the tests run: `SEARCH_MODEL` (else the default), with its files in `SEARCH_MODEL_DIR`.
    fn model_dir() -> Option<(Model, PathBuf)> {
        let model = Model::from_env().ok()?;
        let dir = model.dir();
        [model.vision_file(), model.text_file(), model.tokenizer_file()]
            .iter()
            .all(|f| dir.join(f).exists())
            .then_some((model, dir))
    }

    /// A migrated scratch database, a real bucket (the S3_* variables CI sets) and the model: or None, and the test skips.
    struct Env {
        pool: PgPool,
        ctx: Arc<Ctx>,
        admin_url: String,
        name: String,
        model: Model,
        dir: PathBuf,
    }

    impl Env {
        async fn new() -> Option<Self> {
            let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
            std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty())?;
            let Some((model, dir)) = model_dir() else {
                assert!(
                    std::env::var_os("CYANOTYPE_REQUIRE_MODEL").is_none(),
                    "CYANOTYPE_REQUIRE_MODEL is set but SEARCH_MODEL_DIR has no files for the search model"
                );
                return None;
            };
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
                search_model: model,
            });
            Some(Self {
                pool,
                ctx,
                admin_url,
                name,
                model,
                dir,
            })
        }

        async fn embedder(&self, threads: usize) -> Arc<Embedder> {
            Arc::new(Embedder::new(self.model, threads))
        }

        async fn teardown(self) {
            self.pool.close().await;
            let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
            admin
                .execute(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name).as_str())
                .await
                .unwrap();
        }
    }

    async fn owner(pool: &PgPool) -> Uuid {
        sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ($1, 'x') RETURNING id")
            .bind(format!("{}@example.com", Uuid::new_v4()))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// An asset whose preview.webp is the fixture photo `name`, with an embed job. `preview`: what to upload (None: nothing).
    async fn asset_with_preview(env: &Env, owner: Uuid, name: &str, preview: Option<Vec<u8>>) -> Uuid {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash)
             VALUES ($1, sha256($2::bytea), 'k', $3, 'image/jpeg', 1, '\\x01') RETURNING id",
        )
        .bind(owner)
        .bind(format!("{name}{}", Uuid::new_v4()).as_bytes())
        .bind(format!("{name}.jpg"))
        .fetch_one(&env.pool)
        .await
        .unwrap();
        if let Some(bytes) = preview {
            env.ctx
                .storage
                .put_bytes(&keys::preview(owner, id), bytes, "image/webp")
                .await
                .unwrap();
        }
        jobs::enqueue(&env.pool, jobs::KIND_EMBED, id, env.model.version())
            .await
            .unwrap();
        id
    }

    /// The fixture photo as the worker's own preview encoder writes it (1440 px at most, WebP quality 80).
    fn preview_of(name: &str) -> Vec<u8> {
        let img = image::open(fixtures().join(format!("{name}.jpg"))).unwrap();
        crate::imaging::encode_webp(&img, 80.0).unwrap()
    }

    async fn run_all(env: &Env, embedder: &Arc<Embedder>) -> usize {
        let mut n = 0;
        loop {
            let batch = jobs::claim_batch(&env.pool, jobs::KIND_EMBED, BATCH).await.unwrap();
            if batch.is_empty() {
                return n;
            }
            n += batch.len();
            process(&env.ctx, embedder, batch).await;
        }
    }

    #[tokio::test]
    async fn embed_jobs_store_vectors_that_text_queries_find() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let names = ["dog", "cat", "pizza", "beach", "car", "guitar"];
        let mut ids = Vec::new();
        for n in names {
            ids.push(asset_with_preview(&env, owner, n, Some(preview_of(n))).await);
        }
        let embedder = env.embedder(1).await;
        assert!(!embedder.loaded(), "the model loads on the first job, not before");
        assert_eq!(run_all(&env, &embedder).await, names.len());
        assert!(embedder.loaded());

        let rows: Vec<(String, i32, i64)> =
            sqlx::query_as("SELECT model, version, count(*) FROM asset_embeddings GROUP BY 1, 2")
                .fetch_all(&env.pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            [(env.model.id().to_string(), env.model.version(), names.len() as i64)]
        );
        let states: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT state FROM jobs WHERE kind = 'embed'")
            .fetch_all(&env.pool)
            .await
            .unwrap();
        assert_eq!(states, [("done".to_string(),)]);

        // End to end: a text query's nearest stored vector is the right photo.
        let text = TextEncoder::load(env.model, &env.dir, 1).unwrap();
        for (n, id) in names.iter().zip(&ids) {
            let q = text.embed(&format!("a photo of a {n}")).unwrap();
            let nearest: Uuid = sqlx::query_scalar(
                "SELECT asset_id FROM asset_embeddings ORDER BY embedding <=> $1::text::halfvec LIMIT 1",
            )
            .bind(embeddings::pg_literal(&q))
            .fetch_one(&env.pool)
            .await
            .unwrap();
            assert_eq!(nearest, *id, "{n}");
        }
        env.teardown().await;
    }

    #[tokio::test]
    async fn embed_jobs_skip_what_is_not_searched_and_sort_failures() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let good = asset_with_preview(&env, owner, "dog", Some(preview_of("dog"))).await;
        let primary = asset_with_preview(&env, owner, "cat", Some(preview_of("cat"))).await;
        let hidden_raw = asset_with_preview(&env, owner, "cat", Some(preview_of("cat"))).await;
        let purging = asset_with_preview(&env, owner, "car", Some(preview_of("car"))).await;
        let corrupt = asset_with_preview(&env, owner, "pizza", Some(b"not a webp at all".to_vec())).await;
        let missing = asset_with_preview(&env, owner, "beach", None).await;
        let no_preview_yet = asset_with_preview(&env, owner, "guitar", None).await;
        sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
            .bind(hidden_raw)
            .bind(primary)
            .execute(&env.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE assets SET deleted_at = now(), purging_at = now() WHERE id = $1")
            .bind(purging)
            .execute(&env.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE assets SET thumbhash = NULL WHERE id = $1")
            .bind(no_preview_yet)
            .execute(&env.pool)
            .await
            .unwrap();

        let embedder = env.embedder(1).await;
        run_all(&env, &embedder).await;
        let state = |id: Uuid| {
            let pool = env.pool.clone();
            async move {
                sqlx::query_as::<_, (String, Option<String>)>(
                    "SELECT state, last_error FROM jobs WHERE kind = 'embed' AND asset_id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        for id in [good, primary, hidden_raw, purging, no_preview_yet] {
            assert_eq!(state(id).await.0, "done");
        }
        let embedded: Vec<Uuid> = sqlx::query_scalar("SELECT asset_id FROM asset_embeddings ORDER BY asset_id")
            .fetch_all(&env.pool)
            .await
            .unwrap();
        let mut want = vec![good, primary];
        want.sort();
        assert_eq!(embedded, want, "only visible photos with a preview get a vector");
        // A preview that is not a WebP will not become one by retrying.
        let (st, err) = state(corrupt).await;
        assert_eq!(st, "failed");
        assert!(err.unwrap().contains("not a decodable WebP"));
        // A preview that cannot be fetched (the object is missing) is retried with backoff, never marked done.
        let (st, err) = state(missing).await;
        assert_eq!(st, "queued");
        assert!(err.unwrap().contains("download preview"));
        env.teardown().await;
    }

    /// A swap re-indexes in the background: the older model's vector stays while the photo gets its new one (search keeps
    /// answering from it), and goes once the sweep finds every photo covered.
    #[tokio::test]
    async fn an_older_models_vector_stays_until_every_photo_has_the_new_one() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let a = asset_with_preview(&env, owner, "dog", Some(preview_of("dog"))).await;
        let b = asset_with_preview(&env, owner, "cat", Some(preview_of("cat"))).await;
        for id in [a, b] {
            embeddings::store(&env.pool, id, "an-older-model", 1, &vec![0.04; 512])
                .await
                .unwrap();
        }
        // Only `a` is embedded so far.
        sqlx::query("UPDATE jobs SET run_after = now() + interval '1 hour' WHERE asset_id = $1")
            .bind(b)
            .execute(&env.pool)
            .await
            .unwrap();
        let embedder = env.embedder(1).await;
        run_all(&env, &embedder).await;
        let models_of = |id: Uuid| {
            let pool = env.pool.clone();
            async move {
                sqlx::query_scalar::<_, String>("SELECT model FROM asset_embeddings WHERE asset_id = $1 ORDER BY model")
                    .bind(id)
                    .fetch_all(&pool)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(models_of(a).await, ["an-older-model", env.model.id()]);
        sweep(&env.ctx, env.model).await;
        assert_eq!(
            models_of(a).await.len(),
            2,
            "b still has no vector from the new model: nothing is pruned yet"
        );
        assert_eq!(models_of(b).await, ["an-older-model"]);

        sqlx::query("UPDATE jobs SET run_after = now() WHERE asset_id = $1")
            .bind(b)
            .execute(&env.pool)
            .await
            .unwrap();
        run_all(&env, &embedder).await;
        sweep(&env.ctx, env.model).await;
        for id in [a, b] {
            assert_eq!(models_of(id).await, [env.model.id()]);
        }
        env.teardown().await;
    }

    /// The whole pipeline: a thumbnail job on a real original produces the preview and queues the embed job, which then
    /// reads that preview. (S3, Postgres and the model, like the other tests here.)
    #[tokio::test]
    async fn a_finished_thumbnail_queues_the_embedding_and_the_embedding_reads_its_preview() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let bytes = std::fs::read(fixtures().join("birthday_cake.jpg")).unwrap();
        let sha: [u8; 32] = Sha256::digest(&bytes).into();
        let key = keys::original(owner, &sha, "jpg");
        env.ctx
            .storage
            .put_bytes(&key, bytes.clone(), "image/jpeg")
            .await
            .unwrap();
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes)
             VALUES ($1, $2, $3, 'cake.jpg', 'image/jpeg', $4) RETURNING id",
        )
        .bind(owner)
        .bind(sha.as_slice())
        .bind(&key)
        .bind(bytes.len() as i64)
        .fetch_one(&env.pool)
        .await
        .unwrap();
        for kind in [jobs::KIND_METADATA, jobs::KIND_THUMBNAIL] {
            jobs::enqueue(&env.pool, kind, id, 1).await.unwrap();
            let job = jobs::claim(&env.pool, kind).await.unwrap().unwrap();
            let info = crate::handlers::JobInfo::default();
            crate::handlers::run(&env.ctx, &job, &info).await.unwrap();
            jobs::complete(&env.pool, job.id).await.unwrap();
        }
        let queued: Vec<(i32, i16)> =
            sqlx::query_as("SELECT version, priority FROM jobs WHERE kind = 'embed' AND state = 'queued'")
                .fetch_all(&env.pool)
                .await
                .unwrap();
        assert_eq!(
            queued,
            [(env.model.version(), 0)],
            "an upload's embedding is first in line, ahead of any backfill"
        );

        let embedder = env.embedder(1).await;
        assert_eq!(run_all(&env, &embedder).await, 1);
        let text = TextEncoder::load(env.model, &env.dir, 1).unwrap();
        let sim_to = |q: &str| {
            let q = text.embed_query(q).unwrap();
            let pool = env.pool.clone();
            async move {
                sqlx::query_scalar::<_, f64>(
                    "SELECT 1 - (embedding <=> $1::text::halfvec) FROM asset_embeddings WHERE asset_id = $2",
                )
                .bind(embeddings::pg_literal(&q))
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        // The scale of a cosine differs per model (about 0.12 for SigLIP 2, 0.25 for CLIP), so compare with what is not there.
        let (cake, other) = (sim_to("birthday cake").await, sim_to("an excavator").await);
        assert!(
            cake > 0.08 && cake > other + 0.04,
            "similarity of the cake photo to 'birthday cake' {cake}, to 'an excavator' {other}"
        );
        env.teardown().await;
    }

    /// Photos per minute on this machine, through the whole job path (S3 download, WebP decode, preprocessing, inference,
    /// store). `cargo test --release -p cyanotype-worker embed_throughput -- --ignored --nocapture`, with `EMBED_THREADS=2`
    /// for the two-thread number.
    #[tokio::test]
    #[ignore = "a benchmark"]
    async fn embed_throughput() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let names: Vec<String> = std::fs::read_dir(fixtures())
            .unwrap()
            .filter_map(|e| e.ok()?.path().file_stem()?.to_str().map(str::to_string))
            .filter(|n| fixtures().join(format!("{n}.jpg")).exists())
            .collect();
        let previews: Vec<Vec<u8>> = names.iter().map(|n| preview_of(n)).collect();
        let total = 120;
        for i in 0..total {
            asset_with_preview(
                &env,
                owner,
                &names[i % names.len()],
                Some(previews[i % names.len()].clone()),
            )
            .await;
        }
        let threads = std::env::var("EMBED_THREADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let embedder = env.embedder(threads).await;
        // Load the model before timing.
        embedder.encoder().await.unwrap();
        let t = Instant::now();
        assert_eq!(run_all(&env, &embedder).await, total);
        let secs = t.elapsed().as_secs_f64();
        let rss = std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find(|l| l.starts_with("VmRSS"))
            .unwrap_or_default()
            .to_string();
        println!(
            "\nembedded {total} photos with {threads} thread(s) in {secs:.1} s: {:.0} photos/min ({:.0} ms each); {rss}",
            total as f64 / secs * 60.0,
            secs * 1000.0 / total as f64
        );
        env.teardown().await;
    }
}
