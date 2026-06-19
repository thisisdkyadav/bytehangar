//! Policy catalog. The SDK registers a tenant's policy set at app boot via
//! `PUT /internal/v1/catalog` — idempotent (no-op if unchanged) and versioned.
//! Policies are the resolved, enforceable upload rules (category + size + types).

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::TenantContext;
use crate::crypto;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PolicyRow {
    pub key: String,
    pub category: String,
    pub max_size_bytes: i64,
    pub allow_content_types: Vec<String>,
    pub visibility: String,
}

pub async fn find_policy(db: &PgPool, tenant_id: Uuid, key: &str) -> AppResult<Option<PolicyRow>> {
    let row = sqlx::query_as::<_, PolicyRow>(
        "SELECT key, category, max_size_bytes, allow_content_types, visibility \
         FROM policies WHERE tenant_id = $1 AND key = $2",
    )
    .bind(tenant_id)
    .bind(key)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

#[derive(Deserialize)]
pub struct PolicyInput {
    pub key: String,
    pub category: String,
    pub max_size_bytes: i64,
    #[serde(default)]
    pub allow_content_types: Vec<String>,
    /// "public" | "private" (default "private").
    #[serde(default)]
    pub visibility: Option<String>,
}

#[derive(Deserialize)]
pub struct RegisterCatalogRequest {
    pub policies: Vec<PolicyInput>,
}

#[derive(Serialize)]
pub struct RegisterCatalogResponse {
    pub version: i32,
    pub changed: bool,
    pub policy_count: usize,
}

/// Path-safe category: `^[a-z0-9-]+$`, max 64 chars.
pub fn is_valid_category(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub async fn register_catalog(
    ctx: TenantContext,
    State(state): State<AppState>,
    Json(req): Json<RegisterCatalogRequest>,
) -> AppResult<Json<RegisterCatalogResponse>> {
    if req.policies.is_empty() {
        return Err(AppError::BadRequest("at least one policy is required".into()));
    }

    let mut policies = req.policies;
    for policy in &policies {
        if policy.key.trim().is_empty() {
            return Err(AppError::BadRequest("policy key is required".into()));
        }
        if !is_valid_category(&policy.category) {
            return Err(AppError::BadRequest(format!(
                "invalid category '{}'; must match ^[a-z0-9-]+$",
                policy.category
            )));
        }
        if policy.max_size_bytes <= 0
            || (policy.max_size_bytes as u64) > state.config.max_upload_bytes
        {
            return Err(AppError::BadRequest(format!(
                "max_size_bytes for '{}' must be 1..={}",
                policy.key, state.config.max_upload_bytes
            )));
        }
        if let Some(visibility) = &policy.visibility {
            if visibility != "public" && visibility != "private" {
                return Err(AppError::BadRequest(format!(
                    "visibility for '{}' must be 'public' or 'private'",
                    policy.key
                )));
            }
        }
    }

    policies.sort_by(|a, b| a.key.cmp(&b.key));
    for pair in policies.windows(2) {
        if pair[0].key == pair[1].key {
            return Err(AppError::BadRequest(format!(
                "duplicate policy key '{}'",
                pair[0].key
            )));
        }
    }

    let hash = catalog_hash(&policies);
    let tenant_id = ctx.tenant.id;

    let latest = sqlx::query_as::<_, (i32, String)>(
        "SELECT version, hash FROM catalog_versions WHERE tenant_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(tenant_id)
    .fetch_optional(&state.db)
    .await?;

    if let Some((version, existing_hash)) = &latest {
        if existing_hash == &hash {
            return Ok(Json(RegisterCatalogResponse {
                version: *version,
                changed: false,
                policy_count: policies.len(),
            }));
        }
    }
    let next_version = latest.as_ref().map(|(v, _)| v + 1).unwrap_or(1);

    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM policies WHERE tenant_id = $1")
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;
    for policy in &policies {
        sqlx::query(
            "INSERT INTO policies (tenant_id, key, category, max_size_bytes, allow_content_types, visibility) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(tenant_id)
        .bind(&policy.key)
        .bind(&policy.category)
        .bind(policy.max_size_bytes)
        .bind(&policy.allow_content_types)
        .bind(policy.visibility.as_deref().unwrap_or("private"))
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("INSERT INTO catalog_versions (tenant_id, version, hash) VALUES ($1, $2, $3)")
        .bind(tenant_id)
        .bind(next_version)
        .bind(&hash)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(RegisterCatalogResponse {
        version: next_version,
        changed: true,
        policy_count: policies.len(),
    }))
}

fn catalog_hash(policies: &[PolicyInput]) -> String {
    let mut parts = String::new();
    for policy in policies {
        let mut content_types = policy.allow_content_types.clone();
        content_types.sort();
        parts.push_str(&format!(
            "{}|{}|{}|{}|{}\n",
            policy.key,
            policy.category,
            policy.max_size_bytes,
            content_types.join(","),
            policy.visibility.as_deref().unwrap_or("private")
        ));
    }
    crypto::sha256_hex(parts.as_bytes())
}
