//! Burst grouping over a real Postgres: chains of one owner's photos of one camera, the gap and the visual test, the model
//! fallback, windows wider than the first guess, idempotence, and what queues a regroup (a trigger on trash / restore / pairing,
//! the embed hand-off, the sweep). Need `DATABASE_URL` (see `tests/jobs.rs`); without it the tests return early.
use cyanotype_core::{
    bursts::{self, Rule},
    db, embeddings,
};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

struct TestDb {
    pool: PgPool,
    admin_url: String,
    name: String,
}

impl TestDb {
    async fn new() -> Option<Self> {
        let admin_url = std::env::var("DATABASE_URL").ok().filter(|v| !v.is_empty())?;
        let name = format!("cyanotype_test_{}", Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.unwrap();
        let (base, _) = admin_url.rsplit_once('/').unwrap();
        let pool = db::connect(&format!("{base}/{name}")).await.unwrap();
        db::migrate(&pool).await.unwrap();
        Some(Self { pool, admin_url, name })
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

const NEW: &str = "new-model";
const OLD: &str = "old-model";

fn rules() -> Vec<Rule> {
    vec![
        Rule {
            model: NEW.into(),
            version: 2,
            min_cosine: 0.9,
        },
        Rule {
            model: OLD.into(),
            version: 1,
            min_cosine: 0.8,
        },
    ]
}

async fn user(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ($1, 'x') RETURNING id")
        .bind(format!("{}@example.com", Uuid::new_v4()))
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A photo taken `secs` seconds after a fixed start with `camera`; vector `axis` (photos on one axis look alike, on
/// different axes they do not) for the new model.
async fn photo(pool: &PgPool, owner: Uuid, camera: &str, secs: f64, axis: usize) -> Uuid {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, thumbhash,
                             camera_make, camera_model, taken_at)
         VALUES ($1, sha256($2::bytea), 'k', 'x.jpg', 'image/jpeg', 1, '\\x01', 'Canon', $3,
                 '2024-06-01T12:00:00Z'::timestamptz + make_interval(secs => $4))
         RETURNING id",
    )
    .bind(owner)
    .bind(Uuid::new_v4().as_bytes())
    .bind(camera)
    .bind(secs)
    .fetch_one(pool)
    .await
    .unwrap();
    embed(pool, id, NEW, 2, axis, 0.0).await;
    id
}

/// A unit vector mostly on `axis`, leaning `lean` towards the next axis.
async fn embed(pool: &PgPool, id: Uuid, model: &str, version: i32, axis: usize, lean: f32) {
    let mut v = [0.0f32; 8];
    v[axis % 8] = 1.0;
    v[(axis + 1) % 8] = lean;
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let v: Vec<f32> = v.iter().map(|x| x / n).collect();
    embeddings::store(pool, id, model, version, &v).await.unwrap();
}

async fn burst_of(pool: &PgPool, id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT burst_id FROM assets WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn queued(pool: &PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'burst' AND asset_id = $1 AND state = 'queued'")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn photos_of_one_camera_close_in_time_and_alike_become_a_burst_named_after_the_first() {
    let Some(t) = TestDb::new().await else { return };
    let o = user(&t.pool).await;
    let (a, b, c) = (
        photo(&t.pool, o, "R5", 0.0, 0).await,
        photo(&t.pool, o, "R5", 0.0, 0).await,
        photo(&t.pool, o, "R5", 1.0, 0).await,
    );
    // Two seconds later is still the burst; 2.5 s after that is not; a different picture 1 s later breaks it too.
    let d = photo(&t.pool, o, "R5", 3.0, 0).await;
    let late = photo(&t.pool, o, "R5", 5.6, 0).await;
    let other_scene = photo(&t.pool, o, "R5", 6.0, 3).await;
    let r = bursts::regroup(&t.pool, c, &rules()).await.unwrap();
    assert_eq!((r.photos, r.in_bursts), (6, 6 - 2));

    // The group's id is the earliest photo's (ties by id).
    let first = std::cmp::min(a, b);
    for m in [a, b, c, d] {
        assert_eq!(burst_of(&t.pool, m).await, Some(first), "{m}");
    }
    assert_eq!(burst_of(&t.pool, late).await, None, "2.6 s after the last: on its own");
    assert_eq!(burst_of(&t.pool, other_scene).await, None);
    let v: i32 = sqlx::query_scalar("SELECT burst_version FROM assets WHERE id = $1")
        .bind(other_scene)
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(v, bursts::VERSION, "singles are marked computed too");

    // Idempotent: nothing is written the second time, from any photo of the neighbourhood.
    for anchor in [a, late, other_scene] {
        assert_eq!(bursts::regroup(&t.pool, anchor, &rules()).await.unwrap().changed, 0);
    }
    t.teardown().await;
}

#[tokio::test]
async fn other_cameras_other_owners_and_photos_without_a_vector_are_never_grouped() {
    let Some(t) = TestDb::new().await else { return };
    let (o, p) = (user(&t.pool).await, user(&t.pool).await);
    let a = photo(&t.pool, o, "R5", 0.0, 0).await;
    let b = photo(&t.pool, o, "iPhone", 0.5, 0).await;
    let c = photo(&t.pool, p, "R5", 1.0, 0).await;
    let no_vector = photo(&t.pool, o, "R5", 1.5, 0).await;
    sqlx::query("DELETE FROM asset_embeddings WHERE asset_id = $1")
        .bind(no_vector)
        .execute(&t.pool)
        .await
        .unwrap();
    let no_time = photo(&t.pool, o, "R5", 1.0, 0).await;
    sqlx::query("UPDATE assets SET taken_at = NULL WHERE id = $1")
        .bind(no_time)
        .execute(&t.pool)
        .await
        .unwrap();
    bursts::regroup(&t.pool, a, &rules()).await.unwrap();
    for id in [a, b, c, no_vector, no_time] {
        assert_eq!(burst_of(&t.pool, id).await, None, "{id}");
    }
    // The photo without a capture time is computed (on its own) when asked about.
    let r = bursts::regroup(&t.pool, no_time, &rules()).await.unwrap();
    assert_eq!(r.photos, 0);
    t.teardown().await;
}

#[tokio::test]
async fn while_a_library_is_re_indexed_a_pair_is_judged_by_the_model_that_has_both_vectors() {
    let Some(t) = TestDb::new().await else { return };
    let o = user(&t.pool).await;
    let (a, b, c) = (
        photo(&t.pool, o, "R5", 0.0, 0).await,
        photo(&t.pool, o, "R5", 0.5, 0).await,
        photo(&t.pool, o, "R5", 1.0, 0).await,
    );
    // The new model's vectors: a and b alike; c leans away (cosine 0.894 to b: under the new model's 0.9 ...).
    embed(&t.pool, c, NEW, 2, 0, 0.5).await;
    // ... but the old model, which has all three, sees b and c as alike (cosine 0.894 >= its 0.8).
    for (id, lean) in [(a, 0.0), (b, 0.0), (c, 0.5)] {
        embed(&t.pool, id, OLD, 1, 0, lean).await;
    }
    bursts::regroup(&t.pool, a, &rules()).await.unwrap();
    assert_eq!(burst_of(&t.pool, a).await, burst_of(&t.pool, b).await);
    assert_eq!(
        burst_of(&t.pool, c).await,
        None,
        "the configured model decides when it has both vectors"
    );

    // c has no vector of the new model yet: the old one decides, and c joins.
    sqlx::query("DELETE FROM asset_embeddings WHERE asset_id = $1 AND model = $2")
        .bind(c)
        .bind(NEW)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET burst_version = NULL")
        .execute(&t.pool)
        .await
        .unwrap();
    bursts::regroup(&t.pool, a, &rules()).await.unwrap();
    assert_eq!(burst_of(&t.pool, c).await, burst_of(&t.pool, a).await);
    assert!(burst_of(&t.pool, c).await.is_some());
    t.teardown().await;
}

#[tokio::test]
async fn trashing_restoring_and_pairing_queue_a_regroup_and_the_neighbours_follow() {
    let Some(t) = TestDb::new().await else { return };
    let o = user(&t.pool).await;
    let ids: Vec<Uuid> = {
        let mut v = Vec::new();
        for s in [0.0, 1.0, 2.0] {
            v.push(photo(&t.pool, o, "R5", s, 0).await);
        }
        v
    };
    bursts::regroup(&t.pool, ids[0], &rules()).await.unwrap();
    assert!(burst_of(&t.pool, ids[1]).await.is_some());
    assert_eq!(queued(&t.pool, ids[1]).await, 0, "no job from the grouping itself");

    // Trash the middle photo: the trigger queues its regroup and marks it to be computed.
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(ids[1])
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(queued(&t.pool, ids[1]).await, 1);
    let version: Option<i32> = sqlx::query_scalar("SELECT burst_version FROM assets WHERE id = $1")
        .bind(ids[1])
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(version, None);
    // Running it (the photo is hidden now) regroups the others without it: 0 and 2 are two seconds apart, still linked.
    bursts::regroup(&t.pool, ids[1], &rules()).await.unwrap();
    assert_eq!(burst_of(&t.pool, ids[1]).await, None);
    assert_eq!(burst_of(&t.pool, ids[0]).await, burst_of(&t.pool, ids[2]).await);
    // Trash the last one too: the first is alone.
    sqlx::query("UPDATE assets SET deleted_at = now() WHERE id = $1")
        .bind(ids[2])
        .execute(&t.pool)
        .await
        .unwrap();
    bursts::regroup(&t.pool, ids[2], &rules()).await.unwrap();
    assert_eq!(burst_of(&t.pool, ids[0]).await, None);
    // Restore it: back in the group, once the queued job has run.
    sqlx::query("UPDATE assets SET deleted_at = NULL WHERE id = $1")
        .bind(ids[2])
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        queued(&t.pool, ids[2]).await,
        1,
        "the trash job is still waiting: one job covers both"
    );
    bursts::regroup(&t.pool, ids[2], &rules()).await.unwrap();
    assert!(burst_of(&t.pool, ids[0]).await.is_some());

    // A RAW paired behind a photo hides it: it leaves its group (pairing queues the regroup the same way).
    sqlx::query("UPDATE assets SET primary_id = $2 WHERE id = $1")
        .bind(ids[2])
        .bind(ids[0])
        .execute(&t.pool)
        .await
        .unwrap();
    assert!(queued(&t.pool, ids[2]).await >= 1);
    bursts::regroup(&t.pool, ids[2], &rules()).await.unwrap();
    assert_eq!(burst_of(&t.pool, ids[0]).await, None);
    assert_eq!(burst_of(&t.pool, ids[2]).await, None);
    // Updates that change neither (a favorite) queue nothing and do not reset the computed mark.
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'burst'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET favorite = true WHERE id = $1")
        .bind(ids[0])
        .execute(&t.pool)
        .await
        .unwrap();
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'burst'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    t.teardown().await;
}

#[tokio::test]
async fn a_chain_longer_than_the_first_window_is_cut_the_same_way_from_whichever_photo_asks() {
    let Some(t) = TestDb::new().await else { return };
    let o = user(&t.pool).await;
    // One photo a second for 200 seconds, all alike: one chain, cut at 30 s from its start whatever the window.
    let mut ids = Vec::new();
    for s in 0..200 {
        ids.push(photo(&t.pool, o, "R5", f64::from(s), 0).await);
    }
    bursts::regroup(&t.pool, ids[100], &rules()).await.unwrap();
    let mut groups: std::collections::BTreeMap<Uuid, usize> = Default::default();
    for id in &ids {
        *groups.entry(burst_of(&t.pool, *id).await.unwrap()).or_default() += 1;
    }
    let sizes: Vec<usize> = groups.values().copied().collect();
    assert_eq!(sizes.iter().sum::<usize>(), 200);
    assert!(sizes.iter().all(|s| *s <= 31 && *s >= 2), "{sizes:?}");
    let snapshot: Vec<Option<Uuid>> = burst_ids(&t.pool, &ids).await;
    // Asking from the far end changes nothing.
    assert_eq!(bursts::regroup(&t.pool, ids[0], &rules()).await.unwrap().changed, 0);
    assert_eq!(bursts::regroup(&t.pool, ids[199], &rules()).await.unwrap().changed, 0);
    assert_eq!(burst_ids(&t.pool, &ids).await, snapshot);
    t.teardown().await;
}

async fn burst_ids(pool: &PgPool, ids: &[Uuid]) -> Vec<Option<Uuid>> {
    let mut out = Vec::new();
    for id in ids {
        out.push(burst_of(pool, *id).await);
    }
    out
}

#[tokio::test]
async fn the_sweep_queues_what_was_never_grouped_and_leaves_queued_and_failed_jobs_alone() {
    let Some(t) = TestDb::new().await else { return };
    let o = user(&t.pool).await;
    let a = photo(&t.pool, o, "R5", 0.0, 0).await;
    let b = photo(&t.pool, o, "R5", 1.0, 0).await;
    let c = photo(&t.pool, o, "R5", 50.0, 0).await;
    let no_vector = photo(&t.pool, o, "R5", 80.0, 0).await;
    sqlx::query("DELETE FROM asset_embeddings WHERE asset_id = $1")
        .bind(no_vector)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        bursts::enqueue_missing(&t.pool).await.unwrap(),
        3,
        "a, b and c, not the photo without a vector"
    );
    assert_eq!(bursts::enqueue_missing(&t.pool).await.unwrap(), 0, "queued already");

    // Once computed (version current) the sweep has nothing to do, and a bump of the rules would queue everything.
    bursts::regroup(&t.pool, a, &rules()).await.unwrap();
    bursts::regroup(&t.pool, c, &rules()).await.unwrap();
    sqlx::query("UPDATE jobs SET state = 'done' WHERE kind = 'burst'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(bursts::enqueue_missing(&t.pool).await.unwrap(), 0);
    sqlx::query("UPDATE assets SET burst_version = burst_version - 1")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(bursts::enqueue_missing(&t.pool).await.unwrap(), 3);
    // A failed job is not queued again by every sweep.
    sqlx::query("UPDATE jobs SET state = 'failed' WHERE kind = 'burst' AND asset_id = $1")
        .bind(b)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM jobs WHERE kind = 'burst' AND asset_id <> $1")
        .bind(b)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(bursts::enqueue_missing(&t.pool).await.unwrap(), 2);

    // An embed hand-off marks the photo to be computed and queues one job, not one per call.
    sqlx::query("DELETE FROM jobs WHERE kind = 'burst'")
        .execute(&t.pool)
        .await
        .unwrap();
    bursts::requeue_after_embed(&t.pool, c).await.unwrap();
    bursts::requeue_after_embed(&t.pool, c).await.unwrap();
    assert_eq!(queued(&t.pool, c).await, 1);
    assert!(!bursts::is_current(&t.pool, c).await.unwrap());
    // Photos that can never be in a burst queue nothing.
    sqlx::query("UPDATE assets SET taken_at = NULL WHERE id = $1")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    bursts::requeue_after_embed(&t.pool, a).await.unwrap();
    assert_eq!(queued(&t.pool, a).await, 0);
    // Old finished jobs go.
    sqlx::query("INSERT INTO jobs (kind, asset_id, version, state, finished_at) VALUES ('burst', $1, 9001, 'done', now() - interval '2 days')")
        .bind(a)
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(bursts::prune_jobs(&t.pool).await.unwrap(), 1);
    t.teardown().await;
}

/// The owner's choices about stacks (migration 0033, `stack_id`, `stack_unlinked`, `stack_cover`) are columns the grouping never
/// writes: a regroup, the sweep and a capture-time change rewrite `burst_id` and `burst_version` and leave them as they are.
#[tokio::test]
async fn what_the_owner_decided_about_a_stack_survives_regroups_and_the_sweep() {
    let Some(t) = TestDb::new().await else { return };
    let o = user(&t.pool).await;
    let (a, b, c, d) = (
        photo(&t.pool, o, "R5", 0.0, 0).await,
        photo(&t.pool, o, "R5", 0.5, 0).await,
        photo(&t.pool, o, "R5", 1.0, 0).await,
        photo(&t.pool, o, "R5", 1.5, 0).await,
    );
    bursts::regroup(&t.pool, a, &rules()).await.unwrap();
    let first = burst_of(&t.pool, a).await;
    assert!(first.is_some());

    // b was taken out of the burst, c and d were stacked by hand (c is the cover), a stays with the burst.
    let manual = Uuid::new_v4();
    sqlx::query("UPDATE assets SET stack_unlinked = true WHERE id = $1")
        .bind(b)
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE assets SET stack_id = $2, stack_cover = (id = $3) WHERE id = ANY($1)")
        .bind(vec![c, d])
        .bind(manual)
        .bind(c)
        .execute(&t.pool)
        .await
        .unwrap();
    let decided = |pool: PgPool| async move {
        sqlx::query_as::<_, (Uuid, Option<Uuid>, bool, bool)>(
            "SELECT id, stack_id, stack_unlinked, stack_cover FROM assets ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    let before = decided(t.pool.clone()).await;

    // The neighbourhood is computed again from scratch, by every path that can: from a photo, by the sweep, after a time change.
    sqlx::query("UPDATE assets SET burst_id = NULL, burst_version = NULL")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(bursts::enqueue_missing(&t.pool).await.unwrap(), 4);
    bursts::regroup(&t.pool, d, &rules()).await.unwrap();
    assert_eq!(
        burst_of(&t.pool, a).await,
        first,
        "the burst is found again, on all four"
    );
    assert_eq!(
        burst_of(&t.pool, b).await,
        first,
        "b is still in the stored burst, which the stack rules ignore"
    );
    bursts::requeue_after_time_change(&t.pool, c, first).await.unwrap();
    bursts::regroup(&t.pool, c, &rules()).await.unwrap();
    assert_eq!(
        decided(t.pool.clone()).await,
        before,
        "nothing the owner decided was touched"
    );

    // The stack as the routes see it: a with nobody, b alone, c and d together.
    let keys: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, coalesce(stack_id, CASE WHEN stack_unlinked THEN NULL ELSE burst_id END) FROM assets ORDER BY id",
    )
    .fetch_all(&t.pool)
    .await
    .unwrap();
    let key = |id: Uuid| keys.iter().find(|k| k.0 == id).unwrap().1;
    assert_eq!(key(b), None);
    assert_eq!((key(c), key(d)), (Some(manual), Some(manual)));
    assert_eq!(key(a), first, "a keeps the burst, which still names the earliest photo");
    t.teardown().await;
}

/// How long a regroup takes on the synthetic 50,000-photo library of `scripts/search-perf-seed.sql` (CLIP vectors), after
/// adding 3 near-identical shots a third of a second apart to each of 5,000 photos, and the sweep that groups it all:
///
/// ```text
/// PERF_DATABASE_URL=postgres://postgres:pw@127.0.0.1:5432/cy_perf \
///   cargo test --release -p cyanotype-core --test bursts burst_performance -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "needs the seeded perf database (and adds photos to it)"]
async fn burst_performance_on_50k_photos() {
    use std::time::Instant;
    let Ok(url) = std::env::var("PERF_DATABASE_URL") else {
        return;
    };
    let pool = db::connect(&url).await.unwrap();
    let clip = Rule {
        model: "clip-vit-b32-openai-int8".into(),
        version: 1,
        min_cosine: 0.87,
    };
    let (have,): (i64,) = sqlx::query_as("SELECT count(*) FROM assets WHERE original_key LIKE 'originals/burst/%'")
        .fetch_one(&pool)
        .await
        .unwrap();
    if have == 0 {
        for shot in 1..=3 {
            sqlx::query(
                "WITH src AS (SELECT * FROM assets WHERE original_key LIKE 'originals/perf/%' AND camera_label IS NOT NULL
                              ORDER BY id LIMIT 5000),
                      ins AS (
                        INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, width, height,
                                            taken_at, camera_make, camera_model, thumbhash)
                        SELECT owner_id, sha256(gen_random_uuid()::text::bytea), 'originals/burst/' || id || '/' || $1::text,
                               original_name, content_type, size_bytes, width, height,
                               taken_at + make_interval(secs => 0.33 * $1), camera_make, camera_model, '\\x01'
                        FROM src RETURNING id, original_key)
                 INSERT INTO asset_embeddings (asset_id, model, version, embedding)
                 SELECT ins.id, e.model, e.version, e.embedding FROM ins
                 JOIN assets s ON s.id = split_part(ins.original_key, '/', 3)::uuid
                 JOIN asset_embeddings e ON e.asset_id = s.id",
            )
            .bind(shot)
            .execute(&pool)
            .await
            .unwrap();
        }
    }
    sqlx::query("UPDATE assets SET burst_id = NULL, burst_version = NULL")
        .execute(&pool)
        .await
        .unwrap();
    let started = Instant::now();
    let queued = bursts::enqueue_missing(&pool).await.unwrap();
    println!(
        "\nsweep queued {queued} jobs in {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT asset_id FROM jobs WHERE kind = 'burst' AND state = 'queued' ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let (mut lat, mut ran, mut skipped) = (Vec::new(), 0, 0);
    let all = Instant::now();
    for id in &ids {
        if bursts::is_current(&pool, *id).await.unwrap() {
            skipped += 1;
            continue;
        }
        let t = Instant::now();
        bursts::regroup(&pool, *id, std::slice::from_ref(&clip)).await.unwrap();
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
        ran += 1;
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| lat[((lat.len() as f64 * p) as usize).min(lat.len() - 1)];
    let (grouped,): (i64,) = sqlx::query_as("SELECT count(*) FROM assets WHERE burst_id IS NOT NULL")
        .fetch_one(&pool)
        .await
        .unwrap();
    println!(
        "{} photos: {ran} regroups ran ({skipped} jobs found their photo already grouped), p50 {:.1} ms, p95 {:.1} ms, total {:.1} s; {grouped} photos in bursts",
        ids.len(),
        pct(0.5),
        pct(0.95),
        all.elapsed().as_secs_f64()
    );
}
