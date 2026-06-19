//! Tenants + API keys. Provisioned via admin endpoints; quota is set by the
//! external billing wrapper. Signing secrets stay server-side and are never
//! returned over the API.
//!
//! TODO(security): encrypt `signing_secret_enc` at rest with a server master key.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AdminAuth;
use crate::crypto;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Tenant {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub signing_secret_enc: String,
    pub quota_bytes: i64,
    pub created_at: DateTime<Utc>,
}

impl Tenant {
    /// The per-tenant secret used to sign grants and download URLs.
    pub fn signing_secret(&self) -> &str {
        &self.signing_secret_enc
    }
}

const TENANT_COLUMNS: &str =
    "id, name, status, signing_secret_enc, quota_bytes, created_at";

pub async fn find_tenant_by_id(db: &PgPool, id: Uuid) -> AppResult<Option<Tenant>> {
    let query = format!("SELECT {TENANT_COLUMNS} FROM tenants WHERE id = $1");
    let tenant = sqlx::query_as::<_, Tenant>(&query)
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(tenant)
}

pub async fn find_tenant_by_key_hash(db: &PgPool, key_hash: &str) -> AppResult<Option<Tenant>> {
    let tenant = sqlx::query_as::<_, Tenant>(
        "SELECT t.id, t.name, t.status, t.signing_secret_enc, t.quota_bytes, t.created_at \
         FROM api_keys k JOIN tenants t ON t.id = k.tenant_id \
         WHERE k.key_hash = $1 AND k.revoked_at IS NULL",
    )
    .bind(key_hash)
    .fetch_optional(db)
    .await?;
    Ok(tenant)
}

// ---------------------------------------------------------------------------
// Admin endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateTenantRequest {
    pub name: String,
}

#[derive(Serialize)]
pub struct TenantResponse {
    pub id: Uuid,
    pub name: String,
}

pub async fn create_tenant(
    _admin: AdminAuth,
    State(state): State<AppState>,
    Json(req): Json<CreateTenantRequest>,
) -> AppResult<Json<TenantResponse>> {
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }
    let secret = crypto::random_token(32);

    let query = format!(
        "INSERT INTO tenants (name, signing_secret_enc) VALUES ($1, $2) RETURNING {TENANT_COLUMNS}"
    );
    let mut tx = state.db.begin().await?;
    let tenant = sqlx::query_as::<_, Tenant>(&query)
        .bind(&req.name)
        .bind(&secret)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO usage_counters (tenant_id) VALUES ($1)")
        .bind(tenant.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(TenantResponse {
        id: tenant.id,
        name: tenant.name,
    }))
}

#[derive(Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Serialize)]
pub struct CreateKeyResponse {
    pub id: Uuid,
    /// Plaintext key — shown once, never retrievable again.
    pub key: String,
}

pub async fn create_key(
    _admin: AdminAuth,
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(req): Json<CreateKeyRequest>,
) -> AppResult<Json<CreateKeyResponse>> {
    let tenant = find_tenant_by_id(&state.db, tenant_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let role = req.role.unwrap_or_else(|| "app".to_string());
    if role != "app" && role != "admin" {
        return Err(AppError::BadRequest("role must be 'app' or 'admin'".into()));
    }
    let (key, hash) = crypto::generate_api_key();

    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO api_keys (tenant_id, name, key_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(tenant.id)
    .bind(&req.name)
    .bind(&hash)
    .bind(&role)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(CreateKeyResponse { id, key }))
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct TenantSummary {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub quota_bytes: i64,
    pub created_at: DateTime<Utc>,
    pub used_bytes: i64,
    pub object_count: i64,
}

#[derive(Serialize)]
pub struct TenantListResponse {
    pub tenants: Vec<TenantSummary>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// List tenants with their current usage (admin).
pub async fn list_tenants(
    _admin: AdminAuth,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<TenantListResponse>> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);

    let tenants = sqlx::query_as::<_, TenantSummary>(
        "SELECT t.id, t.name, t.status, t.quota_bytes, t.created_at, \
                COALESCE(c.used_bytes, 0) AS used_bytes, COALESCE(c.object_count, 0) AS object_count \
         FROM tenants t LEFT JOIN usage_counters c ON c.tenant_id = t.id \
         ORDER BY t.created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM tenants")
        .fetch_one(&state.db)
        .await?;

    Ok(Json(TenantListResponse {
        tenants,
        total,
        limit,
        offset,
    }))
}

#[derive(Deserialize)]
pub struct SetQuotaRequest {
    /// 0 = unlimited.
    pub quota_bytes: i64,
}

/// Set a tenant's quota. Called by the external billing wrapper.
pub async fn set_quota(
    _admin: AdminAuth,
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(req): Json<SetQuotaRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let affected = sqlx::query("UPDATE tenants SET quota_bytes = $1 WHERE id = $2")
        .bind(req.quota_bytes)
        .bind(tenant_id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "success": true })))
}
