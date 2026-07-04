-- Per-policy named image transform presets (catalog), e.g.
--   { "thumb": { "w": 256, "h": 256, "fit": "cover", "fmt": "webp", "q": 80 } }
ALTER TABLE policies ADD COLUMN transforms JSONB NOT NULL DEFAULT '{}'::jsonb;

-- Rendered, content-addressed image variants, cached as sibling blobs and reclaimed
-- by GC together with the parent file's blob (dedup-safe). One row per (file, preset).
CREATE TABLE file_variants (
    id UUID PRIMARY KEY,
    file_id UUID NOT NULL REFERENCES files (id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    preset_key TEXT NOT NULL,
    stored_key TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    checksum_sha256 TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (file_id, preset_key)
);

CREATE INDEX idx_file_variants_stored_key ON file_variants (stored_key);
