//! Postgres connection pool. Lazy connect so the process can boot (and serve
//! `/health`) without a live database; the pool connects on first query.

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::error::AppResult;

pub fn connect(database_url: &str) -> AppResult<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect_lazy(database_url)?;
    Ok(pool)
}
