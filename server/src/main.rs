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
mod metrics;
mod rate_limit;
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
    if config.is_production() && config.allowed_origins.is_empty() {
        tracing::warn!("APP_ENV=production but ALLOWED_ORIGINS is empty — CORS allows all origins");
    }

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let rate_limiter = Arc::new(rate_limit::RateLimiter::new(
        config.rate_limit_per_second,
        config.rate_limit_burst,
        config.trust_forwarded_for,
    ));

    let public_addr = format!("{}:{}", config.bind, config.port);
    let internal_addr = format!("{}:{}", config.internal_bind, config.internal_port);

    let state = AppState {
        config: Arc::new(config),
        db,
        blob,
        secrets,
        http_client,
        metrics: Arc::new(metrics::Metrics::default()),
        rate_limiter,
    };

    // Background webhook delivery worker.
    tokio::spawn(webhooks::run_worker(
        state.db.clone(),
        state.http_client.clone(),
        state.secrets.clone(),
    ));

    let public_listener = tokio::net::TcpListener::bind(&public_addr).await?;
    let internal_listener = tokio::net::TcpListener::bind(&internal_addr).await?;
    tracing::info!("public plane on {public_addr}, internal plane on {internal_addr}");

    tokio::try_join!(
        axum::serve(
            public_listener,
            http::public_router(state.clone())
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .into_future(),
        axum::serve(internal_listener, http::internal_router(state))
            .with_graceful_shutdown(shutdown_signal())
            .into_future(),
    )?;

    Ok(())
}

/// Resolve when the process receives SIGINT (Ctrl-C) or SIGTERM, so in-flight
/// requests can drain before the server stops.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received; draining");
}
