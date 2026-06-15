//! Upload grants. The app backend (authenticated by its tenant key) mints a
//! short-lived, single-use, signed token authorizing one upload under one policy.
//! The token is handed to the (untrusted) client; the nonce is persisted so the
//! upload path can consume it exactly once.

use axum::extract::State;
use axum::Json;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::TenantContext;
use crate::catalog;
use crate::crypto;
use crate::domain::GrantClaims;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct MintGrantRequest {
    pub policy_key: String,
    #[serde(default)]
    pub expires_in_seconds: Option<i64>,
}

#[derive(Serialize)]
pub struct MintGrantResponse {
    pub token: String,
    pub expires_at: String,
}

pub async fn mint_grant(
    ctx: TenantContext,
    State(state): State<AppState>,
    Json(req): Json<MintGrantRequest>,
) -> AppResult<Json<MintGrantResponse>> {
    let policy = catalog::find_policy(&state.db, ctx.tenant.id, &req.policy_key)
        .await?
        .ok_or_else(|| AppError::BadRequest(format!("unknown policy '{}'", req.policy_key)))?;

    let ttl = req
        .expires_in_seconds
        .unwrap_or(state.config.signed_url_ttl_seconds)
        .clamp(30, 3600);
    let expires_at = Utc::now() + Duration::seconds(ttl);
    let nonce = Uuid::now_v7();

    sqlx::query(
        "INSERT INTO upload_grants (nonce, tenant_id, policy_key, max_size, content_type, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(nonce)
    .bind(ctx.tenant.id)
    .bind(&policy.key)
    .bind(policy.max_size_bytes)
    .bind(Option::<String>::None)
    .bind(expires_at)
    .execute(&state.db)
    .await?;

    let claims = GrantClaims {
        t: ctx.tenant.id.to_string(),
        p: policy.key.clone(),
        cat: policy.category.clone(),
        max: policy.max_size_bytes as u64,
        ct: policy.allow_content_types.clone(),
        n: nonce.to_string(),
        exp: expires_at.timestamp(),
    };
    let token = crypto::encode_grant(ctx.tenant.signing_secret(), &claims)?;

    Ok(Json(MintGrantResponse {
        token,
        expires_at: expires_at.to_rfc3339(),
    }))
}
