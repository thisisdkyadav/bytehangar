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
mod gc;
mod grants;
mod http;
mod secrets;
mod state;
mod tenants;
mod usage;
mod webhooks;

use std::future::IntoFuture;
use std::sync::Arc;

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let config = Config::from_env()?;
    let db = db::connect(&config.database_url)?;

    // Self-migrate on boot. Best-effort: if the DB is unreachable the server still
    // comes up and serves /health (the pool connects lazily on first real query).
    match sqlx::migrate!("./migrations").run(&db).await {
        Ok(()) => tracing::info!("migrations applied"),
        Err(err) => tracing::warn!("migrations not applied (database unavailable?): {err}"),
    }

    let blob = blob::from_config(&config)?;
    let secrets = Arc::new(secrets::Secrets::new(&config.master_key));
    if secrets.enabled() {
        tracing::info!("tenant secrets encrypted at rest");
    } else {
        tracing::warn!("MASTER_KEY not set — tenant secrets stored as plaintext");
    }

    let public_addr = format!("{}:{}", config.bind, config.port);
    let internal_addr = format!("{}:{}", config.internal_bind, config.internal_port);

    let state = AppState {
        config: Arc::new(config),
        db,
        blob,
        secrets,
    };

    let public_listener = tokio::net::TcpListener::bind(&public_addr).await?;
    let internal_listener = tokio::net::TcpListener::bind(&internal_addr).await?;
    tracing::info!("public plane on {public_addr}, internal plane on {internal_addr}");

    tokio::try_join!(
        axum::serve(public_listener, http::public_router(state.clone())).into_future(),
        axum::serve(internal_listener, http::internal_router(state)).into_future(),
    )?;

    Ok(())
}
