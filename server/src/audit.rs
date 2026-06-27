//! Best-effort audit log of admin / provisioning actions (writes to `audit_log`).
//! Never fails the originating request — a failed audit insert is logged and ignored.

use sqlx::PgPool;
use uuid::Uuid;

pub async fn record(
    db: &PgPool,
    tenant_id: Option<Uuid>,
    actor: &str,
    action: &str,
    target: Option<&str>,
    after: serde_json::Value,
) {
    if let Err(err) = sqlx::query(
        "INSERT INTO audit_log (tenant_id, actor, action, target, after) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tenant_id)
    .bind(actor)
    .bind(action)
    .bind(target)
    .bind(after)
    .execute(db)
    .await
    {
        tracing::warn!("audit log write failed for {action}: {err}");
    }
}
