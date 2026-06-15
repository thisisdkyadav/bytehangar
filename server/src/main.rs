// Scaffolding phase: domain types and helpers land incrementally.
#![allow(dead_code)]

mod auth;
mod blob;
mod catalog;
mod config;
mod crypto;
mod db;
mod domain;
mod error;
mod files;
mod grants;
mod http;
mod state;
mod tenants;
mod usage;

use std::sync::Arc;

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let config = Config::from_env()?;
    let db = db::connect(&config.database_url)?;
    let blob = blob::from_config(&config)?;

    let addr = format!("0.0.0.0:{}", config.port);
    let state = AppState {
        config: Arc::new(config),
        db,
        blob,
    };

    let app = http::router(state);

    tracing::info!("bytehangar listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
