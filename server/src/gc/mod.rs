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

use std::sync::Arc;
use std::time::Duration as StdDuration;

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

/// Fixed key for the GC advisory lock (serializes GC across workers/instances).
const GC_ADVISORY_LOCK: i64 = 4_271_990_001;

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

/// Run GC under a Postgres advisory lock so concurrent runs (multiple workers /
/// instances, or the scheduler racing the admin endpoint) can't overlap. Returns
/// an empty report when another GC already holds the lock.
pub async fn run_gc(
    db: &PgPool,
    blob: &dyn BlobBackend,
    tenant_id: Option<Uuid>,
    older_than_seconds: i64,
) -> AppResult<GcReport> {
    let mut lock = db.acquire().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(GC_ADVISORY_LOCK)
        .fetch_one(&mut *lock)
        .await?;
    if !acquired {
        return Ok(GcReport {
            blobs_deleted: 0,
            rows_purged: 0,
        });
    }
    let result = run_gc_inner(db, blob, tenant_id, older_than_seconds).await;
    // Release on the same session connection (must run before `lock` returns to the pool).
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(GC_ADVISORY_LOCK)
        .execute(&mut *lock)
        .await;
    result
}

async fn run_gc_inner(
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

/// Internal GC scheduler: periodically reclaims soft-deleted blobs (past the
/// retention window) and prunes consumed/expired upload grants. Stops on shutdown.
pub async fn run_scheduler(
    db: PgPool,
    blob: Arc<dyn BlobBackend>,
    interval_secs: u64,
    retention_secs: i64,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(StdDuration::from_secs(interval_secs)) => {}
            _ = shutdown.changed() => {}
        }
        if *shutdown.borrow() {
            break;
        }

        match run_gc(&db, blob.as_ref(), None, retention_secs).await {
            Ok(report) if report.blobs_deleted > 0 || report.rows_purged > 0 => tracing::info!(
                "scheduled GC: {} blobs reclaimed, {} rows purged",
                report.blobs_deleted,
                report.rows_purged
            ),
            Ok(_) => {}
            Err(err) => tracing::error!("scheduled GC failed: {err}"),
        }

        // Prune consumed or long-expired upload grants.
        let _ = sqlx::query(
            "DELETE FROM upload_grants WHERE consumed_at IS NOT NULL OR expires_at < now() - interval '1 day'",
        )
        .execute(&db)
        .await;
    }
    tracing::info!("GC scheduler stopped");
}
