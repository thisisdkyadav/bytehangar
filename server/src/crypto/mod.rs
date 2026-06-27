//! Cryptographic primitives: API-key hashing, HMAC signing, random secrets, and
//! the signed grant/download token codec.
//!
//! API keys and tenant signing secrets are high-entropy random values, so a fast
//! hash (SHA-256) is the correct lookup primitive — argon2 is for low-entropy
//! passwords, not needed here.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::domain::GrantClaims;
use crate::error::{AppError, AppResult};

type HmacSha256 = Hmac<Sha256>;

/// Hex-encoded SHA-256 of the input (used for API-key lookup and file checksums).
pub fn sha256_hex(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hex::encode(hasher.finalize())
}

/// A url-safe random token of `bytes` entropy (base64, no padding).
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Generate a new API key with a recognizable prefix. Returns the plaintext key
/// (shown once) and its SHA-256 hash (stored).
pub fn generate_api_key() -> (String, String) {
    let key = format!("bh_{}", random_token(32));
    let hash = sha256_hex(key.as_bytes());
    (key, hash)
}

fn hmac_sign(secret: &[u8], message: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(message);
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// Hex-encoded HMAC-SHA256 (used for webhook body signatures: `sha256=<hex>`).
pub fn hmac_hex(secret: &str, message: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(message);
    hex::encode(mac.finalize().into_bytes())
}

fn hmac_verify(secret: &[u8], message: &[u8], signature: &str) -> bool {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(message);
    match URL_SAFE_NO_PAD.decode(signature) {
        Ok(sig) => mac.verify_slice(&sig).is_ok(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Grant tokens:  bh1.<base64url(json claims)>.<hmac sig over the claims part>
// ---------------------------------------------------------------------------

const GRANT_PREFIX: &str = "bh1";

pub fn encode_grant(secret: &str, claims: &GrantClaims) -> AppResult<String> {
    let json = serde_json::to_vec(claims)
        .map_err(|err| AppError::Internal(format!("grant encode: {err}")))?;
    let payload = URL_SAFE_NO_PAD.encode(json);
    let sig = hmac_sign(secret.as_bytes(), payload.as_bytes());
    Ok(format!("{GRANT_PREFIX}.{payload}.{sig}"))
}

/// Read a grant's claims WITHOUT verifying the signature. Used only to learn the
/// tenant id so the tenant's secret can be loaded; the caller MUST then call
/// `decode_grant` to actually verify.
pub fn peek_grant_claims(token: &str) -> AppResult<GrantClaims> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 || parts[0] != GRANT_PREFIX {
        return Err(AppError::Unauthorized);
    }
    let json = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| AppError::Unauthorized)?;
    serde_json::from_slice(&json).map_err(|_| AppError::Unauthorized)
}

/// Decode + verify a grant token's signature against the tenant secret.
/// Does NOT check expiry or single-use — caller does that against the DB.
pub fn decode_grant(secret: &str, token: &str) -> AppResult<GrantClaims> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 || parts[0] != GRANT_PREFIX {
        return Err(AppError::Unauthorized);
    }
    if !hmac_verify(secret.as_bytes(), parts[1].as_bytes(), parts[2]) {
        return Err(AppError::Unauthorized);
    }
    let json = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| AppError::Unauthorized)?;
    let claims: GrantClaims =
        serde_json::from_slice(&json).map_err(|_| AppError::Unauthorized)?;
    Ok(claims)
}

// ---------------------------------------------------------------------------
// Signed download URLs:  sign "<tenant>.<file_ref>.<exp>" with the tenant secret
// ---------------------------------------------------------------------------

pub fn sign_download(
    secret: &str,
    tenant_id: &str,
    file_ref: &str,
    exp: i64,
    disposition: &str,
) -> String {
    let message = format!("{tenant_id}.{file_ref}.{exp}.{disposition}");
    hmac_sign(secret.as_bytes(), message.as_bytes())
}

pub fn verify_download(
    secret: &str,
    tenant_id: &str,
    file_ref: &str,
    exp: i64,
    disposition: &str,
    signature: &str,
) -> bool {
    let message = format!("{tenant_id}.{file_ref}.{exp}.{disposition}");
    hmac_verify(secret.as_bytes(), message.as_bytes(), signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::GrantClaims;

    fn claims() -> GrantClaims {
        GrantClaims {
            t: "tenant".into(),
            p: "policy".into(),
            cat: "cat".into(),
            max: 1024,
            ct: vec!["image/png".into()],
            n: "nonce".into(),
            exp: 9_999_999_999,
            vis: "private".into(),
            m: None,
        }
    }

    #[test]
    fn grant_roundtrip() {
        let token = encode_grant("s3cr3t", &claims()).unwrap();
        let decoded = decode_grant("s3cr3t", &token).unwrap();
        assert_eq!(decoded.p, "policy");
        assert_eq!(decoded.max, 1024);
        assert_eq!(decoded.vis, "private");
    }

    #[test]
    fn grant_wrong_secret_rejected() {
        let token = encode_grant("right", &claims()).unwrap();
        assert!(decode_grant("wrong", &token).is_err());
    }

    #[test]
    fn grant_tamper_rejected() {
        let token = encode_grant("s", &claims()).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        let tampered = format!("{}.{}x.{}", parts[0], parts[1], parts[2]);
        assert!(decode_grant("s", &tampered).is_err());
    }

    #[test]
    fn peek_reads_tenant_without_verifying() {
        let token = encode_grant("s", &claims()).unwrap();
        assert_eq!(peek_grant_claims(&token).unwrap().t, "tenant");
    }

    #[test]
    fn download_sign_then_verify() {
        let sig = sign_download("secret", "tenant", "fileref", 123, "inline");
        assert!(verify_download("secret", "tenant", "fileref", 123, "inline", &sig));
        assert!(!verify_download("secret", "tenant", "fileref", 124, "inline", &sig)); // exp
        assert!(!verify_download("other", "tenant", "fileref", 123, "inline", &sig)); // secret
        assert!(!verify_download("secret", "tenant", "fileref", 123, "attachment", &sig)); // disposition
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn api_key_format_and_hash() {
        let (key, hash) = generate_api_key();
        assert!(key.starts_with("bh_"));
        assert_eq!(hash, sha256_hex(key.as_bytes()));
    }

    #[test]
    fn hmac_hex_is_deterministic_and_keyed() {
        assert_eq!(hmac_hex("k", b"msg"), hmac_hex("k", b"msg"));
        assert_ne!(hmac_hex("k", b"msg"), hmac_hex("k2", b"msg"));
    }
}
