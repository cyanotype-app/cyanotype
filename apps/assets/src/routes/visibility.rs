//! Hidden photos, the archive and the per-user library switches (docs/library.md, "Hidden, archive, memories and recently
//! added").
//!
//! - `POST /assets/hide {ids | scope, hidden}`: hide or unhide. Hiding needs nothing special. **Unhiding needs the Hidden view
//!   unlocked** (`403 hidden_locked`): it is only possible from inside that view, and an API key never has it.
//! - `POST /assets/archive {ids | scope, archived}`: archive or restore. A scope-based archive answers a `batch` that
//!   `{"scope": {"archive_batch": ...}}` restores, which is what the Undo of a bulk archive sends.
//! - `GET|PUT /assets/prefs`: the On this day strip on or off, and the library's order (`taken` or `added`).
//!
//! Both flags belong to the visible photo of a RAW + JPEG pair; the hidden RAW file takes the pair's state too, so separating a
//! pair keeps it. Every hide or unhide is written to the audit log with a count and nothing else (no ids, no names), so an
//! administrator sees that it happened, never what.
//!
//! This module also holds the one place that says which photos a list shows: [`view_sql`].
use std::{collections::HashSet, sync::Mutex, time::Duration};

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::scope::Selector;
use crate::AppState;
use cyanotype_core::{manifest, storage::Storage};
use cyanotype_service::{ApiResult, AuthUser, Unlock, audit};

/// The order of the library: capture time (the default) or upload time.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Sort {
    #[default]
    Taken,
    Added,
}

impl Sort {
    /// The sort time as a SQL expression over `assets` (no alias). `assets_timeline_idx` / `assets_added_idx` hold exactly it.
    pub(super) fn expr(self) -> &'static str {
        match self {
            Sort::Taken => "coalesce(taken_at, file_modified_at, created_at)",
            Sort::Added => "created_at",
        }
    }
}

/// The extra WHERE terms (each starting with ` AND`) that make a list one of the views, on top of "the owner's live photos that
/// are not a hidden RAW file":
///
/// | view | photos |
/// |---|---|
/// | timeline (default) | not hidden, not archived |
/// | favorites | favorites, not hidden (archived ones stay) |
/// | archive | archived, not hidden |
/// | hidden | hidden (**refused unless unlocked**) |
///
/// The day counts use the same text, so they always agree with the list.
pub(super) fn view_sql(favorite: bool, archived: bool, hidden: bool, unlock: Unlock) -> ApiResult<String> {
    if hidden {
        unlock.require()?;
        return Ok(if favorite {
            " AND hidden AND favorite".into()
        } else {
            " AND hidden".into()
        });
    }
    let mut s = String::from(" AND NOT hidden");
    if archived {
        s.push_str(" AND archived");
    } else if !favorite {
        s.push_str(" AND NOT archived");
    }
    if favorite {
        s.push_str(" AND favorite");
    }
    Ok(s)
}

#[derive(Deserialize)]
pub(super) struct HideRequest {
    #[serde(flatten)]
    pick: Selector,
    hidden: bool,
}

#[derive(Serialize)]
pub(super) struct Count {
    count: u64,
}

pub(super) async fn hide(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Json(req): Json<HideRequest>,
) -> ApiResult<Json<Count>> {
    if !req.hidden {
        unlock.require()?;
    }
    let pick = req.pick.validated(unlock)?;
    let mut tx = s.db.begin().await?;
    let ids: Vec<Uuid> = pick.resolve(&mut tx, user.id).await?.unwrap_or_default();
    let n = set_flag(&mut tx, user.id, &ids, "hidden", req.hidden).await?;
    if n > 0 {
        audit::record(
            &mut *tx,
            Some(user.id),
            if req.hidden { "photos.hide" } else { "photos.unhide" },
            None,
            None,
            json!({"count": n}),
        )
        .await?;
    }
    tx.commit().await?;
    if n > 0 {
        manifest_soon(&s, user.id);
    }
    Ok(Json(Count { count: n }))
}

#[derive(Deserialize)]
pub(super) struct ArchiveRequest {
    #[serde(flatten)]
    pick: Selector,
    archived: bool,
}

#[derive(Serialize)]
pub(super) struct ArchiveCount {
    count: u64,
    /// For a scope-based archive: pass back as `{"scope": {"archive_batch": ...}}` to restore exactly what it archived.
    #[serde(skip_serializing_if = "Option::is_none", with = "time::serde::rfc3339::option")]
    batch: Option<time::OffsetDateTime>,
}

pub(super) async fn archive(
    State(s): State<AppState>,
    user: AuthUser,
    unlock: Unlock,
    Json(req): Json<ArchiveRequest>,
) -> ApiResult<Json<ArchiveCount>> {
    let pick = req.pick.validated(unlock)?;
    let mut tx = s.db.begin().await?;
    let (now,): (time::OffsetDateTime,) = sqlx::query_as("SELECT now()").fetch_one(&mut *tx).await?;
    let ids: Vec<Uuid> = pick.resolve(&mut tx, user.id).await?.unwrap_or_default();
    let n = set_flag(&mut tx, user.id, &ids, "archived", req.archived).await?;
    tx.commit().await?;
    if n > 0 {
        manifest_soon(&s, user.id);
    }
    Ok(Json(ArchiveCount {
        count: n,
        batch: (pick.is_scope() && req.archived && n > 0).then_some(now),
    }))
}

/// Set `hidden` or `archived` (a constant column name) on the visible photos among `ids` (a RAW id means its primary) and on
/// their paired RAW files, counting photos whose flag changed. Foreign ids, unknown ids and trashed photos are ignored.
async fn set_flag(tx: &mut sqlx::PgConnection, owner: Uuid, ids: &[Uuid], column: &str, on: bool) -> ApiResult<u64> {
    debug_assert!(column == "hidden" || column == "archived");
    // `archived_at` is stamped with the transaction's time so a scope-based archive can be undone as a batch.
    let extra = if column == "archived" {
        ", archived_at = CASE WHEN $3 THEN now() END"
    } else {
        ""
    };
    let changed: Vec<Uuid> = sqlx::query_scalar(&format!(
        "UPDATE assets SET {column} = $3{extra}
         WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND {column} IS DISTINCT FROM $3
           AND (id = ANY($2) OR id IN (SELECT primary_id FROM assets
                                       WHERE owner_id = $1 AND id = ANY($2) AND primary_id IS NOT NULL))
         RETURNING id"
    ))
    .bind(owner)
    .bind(ids)
    .bind(on)
    .fetch_all(&mut *tx)
    .await?;
    if !changed.is_empty() {
        sqlx::query(&format!(
            "UPDATE assets SET {column} = $3 WHERE owner_id = $1 AND primary_id = ANY($2)"
        ))
        .bind(owner)
        .bind(&changed)
        .bind(on)
        .execute(&mut *tx)
        .await?;
    }
    Ok(changed.len() as u64)
}

// ---------------------------------------------------------------------------------------------------------------------------
// Prefs

#[derive(Serialize, sqlx::FromRow)]
pub(super) struct Prefs {
    /// Show the On this day strip at the top of the library.
    pub(super) memories_enabled: bool,
    /// `taken` or `added`.
    library_sort: String,
}

pub(super) async fn prefs(State(s): State<AppState>, user: AuthUser) -> ApiResult<Json<Prefs>> {
    Ok(Json(load_prefs(&s.db, user.id).await?))
}

pub(super) async fn load_prefs(db: &PgPool, user: Uuid) -> ApiResult<Prefs> {
    Ok(
        sqlx::query_as::<_, Prefs>("SELECT memories_enabled, library_sort FROM user_prefs WHERE user_id = $1")
            .bind(user)
            .fetch_optional(db)
            .await?
            .unwrap_or(Prefs {
                memories_enabled: true,
                library_sort: "taken".into(),
            }),
    )
}

#[derive(Deserialize)]
pub(super) struct PrefsPatch {
    memories_enabled: Option<bool>,
    library_sort: Option<Sort>,
}

pub(super) async fn set_prefs(
    State(s): State<AppState>,
    user: AuthUser,
    Json(p): Json<PrefsPatch>,
) -> ApiResult<Json<Prefs>> {
    let sort = p.library_sort.map(|s| match s {
        Sort::Taken => "taken",
        Sort::Added => "added",
    });
    sqlx::query(
        "INSERT INTO user_prefs (user_id, memories_enabled, library_sort)
         VALUES ($1, coalesce($2, true), coalesce($3, 'taken'))
         ON CONFLICT (user_id) DO UPDATE SET memories_enabled = coalesce($2, user_prefs.memories_enabled),
                                             library_sort = coalesce($3, user_prefs.library_sort)",
    )
    .bind(user.id)
    .bind(p.memories_enabled)
    .bind(sort)
    .execute(&s.db)
    .await?;
    Ok(Json(load_prefs(&s.db, user.id).await?))
}

// ---------------------------------------------------------------------------------------------------------------------------
// Recovery manifest, soon

/// How long after a hide or archive the owner's recovery manifest is rewritten (a burst of changes becomes one write).
const MANIFEST_DELAY: Duration = Duration::from_secs(30);

static PENDING: Mutex<Option<HashSet<Uuid>>> = Mutex::new(None);

/// Hiding is a privacy promise, and a rebuild from the bucket alone can only keep it as far as the newest manifest is current:
/// the daily one is up to a day old, so a photo hidden this morning would come back visible. Writing the owner's manifest
/// shortly after every hide, unhide or archive (debounced per owner) shrinks that window to about half a minute. Best effort:
/// a failure is only logged, the daily pass still runs.
pub(super) fn manifest_soon(s: &AppState, owner: Uuid) {
    {
        let mut guard = PENDING.lock().unwrap_or_else(|p| p.into_inner());
        if !guard.get_or_insert_with(HashSet::new).insert(owner) {
            return; // one is already scheduled
        }
    }
    let (db, storage) = (s.db.clone(), s.storage.clone());
    tokio::spawn(async move {
        tokio::time::sleep(MANIFEST_DELAY).await;
        PENDING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_or_insert_with(HashSet::new)
            .remove(&owner);
        write_manifest(&db, &storage, owner).await;
    });
}

async fn write_manifest(db: &PgPool, storage: &Storage, owner: Uuid) {
    // MANIFEST_KEEP=0 turns manifests off, here as for the daily job.
    let keep = match manifest::keep_from_env() {
        Ok(0) | Err(_) => return,
        Ok(k) => k,
    };
    if let Err(e) = manifest::write_user(db, storage, owner, keep).await {
        tracing::warn!(user_id = %owner, error = ?e, "could not write the recovery manifest after a hide or archive");
    }
}
