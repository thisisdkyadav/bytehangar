use crate::error::{AppError, AppResult};

/// Which blob backend stores the actual bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageBackendKind {
    Local,
    S3,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Public/edge listener port (browser uploads + signed downloads).
    pub port: u16,
    /// Internal listener port (key-auth: catalog, grants, admin, S2S).
    pub internal_port: u16,
    pub database_url: String,
    /// Root directory for the local-disk blob backend.
    pub data_root: String,
    pub storage_backend: StorageBackendKind,
    /// Inviolable global ceiling; per-policy limits may only be stricter.
    pub max_upload_bytes: u64,
    /// Bootstrap admin token for tenant/key provisioning. Empty => admin disabled.
    pub admin_token: String,
    /// Default TTL for minted signed download URLs.
    pub signed_url_ttl_seconds: i64,
    /// Public base URL (scheme+host) prefixed onto signed download URLs. Empty => relative path.
    pub public_base_url: String,
}

impl Config {
    pub fn from_env() -> AppResult<Self> {
        Ok(Self {
            port: env_parse("PORT", 5100)?,
            internal_port: env_parse("INTERNAL_PORT", 5101)?,
            database_url: env_string(
                "DATABASE_URL",
                "postgres://bytehangar:bytehangar@localhost:5432/bytehangar",
            ),
            data_root: env_string("DATA_ROOT", "./data"),
            storage_backend: match env_string("STORAGE_BACKEND", "local").as_str() {
                "local" => StorageBackendKind::Local,
                "s3" => StorageBackendKind::S3,
                other => {
                    return Err(AppError::Internal(format!(
                        "Unknown STORAGE_BACKEND '{other}' (expected 'local' or 's3')"
                    )))
                }
            },
            max_upload_bytes: env_parse("MAX_UPLOAD_BYTES", 50 * 1024 * 1024)?,
            admin_token: env_string("ADMIN_TOKEN", ""),
            signed_url_ttl_seconds: env_parse("SIGNED_URL_TTL_SECONDS", 300)?,
            public_base_url: env_string("PUBLIC_BASE_URL", ""),
        })
    }
}

fn env_string(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> AppResult<T> {
    match std::env::var(key) {
        Ok(value) => value
            .parse::<T>()
            .map_err(|_| AppError::Internal(format!("Invalid value for {key}"))),
        Err(_) => Ok(default),
    }
}
