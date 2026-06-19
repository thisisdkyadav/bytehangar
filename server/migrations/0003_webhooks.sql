-- Per-tenant event webhook. The server POSTs file.uploaded / file.deleted events
-- to webhook_url, signed (HMAC-SHA256 over the body) with webhook_secret.
ALTER TABLE tenants ADD COLUMN webhook_url    TEXT;
ALTER TABLE tenants ADD COLUMN webhook_secret TEXT;
