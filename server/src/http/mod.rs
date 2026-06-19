//! HTTP routing. Two route planes share one router for now (single bind); the
//! two-listener split (internal vs public ports) is a later task.
//!
//!   /internal/v1/*  — key/admin auth: tenants, catalog, grants, S2S file ops
//!   /v1/*           — public/edge: grant-authorized upload, signed download
//!   /health         — liveness

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

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
    Router::new()
        .route("/health", get(health))
        .nest("/v1", public_routes())
        .with_state(state)
}

fn internal_routes() -> Router<AppState> {
    Router::new()
        // provisioning (admin token)
        .route("/tenants", post(tenants::create_tenant))
        .route("/tenants/{id}/keys", post(tenants::create_key))
        .route("/tenants/{id}/quota", patch(tenants::set_quota))
        .route("/gc", post(gc::gc_handler))
        // tenant control plane (tenant key)
        .route("/catalog", put(catalog::register_catalog))
        .route("/grants", post(grants::mint_grant))
        .route("/usage", get(usage::get_usage))
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
        // edge routes are browser-facing
        .layer(CorsLayer::permissive())
}

async fn health() -> Json<Value> {
    Json(json!({ "success": true, "service": "bytehangar", "status": "ok" }))
}
