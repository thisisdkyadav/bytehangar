-- ByteHangar core schema (Phase 1)
-- Metadata only; bytes live in the blob backend (local disk or S3).

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- Tenants: one per consuming app. Owns its own signing secret + quota.
CREATE TABLE tenants (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name               TEXT NOT NULL,
    status             TEXT NOT NULL DEFAULT 'active',          -- active | suspended
    signing_secret_enc TEXT NOT NULL,                           -- per-tenant grant/url signing secret
    quota_bytes        BIGINT NOT NULL DEFAULT 0,               -- 0 = unlimited; set by billing wrapper
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- API keys: server-to-server auth. Stored hashed; secret shown once on creation.
CREATE TABLE api_keys (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    key_hash     TEXT NOT NULL,
    role         TEXT NOT NULL DEFAULT 'app',                   -- app | admin
    last_used_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at   TIMESTAMPTZ
);
CREATE INDEX idx_api_keys_tenant ON api_keys(tenant_id);

-- Policy catalog: registered by the SDK at app boot (idempotent, versioned).
CREATE TABLE policies (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    key                TEXT NOT NULL,                           -- e.g. "profile-image"
    category           TEXT NOT NULL,                           -- path prefix, sanitized ^[a-z0-9-]+$
    max_size_bytes     BIGINT NOT NULL,
    allow_content_types TEXT[] NOT NULL DEFAULT '{}',
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, key)
);

CREATE TABLE catalog_versions (
    tenant_id  UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    version    INTEGER NOT NULL,
    hash       TEXT NOT NULL,                                   -- content hash of catalog for idempotency
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, version)
);

-- Files: metadata. stored_key = blob backend key. Soft delete.
CREATE TABLE files (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    file_ref        TEXT NOT NULL,                              -- public opaque reference
    policy_key      TEXT NOT NULL,
    category        TEXT NOT NULL,
    original_name   TEXT NOT NULL,
    stored_key      TEXT NOT NULL,                             -- key in blob backend
    content_type    TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    checksum_sha256 TEXT NOT NULL,
    actor_id        TEXT,
    actor_role      TEXT,
    source_service  TEXT,
    entity_hint     TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at      TIMESTAMPTZ,
    UNIQUE (tenant_id, file_ref)
);
CREATE INDEX idx_files_tenant_category ON files(tenant_id, category);
CREATE INDEX idx_files_checksum ON files(tenant_id, checksum_sha256);

-- Upload grants: single-use, signed. Nonce consumed transactionally to stop replay.
CREATE TABLE upload_grants (
    nonce        UUID PRIMARY KEY,
    tenant_id    UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    policy_key   TEXT NOT NULL,
    max_size     BIGINT NOT NULL,
    content_type TEXT,
    expires_at   TIMESTAMPTZ NOT NULL,
    consumed_at  TIMESTAMPTZ
);

-- Usage: append-only events (for the external billing wrapper) + live counters (for inline enforcement).
CREATE TABLE usage_events (
    id         BIGSERIAL PRIMARY KEY,
    tenant_id  UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    op         TEXT NOT NULL,                                   -- upload | egress | delete
    bytes      BIGINT NOT NULL DEFAULT 0,
    count      INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_usage_events_tenant_time ON usage_events(tenant_id, created_at);

CREATE TABLE usage_counters (
    tenant_id    UUID PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    used_bytes   BIGINT NOT NULL DEFAULT 0,
    object_count BIGINT NOT NULL DEFAULT 0,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE audit_log (
    id         BIGSERIAL PRIMARY KEY,
    tenant_id  UUID REFERENCES tenants(id) ON DELETE SET NULL,
    actor      TEXT,
    action     TEXT NOT NULL,
    target     TEXT,
    before     JSONB,
    after      JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
