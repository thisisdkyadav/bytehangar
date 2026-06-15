//! Usage metering. Core keeps live counters (for inline quota enforcement) and
//! an append-only event log (consumed by the external billing wrapper). Core
//! never knows about price/plan — only the byte numbers.

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::auth::TenantContext;
use crate::error::AppResult;
use crate::state::AppState;

pub async fn record_upload(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    bytes: i64,
) -> AppResult<()> {
    sqlx::query("INSERT INTO usage_events (tenant_id, op, bytes, count) VALUES ($1, 'upload', $2, 1)")
        .bind(tenant_id)
        .bind(bytes)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO usage_counters (tenant_id, used_bytes, object_count, updated_at) \
         VALUES ($1, $2, 1, now()) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
            used_bytes = usage_counters.used_bytes + EXCLUDED.used_bytes, \
            object_count = usage_counters.object_count + 1, \
            updated_at = now()",
    )
    .bind(tenant_id)
    .bind(bytes)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn record_delete(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    bytes: i64,
) -> AppResult<()> {
    sqlx::query("INSERT INTO usage_events (tenant_id, op, bytes, count) VALUES ($1, 'delete', $2, 1)")
        .bind(tenant_id)
        .bind(bytes)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE usage_counters SET \
            used_bytes = GREATEST(0, used_bytes - $2), \
            object_count = GREATEST(0, object_count - 1), \
            updated_at = now() \
         WHERE tenant_id = $1",
    )
    .bind(tenant_id)
    .bind(bytes)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[derive(Serialize, sqlx::FromRow)]
pub struct UsageCounters {
    pub used_bytes: i64,
    pub object_count: i64,
}

/// Tenant-facing usage snapshot (also what a billing wrapper polls).
pub async fn get_usage(
    ctx: TenantContext,
    State(state): State<AppState>,
) -> AppResult<Json<UsageCounters>> {
    let row = sqlx::query_as::<_, UsageCounters>(
        "SELECT used_bytes, object_count FROM usage_counters WHERE tenant_id = $1",
    )
    .bind(ctx.tenant.id)
    .fetch_optional(&state.db)
    .await?;
    Ok(Json(row.unwrap_or(UsageCounters {
        used_bytes: 0,
        object_count: 0,
    })))
}
