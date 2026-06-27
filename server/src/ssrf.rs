//! SSRF guard for outbound requests to tenant-controlled URLs (event webhooks and
//! the download-auth callback). Without this a tenant could point those URLs at
//! cloud metadata (169.254.169.254), loopback, or private ranges to pivot/exfiltrate.
//!
//! `validate` rejects non-http(s) schemes and any host that resolves to a
//! non-public address. Redirects are disabled separately on the HTTP client (a
//! public URL could otherwise 30x to a private one). DNS rebinding (resolve public,
//! connect private) is a residual; set `allow_private` only for trusted internal
//! deployments.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Validate that `url` is safe to call. Resolves the host and rejects if any
/// resolved address is non-public. `allow_private` bypasses the check.
pub async fn validate(url: &str, allow_private: bool) -> Result<(), String> {
    if allow_private {
        return Ok(());
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| "invalid url".to_string())?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("blocked scheme: {other}")),
    }
    let host = parsed.host_str().ok_or_else(|| "missing host".to_string())?;
    let port = parsed.port_or_known_default().unwrap_or(443);

    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| format!("dns resolution failed: {err}"))?;

    let mut resolved = false;
    for addr in addrs {
        resolved = true;
        if is_blocked(addr.ip()) {
            return Err(format!("blocked non-public target: {}", addr.ip()));
        }
    }
    if !resolved {
        return Err("host did not resolve".into());
    }
    Ok(())
}

fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_v4(mapped);
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || is_unique_local_v6(v6)
                || is_link_local_v6(v6)
        }
    }
}

fn is_blocked_v4(a: Ipv4Addr) -> bool {
    let o = a.octets();
    a.is_unspecified()
        || a.is_loopback()
        || a.is_private()
        || a.is_link_local() // 169.254/16 (cloud metadata)
        || a.is_broadcast()
        || a.is_documentation()
        || a.is_multicast()
        || o[0] == 0
        || (o[0] == 100 && (64..=127).contains(&o[1])) // CGNAT 100.64/10
}

fn is_unique_local_v6(a: Ipv6Addr) -> bool {
    (a.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7
}

fn is_link_local_v6(a: Ipv6Addr) -> bool {
    (a.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_private_and_metadata() {
        for s in [
            "127.0.0.1",
            "169.254.169.254", // cloud metadata
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.1",
            "100.64.0.1", // CGNAT
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1", // v4-mapped loopback
        ] {
            assert!(is_blocked(ip(s)), "{s} should be blocked");
        }
    }

    #[test]
    fn allows_public() {
        for s in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(!is_blocked(ip(s)), "{s} should be allowed");
        }
    }
}
