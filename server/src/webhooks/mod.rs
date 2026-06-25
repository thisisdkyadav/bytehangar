//! Event webhooks. On file.uploaded / file.deleted, POST a signed JSON event to
//! the tenant's configured webhook_url. Spawned (non-blocking) with a few bounded
//! retries on failure; the shared HTTP client is reused for connection pooling.
//!
//! Note: delivery is best-effort — there is no persistent retry queue. A
//! deployment that must never lose events would persist them and retry durably.

use serde_json::Value;

use crate::crypto;
use crate::tenants::Tenant;

pub const EVENT_UPLOADED: &str = "file.uploaded";
pub const EVENT_DELETED: &str = "file.deleted";

const MAX_ATTEMPTS: u32 = 3;

/// Dispatch an event to the tenant's webhook, if configured. Returns immediately.
pub fn dispatch(client: reqwest::Client, tenant: &Tenant, event: &str, payload: Value) {
    let (Some(url), Some(secret)) = (tenant.webhook_url.clone(), tenant.webhook_secret.clone())
    else {
        return;
    };
    let event = event.to_string();
    let body = match serde_json::to_vec(&payload) {
        Ok(body) => body,
        Err(err) => {
            tracing::warn!("webhook {event}: failed to serialize payload: {err}");
            return;
        }
    };

    tokio::spawn(async move {
        let signature = crypto::hmac_hex(&secret, &body);
        for attempt in 1..=MAX_ATTEMPTS {
            let result = client
                .post(&url)
                .timeout(std::time::Duration::from_secs(5))
                .header("content-type", "application/json")
                .header("x-bytehangar-event", &event)
                .header("x-bytehangar-signature", format!("sha256={signature}"))
                .body(body.clone())
                .send()
                .await;
            match result {
                Ok(response) if response.status().is_success() => return,
                Ok(response) => {
                    tracing::warn!("webhook {event} -> {url} attempt {attempt}: HTTP {}", response.status())
                }
                Err(err) => tracing::warn!("webhook {event} -> {url} attempt {attempt}: {err}"),
            }
            if attempt < MAX_ATTEMPTS {
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
        }
        tracing::warn!("webhook {event} -> {url} failed after {MAX_ATTEMPTS} attempts");
    });
}
