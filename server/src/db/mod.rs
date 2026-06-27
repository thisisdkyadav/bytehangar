//! Postgres connection pool. Lazy connect so the process can boot (and serve
//! `/health`) without a live database; the pool connects on first query.

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::config::Config;
use crate::error::AppResult;

pub fn connect(config: &Config) -> AppResult<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .min_connections(config.db_min_connections)
        .acquire_timeout(Duration::from_secs(config.db_acquire_timeout_secs))
        .connect_lazy(&config.database_url)?;
    Ok(pool)
}
