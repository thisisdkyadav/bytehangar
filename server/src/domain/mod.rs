//! Core domain types shared across modules. Fleshed out as Phase 1 modules land
//! (catalog, grants, files, tenants, usage).

use serde::{Deserialize, Serialize};

/// A registered upload policy (the resolved, enforceable form of a catalog entry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub key: String,
    pub category: String,
    pub max_size_bytes: u64,
    pub allow_content_types: Vec<String>,
}

/// The signed, single-use authorization the client presents to upload one file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantClaims {
    /// tenant id
    pub t: String,
    /// policy key
    pub p: String,
    /// category (path prefix)
    pub cat: String,
    /// max size bytes
    pub max: u64,
    /// allowed content types
    pub ct: Vec<String>,
    /// nonce (single-use)
    pub n: String,
    /// expiry (unix seconds)
    pub exp: i64,
    /// visibility: "public" | "private"
    pub vis: String,
    /// optional app-supplied upload metadata (attribution)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub m: Option<GrantMeta>,
}

/// App-supplied metadata attached to a grant and persisted on the uploaded file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrantMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_hint: Option<String>,
}
