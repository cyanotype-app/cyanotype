//! `GET /assets/processing` and `POST /assets/processing/retry`: how far the background work on the caller's own
//! photos has got, so the web app can show "Processing 1,234 photos..." after a big import and offer a retry.
//!
//! Everything is scoped to the caller's assets (jobs are joined to `assets.owner_id`); photos queued for permanent
//! deletion are ignored. Trashed photos still count: the worker processes them too.
//!
//! Embedding photos for search (`embed` jobs) is background work that can take hours for a big library, so it is
//! reported on its own (`embedding`) and does not count towards `active`, `queued` or `running` (the library page's
//! "Processing..." banner is about getting photos on screen). Failed embed jobs do show in `failed` and `failures`, like
//! any other failure, and "Retry failed" requeues them.
use axum::{Json, extract::State};
use cyanotype_core::{embeddings, faces, ocr, pets};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::AppState;
use cyanotype_service::{ApiResult, AuthUser, ImportUser};

/// Failures listed (newest first); the counts always cover all of them.
const RECENT_FAILURES: i64 = 20;
/// Longest `last_error` returned, in characters.
const ERROR_CHARS: usize = 200;

#[derive(Serialize, FromRow, Debug)]
pub(super) struct KindCounts {
    kind: String,
    queued: i64,
    running: i64,
    failed: i64,
}

#[derive(Serialize, FromRow, Debug)]
pub(super) struct Failure {
    job_id: i64,
    asset_id: Uuid,
    original_name: String,
    kind: String,
    last_error: Option<String>,
    attempts: i32,
}

#[derive(Serialize, Debug)]
pub(super) struct Processing {
    /// Unfinished work exists (something queued or running): the UI polls while this is true.
    active: bool,
    queued: i64,
    running: i64,
    failed: i64,
    by_kind: Vec<KindCounts>,
    /// Photos in the library (live, not a hidden RAW half of a pair).
    total_photos: i64,
    /// Of those, the ones still waiting for their thumbnail (a thumbnail job is queued or running).
    without_thumbnail: i64,
    failures: Vec<Failure>,
    /// Search indexing: how many of the library's photos search can find, and what is still queued.
    embedding: Embedding,
    /// Reading text off photos (`ocr` jobs), reported apart like search indexing: a backfill takes hours on a big library.
    text: TextProgress,
    /// Finding faces in photos (`faces` jobs), reported apart as well. Present only when the feature is switched on
    /// (`FACES_ENABLED`, off by default); nothing about faces is reported otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    faces: Option<FacesProgress>,
    /// Finding dogs and cats in photos (`pets` jobs), reported apart too. Present only while the feature is on (`PETS_ENABLED`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pets: Option<PetsProgress>,
}

#[derive(Serialize, Debug)]
pub(super) struct PetsProgress {
    /// Photos the current model has looked at, and photos that can be looked at (they have a preview and a search vector).
    scanned: i64,
    total: i64,
    /// Animals found so far, and how many of them have no fingerprint yet (named pets compare animals by it; the job fills them in).
    found: i64,
    unfingerprinted: i64,
    queued: i64,
    running: i64,
    failed: i64,
    model: &'static str,
    percent: u8,
}

#[derive(Serialize, Debug)]
pub(super) struct FacesProgress {
    /// Photos the current model has looked at, and photos that can be looked at (they have a preview).
    scanned: i64,
    total: i64,
    /// Faces found so far.
    found: i64,
    queued: i64,
    running: i64,
    failed: i64,
    model: &'static str,
    /// Whole percent of the library scanned (100 for an empty library).
    percent: u8,
}

#[derive(Serialize, Debug)]
pub(super) struct TextProgress {
    /// Photos whose text the current model has read, and photos that can be read (they have a preview).
    read: i64,
    total: i64,
    queued: i64,
    running: i64,
    failed: i64,
    model: &'static str,
    /// Whole percent of the library read (100 for an empty library).
    percent: u8,
}

#[derive(Serialize, Debug)]
pub(super) struct Embedding {
    /// Photos with derived images that have a vector from the configured model (`SEARCH_MODEL`).
    indexed: i64,
    /// Photos with derived images (what could be indexed).
    total: i64,
    queued: i64,
    running: i64,
    failed: i64,
    /// The configured model, e.g. "siglip2-b16".
    model: &'static str,
    /// Whole percent of the library it has indexed (100 for an empty library).
    percent: u8,
    /// The configured model is still being built and an older one answers searches meanwhile ("re-indexing with the
    /// improved model"). It takes over at `switch_at_percent`.
    reindexing: bool,
    switch_at_percent: u8,
    /// The model that answers searches right now.
    serving_model: &'static str,
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= ERROR_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(ERROR_CHARS).collect();
    out.push('…');
    out
}

pub(super) async fn status(State(s): State<AppState>, user: ImportUser) -> ApiResult<Json<Processing>> {
    let by_kind: Vec<KindCounts> = sqlx::query_as(
        "SELECT j.kind,
                count(*) FILTER (WHERE j.state = 'queued') AS queued,
                count(*) FILTER (WHERE j.state = 'running') AS running,
                count(*) FILTER (WHERE j.state = 'failed') AS failed
         FROM jobs j JOIN assets a ON a.id = j.asset_id
         WHERE a.owner_id = $1 AND a.purging_at IS NULL AND j.state <> 'done'
           AND j.kind NOT IN ('share_copy', 'burst') -- share_copy is made on demand for share links; burst grouping is background work
           AND (j.kind NOT IN ('embed', 'ocr', 'faces', 'pets') OR j.state = 'failed') -- indexing for search, reading text and finding faces and pets are reported separately
         GROUP BY j.kind ORDER BY j.kind",
    )
    .bind(user.id)
    .fetch_all(&s.db)
    .await?;

    let (total_photos, without_thumbnail): (i64, i64) = sqlx::query_as(
        "SELECT count(*),
                count(*) FILTER (WHERE a.thumbhash IS NULL AND EXISTS (
                    SELECT 1 FROM jobs j WHERE j.asset_id = a.id AND j.kind = 'thumbnail' AND j.state IN ('queued', 'running')))
         FROM assets a
         WHERE a.owner_id = $1 AND a.deleted_at IS NULL AND a.purging_at IS NULL AND a.primary_id IS NULL",
    )
    .bind(user.id)
    .fetch_one(&s.db)
    .await?;

    let mut failures: Vec<Failure> = sqlx::query_as(
        "SELECT j.id AS job_id, j.asset_id, a.original_name, j.kind, j.last_error, j.attempts
         FROM jobs j JOIN assets a ON a.id = j.asset_id
         WHERE a.owner_id = $1 AND a.purging_at IS NULL AND j.state = 'failed'
         ORDER BY j.finished_at DESC NULLS LAST, j.id DESC LIMIT $2",
    )
    .bind(user.id)
    .bind(RECENT_FAILURES)
    .fetch_all(&s.db)
    .await?;
    for f in &mut failures {
        f.last_error = f.last_error.as_deref().map(truncate);
    }

    let (e_queued, e_running, e_failed): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE j.state = 'queued'), count(*) FILTER (WHERE j.state = 'running'),
                count(*) FILTER (WHERE j.state = 'failed')
         FROM jobs j JOIN assets a ON a.id = j.asset_id
         WHERE a.owner_id = $1 AND a.purging_at IS NULL AND j.kind = 'embed' AND j.version = $2",
    )
    .bind(user.id)
    .bind(s.search.model.version())
    .fetch_one(&s.db)
    .await?;
    let model = s.search.model;
    let progress = embeddings::progress(&s.db, user.id, model.id(), model.version())
        .await
        .map_err(cyanotype_service::ApiError::Internal)?;
    let serving = s
        .search
        .serving(&s.db, s.text.as_ref(), user.id)
        .await
        .map_err(cyanotype_service::ApiError::Internal)?;
    let percent = if progress.total <= 0 {
        100
    } else {
        (progress.indexed * 100 / progress.total).clamp(0, 100) as u8
    };

    let (t_queued, t_running, t_failed): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE j.state = 'queued'), count(*) FILTER (WHERE j.state = 'running'),
                count(*) FILTER (WHERE j.state = 'failed')
         FROM jobs j JOIN assets a ON a.id = j.asset_id
         WHERE a.owner_id = $1 AND a.purging_at IS NULL AND j.kind = 'ocr' AND j.version = $2",
    )
    .bind(user.id)
    .bind(ocr::VERSION)
    .fetch_one(&s.db)
    .await?;
    let text = ocr::progress(&s.db, user.id, ocr::MODEL, ocr::VERSION)
        .await
        .map_err(cyanotype_service::ApiError::Internal)?;
    let text_percent = if text.total <= 0 {
        100
    } else {
        (text.read * 100 / text.total).clamp(0, 100) as u8
    };

    let faces = faces_progress(&s.db, user.id, faces::enabled()).await?;
    let pets = pets_progress(&s.db, user.id, s.pets_enabled.then(|| pets_basis(model))).await?;

    let (queued, running, failed) = by_kind
        .iter()
        .fold((0, 0, 0), |(q, r, f), k| (q + k.queued, r + k.running, f + k.failed));
    Ok(Json(Processing {
        active: queued + running > 0,
        queued,
        running,
        failed,
        by_kind,
        total_photos,
        without_thumbnail,
        failures,
        embedding: Embedding {
            indexed: progress.indexed,
            total: progress.total,
            queued: e_queued,
            running: e_running,
            failed: e_failed,
            model: model.name(),
            percent,
            reindexing: serving.reindexing.is_some(),
            switch_at_percent: s.search.switch_percent,
            serving_model: serving.model.name(),
        },
        text: TextProgress {
            read: text.read,
            total: text.total,
            queued: t_queued,
            running: t_running,
            failed: t_failed,
            model: ocr::MODEL,
            percent: text_percent,
        },
        faces,
        pets,
    }))
}

/// The search vectors the pets check reads (`pets::Basis`).
pub(super) fn pets_basis(model: cyanotype_clip::Model) -> pets::Basis<'static> {
    pets::Basis {
        model: model.id(),
        version: model.version(),
    }
}

/// How far finding dogs and cats has got for one owner; `None` while the feature is off (`basis` is `None`).
async fn pets_progress(
    db: &sqlx::PgPool,
    owner: Uuid,
    basis: Option<pets::Basis<'_>>,
) -> ApiResult<Option<PetsProgress>> {
    let Some(basis) = basis else {
        return Ok(None);
    };
    let (queued, running, failed): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE j.state = 'queued'), count(*) FILTER (WHERE j.state = 'running'),
                count(*) FILTER (WHERE j.state = 'failed')
         FROM jobs j JOIN assets a ON a.id = j.asset_id
         WHERE a.owner_id = $1 AND a.purging_at IS NULL AND j.kind = 'pets' AND j.version = $2",
    )
    .bind(owner)
    .bind(pets::VERSION)
    .fetch_one(db)
    .await?;
    let p = pets::progress(db, owner, pets::MODEL, pets::VERSION, basis)
        .await
        .map_err(cyanotype_service::ApiError::Internal)?;
    let percent = if p.total <= 0 {
        100
    } else {
        (p.scanned * 100 / p.total).clamp(0, 100) as u8
    };
    Ok(Some(PetsProgress {
        scanned: p.scanned,
        total: p.total,
        found: p.found,
        unfingerprinted: p.unfingerprinted,
        queued,
        running,
        failed,
        model: pets::MODEL,
        percent,
    }))
}

/// How far finding faces has got for one owner; `None` while the feature is off.
async fn faces_progress(db: &sqlx::PgPool, owner: Uuid, enabled: bool) -> ApiResult<Option<FacesProgress>> {
    if !enabled {
        return Ok(None);
    }
    let (queued, running, failed): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE j.state = 'queued'), count(*) FILTER (WHERE j.state = 'running'),
                count(*) FILTER (WHERE j.state = 'failed')
         FROM jobs j JOIN assets a ON a.id = j.asset_id
         WHERE a.owner_id = $1 AND a.purging_at IS NULL AND j.kind = 'faces' AND j.version = $2",
    )
    .bind(owner)
    .bind(faces::VERSION)
    .fetch_one(db)
    .await?;
    let p = faces::progress(db, owner, faces::MODEL, faces::VERSION)
        .await
        .map_err(cyanotype_service::ApiError::Internal)?;
    let percent = if p.total <= 0 {
        100
    } else {
        (p.scanned * 100 / p.total).clamp(0, 100) as u8
    };
    Ok(Some(FacesProgress {
        scanned: p.scanned,
        total: p.total,
        found: p.faces,
        queued,
        running,
        failed,
        model: faces::MODEL,
        percent,
    }))
}

#[derive(Serialize)]
pub(super) struct Requeued {
    requeued: u64,
}

/// Put the caller's failed jobs back in the queue with a fresh set of attempts. The trigger on `jobs` wakes the
/// worker. Other users' jobs are never touched.
pub(super) async fn retry(State(s): State<AppState>, user: AuthUser) -> ApiResult<Json<Requeued>> {
    let done = sqlx::query(
        "UPDATE jobs SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL
         WHERE state = 'failed'
           AND asset_id IN (SELECT id FROM assets WHERE owner_id = $1 AND purging_at IS NULL)",
    )
    .bind(user.id)
    .execute(&s.db)
    .await?;
    let n = done.rows_affected();
    tracing::info!(requeued = n, "failed jobs requeued");
    Ok(Json(Requeued { requeued: n }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TestDb, app, asset, send, user};
    use axum::http::StatusCode;
    use serde_json::json;

    async fn set_state(pool: &sqlx::PgPool, asset: Uuid, kind: &str, state: &str, err: Option<&str>) {
        sqlx::query("DELETE FROM jobs WHERE asset_id = $1 AND kind = $2")
            .bind(asset)
            .bind(kind)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO jobs (kind, asset_id, state, attempts, last_error, finished_at) VALUES ($1,$2,$3,5,$4, CASE WHEN $3 = 'failed' THEN now() END)")
            .bind(kind)
            .bind(asset)
            .bind(state)
            .bind(err)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn counts_failures_and_owner_isolation() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(&t.pool);
        let (alice, ak) = user(&t.pool, "a@example.com").await;
        let (bob, bk) = user(&t.pool, "b@example.com").await;

        // Idle: nothing active.
        let (st, body) = send(&app, "GET", "/assets/processing", &ak, None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            (
                body["active"].clone(),
                body["queued"].clone(),
                body["total_photos"].clone()
            ),
            (json!(false), json!(0), json!(0))
        );

        let a1 = asset(&t.pool, alice, "a1.jpg").await; // testutil queues a thumbnail job
        let a2 = asset(&t.pool, alice, "a2.jpg").await;
        let a3 = asset(&t.pool, alice, "a3.jpg").await;
        let b1 = asset(&t.pool, bob, "b1.jpg").await;
        set_state(&t.pool, a2, "thumbnail", "running", None).await;
        set_state(&t.pool, a3, "thumbnail", "failed", Some(&"x".repeat(500))).await;
        set_state(&t.pool, a1, "metadata", "queued", None).await;
        set_state(&t.pool, b1, "thumbnail", "failed", Some("bob's problem")).await;

        let (_, body) = send(&app, "GET", "/assets/processing", &ak, None).await;
        assert_eq!(body["active"], json!(true));
        assert_eq!(
            (body["queued"].clone(), body["running"].clone(), body["failed"].clone()),
            (json!(2), json!(1), json!(1))
        );
        assert_eq!(body["total_photos"], json!(3));
        // a1 (thumbnail queued) and a2 (running) are waiting for a thumbnail; a3's job failed.
        assert_eq!(body["without_thumbnail"], json!(2));
        let kinds = body["by_kind"].as_array().unwrap();
        let thumb = kinds.iter().find(|k| k["kind"] == "thumbnail").unwrap();
        assert_eq!(
            (
                thumb["queued"].clone(),
                thumb["running"].clone(),
                thumb["failed"].clone()
            ),
            (json!(1), json!(1), json!(1))
        );
        let failures = body["failures"].as_array().unwrap();
        assert_eq!(failures.len(), 1, "only Alice's failure, never Bob's");
        assert_eq!(failures[0]["asset_id"], json!(a3));
        assert_eq!(failures[0]["original_name"], json!("a3.jpg"));
        assert_eq!(failures[0]["kind"], json!("thumbnail"));
        assert_eq!(failures[0]["attempts"], json!(5));
        let err = failures[0]["last_error"].as_str().unwrap();
        assert_eq!(err.chars().count(), ERROR_CHARS + 1, "truncated with an ellipsis");

        // Bob sees only his own.
        let (_, body) = send(&app, "GET", "/assets/processing", &bk, None).await;
        assert_eq!(
            (
                body["queued"].clone(),
                body["failed"].clone(),
                body["total_photos"].clone()
            ),
            (json!(0), json!(1), json!(1))
        );
        assert_eq!(body["failures"][0]["last_error"], json!("bob's problem"));

        let (st, _) = send(&app, "GET", "/assets/processing", "cyk_nope", None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        t.teardown().await;
    }

    #[tokio::test]
    async fn search_indexing_is_reported_separately_and_does_not_keep_the_library_busy() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(&t.pool);
        let (alice, ak) = user(&t.pool, "a@example.com").await;
        let (bob, bk) = user(&t.pool, "b@example.com").await;
        let a1 = asset(&t.pool, alice, "a1.jpg").await; // has a vector (testutil)
        let a2 = asset(&t.pool, alice, "a2.jpg").await;
        let a3 = asset(&t.pool, alice, "a3.jpg").await;
        let b1 = asset(&t.pool, bob, "b1.jpg").await;
        for id in [a1, a2, a3, b1] {
            sqlx::query("UPDATE assets SET thumbhash = '\\x01' WHERE id = $1")
                .bind(id)
                .execute(&t.pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM jobs WHERE asset_id = $1")
                .bind(id)
                .execute(&t.pool)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM asset_embeddings WHERE asset_id = ANY($1)")
            .bind(vec![a2, a3, b1])
            .execute(&t.pool)
            .await
            .unwrap();
        let v = crate::testutil::TEST_MODEL.version();
        for (id, state) in [(a2, "queued"), (a3, "running"), (b1, "queued")] {
            sqlx::query("INSERT INTO jobs (kind, asset_id, version, state) VALUES ('embed', $1, $2, $3)")
                .bind(id)
                .bind(v)
                .bind(state)
                .execute(&t.pool)
                .await
                .unwrap();
        }
        let (_, body) = send(&app, "GET", "/assets/processing", &ak, None).await;
        assert_eq!(body["active"], json!(false), "indexing alone is not 'processing'");
        assert_eq!((body["queued"].clone(), body["running"].clone()), (json!(0), json!(0)));
        assert_eq!(body["by_kind"], json!([]));
        assert_eq!(
            body["embedding"],
            json!({"indexed": 1, "total": 3, "queued": 1, "running": 1, "failed": 0, "model": "siglip2-b16", "percent": 33,
                   "reindexing": false, "switch_at_percent": 95, "serving_model": "siglip2-b16"})
        );
        // Bob's counts are his own.
        let (_, body) = send(&app, "GET", "/assets/processing", &bk, None).await;
        assert_eq!(
            body["embedding"],
            json!({"indexed": 0, "total": 1, "queued": 1, "running": 0, "failed": 0, "model": "siglip2-b16", "percent": 0,
                   "reindexing": false, "switch_at_percent": 95, "serving_model": "siglip2-b16"})
        );
        // A failed embed job is a failure like any other (and is retried by "Retry failed").
        sqlx::query("UPDATE jobs SET state = 'failed', last_error = 'preview is not a decodable WebP', finished_at = now() WHERE asset_id = $1").bind(a2).execute(&t.pool).await.unwrap();
        let (_, body) = send(&app, "GET", "/assets/processing", &ak, None).await;
        assert_eq!(
            (body["failed"].clone(), body["embedding"]["failed"].clone()),
            (json!(1), json!(1))
        );
        assert_eq!(body["failures"][0]["kind"], json!("embed"));
        t.teardown().await;
    }

    #[tokio::test]
    async fn faces_are_not_mentioned_until_the_feature_is_on_and_then_reported_apart() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(&t.pool);
        let (alice, ak) = user(&t.pool, "a@example.com").await;
        let (bob, _) = user(&t.pool, "b@example.com").await;
        let a1 = asset(&t.pool, alice, "a1.jpg").await;
        let a2 = asset(&t.pool, alice, "a2.jpg").await;
        let b1 = asset(&t.pool, bob, "b1.jpg").await;
        for id in [a1, a2, b1] {
            sqlx::query("UPDATE assets SET thumbhash = '\\x01' WHERE id = $1")
                .bind(id)
                .execute(&t.pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM jobs WHERE asset_id = $1")
                .bind(id)
                .execute(&t.pool)
                .await
                .unwrap();
        }
        // A scan (with two faces), a queued job and a failed one, and one of Bob's.
        faces::store(
            &t.pool,
            a1,
            faces::MODEL,
            faces::VERSION,
            &std::array::from_fn::<_, 2, _>(|_| faces::NewFace {
                bbox: [0.1, 0.1, 0.2, 0.2],
                landmarks: [0.1; 10],
                score: 0.9,
                embedding: None,
            }),
        )
        .await
        .unwrap();
        faces::store(&t.pool, b1, faces::MODEL, faces::VERSION, &[])
            .await
            .unwrap();
        for (id, state) in [(a2, "queued"), (b1, "failed")] {
            sqlx::query("INSERT INTO jobs (kind, asset_id, version, state) VALUES ('faces', $1, $2, $3)")
                .bind(id)
                .bind(faces::VERSION)
                .bind(state)
                .execute(&t.pool)
                .await
                .unwrap();
        }
        // Off (the default): the report has no faces entry at all, and the library is not "busy" with them.
        let (_, body) = send(&app, "GET", "/assets/processing", &ak, None).await;
        assert!(body.get("faces").is_none(), "{body}");
        assert_eq!(body["active"], json!(false));
        // On: this owner's progress.
        let p = faces_progress(&t.pool, alice, true).await.unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            json!({"scanned": 1, "total": 2, "found": 2, "queued": 1, "running": 0, "failed": 0,
                   "model": faces::MODEL, "percent": 50})
        );
        let p = faces_progress(&t.pool, bob, true).await.unwrap().unwrap();
        assert_eq!((p.scanned, p.failed, p.found), (1, 1, 0));
        assert!(faces_progress(&t.pool, alice, false).await.unwrap().is_none());
        t.teardown().await;
    }

    #[tokio::test]
    async fn photos_queued_for_deletion_and_finished_jobs_are_not_counted() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(&t.pool);
        let (alice, ak) = user(&t.pool, "a@example.com").await;
        let a1 = asset(&t.pool, alice, "a1.jpg").await;
        let a2 = asset(&t.pool, alice, "a2.jpg").await;
        set_state(&t.pool, a1, "thumbnail", "done", None).await;
        sqlx::query("UPDATE assets SET purging_at = now(), deleted_at = now() WHERE id = $1")
            .bind(a2)
            .execute(&t.pool)
            .await
            .unwrap();
        let (_, body) = send(&app, "GET", "/assets/processing", &ak, None).await;
        assert_eq!(
            (body["active"].clone(), body["total_photos"].clone()),
            (json!(false), json!(1))
        );
        t.teardown().await;
    }

    #[tokio::test]
    async fn retry_requeues_only_the_callers_failed_jobs() {
        let Some(t) = TestDb::new().await else { return };
        let app = app(&t.pool);
        let (alice, ak) = user(&t.pool, "a@example.com").await;
        let (bob, _bk) = user(&t.pool, "b@example.com").await;
        let a1 = asset(&t.pool, alice, "a1.jpg").await;
        let a2 = asset(&t.pool, alice, "a2.jpg").await;
        let b1 = asset(&t.pool, bob, "b1.jpg").await;
        set_state(&t.pool, a1, "thumbnail", "failed", Some("boom")).await;
        set_state(&t.pool, a1, "metadata", "failed", Some("boom")).await;
        set_state(&t.pool, a2, "thumbnail", "running", None).await;
        set_state(&t.pool, b1, "thumbnail", "failed", Some("boom")).await;

        let (st, body) = send(&app, "POST", "/assets/processing/retry", &ak, None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["requeued"], json!(2));

        let states: Vec<(Uuid, String, String, i32)> =
            sqlx::query_as("SELECT asset_id, kind, state, attempts FROM jobs ORDER BY asset_id, kind")
                .fetch_all(&t.pool)
                .await
                .unwrap();
        let state_of = |a: Uuid, k: &str| states.iter().find(|r| r.0 == a && r.1 == k).unwrap().clone();
        assert_eq!(state_of(a1, "thumbnail").2, "queued");
        assert_eq!(state_of(a1, "thumbnail").3, 0, "a fresh set of attempts");
        assert_eq!(state_of(a1, "metadata").2, "queued");
        assert_eq!(state_of(a2, "thumbnail").2, "running", "running jobs are left alone");
        assert_eq!(
            state_of(b1, "thumbnail").2,
            "failed",
            "another user's failed job is untouched"
        );

        // Nothing left to retry.
        let (_, body) = send(&app, "POST", "/assets/processing/retry", &ak, None).await;
        assert_eq!(body["requeued"], json!(0));
        t.teardown().await;
    }
}
