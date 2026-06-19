-- Per-file visibility + per-tenant download authorization callback.

-- Visibility: "private" (default; requires a signed URL or app-callback) or
-- "public" (served without a signature).
ALTER TABLE policies ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private';
ALTER TABLE files    ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private';

-- Optional per-tenant callback. For a private file without a valid signed URL,
-- the server GETs this URL (forwarding the requester's Authorization/Cookie);
-- a 2xx response authorizes the download.
ALTER TABLE tenants  ADD COLUMN download_auth_url TEXT;
