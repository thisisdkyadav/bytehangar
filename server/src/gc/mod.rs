//! Garbage collection. Reclaims physical blobs for soft-deleted files and purges
//! their tombstone rows. Dedup-safe by construction: a blob is deleted only when
//! NO live (non-deleted) file references its `stored_key`. (Upload dedup only ever
//! attaches to live files, so once a key has zero live refs no new file can adopt
//! it — making reclamation race-free.)
//!
//! Operational/admin action: `POST /internal/v1/gc`. Not a tenant capability.
//!
//! Note: this reclaims blobs reachable from tombstone rows. Blobs with NO file row
//! at all (e.g. from a crash mid-upload) need a store-listing reconcile — a
//! separate, backend-specific sweep, not handled here.

use axum::extract::State;
use axum::Json;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AdminAuth;
use crate::blob::BlobBackend;
use crate::error::AppResult;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct GcRequest {
    /// Limit to one tenant (default: all tenants).
    #[serde(default)]
    pub tenant_id: Option<Uuid>,
    /// Only collect files soft-deleted at least this long ago (default: 0 — all).
    #[serde(default)]
    pub older_than_seconds: Option<i64>,
}

#[derive(Serialize)]
pub struct GcReport {
    pub blobs_deleted: u64,
    pub rows_purged: u64,
}

pub async fn run_gc(
    db: &PgPool,
    blob: &dyn BlobBackend,
    tenant_id: Option<Uuid>,
    older_than_seconds: i64,
) -> AppResult<GcReport> {
    let cutoff = Utc::now() - Duration::seconds(older_than_seconds.max(0));

    // Distinct stored_keys reachable from eligible tombstones.
    let candidates: Vec<String> = match tenant_id {
        Some(tenant) => {
            sqlx::query_scalar(
                "SELECT DISTINCT stored_key FROM files \
                 WHERE tenant_id = $1 AND deleted_at IS NOT NULL AND deleted_at < $2",
            )
            .bind(tenant)
            .bind(cutoff)
            .fetch_all(db)
            .await?
        }
        None => {
            sqlx::query_scalar(
                "SELECT DISTINCT stored_key FROM files \
                 WHERE deleted_at IS NOT NULL AND deleted_at < $1",
            )
            .bind(cutoff)
            .fetch_all(db)
            .await?
        }
    };

    let mut blobs_deleted = 0u64;
    let mut rows_purged = 0u64;

    for key in candidates {
        // Reclaim the physical blob only if nothing live still points at it.
        let live: i64 =
            sqlx::query_scalar("SELECT count(*) FROM files WHERE stored_key = $1 AND deleted_at IS NULL")
                .bind(&key)
                .fetch_one(db)
                .await?;
        if live == 0 {
            blob.delete(&key).await?;
            blobs_deleted += 1;
        }
        // Purge the tombstone rows for this key either way.
        let purged = sqlx::query("DELETE FROM files WHERE stored_key = $1 AND deleted_at IS NOT NULL")
            .bind(&key)
            .execute(db)
            .await?
            .rows_affected();
        rows_purged += purged;
    }

    Ok(GcReport {
        blobs_deleted,
        rows_purged,
    })
}

pub async fn gc_handler(
    _admin: AdminAuth,
    State(state): State<AppState>,
    Json(req): Json<GcRequest>,
) -> AppResult<Json<GcReport>> {
    let report = run_gc(
        &state.db,
        state.blob.as_ref(),
        req.tenant_id,
        req.older_than_seconds.unwrap_or(0),
    )
    .await?;
    Ok(Json(report))
}
