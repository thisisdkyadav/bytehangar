//! Request authentication extractors.
//!
//! - `AdminAuth`     — bootstrap admin token (`x-bytehangar-admin`) for provisioning.
//! - `TenantContext` — tenant API key (`x-bytehangar-key`) → resolves the tenant.
//!
//! Edge upload/download auth (grant token, signed URL) is validated inline in the
//! file handlers, since it interleaves with the request body / query.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use chrono::Utc;
use uuid::Uuid;

use crate::crypto;
use crate::domain::GrantClaims;
use crate::error::AppError;
use crate::state::AppState;
use crate::tenants::{self, Tenant};

pub struct AdminAuth;

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let expected = &state.config.admin_token;
        if expected.is_empty() {
            // No admin token configured => provisioning endpoints are disabled.
            return Err(AppError::Forbidden);
        }
        let provided = parts
            .headers
            .get("x-bytehangar-admin")
            .and_then(|value| value.to_str().ok());
        match provided {
            Some(token) if constant_time_eq(token.as_bytes(), expected.as_bytes()) => Ok(AdminAuth),
            _ => Err(AppError::Unauthorized),
        }
    }
}

/// Constant-time byte comparison (avoids timing oracles on the admin token).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub struct TenantContext {
    pub tenant: Tenant,
}

impl FromRequestParts<AppState> for TenantContext {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let key = parts
            .headers
            .get("x-bytehangar-key")
            .and_then(|value| value.to_str().ok())
            .ok_or(AppError::Unauthorized)?;

        let hash = crypto::sha256_hex(key.as_bytes());
        let tenant = tenants::find_tenant_by_key_hash(&state.db, &state.secrets, &hash)
            .await?
            .ok_or(AppError::Unauthorized)?;

        if tenant.status != "active" {
            return Err(AppError::Forbidden);
        }
        Ok(TenantContext { tenant })
    }
}

/// Edge upload auth: validates the signed grant token (`x-bytehangar-grant`)
/// BEFORE the request body is read, so unauthenticated uploads never buffer a
/// payload. Single-use nonce consumption still happens later, in the upload tx.
pub struct GrantContext {
    pub tenant: Tenant,
    pub claims: GrantClaims,
}

impl FromRequestParts<AppState> for GrantContext {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let token = parts
            .headers
            .get("x-bytehangar-grant")
            .and_then(|value| value.to_str().ok())
            .ok_or(AppError::Unauthorized)?
            .to_string();

        // Peek tenant id (unverified) -> load tenant secret -> verify signature.
        let peek = crypto::peek_grant_claims(&token)?;
        let tenant_id = Uuid::parse_str(&peek.t).map_err(|_| AppError::Unauthorized)?;
        let tenant = tenants::find_tenant_by_id(&state.db, &state.secrets, tenant_id)
            .await?
            .ok_or(AppError::Unauthorized)?;
        if tenant.status != "active" {
            return Err(AppError::Forbidden);
        }
        let claims = crypto::decode_grant(tenant.signing_secret(), &token)?;
        if claims.exp < Utc::now().timestamp() {
            return Err(AppError::Unauthorized);
        }
        Ok(GrantContext { tenant, claims })
    }
}
