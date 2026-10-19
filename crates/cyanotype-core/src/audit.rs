//! The admin audit log (`audit_log`, migration 0016): one row per administrative action, written by whichever service
//! performed it. Details are small JSON objects and never hold a secret, a token or a photo name.
use serde_json::Value;
use sqlx::PgExecutor;
use uuid::Uuid;

/// Records `action` (e.g. `invite.create`) by `actor` on a target. The actor's email is copied in, so the entry still
/// reads after the account is deleted. Pass `None` for an action nobody performed (a scheduled job).
pub async fn record<'e>(
    db: impl PgExecutor<'e>,
    actor: Option<Uuid>,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    detail: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_log (actor_id, actor_email, action, target_type, target_id, detail) \
         VALUES ($1, (SELECT email FROM users WHERE id = $1), $2, $3, $4, $5)",
    )
    .bind(actor)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(detail)
    .execute(db)
    .await?;
    Ok(())
}
