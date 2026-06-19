//! Files: the edge upload + download pipeline and internal file operations.
//!
//! Upload streams the body straight to the blob backend (bounded memory): it
//! buffers only a small head for content sniffing, hashes + size-caps on the fly,
//! and dedupes after the write (dropping the duplicate). Download/content stream
//! the bytes back without buffering.

use axum::body::Body;
use axum::extract::multipart::Field;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use bytes::Bytes;
use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{GrantContext, TenantContext};
use crate::blob::BlobBackend;
use crate::crypto;
use crate::domain::GrantClaims;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{tenants, usage};

/// Inviolable master content-type allowlist (no executables ever land).
const MASTER_CONTENT_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "application/pdf",
];

/// Bytes buffered up-front for magic-byte content sniffing.
const SNIFF_LEN: usize = 8192;

fn master_allowed(content_type: &str) -> bool {
    MASTER_CONTENT_TYPES.contains(&content_type)
}

fn ext_for(content_type: &str, original_name: Option<&str>) -> String {
    match content_type {
        "application/pdf" => "pdf".to_string(),
        "image/png" => "png".to_string(),
        "image/jpeg" => "jpg".to_string(),
        "image/webp" => "webp".to_string(),
        "image/gif" => "gif".to_string(),
        _ => original_name
            .and_then(|name| std::path::Path::new(name).extension())
            .and_then(|ext| ext.to_str())
            .unwrap_or("bin")
            .to_string(),
    }
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct FileRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub file_ref: String,
    pub policy_key: String,
    pub category: String,
    pub original_name: String,
    pub stored_key: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub checksum_sha256: String,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

const FILE_COLUMNS: &str = "id, tenant_id, file_ref, policy_key, category, original_name, \
     stored_key, content_type, size_bytes, checksum_sha256, created_at, deleted_at";

async fn find_file(db: &PgPool, tenant_id: Uuid, file_ref: &str) -> AppResult<Option<FileRecord>> {
    let query = format!(
        "SELECT {FILE_COLUMNS} FROM files WHERE tenant_id = $1 AND file_ref = $2 AND deleted_at IS NULL"
    );
    let file = sqlx::query_as::<_, FileRecord>(&query)
        .bind(tenant_id)
        .bind(file_ref)
        .fetch_optional(db)
        .await?;
    Ok(file)
}

// ---------------------------------------------------------------------------
// Upload (public/edge)  —  POST /v1/upload  (auth: x-bytehangar-grant)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct UploadResponse {
    pub file_ref: String,
    pub content_type: String,
    pub size: i64,
    pub original_name: Option<String>,
    pub deduplicated: bool,
}

/// What `stream_field` produced after writing the body to the backend.
struct Staged {
    file_id: Uuid,
    stored_key: String,
    content_type: String,
    size: i64,
    checksum: String,
    file_name: Option<String>,
}

pub async fn upload(
    grant: GrantContext,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<UploadResponse>> {
    // Grant already verified (signature + expiry) by the extractor, before the body.
    let GrantContext { tenant, claims } = grant;
    let tenant_id = tenant.id;
    let max = std::cmp::min(claims.max, state.config.max_upload_bytes);

    // Find and stream the `file` field to the backend (bounded memory).
    let mut staged: Option<Staged> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| AppError::BadRequest(err.to_string()))?
    {
        if field.name() == Some("file") {
            staged = Some(stream_field(state.blob.as_ref(), tenant_id, &claims, max, field).await?);
            break;
        } else {
            let _ = field
                .bytes()
                .await
                .map_err(|err| AppError::BadRequest(err.to_string()))?;
        }
    }
    let staged = staged.ok_or_else(|| AppError::BadRequest("missing 'file' field".into()))?;

    // Write-then-dedupe: if identical bytes already exist, drop the one we wrote.
    let existing_key: Option<String> = sqlx::query_scalar(
        "SELECT stored_key FROM files WHERE tenant_id = $1 AND checksum_sha256 = $2 AND deleted_at IS NULL LIMIT 1",
    )
    .bind(tenant_id)
    .bind(&staged.checksum)
    .fetch_optional(&state.db)
    .await?;

    let (stored_key, wrote_new, deduplicated) = match existing_key {
        Some(existing) if existing != staged.stored_key => {
            let _ = state.blob.delete(&staged.stored_key).await;
            (existing, false, true)
        }
        _ => (staged.stored_key.clone(), true, false),
    };

    let file_ref = crypto::random_token(16);
    let size = staged.size;
    let content_type = staged.content_type.clone();
    let file_name = staged.file_name.clone();

    // Atomic: consume the single-use nonce, check quota, insert metadata, meter.
    let committed: AppResult<()> = async {
        let mut tx = state.db.begin().await?;

        let nonce = Uuid::parse_str(&claims.n).map_err(|_| AppError::Unauthorized)?;
        let consumed: Option<Uuid> = sqlx::query_scalar(
            "UPDATE upload_grants SET consumed_at = now() \
             WHERE nonce = $1 AND tenant_id = $2 AND consumed_at IS NULL AND expires_at > now() \
             RETURNING nonce",
        )
        .bind(nonce)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await?;
        if consumed.is_none() {
            return Err(AppError::Unauthorized);
        }

        if tenant.quota_bytes > 0 {
            let used: i64 = sqlx::query_scalar(
                "SELECT used_bytes FROM usage_counters WHERE tenant_id = $1 FOR UPDATE",
            )
            .bind(tenant_id)
            .fetch_optional(&mut *tx)
            .await?
            .unwrap_or(0);
            if used + size > tenant.quota_bytes {
                return Err(AppError::BadRequest("storage quota exceeded".into()));
            }
        }

        sqlx::query(
            "INSERT INTO files \
             (id, tenant_id, file_ref, policy_key, category, original_name, stored_key, content_type, size_bytes, checksum_sha256) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(staged.file_id)
        .bind(tenant_id)
        .bind(&file_ref)
        .bind(&claims.p)
        .bind(&claims.cat)
        .bind(file_name.clone().unwrap_or_default())
        .bind(&stored_key)
        .bind(&content_type)
        .bind(size)
        .bind(&staged.checksum)
        .execute(&mut *tx)
        .await?;

        usage::record_upload(&mut tx, tenant_id, size).await?;
        tx.commit().await?;
        Ok(())
    }
    .await;

    if let Err(err) = committed {
        if wrote_new {
            let _ = state.blob.delete(&stored_key).await; // best-effort orphan cleanup
        }
        return Err(err);
    }

    Ok(Json(UploadResponse {
        file_ref,
        content_type,
        size,
        original_name: file_name,
        deduplicated,
    }))
}

/// Stream one multipart field to the blob backend: sniff (head) -> enforce ->
/// stream (hash + size-cap) -> commit. Aborts the writer on any failure.
async fn stream_field(
    blob: &dyn BlobBackend,
    tenant_id: Uuid,
    claims: &GrantClaims,
    max: u64,
    field: Field<'_>,
) -> AppResult<Staged> {
    let file_name = field.file_name().map(|s| s.to_string());
    let declared_ct = field.content_type().map(|s| s.to_string());
    let mut field = field;

    // 1. Buffer a small head for content sniffing (still under the size cap).
    let mut head: Vec<u8> = Vec::new();
    loop {
        match field
            .chunk()
            .await
            .map_err(|err| AppError::BadRequest(err.to_string()))?
        {
            Some(chunk) => {
                if head.len() as u64 + chunk.len() as u64 > max {
                    return Err(AppError::PayloadTooLarge);
                }
                head.extend_from_slice(&chunk);
                if head.len() >= SNIFF_LEN {
                    break;
                }
            }
            None => break,
        }
    }
    if head.is_empty() {
        return Err(AppError::BadRequest("empty file".into()));
    }

    // 2. Resolve + enforce content type before writing anything.
    let sniffed = infer::get(&head).map(|t| t.mime_type().to_string());
    let content_type = sniffed
        .or(declared_ct)
        .unwrap_or_else(|| "application/octet-stream".to_string());
    if !master_allowed(&content_type) {
        return Err(AppError::Forbidden);
    }
    if !claims.ct.is_empty() && !claims.ct.iter().any(|c| c == &content_type) {
        return Err(AppError::BadRequest(format!(
            "content type '{content_type}' not allowed by policy"
        )));
    }

    // 3. Choose the key and stream head + remainder to the backend.
    let file_id = Uuid::now_v7();
    let ext = ext_for(&content_type, file_name.as_deref());
    let id_hex = file_id.simple().to_string();
    let now = Utc::now();
    let stored_key = format!(
        "{}/{}/{}/{:02}/{}/{}.{}",
        tenant_id,
        claims.cat,
        now.year(),
        now.month(),
        &id_hex[0..2],
        id_hex,
        ext
    );

    let mut writer = blob.open_writer(&stored_key).await?;
    let mut hasher = Sha256::new();
    let mut count: u64 = head.len() as u64;
    hasher.update(&head);
    if let Err(err) = writer.write(Bytes::from(head)).await {
        let _ = writer.abort().await;
        return Err(err);
    }

    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                count += chunk.len() as u64;
                if count > max {
                    let _ = writer.abort().await;
                    return Err(AppError::PayloadTooLarge);
                }
                hasher.update(&chunk);
                if let Err(err) = writer.write(chunk).await {
                    let _ = writer.abort().await;
                    return Err(err);
                }
            }
            Ok(None) => break,
            Err(err) => {
                let _ = writer.abort().await;
                return Err(AppError::BadRequest(err.to_string()));
            }
        }
    }
    if let Err(err) = writer.commit().await {
        let _ = writer.abort().await;
        return Err(err);
    }

    Ok(Staged {
        file_id,
        stored_key,
        content_type,
        size: count as i64,
        checksum: hex::encode(hasher.finalize()),
        file_name,
    })
}

// ---------------------------------------------------------------------------
// Download (public/edge)  —  GET /v1/files/:file_ref?t&exp&sig
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct DownloadQuery {
    t: String,
    exp: i64,
    sig: String,
    #[serde(default)]
    disposition: Option<String>,
}

pub async fn download(
    State(state): State<AppState>,
    Path(file_ref): Path<String>,
    Query(query): Query<DownloadQuery>,
) -> AppResult<Response> {
    let tenant_id = Uuid::parse_str(&query.t).map_err(|_| AppError::Unauthorized)?;
    let tenant = tenants::find_tenant_by_id(&state.db, tenant_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if query.exp < Utc::now().timestamp() {
        return Err(AppError::Unauthorized);
    }
    if !crypto::verify_download(
        tenant.signing_secret(),
        &query.t,
        &file_ref,
        query.exp,
        &query.sig,
    ) {
        return Err(AppError::Unauthorized);
    }

    let file = find_file(&state.db, tenant_id, &file_ref)
        .await?
        .ok_or(AppError::NotFound)?;
    let stream = state.blob.open_reader(&file.stored_key).await?;

    // best-effort egress metering
    let _ = sqlx::query("INSERT INTO usage_events (tenant_id, op, bytes) VALUES ($1, 'egress', $2)")
        .bind(tenant_id)
        .bind(file.size_bytes)
        .execute(&state.db)
        .await;

    let disposition = match query.disposition.as_deref() {
        Some("attachment") => "attachment",
        _ => "inline",
    };
    let content_disposition = format!("{}; filename=\"{}\"", disposition, file.original_name);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, file.content_type)
        .header(header::CONTENT_LENGTH, file.size_bytes.to_string())
        .header(header::CONTENT_DISPOSITION, content_disposition)
        .body(Body::from_stream(stream))
        .map_err(|err| AppError::Internal(err.to_string()))
}

// ---------------------------------------------------------------------------
// Internal file operations  (auth: tenant key)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListFilesQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    category: Option<String>,
}

#[derive(Serialize)]
pub struct FileListResponse {
    pub files: Vec<FileRecord>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// List this tenant's (live) files, newest first; optional category filter.
pub async fn list_files(
    ctx: TenantContext,
    State(state): State<AppState>,
    Query(query): Query<ListFilesQuery>,
) -> AppResult<Json<FileListResponse>> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);
    let tenant_id = ctx.tenant.id;

    let (files, total) = if let Some(category) = &query.category {
        let files = sqlx::query_as::<_, FileRecord>(&format!(
            "SELECT {FILE_COLUMNS} FROM files \
             WHERE tenant_id = $1 AND deleted_at IS NULL AND category = $2 \
             ORDER BY created_at DESC LIMIT $3 OFFSET $4"
        ))
        .bind(tenant_id)
        .bind(category)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM files WHERE tenant_id = $1 AND deleted_at IS NULL AND category = $2",
        )
        .bind(tenant_id)
        .bind(category)
        .fetch_one(&state.db)
        .await?;
        (files, total)
    } else {
        let files = sqlx::query_as::<_, FileRecord>(&format!(
            "SELECT {FILE_COLUMNS} FROM files \
             WHERE tenant_id = $1 AND deleted_at IS NULL \
             ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        ))
        .bind(tenant_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
        let total: i64 =
            sqlx::query_scalar("SELECT count(*) FROM files WHERE tenant_id = $1 AND deleted_at IS NULL")
                .bind(tenant_id)
                .fetch_one(&state.db)
                .await?;
        (files, total)
    };

    Ok(Json(FileListResponse {
        files,
        total,
        limit,
        offset,
    }))
}

pub async fn metadata(
    ctx: TenantContext,
    State(state): State<AppState>,
    Path(file_ref): Path<String>,
) -> AppResult<Json<FileRecord>> {
    let file = find_file(&state.db, ctx.tenant.id, &file_ref)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(file))
}

/// Server-to-server byte stream.
pub async fn content(
    ctx: TenantContext,
    State(state): State<AppState>,
    Path(file_ref): Path<String>,
) -> AppResult<Response> {
    let file = find_file(&state.db, ctx.tenant.id, &file_ref)
        .await?
        .ok_or(AppError::NotFound)?;
    let stream = state.blob.open_reader(&file.stored_key).await?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, file.content_type)
        .header(header::CONTENT_LENGTH, file.size_bytes.to_string())
        .body(Body::from_stream(stream))
        .map_err(|err| AppError::Internal(err.to_string()))
}

#[derive(Deserialize)]
pub struct SignRequest {
    #[serde(default)]
    expires_in_seconds: Option<i64>,
    #[serde(default)]
    disposition: Option<String>,
}

#[derive(Serialize)]
pub struct SignResponse {
    pub url: String,
    pub expires_at: String,
}

pub async fn sign(
    ctx: TenantContext,
    State(state): State<AppState>,
    Path(file_ref): Path<String>,
    Json(req): Json<SignRequest>,
) -> AppResult<Json<SignResponse>> {
    find_file(&state.db, ctx.tenant.id, &file_ref)
        .await?
        .ok_or(AppError::NotFound)?;

    let ttl = req
        .expires_in_seconds
        .unwrap_or(state.config.signed_url_ttl_seconds)
        .clamp(30, 86_400);
    let expires_at = Utc::now() + Duration::seconds(ttl);
    let exp = expires_at.timestamp();
    let tenant_id = ctx.tenant.id.to_string();
    let sig = crypto::sign_download(ctx.tenant.signing_secret(), &tenant_id, &file_ref, exp);

    let mut url = format!("/v1/files/{file_ref}?t={tenant_id}&exp={exp}&sig={sig}");
    if let Some(disposition) = &req.disposition {
        url.push_str(&format!("&disposition={disposition}"));
    }
    if !state.config.public_base_url.is_empty() {
        url = format!("{}{}", state.config.public_base_url.trim_end_matches('/'), url);
    }

    Ok(Json(SignResponse {
        url,
        expires_at: expires_at.to_rfc3339(),
    }))
}

pub async fn delete_file(
    ctx: TenantContext,
    State(state): State<AppState>,
    Path(file_ref): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let mut tx = state.db.begin().await?;
    let size: Option<i64> = sqlx::query_scalar(
        "UPDATE files SET deleted_at = now() \
         WHERE tenant_id = $1 AND file_ref = $2 AND deleted_at IS NULL \
         RETURNING size_bytes",
    )
    .bind(ctx.tenant.id)
    .bind(&file_ref)
    .fetch_optional(&mut *tx)
    .await?;
    let size = size.ok_or(AppError::NotFound)?;
    usage::record_delete(&mut tx, ctx.tenant.id, size).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "success": true })))
}
