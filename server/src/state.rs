use std::sync::Arc;

use sqlx::PgPool;

use crate::blob::BlobBackend;
use crate::config::Config;
use crate::secrets::Secrets;

/// Shared application state passed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: PgPool,
    pub blob: Arc<dyn BlobBackend>,
    pub secrets: Arc<Secrets>,
}
