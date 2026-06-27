use crate::error::{AppError, AppResult};

/// Which blob backend stores the actual bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageBackendKind {
    Local,
    S3,
}

/// S3-compatible backend settings (S3, MinIO, R2, B2). `endpoint` set => custom
/// provider; `force_path_style` is required for MinIO.
#[derive(Clone, Debug)]
pub struct S3Config {
    pub bucket: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub force_path_style: bool,
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
    pub s3: S3Config,
    /// Inviolable global ceiling; per-policy limits may only be stricter.
    pub max_upload_bytes: u64,
    /// Bootstrap admin token for tenant/key provisioning. Empty => admin disabled.
    pub admin_token: String,
    /// Default TTL for minted signed download URLs.
    pub signed_url_ttl_seconds: i64,
    /// Public base URL (scheme+host) prefixed onto signed download URLs. Empty => relative path.
    pub public_base_url: String,
    /// Bind address for the public/edge listener.
    pub bind: String,
    /// Bind address for the internal listener (defaults to loopback — keep it private).
    pub internal_bind: String,
    /// Master key for encrypting tenant secrets at rest. Empty => stored as plaintext.
    pub master_key: String,
    /// Deployment environment ("development" | "production"). Tightens defaults.
    pub environment: String,
    /// Allowed CORS origins for the public plane. Empty => allow-all (dev only).
    pub allowed_origins: Vec<String>,
    /// Per-client-IP rate limit on the public plane (tokens/sec). 0 => disabled.
    pub rate_limit_per_second: u32,
    /// Burst capacity for the public-plane rate limiter.
    pub rate_limit_burst: u32,
    /// Trust X-Forwarded-For / X-Real-IP for the client IP. Enable ONLY behind a
    /// trusted proxy; otherwise the limiter keys on the (unspoofable) socket peer.
    pub trust_forwarded_for: bool,
    /// Allow outbound webhook / download-auth-callback requests to private,
    /// loopback, and link-local targets. Default false (SSRF guard on).
    pub allow_private_outbound: bool,
}

impl Config {
    pub fn is_production(&self) -> bool {
        self.environment == "production"
    }
}

impl Config {
    pub fn from_env() -> AppResult<Self> {
        let environment = env_string("APP_ENV", "development");
        let master_key = env_string("MASTER_KEY", "");
        let admin_token = env_string("ADMIN_TOKEN", "");
        if environment == "production" {
            if master_key.is_empty() {
                return Err(AppError::Internal(
                    "MASTER_KEY is required when APP_ENV=production".into(),
                ));
            }
            if !admin_token.is_empty() && admin_token.len() < 24 {
                return Err(AppError::Internal(
                    "ADMIN_TOKEN must be at least 24 characters when APP_ENV=production".into(),
                ));
            }
        }
        Ok(Self {
            port: env_parse("PORT", 5100)?,
            internal_port: env_parse("INTERNAL_PORT", 5101)?,
            database_url: env_string(
                "DATABASE_URL",
                "postgres://bytehangar:bytehangar@localhost:5433/bytehangar",
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
            s3: S3Config {
                bucket: env_string("S3_BUCKET", ""),
                region: env_string("S3_REGION", "us-east-1"),
                endpoint: match env_string("S3_ENDPOINT", "") {
                    value if value.is_empty() => None,
                    value => Some(value),
                },
                access_key_id: env_string("S3_ACCESS_KEY_ID", ""),
                secret_access_key: env_string("S3_SECRET_ACCESS_KEY", ""),
                force_path_style: env_parse("S3_FORCE_PATH_STYLE", false)?,
            },
            max_upload_bytes: env_parse("MAX_UPLOAD_BYTES", 50 * 1024 * 1024)?,
            admin_token,
            signed_url_ttl_seconds: env_parse("SIGNED_URL_TTL_SECONDS", 300)?,
            public_base_url: env_string("PUBLIC_BASE_URL", ""),
            bind: env_string("BIND_ADDRESS", "0.0.0.0"),
            internal_bind: env_string("INTERNAL_BIND_ADDRESS", "127.0.0.1"),
            master_key,
            environment,
            allowed_origins: split_csv(&env_string("ALLOWED_ORIGINS", "")),
            rate_limit_per_second: env_parse("RATE_LIMIT_PER_SECOND", 50)?,
            rate_limit_burst: env_parse("RATE_LIMIT_BURST", 100)?,
            trust_forwarded_for: env_parse("TRUST_FORWARDED_FOR", false)?,
            allow_private_outbound: env_parse("ALLOW_PRIVATE_OUTBOUND", false)?,
        })
    }
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
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
