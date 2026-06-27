-- Index backing the always-on upload_grants pruner (gc::run_grant_pruner).
-- The prune predicate filters on expires_at; without this it is a full scan.
CREATE INDEX IF NOT EXISTS idx_upload_grants_expires ON upload_grants (expires_at);
