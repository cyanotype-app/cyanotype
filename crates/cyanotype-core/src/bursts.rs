//! Bursts: rapid-fire shots of one moment, grouped so search can show them as one result (`docs/library.md`, "Bursts").
//!
//! A burst is a chain of one owner's visible photos taken with one camera (`assets.camera_label`), in capture-time order
//! (`taken_at`, then id), where every photo is at most [`MAX_GAP_MS`] after the one before it **and** looks like it:
//! the cosine of their image embeddings reaches the model's threshold (`cyanotype-clip`, `burst.rs`). A chain is cut
//! where it would span more than [`MAX_SPAN_MS`] or hold more than [`MAX_SIZE`] photos, counting from its start. A photo
//! without a capture time or camera, or with no vector yet, is never grouped (a time-only fallback was left out on purpose:
//! two shots two seconds apart of different things are common, two seconds apart of the same thing is what the
//! embedding is there to say).
//!
//! The result is stored on the rows (`burst_id` = the earliest member's id on every member of a group of two or more, NULL
//! otherwise; `burst_version` = [`VERSION`]) by `burst` jobs, so a search only reads it. A job regroups the neighbourhood
//! of one photo from scratch, which makes it idempotent and safe to run twice: it loads the owner's photos of that camera
//! around the photo's capture time, widening the window until its edges fall in gaps (so chains are never cut by the
//! window), links them, and writes what changed. What queues jobs: the worker after it embedded a photo
//! ([`requeue_after_embed`]), a trigger when a photo is trashed, restored, paired or unpaired (migration 0014), and the
//! worker's sweep for photos never computed or computed with older rules ([`enqueue_missing`]).
use std::collections::HashMap;

use anyhow::Result;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::jobs;

/// The grouping rules' version (the gap, span and size limits below, how chains are cut). Raising it makes the sweep
/// recompute every photo. (A new embedding model needs no bump: each vector it stores queues a regroup.)
pub const VERSION: i32 = 1;
/// The most that may pass between two consecutive shots of a burst. Capture times are whole seconds for most cameras (a
/// burst at 10 frames per second has many photos in one second), so this is "the same or the next second".
pub const MAX_GAP_MS: i64 = 2_000;
/// The most a burst may span from its first to its last photo (a held shutter button, not a walk).
pub const MAX_SPAN_MS: i64 = 30_000;
/// The most photos a burst holds; the chain goes on in the next group.
pub const MAX_SIZE: usize = 40;
/// The first window of a regroup, either side of the photo; it doubles while a chain reaches its edge.
const WINDOW_MS: i64 = 60_000;
const MAX_WIDENINGS: u32 = 8;
/// Priority of a regroup queued by an event (between a new upload's 0 and the backfill's [`jobs::PRIORITY_BACKFILL`]).
const PRIORITY_EVENT: i16 = 5;

/// A model's threshold for "the same shot", in priority order: a pair is judged by the first model that has a vector for
/// both photos (the configured model, then the one it replaces while a library is re-indexed).
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub model: String,
    pub version: i32,
    pub min_cosine: f32,
}

/// One photo of a chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shot {
    pub id: Uuid,
    /// Capture time, milliseconds since the epoch.
    pub at_ms: i64,
}

/// Cut `shots` (capture-time order) into bursts. `similar(i)`: is shot `i` visually the same as shot `i - 1` (asked only
/// when the time gap allows it). Returns the shots' positions per group, in order; a single photo is a group of one.
pub fn group(shots: &[Shot], similar: &dyn Fn(usize) -> bool) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in 0..shots.len() {
        let joins = i > 0
            && groups.last().is_some_and(|g| {
                g.len() < MAX_SIZE
                    && shots[i].at_ms - shots[i - 1].at_ms <= MAX_GAP_MS
                    && shots[i].at_ms - shots[g[0]].at_ms <= MAX_SPAN_MS
            })
            && similar(i);
        match (joins, groups.last_mut()) {
            (true, Some(g)) => g.push(i),
            _ => groups.push(vec![i]),
        }
    }
    groups
}

/// What a regroup found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Regrouped {
    /// Visible photos of the camera in the window.
    pub photos: usize,
    /// Of them, how many are in a burst.
    pub in_bursts: usize,
    /// Rows written (those whose group or version differed).
    pub changed: u64,
}

fn ms(t: OffsetDateTime) -> i64 {
    (t.unix_timestamp_nanos() / 1_000_000) as i64
}

fn at(ms: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// Is the photo visible and already grouped under the current rules? Then a regroup has nothing to add (a sweep job queued
/// before an earlier job covered it).
pub async fn is_current(db: &PgPool, asset_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT burst_version = $2 AND deleted_at IS NULL AND primary_id IS NULL FROM assets WHERE id = $1",
    )
    .bind(asset_id)
    .bind(VERSION)
    .fetch_optional(db)
    .await?
    .flatten()
    .unwrap_or(false))
}

#[derive(sqlx::FromRow)]
struct Anchor {
    owner_id: Uuid,
    camera_label: Option<String>,
    taken_at: Option<OffsetDateTime>,
}

/// Regroup the neighbourhood of `asset_id`: the owner's visible photos of the same camera around its capture time. The photo
/// itself may be trashed or hidden (then it is simply not in the result, and it leaves its old group). Safe to run any
/// number of times and concurrently (runs for one owner are serialised by an advisory lock).
pub async fn regroup(db: &PgPool, asset_id: Uuid, rules: &[Rule]) -> Result<Regrouped> {
    let mut tx = db.begin().await?;
    let Some(anchor): Option<Anchor> =
        sqlx::query_as("SELECT owner_id, camera_label, taken_at FROM assets WHERE id = $1")
            .bind(asset_id)
            .fetch_optional(&mut *tx)
            .await?
    else {
        return Ok(Regrouped::default());
    };
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('burst:' || $1::text, 0))")
        .bind(anchor.owner_id)
        .execute(&mut *tx)
        .await?;

    let (Some(camera), Some(taken)) = (anchor.camera_label, anchor.taken_at) else {
        // Nothing to chain it with: on its own, and computed.
        let changed = set_groups(&mut tx, &[asset_id], &[None]).await?;
        tx.commit().await?;
        return Ok(Regrouped {
            changed,
            ..Default::default()
        });
    };

    // The window: widened while a photo sits within one gap of its edge (a photo just outside might still be linked), so
    // that chains are never cut by it. Finite because the library is.
    let t = ms(taken);
    let (mut lo, mut hi, mut span) = (t - WINDOW_MS, t + WINDOW_MS, WINDOW_MS);
    let mut shots: Vec<Shot>;
    let mut widenings = 0;
    loop {
        let rows: Vec<(Uuid, OffsetDateTime)> = sqlx::query_as(
            "SELECT id, taken_at FROM assets
             WHERE owner_id = $1 AND camera_label = $2 AND taken_at BETWEEN $3 AND $4
               AND deleted_at IS NULL AND primary_id IS NULL
             ORDER BY taken_at, id",
        )
        .bind(anchor.owner_id)
        .bind(&camera)
        .bind(at(lo))
        .bind(at(hi))
        .fetch_all(&mut *tx)
        .await?;
        shots = rows.into_iter().map(|(id, t)| Shot { id, at_ms: ms(t) }).collect();
        let near_edge = shots.first().is_some_and(|s| s.at_ms - lo < MAX_GAP_MS)
            || shots.last().is_some_and(|s| hi - s.at_ms < MAX_GAP_MS);
        if !near_edge || widenings >= MAX_WIDENINGS {
            break;
        }
        span *= 2;
        widenings += 1;
        (lo, hi) = (t - span, t + span);
    }

    // The visual test, only for pairs close enough in time: the first model with a vector for both photos decides.
    let candidates: Vec<usize> = (1..shots.len())
        .filter(|&i| shots[i].at_ms - shots[i - 1].at_ms <= MAX_GAP_MS)
        .collect();
    let mut same: HashMap<usize, bool> = HashMap::new();
    for rule in rules {
        let todo: Vec<usize> = candidates.iter().copied().filter(|i| !same.contains_key(i)).collect();
        if todo.is_empty() {
            break;
        }
        let a: Vec<Uuid> = todo.iter().map(|&i| shots[i - 1].id).collect();
        let b: Vec<Uuid> = todo.iter().map(|&i| shots[i].id).collect();
        let idx: Vec<i32> = todo.iter().map(|&i| i as i32).collect();
        let found: Vec<(i32, f64)> = sqlx::query_as(
            "SELECT p.i, 1 - (ea.embedding <=> eb.embedding)
             FROM unnest($1::uuid[], $2::uuid[], $3::int[]) AS p(a_id, b_id, i)
             JOIN asset_embeddings ea ON ea.asset_id = p.a_id AND ea.model = $4 AND ea.version = $5
             JOIN asset_embeddings eb ON eb.asset_id = p.b_id AND eb.model = $4 AND eb.version = $5",
        )
        .bind(&a)
        .bind(&b)
        .bind(&idx)
        .bind(&rule.model)
        .bind(rule.version)
        .fetch_all(&mut *tx)
        .await?;
        for (i, cos) in found {
            same.insert(i as usize, cos >= f64::from(rule.min_cosine));
        }
    }

    let groups = group(&shots, &|i| same.get(&i).copied().unwrap_or(false));
    let mut ids: Vec<Uuid> = Vec::with_capacity(shots.len() + 1);
    let mut bursts: Vec<Option<Uuid>> = Vec::with_capacity(shots.len() + 1);
    let mut in_bursts = 0;
    for g in &groups {
        let first = shots[g[0]].id;
        for &i in g {
            ids.push(shots[i].id);
            bursts.push((g.len() > 1).then_some(first));
        }
        if g.len() > 1 {
            in_bursts += g.len();
        }
    }
    // The photo itself, when it is not one of the visible ones (trashed, hidden): out of every group, computed.
    let listed = ids.contains(&asset_id);
    if !listed {
        ids.push(asset_id);
        bursts.push(None);
    }
    let changed = set_groups(&mut tx, &ids, &bursts).await?;
    tx.commit().await?;
    Ok(Regrouped {
        photos: shots.len(),
        in_bursts,
        changed,
    })
}

/// Store `bursts[i]` for `ids[i]` and mark the rows computed; only rows that differ are written.
async fn set_groups(tx: &mut sqlx::PgConnection, ids: &[Uuid], bursts: &[Option<Uuid>]) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE assets a SET burst_id = v.b, burst_version = $3
         FROM unnest($1::uuid[], $2::uuid[]) AS v(id, b)
         WHERE a.id = v.id AND (a.burst_id IS DISTINCT FROM v.b OR a.burst_version IS DISTINCT FROM $3)",
    )
    .bind(ids)
    .bind(bursts)
    .bind(VERSION)
    .execute(&mut *tx)
    .await?
    .rows_affected())
}

/// Queue a regroup around `asset_id` unless one is already waiting. Returns whether a job was queued.
pub async fn enqueue(db: &PgPool, asset_id: Uuid, priority: i16) -> Result<bool> {
    Ok(sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version, priority)
         SELECT 'burst', $1, nextval('burst_job_seq'), $2
         WHERE NOT EXISTS (SELECT 1 FROM jobs WHERE kind = 'burst' AND asset_id = $1 AND state = 'queued')",
    )
    .bind(asset_id)
    .bind(priority)
    .execute(db)
    .await?
    .rows_affected()
        > 0)
}

/// A photo's capture time was changed by an edit (`edits.rs`): the burst it was in loses it and the neighbourhood it moved
/// into may gain it, and a regroup around a photo reads that photo's *current* time. So mark the photo and the rest of
/// its old burst as "to be computed" and queue one job at the photo (its new neighbourhood) and one at another member of the
/// old burst (its old neighbourhood, without the photo); the sweep covers anything a lost job leaves.
pub async fn requeue_after_time_change(db: &PgPool, asset_id: Uuid, old_burst: Option<Uuid>) -> Result<()> {
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT owner_id FROM assets WHERE id = $1")
        .bind(asset_id)
        .fetch_optional(db)
        .await?;
    let Some(owner) = owner else { return Ok(()) };
    sqlx::query(
        "UPDATE assets SET burst_version = NULL
         WHERE id = $1 OR (owner_id = $2 AND burst_id IS NOT NULL AND burst_id = $3)",
    )
    .bind(asset_id)
    .bind(owner)
    .bind(old_burst)
    .execute(db)
    .await?;
    enqueue(db, asset_id, PRIORITY_EVENT).await?;
    if let Some(b) = old_burst {
        let other: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM assets WHERE owner_id = $1 AND burst_id = $2 AND id <> $3 AND deleted_at IS NULL
             ORDER BY id LIMIT 1",
        )
        .bind(owner)
        .bind(b)
        .bind(asset_id)
        .fetch_optional(db)
        .await?;
        if let Some(o) = other {
            enqueue(db, o, PRIORITY_EVENT).await?;
        }
    }
    Ok(())
}

/// A photo just got a vector (its first, or the configured model's after a swap): its neighbourhood can be judged now, so
/// mark it to be computed and queue the regroup. Does nothing for photos that can never be in a burst.
pub async fn requeue_after_embed(db: &PgPool, asset_id: Uuid) -> Result<()> {
    let eligible: Option<Uuid> = sqlx::query_scalar(
        "UPDATE assets SET burst_version = NULL
         WHERE id = $1 AND taken_at IS NOT NULL AND camera_label IS NOT NULL AND deleted_at IS NULL AND primary_id IS NULL
         RETURNING id",
    )
    .bind(asset_id)
    .fetch_optional(db)
    .await?;
    if eligible.is_some() {
        enqueue(db, asset_id, PRIORITY_EVENT).await?;
    }
    Ok(())
}

/// The sweep: queue a regroup for every visible photo with a capture time, a camera and a vector that was never computed or
/// was computed under older rules (the backfill of an existing library, a bump of [`VERSION`], jobs that were lost). A job
/// that is queued, running or failed is left alone. Returns how many were queued.
pub async fn enqueue_missing(db: &PgPool) -> Result<u64> {
    Ok(sqlx::query(
        "INSERT INTO jobs (kind, asset_id, version, priority)
         SELECT 'burst', a.id, nextval('burst_job_seq'), $1 FROM assets a
         WHERE a.deleted_at IS NULL AND a.primary_id IS NULL AND a.taken_at IS NOT NULL AND a.camera_label IS NOT NULL
           AND a.burst_version IS DISTINCT FROM $2
           AND EXISTS (SELECT 1 FROM asset_embeddings e WHERE e.asset_id = a.id)
           AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.kind = 'burst' AND j.asset_id = a.id
                           AND j.state IN ('queued', 'running', 'failed'))",
    )
    .bind(jobs::PRIORITY_BACKFILL)
    .bind(VERSION)
    .execute(db)
    .await?
    .rows_affected())
}

/// Finished `burst` jobs are one row per event; keep a day of them.
pub async fn prune_jobs(db: &PgPool) -> Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM jobs WHERE kind = 'burst' AND state = 'done' AND finished_at < now() - interval '1 day'",
    )
    .execute(db)
    .await?
    .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shots(secs: &[f64]) -> Vec<Shot> {
        secs.iter()
            .map(|s| Shot {
                id: Uuid::new_v4(),
                at_ms: (s * 1000.0) as i64,
            })
            .collect()
    }

    fn sizes(g: &[Vec<usize>]) -> Vec<usize> {
        g.iter().map(Vec::len).collect()
    }

    #[test]
    fn photos_close_in_time_and_alike_form_one_burst() {
        let s = shots(&[0.0, 0.0, 1.0, 1.5, 3.0]);
        assert_eq!(group(&s, &|_| true), vec![vec![0, 1, 2, 3, 4]]);
    }

    #[test]
    fn a_gap_of_more_than_two_seconds_ends_the_burst() {
        let s = shots(&[0.0, 1.0, 3.0, 5.1, 6.0]);
        assert_eq!(sizes(&group(&s, &|_| true)), vec![3, 2]);
        // Exactly the limit still joins.
        let s = shots(&[0.0, 2.0]);
        assert_eq!(sizes(&group(&s, &|_| true)), vec![2]);
    }

    #[test]
    fn a_photo_that_looks_different_starts_a_new_group() {
        let s = shots(&[0.0, 0.5, 1.0, 1.5]);
        // Photo 2 is a different picture: [0, 1], then [2, 3].
        assert_eq!(group(&s, &|i| i != 2), vec![vec![0, 1], vec![2, 3]]);
        // Nothing is alike: every photo alone.
        assert_eq!(sizes(&group(&s, &|_| false)), vec![1, 1, 1, 1]);
    }

    #[test]
    fn the_visual_test_is_only_asked_when_the_time_allows() {
        let s = shots(&[0.0, 10.0]);
        let asked = std::cell::Cell::new(0);
        group(&s, &|_| {
            asked.set(asked.get() + 1);
            true
        });
        assert_eq!(asked.get(), 0);
    }

    #[test]
    fn a_burst_spans_at_most_thirty_seconds_and_forty_photos() {
        // One photo a second for a minute, all alike: cut where the span would pass 30 s.
        let secs: Vec<f64> = (0..=60).map(f64::from).collect();
        let g = group(&shots(&secs), &|_| true);
        assert_eq!(sizes(&g), vec![31, 30]);
        // Ten a second for a while: the size limit.
        let secs: Vec<f64> = (0..100).map(|i| f64::from(i) * 0.1).collect();
        let g = group(&shots(&secs), &|_| true);
        assert_eq!(sizes(&g), vec![40, 40, 20]);
    }

    #[test]
    fn nothing_is_nothing() {
        assert!(group(&[], &|_| true).is_empty());
        assert_eq!(group(&shots(&[5.0]), &|_| true), vec![vec![0]]);
    }
}
