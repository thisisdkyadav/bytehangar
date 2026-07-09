//! Encryption-at-rest for tenant secrets (signing + webhook secrets).
//!
//! AES-256-GCM with a key derived (SHA-256) from `MASTER_KEY`. Stored values are
//! `enc:v1:<base64(nonce||ciphertext+tag)>`. Values without the prefix are treated
//! as legacy plaintext, so enabling a master key migrates gracefully (old rows keep
//! working; new/updated rows get encrypted). If no `MASTER_KEY` is set, secrets are
//! stored as plaintext (dev default).

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

const PREFIX: &str = "enc:v1:";

pub struct Secrets {
    cipher: Option<Aes256Gcm>,
    /// Optional previous key, tried on DECRYPT only — enables zero-downtime rotation.
    previous: Option<Aes256Gcm>,
}

fn build_cipher(master_key: &str) -> Option<Aes256Gcm> {
    if master_key.is_empty() {
        return None;
    }
    let key = Sha256::digest(master_key.as_bytes());
    Some(Aes256Gcm::new_from_slice(&key).expect("sha256 yields a 32-byte key"))
}

impl Secrets {
    pub fn new(master_key: &str) -> Self {
        Self::with_previous(master_key, "")
    }

    /// `previous_key` (if set) is accepted on decrypt after the current key, so both the
    /// old and new master key work mid-rotation. New writes always use the current key.
    pub fn with_previous(master_key: &str, previous_key: &str) -> Self {
        Self {
            cipher: build_cipher(master_key),
            previous: build_cipher(previous_key),
        }
    }

    pub fn enabled(&self) -> bool {
        self.cipher.is_some()
    }

    /// Re-encrypt a stored value under the CURRENT key (used by key rotation). Decrypts
    /// with the current-or-previous key first. Returns `None` when a ciphertext can be
    /// decrypted by NEITHER key — the caller must NOT overwrite it (that would preserve
    /// an unreadable secret while falsely reporting a successful rotation). Legacy
    /// plaintext is migrated to ciphertext.
    pub fn reencrypt(&self, stored: &str) -> Option<String> {
        if stored.starts_with(PREFIX) {
            let plain = self.decrypt(stored);
            if plain.is_empty() {
                return None;
            }
            return Some(self.encrypt(&plain));
        }
        Some(self.encrypt(stored))
    }

    /// Encrypt a secret for storage. Returns plaintext unchanged when no master key.
    pub fn encrypt(&self, plaintext: &str) -> String {
        let Some(cipher) = &self.cipher else {
            return plaintext.to_string();
        };
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        match cipher.encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes()) {
            Ok(ciphertext) => {
                let mut buf = nonce.to_vec();
                buf.extend_from_slice(&ciphertext);
                format!("{PREFIX}{}", STANDARD.encode(buf))
            }
            Err(_) => plaintext.to_string(),
        }
    }

    /// Decrypt a stored value. Legacy (unprefixed) values are returned as-is. Tries the
    /// current key then the previous key (rotation).
    pub fn decrypt(&self, stored: &str) -> String {
        let Some(encoded) = stored.strip_prefix(PREFIX) else {
            return stored.to_string();
        };
        if self.cipher.is_none() && self.previous.is_none() {
            tracing::warn!("encrypted secret present but no MASTER_KEY configured");
            return String::new();
        }
        let buf = match STANDARD.decode(encoded) {
            Ok(buf) if buf.len() > 12 => buf,
            _ => {
                tracing::warn!("malformed encrypted secret");
                return String::new();
            }
        };
        let (nonce, ciphertext) = buf.split_at(12);
        for cipher in [self.cipher.as_ref(), self.previous.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Ok(plaintext) = cipher.decrypt(Nonce::from_slice(nonce), ciphertext) {
                return String::from_utf8(plaintext).unwrap_or_default();
            }
        }
        tracing::warn!("failed to decrypt secret (wrong MASTER_KEY?)");
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_key() {
        let secrets = Secrets::new("master");
        assert!(secrets.enabled());
        let encrypted = secrets.encrypt("hello");
        assert!(encrypted.starts_with("enc:v1:"));
        assert_eq!(secrets.decrypt(&encrypted), "hello");
    }

    #[test]
    fn no_key_is_passthrough() {
        let secrets = Secrets::new("");
        assert!(!secrets.enabled());
        assert_eq!(secrets.encrypt("hello"), "hello");
        assert_eq!(secrets.decrypt("hello"), "hello");
    }

    #[test]
    fn legacy_plaintext_is_read_as_is() {
        let secrets = Secrets::new("master");
        assert_eq!(secrets.decrypt("legacy-plaintext"), "legacy-plaintext");
    }

    #[test]
    fn wrong_key_fails_closed() {
        let encrypted = Secrets::new("right").encrypt("hello");
        assert_eq!(Secrets::new("wrong").decrypt(&encrypted), "");
    }

    #[test]
    fn nonce_is_randomized() {
        let secrets = Secrets::new("master");
        assert_ne!(secrets.encrypt("x"), secrets.encrypt("x"));
    }

    #[test]
    fn previous_key_decrypts_during_rotation() {
        let old = Secrets::new("old-key");
        let ciphertext = old.encrypt("secret");
        // Mid-rotation: current = new, previous = old.
        let rotating = Secrets::with_previous("new-key", "old-key");
        assert_eq!(rotating.decrypt(&ciphertext), "secret"); // old value still readable
        let reencrypted = rotating.reencrypt(&ciphertext).unwrap();
        // After re-encrypt, only the new key can read it.
        assert_eq!(Secrets::new("new-key").decrypt(&reencrypted), "secret");
        assert_eq!(old.decrypt(&reencrypted), "");
    }

    #[test]
    fn reencrypt_migrates_legacy_plaintext() {
        let secrets = Secrets::new("k");
        let reencrypted = secrets.reencrypt("legacy").unwrap();
        assert!(reencrypted.starts_with(PREFIX));
        assert_eq!(secrets.decrypt(&reencrypted), "legacy");
    }

    #[test]
    fn reencrypt_signals_undecryptable_value() {
        // A value encrypted with a key we don't have must report failure, not silently
        // "succeed" with the old ciphertext (which would falsely report a rotation).
        let orphan = Secrets::new("some-other-key").encrypt("x");
        let secrets = Secrets::new("our-key");
        assert_eq!(secrets.reencrypt(&orphan), None);
    }
}
