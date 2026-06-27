# Changelog

All notable changes to ByteHangar (server + `@bytehangar/sdk`) are documented here.
This project adheres to [Semantic Versioning](https://semver.org/).

## [1.1.0] — Harden & unblock

### Added — product
- **Configurable content-type allowlist** (`BLOB_ALLOWED_CONTENT_TYPES`; default
  images + pdf, `*` = allow-all) with an inviolable denylist for executables and
  active/render-unsafe types. Unblocks video/audio/office/archives without forking.
- **App-supplied upload metadata** carried in the grant (`actorId`, `actorRole`,
  `sourceService`, `entityHint`) → persisted on the file → returned in `FileRecord`.
  SDK: `createGrant(policy, { metadata })`.
- **Soft-delete restore**: `POST /internal/v1/files/{ref}/restore` (within the GC
  retention window), with blob re-verification, 410 Gone if already reclaimed, and a
  quota gate. SDK: `restoreFile(ref)`.
- **Cache-Control / ETag / conditional GET** on downloads — `If-None-Match` → 304
  (multi-value + weak validators), immutable caching for public files.

### Added — ops / reliability
- Internal **GC scheduler** (`GC_INTERVAL_SECONDS`) + an **always-on `upload_grants`
  pruner** (index-backed, migration 0005) so grants never grow unbounded.
- **Transactional GC** via `pg_try_advisory_xact_lock`: single-flight, runs on one
  connection, and auto-releases on commit/rollback/panic — a failed unlock can no
  longer wedge GC.
- **Configurable DB pool** (max/min/acquire-timeout), **boot config validation** +
  DB ping (fail-fast in production), **graceful drain** of the webhook worker on
  SIGTERM.
- **Audit-log writes** for admin/provisioning actions (tenant/key/quota/webhook/
  download-auth) to the `audit_log` table.

### Added — quality / CI / Docker
- Hardened **Dockerfile** (non-root uid 10001, `HEALTHCHECK`, `.dockerignore`).
- **Release automation**: GHCR image-on-tag, `cargo-audit` job, Dependabot
  (cargo/npm/actions), pinned `rust-toolchain.toml`.

### Security
- **Stored-XSS hardening** (from an adversarial review): the denylist now blocks
  active/render-unsafe types (`text/html`, `image/svg+xml`, `*/javascript`, xhtml,
  xml) regardless of allowlist mode, checked against both the sniffed and the
  declared content-type; all downloads send `X-Content-Type-Options: nosniff`.

## [1.0.1] — Security follow-ups

- **SSRF guard** on webhook + download-auth callback URLs (block private/link-local/
  loopback/metadata ranges; scheme + redirect restrictions).
- Constant-time admin-token compare + boot strength check in production.
- Enforce suspended-tenant status on the public edge.
- Sign the download `disposition` (no longer tamperable).

## [1.0.0] — Initial release

Security-first open-source file storage: catalog→grant→enforcement uploads, streaming
Rust core, local + S3 backends, multi-tenant (encrypted secrets, quotas, usage), dedup,
public/private + callback download auth, dedup-safe GC, durable signed webhooks, rate
limiting, request IDs, graceful shutdown, `/health` `/ready` `/metrics`, CI, and an
isomorphic TypeScript SDK (`server` / `client` / `react`).
