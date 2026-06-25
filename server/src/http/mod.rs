//! HTTP routing. Two route planes share one router for now (single bind); the
//! two-listener split (internal vs public ports) is a later task.
//!
//!   /internal/v1/*  — key/admin auth: tenants, catalog, grants, S2S file ops
//!   /v1/*           — public/edge: grant-authorized upload, signed download
//!   /health         — liveness

use axum::extract::DefaultBodyLimit;
use axum::http::HeaderValue;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};
use tower_http::cors::{Any, CorsLayer};

use crate::state::AppState;
use crate::{catalog, files, gc, grants, tenants, usage};

/// Internal plane: key/admin auth. Bind privately (loopback or a private network).
pub fn internal_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .nest("/internal/v1", internal_routes())
        .with_state(state)
}

/// Public/edge plane: browser-facing (grant upload, signed download).
pub fn public_router(state: AppState) -> Router {
    let cors = build_cors(&state.config.allowed_origins);
    Router::new()
        .route("/health", get(health))
        .nest("/v1", public_routes())
        .layer(cors)
        .with_state(state)
}

/// Restrict CORS to configured origins; allow-all only when none are set (dev).
fn build_cors(origins: &[String]) -> CorsLayer {
    if origins.is_empty() {
        return CorsLayer::permissive();
    }
    let parsed: Vec<HeaderValue> = origins.iter().filter_map(|o| o.parse().ok()).collect();
    CorsLayer::new()
        .allow_origin(parsed)
        .allow_methods(Any)
        .allow_headers(Any)
}

fn internal_routes() -> Router<AppState> {
    Router::new()
        // provisioning (admin token)
        .route("/tenants", post(tenants::create_tenant).get(tenants::list_tenants))
        .route("/tenants/{id}/keys", post(tenants::create_key))
        .route("/tenants/{id}/quota", patch(tenants::set_quota))
        .route("/tenants/{id}/download-auth", patch(tenants::set_download_auth))
        .route("/tenants/{id}/webhook", patch(tenants::set_webhook))
        .route("/gc", post(gc::gc_handler))
        // tenant control plane (tenant key)
        .route("/catalog", put(catalog::register_catalog))
        .route("/grants", post(grants::mint_grant))
        .route("/usage", get(usage::get_usage))
        .route("/files", get(files::list_files))
        // server-to-server file ops (tenant key)
        .route(
            "/files/{file_ref}",
            get(files::metadata).delete(files::delete_file),
        )
        .route("/files/{file_ref}/content", get(files::content))
        .route("/files/{file_ref}/sign", post(files::sign))
}

fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        // disable axum's body cap on uploads; size is enforced in-stream
        // (grant ∩ global cap) by the upload handler.
        .route(
            "/upload",
            post(files::upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/files/{file_ref}", get(files::download))
}

async fn health() -> Json<Value> {
    Json(json!({ "success": true, "service": "bytehangar", "status": "ok" }))
}
