//! Stacks made by hand (`assets.stack_id`, migration 0033; docs/library.md, "Stacks in the timeline"): the rule that a stack
//! is two or more photos, kept by every path that takes a photo out of one.
//!
//! A member counts while it is live (not trashed, not the hidden RAW half of a pair). A trashed member keeps its `stack_id`
//! as long as the stack still has two live photos, so restoring it puts it back; when the live photos drop below two the
//! whole stack is dissolved, trashed members included: the stack is gone, and a restored photo is an ordinary one (back in
//! its burst, if it was in one). The cover flag goes with the stack; when only the cover leaves, nothing is set: the cover
//! falls back to the first favorite, else the earliest photo.
use sqlx::PgConnection;
use uuid::Uuid;

/// Dissolve every stack among `stacks` (stack ids) of `owner` that has fewer than two live photos left: all its members
/// (trashed ones too) lose `stack_id` and the cover flag. Returns how many photos were freed. Call it in the transaction
/// that shrank the stack.
pub async fn dissolve_small(conn: &mut PgConnection, owner: Uuid, stacks: &[Uuid]) -> sqlx::Result<u64> {
    if stacks.is_empty() {
        return Ok(0);
    }
    let done = sqlx::query(
        "UPDATE assets SET stack_id = NULL, stack_cover = false
         WHERE owner_id = $1 AND stack_id = ANY($2)
           AND stack_id IN (SELECT stack_id FROM assets WHERE owner_id = $1 AND stack_id = ANY($2)
                            GROUP BY stack_id
                            HAVING count(*) FILTER (WHERE deleted_at IS NULL AND primary_id IS NULL) < 2)",
    )
    .bind(owner)
    .bind(stacks)
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

/// After `assets` were trashed: dissolve the stacks they were in that are down to fewer than two live photos. A photo not
/// in a stack is ignored.
pub async fn dissolve_small_around(conn: &mut PgConnection, owner: Uuid, assets: &[Uuid]) -> sqlx::Result<u64> {
    let stacks: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT stack_id FROM assets WHERE owner_id = $1 AND id = ANY($2) AND stack_id IS NOT NULL",
    )
    .bind(owner)
    .bind(assets)
    .fetch_all(&mut *conn)
    .await?;
    dissolve_small(conn, owner, &stacks).await
}

/// Take `assets` out of their manual stacks (hidden photos, a file that became the RAW of a photo), then dissolve a stack
/// that is down to fewer than two live photos. They are not marked as taken out (`stack_unlinked`): they may join a burst,
/// like any photo. Returns how many photos left a stack.
pub async fn leave(conn: &mut PgConnection, owner: Uuid, assets: &[Uuid]) -> sqlx::Result<u64> {
    let mut stacks: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT stack_id FROM assets WHERE owner_id = $1 AND id = ANY($2) AND stack_id IS NOT NULL",
    )
    .bind(owner)
    .bind(assets)
    .fetch_all(&mut *conn)
    .await?;
    stacks.sort();
    let left = sqlx::query(
        "UPDATE assets SET stack_id = NULL, stack_cover = false WHERE owner_id = $1 AND id = ANY($2) AND stack_id IS NOT NULL",
    )
    .bind(owner)
    .bind(assets)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    dissolve_small(conn, owner, &stacks).await?;
    Ok(left)
}
