//! Blob backend abstraction. The metadata/control plane is identical regardless
//! of where bytes physically land; only the driver changes.
//!
//! Backend is always in the byte path (no client->S3 presign in core), so the
//! driver just streams put/get/delete. Streaming-Body signatures are a Phase-1
//! refinement; the scaffold uses owned buffers.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::config::{Config, StorageBackendKind};
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
        StorageBackendKind::S3 => Ok(Arc::new(S3Backend::new())),
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
// S3-compatible (stub — wired with aws-sdk-s3 in Phase 1)
// ---------------------------------------------------------------------------

pub struct S3Backend {}

impl S3Backend {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl BlobBackend for S3Backend {
    async fn put(&self, _key: &str, _data: Vec<u8>) -> AppResult<()> {
        Err(AppError::NotImplemented)
    }
    async fn get(&self, _key: &str) -> AppResult<Vec<u8>> {
        Err(AppError::NotImplemented)
    }
    async fn delete(&self, _key: &str) -> AppResult<()> {
        Err(AppError::NotImplemented)
    }
    async fn stat(&self, _key: &str) -> AppResult<BlobStat> {
        Err(AppError::NotImplemented)
    }
}
