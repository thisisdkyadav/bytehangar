//! Durable event webhooks. On file.uploaded / file.deleted the event is enqueued
//! into `webhook_deliveries` **atomically within the same DB transaction** as the
//! file operation — so an event is persisted iff the operation committed. A
//! background worker then delivers it (signed) with retries + exponential backoff,
//! using lease-based claiming (FOR UPDATE SKIP LOCKED) so multiple workers/instances
//! are safe.
//!
//! Delivery is **at-least-once**: a crash after sending but before marking
//! 'delivered', or a lease expiry, can re-send an event. Consumers must dedupe
//! (e.g. on event + file_ref).

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{Duration, Utc};
use futures::stream::{self, StreamExt};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

use crate::crypto;
use crate::error::AppResult;
use crate::secrets::Secrets;
use crate::tenants::Tenant;

pub const EVENT_UPLOADED: &str = "file.uploaded";
pub const EVENT_DELETED: &str = "file.deleted";

const MAX_ATTEMPTS: i32 = 6;
const POLL_INTERVAL_SECS: u64 = 1;
/// How long a claimed batch is hidden from other workers while we attempt it.
/// Must comfortably exceed worst-case batch wall-clock (BATCH/CONCURRENCY * timeout).
const CLAIM_LEASE_SECS: i64 = 60;
const BATCH: i64 = 20;
/// In-flight deliveries per batch (bounds wall-clock and isolates slow endpoints).
const DELIVERY_CONCURRENCY: usize = 8;

/// Enqueue an event for durable delivery, within the caller's transaction. No-op
/// if the tenant has no webhook configured. The signing secret is snapshotted
/// (encrypted) so delivery is independent of later rotation/clearing.
pub async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    secrets: &Secrets,
    tenant: &Tenant,
    event: &str,
    payload: Value,
) -> AppResult<()> {
    let (Some(url), Some(secret)) = (
        tenant.webhook_url.as_deref(),
        tenant.webhook_secret.as_deref(),
    ) else {
        return Ok(());
    };
    let secret_enc = secrets.encrypt(secret);
    sqlx::query(
        "INSERT INTO webhook_deliveries (tenant_id, event, payload, url, secret_enc, status, next_attempt_at) \
         VALUES ($1, $2, $3, $4, $5, 'pending', now())",
    )
    .bind(tenant.id)
    .bind(event)
    .bind(&payload)
    .bind(url)
    .bind(&secret_enc)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct DueDelivery {
    id: i64,
    event: String,
    payload: Value,
    url: String,
    secret_enc: String,
    attempts: i32,
}

/// Background worker loop. Spawn once at startup.
pub async fn run_worker(db: PgPool, client: reqwest::Client, secrets: Arc<Secrets>) {
    loop {
        if let Err(err) = process_batch(&db, &client, &secrets).await {
            tracing::error!("webhook worker error: {err}");
        }
        tokio::time::sleep(StdDuration::from_secs(POLL_INTERVAL_SECS)).await;
    }
}

async fn process_batch(db: &PgPool, client: &reqwest::Client, secrets: &Secrets) -> AppResult<()> {
    // Claim a batch atomically: bump attempts and push next_attempt_at out by the
    // lease so concurrent workers skip these rows while we deliver them.
    let lease = Utc::now() + Duration::seconds(CLAIM_LEASE_SECS);
    let claimed: Vec<DueDelivery> = sqlx::query_as(
        "UPDATE webhook_deliveries SET attempts = attempts + 1, next_attempt_at = $1 \
         WHERE id IN ( \
            SELECT id FROM webhook_deliveries \
            WHERE status = 'pending' AND next_attempt_at <= now() \
            ORDER BY next_attempt_at LIMIT $2 FOR UPDATE SKIP LOCKED \
         ) \
         RETURNING id, event, payload, url, secret_enc, attempts",
    )
    .bind(lease)
    .bind(BATCH)
    .fetch_all(db)
    .await?;

    // Deliver concurrently so one slow/failing endpoint can't block the batch and
    // the whole batch finishes well within the claim lease.
    stream::iter(claimed)
        .for_each_concurrent(DELIVERY_CONCURRENCY, |delivery| {
            deliver(db, client, secrets, delivery)
        })
        .await;
    Ok(())
}

async fn deliver(db: &PgPool, client: &reqwest::Client, secrets: &Secrets, delivery: DueDelivery) {
    let body = match serde_json::to_vec(&delivery.payload) {
        Ok(body) => body,
        Err(err) => return mark_failed(db, delivery.id, &format!("serialize: {err}")).await,
    };
    let secret = secrets.decrypt(&delivery.secret_enc);
    let signature = crypto::hmac_hex(&secret, &body);

    let outcome = client
        .post(&delivery.url)
        .timeout(StdDuration::from_secs(5))
        .header("content-type", "application/json")
        .header("x-bytehangar-event", &delivery.event)
        .header("x-bytehangar-signature", format!("sha256={signature}"))
        .body(body)
        .send()
        .await;

    let error = match outcome {
        Ok(response) if response.status().is_success() => {
            let _ = sqlx::query(
                "UPDATE webhook_deliveries SET status = 'delivered', delivered_at = now(), last_error = NULL WHERE id = $1",
            )
            .bind(delivery.id)
            .execute(db)
            .await;
            return;
        }
        Ok(response) => format!("HTTP {}", response.status()),
        Err(err) => err.to_string(),
    };

    if delivery.attempts >= MAX_ATTEMPTS {
        mark_failed(db, delivery.id, &error).await;
    } else {
        let next = Utc::now() + Duration::seconds(backoff_secs(delivery.attempts));
        let _ = sqlx::query(
            "UPDATE webhook_deliveries SET next_attempt_at = $1, last_error = $2 WHERE id = $3",
        )
        .bind(next)
        .bind(&error)
        .bind(delivery.id)
        .execute(db)
        .await;
        tracing::warn!(
            "webhook delivery {} failed (attempt {}): {error}; retrying",
            delivery.id,
            delivery.attempts
        );
    }
}

async fn mark_failed(db: &PgPool, id: i64, error: &str) {
    let _ = sqlx::query(
        "UPDATE webhook_deliveries SET status = 'failed', last_error = $1 WHERE id = $2",
    )
    .bind(error)
    .bind(id)
    .execute(db)
    .await;
    tracing::error!("webhook delivery {id} permanently failed: {error}");
}

/// Backoff (seconds) before the next attempt, given the just-completed attempt #.
fn backoff_secs(attempt: i32) -> i64 {
    match attempt {
        1 => 5,
        2 => 30,
        3 => 120,
        4 => 600,
        _ => 3600,
    }
}
