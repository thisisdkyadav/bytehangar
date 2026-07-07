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
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::AdminAuth;
use crate::blob::BlobBackend;
use crate::error::AppResult;
use crate::state::AppState;

/// Fixed key for the GC advisory lock. Serializes GC runs across workers/instances
/// AND with `files::restore_file`, so a restore can't resurrect a blob mid-reclaim.
pub const GC_ADVISORY_LOCK: i64 = 4_271_990_001;

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

/// Run GC under a *transaction-scoped* Postgres advisory lock so concurrent runs
/// (multiple workers/instances, or the scheduler racing the admin endpoint) can't
/// overlap, and a concurrent `restore_file` can't resurrect a blob mid-reclaim.
/// The xact lock auto-releases on commit/rollback/panic/disconnect — it can never
/// leak and wedge GC. Returns an empty report when another holder has the lock.
///
/// The whole sweep runs in one transaction on a single connection (no extra pool
/// connection held for the lock). Blob deletes are external side effects: if the
/// transaction rolls back after some blobs were deleted, the (idempotent) deletes
/// are simply retried on the next run — the row purges are undone, so state stays
/// consistent.
pub async fn run_gc(
    db: &PgPool,
    blob: &dyn BlobBackend,
    tenant_id: Option<Uuid>,
    older_than_seconds: i64,
) -> AppResult<GcReport> {
    let mut tx = db.begin().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(GC_ADVISORY_LOCK)
        .fetch_one(&mut *tx)
        .await?;
    if !acquired {
        return Ok(GcReport {
            blobs_deleted: 0,
            rows_purged: 0,
        });
    }
    let report = run_gc_inner(&mut tx, blob, tenant_id, older_than_seconds).await?;
    tx.commit().await?;
    Ok(report)
}

async fn run_gc_inner(
    tx: &mut Transaction<'_, Postgres>,
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
            .fetch_all(&mut **tx)
            .await?
        }
        None => {
            sqlx::query_scalar(
                "SELECT DISTINCT stored_key FROM files \
                 WHERE deleted_at IS NOT NULL AND deleted_at < $1",
            )
            .bind(cutoff)
            .fetch_all(&mut **tx)
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
                .fetch_one(&mut **tx)
                .await?;
        if live == 0 {
            blob.delete(&key).await?;
            blobs_deleted += 1;
        }
        // Variant blobs of the tombstones we're about to purge. Collected BEFORE the
        // purge (the join needs the file rows); reclaimed AFTER, but only if no other
        // file_variants row still references them. That reference check is what makes it
        // dedup-safe and also handles a deduped sibling whose parent purged first.
        let variant_candidates: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT fv.stored_key FROM file_variants fv \
             JOIN files f ON f.id = fv.file_id \
             WHERE f.stored_key = $1 AND f.deleted_at IS NOT NULL AND f.deleted_at < $2",
        )
        .bind(&key)
        .bind(cutoff)
        .fetch_all(&mut **tx)
        .await?;

        // Purge only tombstones past the retention cutoff — never sibling tombstones
        // (same stored_key, dedup) that are still inside their restore window. This
        // cascade-deletes the purged files' file_variants rows.
        let purged = sqlx::query(
            "DELETE FROM files WHERE stored_key = $1 AND deleted_at IS NOT NULL AND deleted_at < $2",
        )
        .bind(&key)
        .bind(cutoff)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        rows_purged += purged;

        // Reclaim variant blobs that no file_variants row references anymore.
        for vk in variant_candidates {
            let refs: i64 =
                sqlx::query_scalar("SELECT count(*) FROM file_variants WHERE stored_key = $1")
                    .bind(&vk)
                    .fetch_one(&mut **tx)
                    .await?;
            if refs == 0 {
                blob.delete(&vk).await?;
                blobs_deleted += 1;
            }
        }
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

#[derive(Debug, Default, Deserialize)]
pub struct ReconcileRequest {
    /// Skip blobs modified more recently than this (protects in-flight uploads whose
    /// DB row hasn't committed yet). Default 3600.
    #[serde(default)]
    pub grace_seconds: Option<u64>,
    /// Report only, delete nothing. Default false.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

#[derive(Serialize)]
pub struct ReconcileReport {
    pub orphans_found: u64,
    pub blobs_deleted: u64,
    pub dry_run: bool,
}

/// Reclaim orphan blobs: physical blobs in the store that NO `files` or
/// `file_variants` row references (e.g. from an upload that wrote the blob then
/// crashed before committing its DB row — the dedup-safe GC can't see those).
/// Conservative: only deletes blobs older than `grace_seconds`.
pub async fn run_reconcile(
    db: &PgPool,
    blob: &dyn BlobBackend,
    grace_seconds: u64,
    dry_run: bool,
) -> AppResult<ReconcileReport> {
    use std::collections::HashSet;

    // Every referenced key: live AND tombstoned files (their blobs live until GC),
    // plus rendered variants.
    let referenced: HashSet<String> = sqlx::query_scalar::<_, String>(
        "SELECT stored_key FROM files UNION SELECT stored_key FROM file_variants",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .collect();

    let now = std::time::SystemTime::now();
    let mut orphans_found = 0u64;
    let mut blobs_deleted = 0u64;

    for entry in blob.list().await? {
        if referenced.contains(&entry.key) {
            continue;
        }
        // Unknown or within-grace age => treat as recent and leave it alone.
        let recent = match entry.modified {
            Some(m) => now
                .duration_since(m)
                .map(|d| d.as_secs() < grace_seconds)
                .unwrap_or(true),
            None => true,
        };
        if recent {
            continue;
        }
        orphans_found += 1;
        if !dry_run {
            blob.delete(&entry.key).await?;
            blobs_deleted += 1;
        }
    }

    Ok(ReconcileReport {
        orphans_found,
        blobs_deleted,
        dry_run,
    })
}

pub async fn reconcile_handler(
    _admin: AdminAuth,
    State(state): State<AppState>,
    body: Option<Json<ReconcileRequest>>,
) -> AppResult<Json<ReconcileReport>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let report = run_reconcile(
        &state.db,
        state.blob.as_ref(),
        req.grace_seconds.unwrap_or(3600),
        req.dry_run.unwrap_or(false),
    )
    .await?;
    Ok(Json(report))
}

/// Internal GC scheduler: periodically reclaims soft-deleted blobs past the
/// retention window. Stops on shutdown. (Grant pruning is a separate, always-on
/// task — see `run_grant_pruner` — since grants accrue regardless of GC config.)
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
    }
    tracing::info!("GC scheduler stopped");
}

/// Always-on pruner for the `upload_grants` table. A row is inserted on every grant
/// mint; without this the table grows unbounded (the GC scheduler is off by default).
/// Runs independently of GC config. The predicate is index-backed by
/// `idx_upload_grants_expires`.
pub async fn run_grant_pruner(db: PgPool, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    const PRUNE_INTERVAL_SECS: u64 = 300;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(StdDuration::from_secs(PRUNE_INTERVAL_SECS)) => {}
            _ = shutdown.changed() => {}
        }
        if *shutdown.borrow() {
            break;
        }
        match sqlx::query(
            "DELETE FROM upload_grants WHERE consumed_at IS NOT NULL OR expires_at < now() - interval '1 day'",
        )
        .execute(&db)
        .await
        {
            Ok(res) if res.rows_affected() > 0 => {
                tracing::debug!("pruned {} stale upload grants", res.rows_affected());
            }
            Ok(_) => {}
            Err(err) => tracing::error!("grant prune failed: {err}"),
        }
    }
    tracing::info!("grant pruner stopped");
}
