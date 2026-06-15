//! Blob backend abstraction. The metadata/control plane is identical regardless
//! of where bytes physically land; only the driver changes.
//!
//! Backend is always in the byte path (no client->S3 presign in core), so the
//! driver just streams put/get/delete. Streaming-Body signatures are a Phase-1
//! refinement; the scaffold uses owned buffers.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;

use crate::config::{Config, S3Config, StorageBackendKind};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct BlobStat {
    pub size_bytes: u64,
}

#[async_trait]
pub trait BlobBackend: Send + Sync {
    async fn put(&self, key: &str, data: Vec<u8>) -> AppResult<()>;
    async fn get(&self, key: &str) -> AppResult<Vec<u8>>;
    async fn delete(&self, key: &str) -> AppResult<()>;
    async fn stat(&self, key: &str) -> AppResult<BlobStat>;
}

/// Construct the configured backend.
pub fn from_config(config: &Config) -> AppResult<Arc<dyn BlobBackend>> {
    match config.storage_backend {
        StorageBackendKind::Local => Ok(Arc::new(LocalDisk::new(&config.data_root))),
        StorageBackendKind::S3 => Ok(Arc::new(S3Backend::new(&config.s3)?)),
    }
}

// ---------------------------------------------------------------------------
// Local disk
// ---------------------------------------------------------------------------

pub struct LocalDisk {
    root: PathBuf,
}

impl LocalDisk {
    pub fn new(root: &str) -> Self {
        Self {
            root: PathBuf::from(root),
        }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }
}

#[async_trait]
impl BlobBackend for LocalDisk {
    async fn put(&self, key: &str, data: Vec<u8>) -> AppResult<()> {
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, data).await?;
        Ok(())
    }

    async fn get(&self, key: &str) -> AppResult<Vec<u8>> {
        let data = tokio::fs::read(self.path_for(key)).await?;
        Ok(data)
    }

    async fn delete(&self, key: &str) -> AppResult<()> {
        match tokio::fs::remove_file(self.path_for(key)).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    async fn stat(&self, key: &str) -> AppResult<BlobStat> {
        let meta = tokio::fs::metadata(self.path_for(key)).await?;
        Ok(BlobStat {
            size_bytes: meta.len(),
        })
    }
}

// ---------------------------------------------------------------------------
// S3-compatible (S3, MinIO, R2, B2)
// ---------------------------------------------------------------------------

pub struct S3Backend {
    client: S3Client,
    bucket: String,
}

impl S3Backend {
    pub fn new(cfg: &S3Config) -> AppResult<Self> {
        if cfg.bucket.is_empty() {
            return Err(AppError::Internal(
                "S3_BUCKET is required for the s3 backend".into(),
            ));
        }
        let credentials = Credentials::new(
            cfg.access_key_id.clone(),
            cfg.secret_access_key.clone(),
            None,
            None,
            "bytehangar",
        );
        let mut builder = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(cfg.region.clone()))
            .credentials_provider(credentials);
        if let Some(endpoint) = &cfg.endpoint {
            // Custom provider (MinIO/R2): explicit endpoint + path-style addressing.
            builder = builder
                .endpoint_url(endpoint)
                .force_path_style(cfg.force_path_style);
        }
        let client = S3Client::from_conf(builder.build());
        Ok(Self {
            client,
            bucket: cfg.bucket.clone(),
        })
    }
}

#[async_trait]
impl BlobBackend for S3Backend {
    async fn put(&self, key: &str, data: Vec<u8>) -> AppResult<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data))
            .send()
            .await
            .map_err(|err| AppError::Internal(format!("s3 put: {err}")))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> AppResult<Vec<u8>> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|err| {
                let svc = err.into_service_error();
                if svc.is_no_such_key() {
                    AppError::NotFound
                } else {
                    AppError::Internal(format!("s3 get: {svc}"))
                }
            })?;
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|err| AppError::Internal(format!("s3 read: {err}")))?
            .into_bytes();
        Ok(bytes.to_vec())
    }

    async fn delete(&self, key: &str) -> AppResult<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|err| AppError::Internal(format!("s3 delete: {err}")))?;
        Ok(())
    }

    async fn stat(&self, key: &str) -> AppResult<BlobStat> {
        let output = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|err| {
                let svc = err.into_service_error();
                if svc.is_not_found() {
                    AppError::NotFound
                } else {
                    AppError::Internal(format!("s3 stat: {svc}"))
                }
            })?;
        Ok(BlobStat {
            size_bytes: output.content_length().unwrap_or(0).max(0) as u64,
        })
    }
}
