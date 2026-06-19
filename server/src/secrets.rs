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
}

impl Secrets {
    pub fn new(master_key: &str) -> Self {
        if master_key.is_empty() {
            return Self { cipher: None };
        }
        let key = Sha256::digest(master_key.as_bytes());
        let cipher = Aes256Gcm::new_from_slice(&key).expect("sha256 yields a 32-byte key");
        Self {
            cipher: Some(cipher),
        }
    }

    pub fn enabled(&self) -> bool {
        self.cipher.is_some()
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

    /// Decrypt a stored value. Legacy (unprefixed) values are returned as-is.
    pub fn decrypt(&self, stored: &str) -> String {
        let Some(encoded) = stored.strip_prefix(PREFIX) else {
            return stored.to_string();
        };
        let Some(cipher) = &self.cipher else {
            tracing::warn!("encrypted secret present but no MASTER_KEY configured");
            return String::new();
        };
        let buf = match STANDARD.decode(encoded) {
            Ok(buf) if buf.len() > 12 => buf,
            _ => {
                tracing::warn!("malformed encrypted secret");
                return String::new();
            }
        };
        let (nonce, ciphertext) = buf.split_at(12);
        match cipher.decrypt(Nonce::from_slice(nonce), ciphertext) {
            Ok(plaintext) => String::from_utf8(plaintext).unwrap_or_default(),
            Err(_) => {
                tracing::warn!("failed to decrypt secret (wrong MASTER_KEY?)");
                String::new()
            }
        }
    }
}
