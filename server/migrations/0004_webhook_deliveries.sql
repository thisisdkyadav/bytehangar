-- Durable webhook delivery queue. Events are enqueued atomically with the file
-- operation; a background worker delivers them with retries + exponential backoff.
CREATE TABLE webhook_deliveries (
    id              BIGSERIAL PRIMARY KEY,
    tenant_id       UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event           TEXT NOT NULL,
    payload         JSONB NOT NULL,
    url             TEXT NOT NULL,              -- snapshot of webhook_url at enqueue
    secret_enc      TEXT NOT NULL,              -- encrypted snapshot of the signing secret
    status          TEXT NOT NULL DEFAULT 'pending',  -- pending | delivered | failed
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at    TIMESTAMPTZ
);

-- Worker scan: due pending deliveries, oldest first.
CREATE INDEX idx_webhook_deliveries_due
    ON webhook_deliveries (next_attempt_at)
    WHERE status = 'pending';

-- Per-tenant listing.
CREATE INDEX idx_webhook_deliveries_tenant
    ON webhook_deliveries (tenant_id, created_at DESC);
