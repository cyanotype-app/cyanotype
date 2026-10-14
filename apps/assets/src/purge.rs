//! Permanent deletion. A purge is two steps so it can be retried and run next to the API:
//!
//! 1. *Mark*: `assets.purging_at = now()` (user "delete forever", "empty trash", or the janitor finding trash older
//!    than the retention period). From here the row is hidden from the trash, cannot be restored, and uploads of the
//!    same content are refused until it is gone.
//! 2. *Sweep*: for each marked row, remove every version and delete marker of the original and of everything under
//!    its `derived/` prefix, then delete the `jobs`, `asset_embeddings` and `assets` rows in one transaction. If
//!    storage fails the row stays marked and the next sweep retries; deleting what is already gone is not an error.
use std::future::Future;

use cyanotype_core::{keys, storage::Storage};
use sqlx::PgPool;
use tokio::task::JoinSet;
use uuid::Uuid;

/// Rows fetched per round, and how many are purged concurrently.
const BATCH: i64 = 100;
const CONCURRENCY: usize = 8;

/// What the sweep needs from object storage; [`Storage`] in production, a fake in tests.
pub trait Objects: Clone + Send + Sync + 'static {
    /// Permanently delete all versions of `original_key` (exactly) and of everything under `derived_prefix`.
    fn delete_asset_objects(
        &self,
        original_key: &str,
        derived_prefix: &str,
    ) -> impl Future<Output = anyhow::Result<usize>> + Send;
}

impl Objects for Storage {
    async fn delete_asset_objects(&self, original_key: &str, derived_prefix: &str) -> anyhow::Result<usize> {
        let a = self.delete_all_versions(&[original_key.to_string()]).await?;
        let b = self.delete_prefix_all_versions(derived_prefix).await?;
        Ok(a + b)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub purged: usize,
    pub failed: usize,
}

/// Mark trashed rows older than `retention_days` for purging. Returns how many were marked.
pub async fn mark_expired(db: &PgPool, retention_days: i32) -> anyhow::Result<u64> {
    Ok(sqlx::query(
        "UPDATE assets SET purging_at = now()
         WHERE purging_at IS NULL AND deleted_at IS NOT NULL AND deleted_at < now() - make_interval(days => $1)",
    )
    .bind(retention_days)
    .execute(db)
    .await?
    .rows_affected())
}

/// How many rows are marked and still waiting for their objects to be removed.
#[cfg(test)]
pub async fn pending(db: &PgPool) -> anyhow::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM assets WHERE purging_at IS NOT NULL")
        .fetch_one(db)
        .await?;
    Ok(n)
}

#[derive(sqlx::FromRow)]
struct Marked {
    id: Uuid,
    owner_id: Uuid,
    original_key: String,
}

/// Purge every marked row once. Rows whose storage calls fail are counted in `failed` and stay marked.
pub async fn sweep<O: Objects>(db: &PgPool, objects: &O) -> anyhow::Result<Swept> {
    let mut total = Swept::default();
    let mut after = Uuid::nil();
    loop {
        // Keyset on id so a row that keeps failing is not fetched again within this sweep.
        let rows: Vec<Marked> = sqlx::query_as(
            "SELECT id, owner_id, original_key FROM assets
             WHERE purging_at IS NOT NULL AND id > $1 ORDER BY id LIMIT $2",
        )
        .bind(after)
        .bind(BATCH)
        .fetch_all(db)
        .await?;
        let Some(last) = rows.last() else {
            return Ok(total);
        };
        after = last.id;
        for chunk in rows.chunks(CONCURRENCY) {
            let mut set = JoinSet::new();
            for row in chunk {
                let (db, objects) = (db.clone(), objects.clone());
                let (id, owner_id, key) = (row.id, row.owner_id, row.original_key.clone());
                set.spawn(async move {
                    let r = purge_one(&db, &objects, id, owner_id, &key).await;
                    (id, r)
                });
            }
            while let Some(joined) = set.join_next().await {
                match joined {
                    Ok((_, Ok(()))) => total.purged += 1,
                    Ok((id, Err(e))) => {
                        total.failed += 1;
                        tracing::warn!(asset_id = %id, error = ?e, "purge failed; will retry next sweep");
                    }
                    Err(e) => {
                        total.failed += 1;
                        tracing::error!(error = ?e, "purge task panicked");
                    }
                }
            }
        }
    }
}

async fn purge_one<O: Objects>(
    db: &PgPool,
    objects: &O,
    id: Uuid,
    owner: Uuid,
    original_key: &str,
) -> anyhow::Result<()> {
    objects
        .delete_asset_objects(original_key, &keys::derived_prefix(owner, id))
        .await?;
    // Jobs and embeddings cascade from assets, but delete them explicitly so the order is obvious and a
    // running job's later `complete` simply matches nothing.
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM jobs WHERE asset_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM asset_embeddings WHERE asset_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM assets WHERE id = $1 AND purging_at IS NOT NULL")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
