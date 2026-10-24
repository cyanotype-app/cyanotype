//! The `faces` job: the faces in each photo and a fingerprint of each, found in the photo's `preview.webp` (never the original, so
//! there is no big decode), stored in `faces` for the People feature to group (`docs/faces.md`). Switched on by `FACES_ENABLED`; off, none of
//! this runs and no job is queued.
//!
//! Like text reading it is background work in a loop of its own, and the *least* urgent kind: it takes a job only when no
//! thumbnail, metadata, embed, text or other foreground job is runnable, and it shares the embed loop's one-at-a-time permit, so
//! the ONNX models of the three never work at the same time. A photo's job is queued when its embedding is stored; the sweep
//! queues the rest (a library that existed before, a model switch, a rotate or crop). The models load on the first job and
//! are dropped again after a quiet spell (`IDLE_UNLOAD`), so a library that is fully scanned does not keep them resident.
//! While a job runs it holds `TRANSIENT_BYTES` of the shared byte budget (`WORKER_MEMORY_BUDGET_MB`): the decoded preview, the
//! detector's input and the networks' activations, measured in docs/faces.md.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use cyanotype_core::{
    faces::{self as store, NewFace},
    jobs, keys, metrics,
};
use cyanotype_faces::{Engine, EngineConfig, Face};
use tokio::sync::Semaphore;
use tracing::Instrument;
use uuid::Uuid;

use crate::{
    finish,
    handlers::{Ctx, Outcome, Permanent},
};

/// A preview is at most 1440 px on the long edge; anything bigger than this is not ours.
const MAX_PREVIEW_BYTES: u64 = 16 << 20;
/// Budget held while a photo is scanned. A 1440 x 1080 preview decodes to 6 MB (RGBA) and is copied to 4.7 MB of RGB; the
/// detector's input at that size is 3 x 1440 x 1088 floats (19 MB) and its activations a few times that; each face adds a
/// 112 x 112 crop. The highest process peak measured over the bake-off photos, with the models loaded (docs/faces.md), is
/// what the figure is read from.
pub const TRANSIENT_BYTES: u64 = 96 << 20;
/// How long the models stay loaded with nothing to scan.
const IDLE_UNLOAD: Duration = Duration::from_secs(300);
/// How often the sweep that queues unscanned photos runs.
const SWEEP_EVERY: Duration = Duration::from_secs(900);
const FALLBACK_POLL: Duration = Duration::from_secs(30);
const FOREGROUND_RECHECK: Duration = Duration::from_secs(1);

/// Where the model files are: `FACES_MODEL_DIR`, else `<SEARCH_MODEL_DIR>/faces` (the images install them there).
pub fn model_dir() -> PathBuf {
    cyanotype_faces::model_dir()
}

/// Are the model files installed? A dev checkout that skipped `scripts/fetch-models.sh` runs without scanning faces, and its
/// `faces` jobs stay queued.
pub fn model_installed() -> bool {
    cyanotype_faces::models_installed(&model_dir())
}

/// The settings the bake-off chose (docs/faces.md, "Settings").
pub fn config(threads: usize) -> EngineConfig {
    EngineConfig {
        threads,
        ..EngineConfig::default()
    }
}

/// The (lazily loaded) networks.
pub struct Finder {
    threads: usize,
    engine: Mutex<Option<(Arc<Engine>, Instant)>>,
}

impl Finder {
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
        let e = tokio::task::spawn_blocking(move || Engine::load(&model_dir(), config(threads)))
            .await
            .context("face model load task panicked")??;
        let load_ms = started.elapsed().as_millis() as u64;
        tracing::info!(model = store::MODEL, threads, load_ms, "face models loaded");
        metrics::global().histogram("faces.model_load_ms", load_ms as f64, &[("model", store::MODEL)]);
        let e = Arc::new(e);
        *self.engine.lock().unwrap_or_else(|p| p.into_inner()) = Some((e.clone(), Instant::now()));
        Ok(e)
    }

    fn unload_if_idle(&self) {
        let mut guard = self.engine.lock().unwrap_or_else(|p| p.into_inner());
        if guard.as_ref().is_some_and(|(_, used)| used.elapsed() >= IDLE_UNLOAD) {
            *guard = None;
            tracing::info!("face models unloaded after a quiet spell");
        }
    }

    fn loaded(&self) -> bool {
        self.engine.lock().unwrap_or_else(|p| p.into_inner()).is_some()
    }
}

/// Kinds that must not wait behind scanning faces: everything else, embedding and text included.
pub(crate) fn foreground_kinds() -> impl Iterator<Item = &'static str> {
    jobs::DEFAULT_PIPELINE.iter().copied().chain([
        jobs::KIND_EMBED,
        jobs::KIND_OCR,
        jobs::KIND_PAIR,
        jobs::KIND_GEOCODE,
        jobs::KIND_SHARE_COPY,
        jobs::KIND_EDITED_COPY,
        jobs::KIND_TAKEOUT,
    ])
}

/// The faces loop. `busy` is the embed loop's permit: held for each job, so shutdown can wait for it and the models never work at
/// the same time.
pub async fn run_loop(ctx: Arc<Ctx>, finder: Arc<Finder>, waker: jobs::JobWaker, busy: Arc<Semaphore>) {
    let foreground: Vec<&str> = foreground_kinds().collect();
    let mut last_sweep: Option<Instant> = None;
    loop {
        metrics::global().gauge("faces.model_loaded", if finder.loaded() { 1.0 } else { 0.0 }, &[]);
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
        let job = match jobs::claim(&ctx.db, jobs::KIND_FACES).await {
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
            jobs::wait_for_work(&ctx.db, &waker, &[jobs::KIND_FACES], SWEEP_EVERY.min(FALLBACK_POLL)).await;
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
            metrics::global().histogram("faces.duration", elapsed, &[("model", store::MODEL)]);
        }
        finish::record(&ctx, &job, "image", result, elapsed)
            .instrument(span)
            .await;
        drop(permit);
    }
}

/// Queue what has not been scanned by the current model (a library that existed before, a model switch, lost jobs); once
/// nothing is missing, drop the rows of the models it replaced.
async fn sweep(ctx: &Ctx) {
    match store::enqueue_missing(&ctx.db, store::MODEL, store::VERSION).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            queued = n,
            model = store::MODEL,
            "queued photos whose faces have not been scanned"
        ),
        Err(e) => tracing::warn!(error = %format!("{e:#}"), "faces sweep failed"),
    }
    match store::missing_count(&ctx.db, store::MODEL, store::VERSION).await {
        Ok(0) => match store::prune_superseded(&ctx.db, store::MODEL, store::VERSION).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                removed = n,
                model = store::MODEL,
                "removed faces found by replaced models"
            ),
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "pruning replaced faces failed"),
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
    let bytes = ctx
        .storage
        .get_bytes_limited(&keys::preview(r.owner_id, job.asset_id), MAX_PREVIEW_BYTES)
        .await
        .context("download preview")?;
    let _held = ctx.budget.reserve(TRANSIENT_BYTES).await?;
    let engine = finder.engine().await?;
    let found = tokio::task::spawn_blocking(move || scan(&engine, &bytes))
        .await
        .context("faces task panicked")??;
    metrics::global().histogram("faces.per_photo", found.len() as f64, &[("model", store::MODEL)]);
    let stored = store::store(&ctx.db, job.asset_id, store::MODEL, store::VERSION, &found)
        .await
        .context("store faces")?;
    if !stored {
        return Ok(Outcome::Skipped("asset deleted while scanning"));
    }
    Ok(Outcome::Done)
}

/// Decode a preview and find its faces. A preview that cannot be decoded is a permanent failure of that photo alone.
pub fn scan(engine: &Engine, webp: &[u8]) -> Result<Vec<NewFace>> {
    let img = webp::Decoder::new(webp)
        .decode()
        .ok_or_else(|| anyhow::Error::from(Permanent("preview is not a decodable WebP".into())))?;
    let (w, h) = (img.width(), img.height());
    let px: &[u8] = &img;
    let rgb: std::borrow::Cow<[u8]> = if img.is_alpha() {
        // Over white, like the other models read it.
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
    let faces = engine.analyse(&rgb, w, h)?;
    Ok(to_new_faces(&faces, w, h))
}

/// The stored shape of what was found: coordinates as fractions of the preview, strongest face first.
pub fn to_new_faces(faces: &[Face], w: u32, h: u32) -> Vec<NewFace> {
    let (fw, fh) = (w.max(1) as f32, h.max(1) as f32);
    let unit = |v: f32| v.clamp(0.0, 1.0);
    faces
        .iter()
        .map(|f| {
            let d = &f.detection;
            let mut landmarks = [0.0f32; 10];
            for (i, p) in d.landmarks.iter().enumerate() {
                landmarks[i * 2] = unit(p[0] / fw);
                landmarks[i * 2 + 1] = unit(p[1] / fh);
            }
            NewFace {
                bbox: [
                    unit(d.bbox[0] / fw),
                    unit(d.bbox[1] / fh),
                    unit(d.bbox[2] / fw),
                    unit(d.bbox[3] / fh),
                ],
                landmarks,
                score: d.score,
                embedding: f.embedding.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use cyanotype_faces::Detection;

    use super::*;

    #[test]
    fn faces_are_stored_as_fractions_of_the_preview() {
        let f = Face {
            detection: Detection {
                score: 0.93,
                bbox: [144.0, 108.0, 288.0, 216.0],
                landmarks: [
                    [200.0, 150.0],
                    [300.0, 150.0],
                    [250.0, 200.0],
                    [210.0, 250.0],
                    [290.0, 250.0],
                ],
            },
            embedding: Some(vec![0.5; 4]),
        };
        let out = to_new_faces(&[f], 1440, 1080);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bbox, [0.1, 0.1, 0.2, 0.2]);
        assert!(
            (out[0].landmarks[0] - 200.0 / 1440.0).abs() < 1e-6 && (out[0].landmarks[1] - 150.0 / 1080.0).abs() < 1e-6
        );
        assert_eq!(out[0].score, 0.93);
        assert_eq!(out[0].embedding.as_deref(), Some(&[0.5f32; 4][..]));
    }

    #[test]
    fn a_face_without_a_fingerprint_is_kept_without_one_and_coordinates_stay_in_range() {
        let f = Face {
            detection: Detection {
                score: 0.8,
                bbox: [-3.0, 0.0, 2000.0, 10.0],
                landmarks: [[-1.0, 5.0]; 5],
            },
            embedding: None,
        };
        let out = to_new_faces(&[f], 100, 100);
        assert!(out[0].embedding.is_none());
        assert!(
            out[0]
                .bbox
                .iter()
                .chain(&out[0].landmarks)
                .all(|v| (0.0..=1.0).contains(v))
        );
    }

    #[test]
    fn a_preview_that_is_not_a_webp_is_not_decodable() {
        assert!(webp::Decoder::new(b"GIF89a....").decode().is_none());
    }

    #[test]
    fn faces_wait_behind_every_other_kind_of_job() {
        let kinds: Vec<&str> = foreground_kinds().collect();
        for k in [
            jobs::KIND_THUMBNAIL,
            jobs::KIND_EMBED,
            jobs::KIND_OCR,
            jobs::KIND_TAKEOUT,
        ] {
            assert!(kinds.contains(&k), "{k}");
        }
        assert!(!kinds.contains(&jobs::KIND_FACES));
    }

    // ---- the job against a real database, bucket and models -------------------------------------------------------------

    use cyanotype_core::{config::S3Config, db, storage::Storage};
    use sqlx::{Connection, Executor, PgConnection, PgPool};

    use crate::budget::MemoryBudget;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/cyanotype-clip/tests/fixtures/images")
    }

    /// A migrated scratch database, a real bucket (the S3_* variables CI sets) and the face models: or None, and the test skips.
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
            if !model_installed() {
                assert!(
                    std::env::var_os("CYANOTYPE_REQUIRE_MODEL").is_none(),
                    "CYANOTYPE_REQUIRE_MODEL is set but {} has no face models",
                    model_dir().display()
                );
                return None;
            }
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
                search_model: cyanotype_clip::Model::Siglip2B16,
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

        /// An asset with a claimed `faces` job whose preview.webp is `preview` (None: nothing uploaded).
        async fn asset(&self, owner: Uuid, preview: Option<Vec<u8>>) -> (Uuid, jobs::Job) {
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
            store::enqueue_for_asset(&self.pool, id, store::VERSION).await.unwrap();
            let job = jobs::claim(&self.pool, jobs::KIND_FACES).await.unwrap().unwrap();
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

    async fn scans(pool: &PgPool) -> Vec<(i32, i32)> {
        sqlx::query_as("SELECT version, faces FROM face_scans ORDER BY scanned_at")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_photo_without_faces_is_scanned_and_recorded_and_scanning_again_changes_nothing() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = Arc::new(Finder::new(1));
        let (id, job) = env.asset(owner, Some(preview_of("beach"))).await;
        assert!(!finder.loaded());
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        assert!(finder.loaded(), "the models load on the first job");
        assert_eq!(scans(&env.pool).await, vec![(store::VERSION, 0)]);
        // The same job again (a retry, a duplicate delivery): still one scan row, no faces.
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        assert_eq!(scans(&env.pool).await, vec![(store::VERSION, 0)]);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM faces WHERE asset_id = $1")
            .bind(id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
        env.teardown().await;
    }

    #[tokio::test]
    async fn a_portrait_gets_one_face_with_a_fingerprint_and_running_again_replaces_it() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = Arc::new(Finder::new(1));
        let img = image::open(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/cyanotype-faces/tests/fixtures/person_b2.jpg"),
        )
        .unwrap();
        let (id, job) = env
            .asset(owner, Some(crate::imaging::encode_webp(&img, 80.0).unwrap()))
            .await;
        for _ in 0..2 {
            assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
            assert_eq!(scans(&env.pool).await, vec![(store::VERSION, 1)]);
            #[allow(clippy::type_complexity)]
            let rows: Vec<(Uuid, String, f32, f32, f32, f32, f32, Option<i32>)> = sqlx::query_as(
                "SELECT owner_id, model, box_x, box_y, box_w, box_h, score, vector_dims(embedding) FROM faces WHERE asset_id = $1",
            )
            .bind(id)
            .fetch_all(&env.pool)
            .await
            .unwrap();
            assert_eq!(rows.len(), 1, "one face, however often the job runs");
            let (o, model, x, y, w, h, score, dims) = rows[0].clone();
            assert_eq!(
                (o, model.as_str(), dims),
                (owner, store::MODEL, Some(store::DIM as i32))
            );
            assert!(
                x > 0.0 && y >= 0.0 && x + w <= 1.0 && y + h <= 1.0 && w > 0.1 && h > 0.05,
                "{x} {y} {w} {h}"
            );
            assert!(score > 0.7);
        }
        // The owner names the person; scanning the photo again (a new version, or the same job) keeps the person and who decided.
        let person: Uuid =
            sqlx::query_scalar("INSERT INTO people (owner_id, name) VALUES ($1, 'Person A') RETURNING id")
                .bind(owner)
                .fetch_one(&env.pool)
                .await
                .unwrap();
        sqlx::query("UPDATE faces SET person_id = $1, assigned_by = 'user' WHERE asset_id = $2")
            .bind(person)
            .bind(id)
            .execute(&env.pool)
            .await
            .unwrap();
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        let kept: Vec<(Option<Uuid>, String)> =
            sqlx::query_as("SELECT person_id, assigned_by FROM faces WHERE asset_id = $1")
                .bind(id)
                .fetch_all(&env.pool)
                .await
                .unwrap();
        assert_eq!(kept, vec![(Some(person), "user".to_string())]);
        jobs::complete(&env.pool, job.id).await.unwrap();
        // The picture changes (a rotate or crop): the scan is forgotten and the photo queued again. The face in a person waits, unread,
        // and the new scan hands the person over to the face it finds.
        assert!(
            store::forget_and_requeue(&env.pool, id, store::VERSION, true)
                .await
                .unwrap()
        );
        assert_eq!(scans(&env.pool).await, vec![]);
        let job = jobs::claim(&env.pool, jobs::KIND_FACES).await.unwrap().unwrap();
        assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
        let kept: Vec<(Option<Uuid>, String)> =
            sqlx::query_as("SELECT person_id, assigned_by FROM faces WHERE asset_id = $1")
                .bind(id)
                .fetch_all(&env.pool)
                .await
                .unwrap();
        assert_eq!(
            kept,
            vec![(Some(person), "user".to_string())],
            "a rescan after an edit keeps the person"
        );
        env.teardown().await;
    }

    #[tokio::test]
    async fn photos_that_are_not_scanned_are_skipped_and_failures_are_sorted() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let finder = Arc::new(Finder::new(1));
        let (purging, j_purging) = env.asset(owner, Some(preview_of("car"))).await;
        let (hidden, j_hidden) = env.asset(owner, Some(preview_of("cat"))).await;
        let (gone, j_gone) = env.asset(owner, Some(preview_of("dog"))).await;
        let (no_preview, j_no_preview) = env.asset(owner, None).await;
        let (_, j_corrupt) = env.asset(owner, Some(b"not a webp at all".to_vec())).await;
        let (_, j_missing) = env.asset(owner, None).await;
        sqlx::query("UPDATE assets SET deleted_at = now(), purging_at = now() WHERE id = $1")
            .bind(purging)
            .execute(&env.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
            .bind(hidden)
            .bind(j_corrupt.asset_id)
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
        // A preview that is not a WebP will not become one by retrying; a preview that cannot be fetched is retried.
        assert!(
            process(&env.ctx, &finder, &j_corrupt)
                .await
                .err()
                .unwrap()
                .is::<Permanent>()
        );
        let err = process(&env.ctx, &finder, &j_missing).await.err().unwrap();
        assert!(!err.is::<Permanent>() && format!("{err:#}").contains("download preview"));
        assert_eq!(scans(&env.pool).await, vec![], "nothing was recorded for any of them");
        env.teardown().await;
    }

    #[tokio::test]
    async fn the_sweep_queues_what_is_missing_and_only_then_prunes_older_scans() {
        let Some(env) = Env::new().await else { return };
        let owner = owner(&env.pool).await;
        let a = env.asset(owner, Some(preview_of("dog"))).await.0;
        let b = env.asset(owner, Some(preview_of("cat"))).await.0;
        sqlx::query("DELETE FROM jobs").execute(&env.pool).await.unwrap();
        // An older model's records for both photos.
        for id in [a, b] {
            store::store(&env.pool, id, "an-older-model", 1, &[]).await.unwrap();
        }
        sweep(&env.ctx).await;
        let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'faces' AND state = 'queued'")
            .fetch_one(&env.pool)
            .await
            .unwrap();
        assert_eq!(queued, 2);
        let old = || async {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM face_scans WHERE model = 'an-older-model'")
                .fetch_one(&env.pool)
                .await
                .unwrap()
        };
        assert_eq!(
            old().await,
            2,
            "the older model's records stay while the new one has not covered the library"
        );
        // Scan both; the next sweep finds nothing missing and drops the older model's records.
        let finder = Arc::new(Finder::new(1));
        while let Some(job) = jobs::claim(&env.pool, jobs::KIND_FACES).await.unwrap() {
            assert!(matches!(process(&env.ctx, &finder, &job).await.unwrap(), Outcome::Done));
            jobs::complete(&env.pool, job.id).await.unwrap();
        }
        sweep(&env.ctx).await;
        assert_eq!(scans(&env.pool).await.len(), 2);
        assert_eq!(old().await, 0);
        env.teardown().await;
    }
}
