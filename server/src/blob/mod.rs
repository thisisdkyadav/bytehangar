//! Blob backend abstraction — streaming. The control/metadata plane is identical
//! regardless of where bytes land; only the driver changes.
//!
//! Writes go through a `BlobWriter` (incremental, so the handler can hash + size-cap
//! mid-stream); reads return a byte stream the HTTP layer forwards directly. Neither
//! path buffers the whole file in memory (S3 buffers at most one ~5 MiB part).

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use aws_sdk_s3::Client as S3Client;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::config::{Config, S3Config, StorageBackendKind};
use crate::error::{AppError, AppResult};

/// A stream of bytes read from a backend (forwarded straight to an HTTP body).
pub type ByteStreamBody = Pin<Box<dyn Stream<Item = AppResult<Bytes>> + Send>>;

#[derive(Debug, Clone)]
pub struct BlobStat {
    pub size_bytes: u64,
}

#[async_trait]
pub trait BlobBackend: Send + Sync {
    /// Open an incremental writer for `key`. The caller writes chunks then commits.
    async fn open_writer(&self, key: &str) -> AppResult<Box<dyn BlobWriter>>;
    /// Open a streaming reader for `key`.
    async fn open_reader(&self, key: &str) -> AppResult<ByteStreamBody>;
    async fn delete(&self, key: &str) -> AppResult<()>;
    async fn stat(&self, key: &str) -> AppResult<BlobStat>;
}

/// Incremental, abortable writer. Drop without `commit` leaves nothing committed.
#[async_trait]
pub trait BlobWriter: Send {
    async fn write(&mut self, chunk: Bytes) -> AppResult<()>;
    async fn commit(&mut self) -> AppResult<()>;
    async fn abort(&mut self) -> AppResult<()>;
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
    async fn open_writer(&self, key: &str) -> AppResult<Box<dyn BlobWriter>> {
        let final_path = self.path_for(key);
        if let Some(parent) = final_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let temp = final_path.with_extension("part");
        let file = tokio::fs::File::create(&temp).await?;
        Ok(Box::new(LocalWriter {
            file: Some(file),
            temp,
            final_path,
        }))
    }

    async fn open_reader(&self, key: &str) -> AppResult<ByteStreamBody> {
        let file = tokio::fs::File::open(self.path_for(key)).await?;
        let stream = ReaderStream::new(file).map(|item| item.map_err(AppError::from));
        Ok(Box::pin(stream))
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

struct LocalWriter {
    file: Option<tokio::fs::File>,
    temp: PathBuf,
    final_path: PathBuf,
}

#[async_trait]
impl BlobWriter for LocalWriter {
    async fn write(&mut self, chunk: Bytes) -> AppResult<()> {
        if let Some(file) = self.file.as_mut() {
            file.write_all(&chunk).await?;
        }
        Ok(())
    }

    async fn commit(&mut self) -> AppResult<()> {
        if let Some(mut file) = self.file.take() {
            file.flush().await?;
            let _ = file.sync_all().await;
        }
        tokio::fs::rename(&self.temp, &self.final_path).await?;
        Ok(())
    }

    async fn abort(&mut self) -> AppResult<()> {
        self.file.take(); // drop the handle
        let _ = tokio::fs::remove_file(&self.temp).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// S3-compatible (S3, MinIO, R2, B2) — multipart upload, streamed download
// ---------------------------------------------------------------------------

const S3_PART_SIZE: usize = 5 * 1024 * 1024; // S3 minimum part size (except the last)

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
            builder = builder
                .endpoint_url(endpoint)
                .force_path_style(cfg.force_path_style);
        }
        Ok(Self {
            client: S3Client::from_conf(builder.build()),
            bucket: cfg.bucket.clone(),
        })
    }
}

#[async_trait]
impl BlobBackend for S3Backend {
    async fn open_writer(&self, key: &str) -> AppResult<Box<dyn BlobWriter>> {
        let created = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|err| AppError::Internal(format!("s3 create multipart: {err}")))?;
        let upload_id = created
            .upload_id()
            .ok_or_else(|| AppError::Internal("s3 returned no upload id".into()))?
            .to_string();
        Ok(Box::new(S3Writer {
            client: self.client.clone(),
            bucket: self.bucket.clone(),
            key: key.to_string(),
            upload_id,
            buf: Vec::with_capacity(S3_PART_SIZE),
            parts: Vec::new(),
            next_part: 1,
        }))
    }

    async fn open_reader(&self, key: &str) -> AppResult<ByteStreamBody> {
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
        let reader = output.body.into_async_read();
        let stream = ReaderStream::new(reader).map(|item| item.map_err(AppError::from));
        Ok(Box::pin(stream))
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

struct S3Writer {
    client: S3Client,
    bucket: String,
    key: String,
    upload_id: String,
    buf: Vec<u8>,
    parts: Vec<CompletedPart>,
    next_part: i32,
}

impl S3Writer {
    async fn flush_part(&mut self) -> AppResult<()> {
        let body = std::mem::take(&mut self.buf);
        let output = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(&self.key)
            .upload_id(&self.upload_id)
            .part_number(self.next_part)
            .body(ByteStream::from(body))
            .send()
            .await
            .map_err(|err| AppError::Internal(format!("s3 upload part: {err}")))?;
        self.parts.push(
            CompletedPart::builder()
                .e_tag(output.e_tag().unwrap_or_default())
                .part_number(self.next_part)
                .build(),
        );
        self.next_part += 1;
        Ok(())
    }
}

#[async_trait]
impl BlobWriter for S3Writer {
    async fn write(&mut self, chunk: Bytes) -> AppResult<()> {
        self.buf.extend_from_slice(&chunk);
        if self.buf.len() >= S3_PART_SIZE {
            self.flush_part().await?;
        }
        Ok(())
    }

    async fn commit(&mut self) -> AppResult<()> {
        // Flush the trailing bytes (S3 requires at least one part).
        if !self.buf.is_empty() || self.parts.is_empty() {
            self.flush_part().await?;
        }
        let completed = CompletedMultipartUpload::builder()
            .set_parts(Some(self.parts.clone()))
            .build();
        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(&self.key)
            .upload_id(&self.upload_id)
            .multipart_upload(completed)
            .send()
            .await
            .map_err(|err| AppError::Internal(format!("s3 complete multipart: {err}")))?;
        Ok(())
    }

    async fn abort(&mut self) -> AppResult<()> {
        let _ = self
            .client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(&self.key)
            .upload_id(&self.upload_id)
            .send()
            .await;
        Ok(())
    }
}
