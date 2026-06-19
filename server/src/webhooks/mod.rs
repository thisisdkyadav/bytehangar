//! Event webhooks. On file.uploaded / file.deleted, POST a signed JSON event to
//! the tenant's configured webhook_url. Fire-and-forget (spawned), with a short
//! timeout; failures are logged.
//!
//! Note: delivery is best-effort — no persistent retry queue yet. A production
//! deployment would persist events and retry with backoff.

use serde_json::Value;

use crate::crypto;
use crate::tenants::Tenant;

pub const EVENT_UPLOADED: &str = "file.uploaded";
pub const EVENT_DELETED: &str = "file.deleted";

/// Dispatch an event to the tenant's webhook, if configured. Returns immediately.
pub fn dispatch(tenant: &Tenant, event: &str, payload: Value) {
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
        let result = reqwest::Client::new()
            .post(&url)
            .timeout(std::time::Duration::from_secs(5))
            .header("content-type", "application/json")
            .header("x-bytehangar-event", &event)
            .header("x-bytehangar-signature", format!("sha256={signature}"))
            .body(body)
            .send()
            .await;
        match result {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                tracing::warn!("webhook {event} -> {url} returned {}", response.status())
            }
            Err(err) => tracing::warn!("webhook {event} -> {url} failed: {err}"),
        }
    });
}
