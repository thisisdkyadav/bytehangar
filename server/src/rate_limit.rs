//! Per-client-IP token-bucket rate limiter for the public plane. In-process and
//! dependency-free, defense-in-depth (TLS + global/L7 DDoS protection still belong
//! at the reverse proxy / ingress).
//!
//! Keying: by default the limiter keys on the **socket peer IP** (unspoofable). It
//! only honours `X-Forwarded-For` / `X-Real-IP` when `trust_forwarded` is enabled
//! (operator is behind a trusted proxy) — and then takes the **right-most** XFF
//! entry (the address the trusted proxy actually saw), not the left-most
//! client-supplied one.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Hard cap on tracked source IPs. The map is bounded to this size regardless of
/// load (oldest entries are evicted), so a flood cannot grow memory without bound.
const MAX_TRACKED_IPS: usize = 100_000;
/// Idle buckets older than this are dropped first during eviction.
const IDLE_EVICT_SECS: u64 = 600;

struct Bucket {
    tokens: f64,
    last: Instant,
}

pub struct RateLimiter {
    capacity: f64,
    refill_per_sec: f64,
    enabled: bool,
    trust_forwarded: bool,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimiter {
    pub fn new(per_second: u32, burst: u32, trust_forwarded: bool) -> Self {
        Self {
            capacity: burst.max(1) as f64,
            refill_per_sec: per_second as f64,
            enabled: per_second > 0,
            trust_forwarded,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Consume one token for `ip`; returns false when the bucket is empty.
    /// `now` is a parameter for testability.
    pub fn check(&self, ip: IpAddr, now: Instant) -> bool {
        if !self.enabled {
            return true;
        }
        let mut buckets = self.buckets.lock().unwrap_or_else(|p| p.into_inner());

        // Keep the map hard-bounded so an attacker (or a huge legitimate IP set)
        // cannot grow memory without limit. Only runs when over the cap.
        if buckets.len() > MAX_TRACKED_IPS && !buckets.contains_key(&ip) {
            evict(&mut buckets, now);
        }

        let bucket = buckets.entry(ip).or_insert(Bucket {
            tokens: self.capacity,
            last: now,
        });
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        bucket.last = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Bound the map: drop long-idle entries, then (if still over) the oldest down to
/// 90% of the cap. Amortized O(1)/request since it only fires above the cap and
/// frees ~10% each time.
fn evict(buckets: &mut HashMap<IpAddr, Bucket>, now: Instant) {
    buckets.retain(|_, bucket| now.saturating_duration_since(bucket.last).as_secs() < IDLE_EVICT_SECS);
    if buckets.len() <= MAX_TRACKED_IPS {
        return;
    }
    let target = MAX_TRACKED_IPS * 9 / 10;
    let excess = buckets.len() - target;
    let mut by_age: Vec<(IpAddr, Instant)> =
        buckets.iter().map(|(ip, bucket)| (*ip, bucket.last)).collect();
    by_age.sort_unstable_by_key(|(_, last)| *last); // oldest first
    for (ip, _) in by_age.into_iter().take(excess) {
        buckets.remove(&ip);
    }
}

/// Resolve the client IP. Default: the socket peer (unspoofable). When behind a
/// trusted proxy, prefer the right-most X-Forwarded-For entry, then X-Real-IP.
fn client_ip(headers: &HeaderMap, peer: IpAddr, trust_forwarded: bool) -> IpAddr {
    if trust_forwarded {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit(',').next())
            .and_then(|last| last.trim().parse::<IpAddr>().ok())
        {
            return ip;
        }
        if let Some(ip) = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<IpAddr>().ok())
        {
            return ip;
        }
    }
    peer
}

pub async fn middleware(
    State(limiter): State<Arc<RateLimiter>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let ip = client_ip(request.headers(), peer.ip(), limiter.trust_forwarded);
    if limiter.check(ip, Instant::now()) {
        next.run(request).await
    } else {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "success": false,
                "message": "rate limit exceeded",
                "data": null,
                "errors": null,
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ip() -> IpAddr {
        "1.2.3.4".parse().unwrap()
    }

    #[test]
    fn allows_burst_then_denies_then_refills() {
        let rl = RateLimiter::new(1, 3, false); // 3 burst, +1/sec
        let t0 = Instant::now();
        assert!(rl.check(ip(), t0));
        assert!(rl.check(ip(), t0));
        assert!(rl.check(ip(), t0));
        assert!(!rl.check(ip(), t0)); // burst exhausted

        let t1 = t0 + Duration::from_secs(2);
        assert!(rl.check(ip(), t1)); // +2 tokens
        assert!(rl.check(ip(), t1));
        assert!(!rl.check(ip(), t1));
    }

    #[test]
    fn disabled_always_allows() {
        let rl = RateLimiter::new(0, 0, false);
        let now = Instant::now();
        for _ in 0..1000 {
            assert!(rl.check(ip(), now));
        }
    }

    #[test]
    fn limits_are_per_ip() {
        let rl = RateLimiter::new(1, 1, false); // 1 burst
        let now = Instant::now();
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        assert!(rl.check(a, now));
        assert!(!rl.check(a, now)); // a exhausted
        assert!(rl.check(b, now)); // b independent
    }

    #[test]
    fn ignores_xff_unless_trusted() {
        let peer: IpAddr = "9.9.9.9".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.1.1.1, 2.2.2.2".parse().unwrap());
        // untrusted: peer wins (XFF is spoofable)
        assert_eq!(client_ip(&headers, peer, false), peer);
        // trusted: right-most entry (what the proxy saw), not the left-most client value
        assert_eq!(client_ip(&headers, peer, true), "2.2.2.2".parse::<IpAddr>().unwrap());
    }
}
