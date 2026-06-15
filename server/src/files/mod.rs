//! Files: the edge upload + download pipeline and internal file operations.
//!
//! Upload enforces, in order: grant signature → grant not expired → size cap
//! (grant ∩ global) → content-type (sniffed) ∈ policy ∩ master allowlist →
//! content-addressed dedup → blob write → single-use nonce consume + metadata +
//! usage, atomically. New blobs are best-effort cleaned up if the tx fails.

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{GrantContext, TenantContext};
use crate::crypto;
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

pub async fn upload(
    grant: GrantContext,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<UploadResponse>> {
    // Grant already verified (signature + expiry) by the extractor, before the body.
    let GrantContext { tenant, claims } = grant;
    let tenant_id = tenant.id;

    let max = std::cmp::min(claims.max, state.config.max_upload_bytes) as usize;

    // 2. Read the `file` field, capping size as we stream.
    let mut payload: Option<(Vec<u8>, Option<String>, Option<String>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| AppError::BadRequest(err.to_string()))?
    {
        if field.name() == Some("file") {
            let file_name = field.file_name().map(|s| s.to_string());
            let declared_ct = field.content_type().map(|s| s.to_string());
            let mut field = field;
            let mut data = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|err| AppError::BadRequest(err.to_string()))?
            {
                if data.len() + chunk.len() > max {
                    return Err(AppError::PayloadTooLarge);
                }
                data.extend_from_slice(&chunk);
            }
            payload = Some((data, file_name, declared_ct));
            break;
        } else {
            let _ = field
                .bytes()
                .await
                .map_err(|err| AppError::BadRequest(err.to_string()))?;
        }
    }
    let (data, file_name, declared_ct) =
        payload.ok_or_else(|| AppError::BadRequest("missing 'file' field".into()))?;
    if data.is_empty() {
        return Err(AppError::BadRequest("empty file".into()));
    }

    // 3. Resolve + enforce content type (sniffed wins).
    let sniffed = infer::get(&data).map(|t| t.mime_type().to_string());
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

    let size = data.len() as i64;
    let checksum = crypto::sha256_hex(&data);

    // 4. Content-addressed dedup: reuse an existing blob with the same checksum.
    let existing_key: Option<String> = sqlx::query_scalar(
        "SELECT stored_key FROM files WHERE tenant_id = $1 AND checksum_sha256 = $2 AND deleted_at IS NULL LIMIT 1",
    )
    .bind(tenant_id)
    .bind(&checksum)
    .fetch_optional(&state.db)
    .await?;

    let file_id = Uuid::now_v7();
    let ext = ext_for(&content_type, file_name.as_deref());
    let (stored_key, wrote_new, deduplicated) = match existing_key {
        Some(key) => (key, false, true),
        None => {
            let id_hex = file_id.simple().to_string();
            let now = Utc::now();
            let key = format!(
                "{}/{}/{}/{:02}/{}/{}.{}",
                tenant_id,
                claims.cat,
                now.year(),
                now.month(),
                &id_hex[0..2],
                id_hex,
                ext
            );
            state.blob.put(&key, data).await?;
            (key, true, false)
        }
    };

    let file_ref = crypto::random_token(16);

    // 5. Atomic: consume the single-use nonce, check quota, insert metadata, meter.
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
        .bind(file_id)
        .bind(tenant_id)
        .bind(&file_ref)
        .bind(&claims.p)
        .bind(&claims.cat)
        .bind(file_name.clone().unwrap_or_default())
        .bind(&stored_key)
        .bind(&content_type)
        .bind(size)
        .bind(&checksum)
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
    let data = state.blob.get(&file.stored_key).await?;

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
        .header(header::CONTENT_DISPOSITION, content_disposition)
        .body(Body::from(data))
        .map_err(|err| AppError::Internal(err.to_string()))
}

// ---------------------------------------------------------------------------
// Internal file operations  (auth: tenant key)
// ---------------------------------------------------------------------------

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
    let data = state.blob.get(&file.stored_key).await?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, file.content_type)
        .body(Body::from(data))
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
