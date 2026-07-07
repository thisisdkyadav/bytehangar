# ByteHangar

[![CI](https://github.com/thisisdkyadav/bytehangar/actions/workflows/ci.yml/badge.svg)](https://github.com/thisisdkyadav/bytehangar/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![npm](https://img.shields.io/npm/v/@bytehangar/sdk.svg)](https://www.npmjs.com/package/@bytehangar/sdk)
[![GHCR](https://img.shields.io/badge/ghcr.io-bytehangar-2496ed?logo=docker&logoColor=white)](https://github.com/thisisdkyadav/bytehangar/pkgs/container/bytehangar)

Open-source, self-hostable file storage + SDK — an alternative to UploadThing/Cloudinary that runs on **your** server, with **your** choice of byte backend (local disk or any S3-compatible store).

**Why ByteHangar?** UploadThing and Cloudinary hold your bytes; raw S3 gives you bytes but no upload policies, grants, quotas, or SDK. ByteHangar is the control plane **and** the SDK on top of storage **you** own. Reach for it when self-hosting is a requirement — compliance, data residency, cost, or vendor independence.

- **Rust** core (Axum/Tokio) — streaming, backend-always-in-path, two listeners (internal + public).
- **Postgres** metadata; pluggable blob backends: **local disk** or **S3-compatible** (S3, MinIO, R2, B2).
- **Multi-tenant**: per-tenant API keys, signing secrets (encrypted at rest), quotas, usage metering.
- Typed upload **policies**, signed single-use **grants**, **public/private** files with signed-URL or app-callback download auth.
- **Dedup-safe GC**, signed **event webhooks**, list/admin endpoints, `/metrics` + `/ready`.
- Isomorphic **TypeScript SDK**: `@bytehangar/sdk/server`, `/client`, `/react`.

See [plan.md](./plan.md) for the full design.

---

## How it works

```
Catalog (boot)            Grant (per-upload)             Enforcement (server)
app registers policies →  app backend mints a signed,  → server verifies sig +
(category, size, types,   single-use token for one        single-use nonce, then
visibility)               upload, hands it to client       enforces grant ∩ policy ∩
                                                           global caps, dedupes, stores
```

Two HTTP planes:
- **Internal** (`/internal/v1/*`, key/admin auth) — provisioning, catalog, grants, server-to-server file ops. Bind privately.
- **Public/edge** (`/v1/*`) — grant-authorized upload, signed/public download. Internet-facing.

---

## Quickstart (dev)

```bash
# 1. Postgres + MinIO (host ports 5433 / 9100 to avoid clashes)
docker compose up -d --wait

# 2. run the server (auto-migrates on boot)
cp server/.env.example server/.env        # then set ADMIN_TOKEN + MASTER_KEY
cargo run --manifest-path server/Cargo.toml
# public plane :5100, internal plane :5101
```

End-to-end smoke test (provision → catalog → grant → upload → download → dedup → GC, on both backends):

```bash
bash scripts/run-e2e.sh                    # local-disk backend
# S3/MinIO backend:
STORAGE_BACKEND=s3 S3_ENDPOINT=http://localhost:9100 S3_BUCKET=bytehangar \
  S3_ACCESS_KEY_ID=bytehangar S3_SECRET_ACCESS_KEY=bytehangar-secret \
  S3_FORCE_PATH_STYLE=true PORT=5181 bash scripts/run-e2e.sh
```

### Docker

```bash
docker build -t bytehangar .
docker run --rm -p 5100:5100 -p 5101:5101 \
  -e DATABASE_URL=postgres://user:pass@db:5432/bytehangar \
  -e ADMIN_TOKEN=... -e MASTER_KEY=... bytehangar
```

### Run with Docker (GHCR)

Prebuilt images are published to GitHub Container Registry on every tagged release:

```bash
docker pull ghcr.io/thisisdkyadav/bytehangar:latest

# ports: 5100 = public/edge plane, 5101 = internal plane (keep private)
docker run --rm \
  -p 5100:5100 -p 5101:5101 \
  -e DATABASE_URL=postgres://user:pass@host:5432/bytehangar \
  -e ADMIN_TOKEN=your-admin-token \
  -e MASTER_KEY=your-32-byte-master-key \
  ghcr.io/thisisdkyadav/bytehangar:latest
```

`DATABASE_URL`, `ADMIN_TOKEN`, and `MASTER_KEY` are the required env; the container needs a **reachable Postgres** (run one alongside it or point at a managed instance). For a full stack (server + Postgres + an example app) see [examples/quickstart](./examples/quickstart).

---

## SDK

```ts
// server (Node) — holds the tenant key; mints grants
import { ByteHangarServer } from "@bytehangar/sdk/server";
const storage = new ByteHangarServer({ baseUrl, apiKey });
await storage.registerCatalog([
  { key: "avatar", category: "avatars", maxSizeBytes: 512_000,
    allowContentTypes: ["image/png", "image/jpeg"], visibility: "public" },
]);
const { token } = await storage.createGrant("avatar");      // hand `token` to the client

// browser — no secrets, uploads with the grant
import { ByteHangarClient } from "@bytehangar/sdk/client";
const res = await new ByteHangarClient({ baseUrl }).upload(token, file);
```

Full SDK docs (incl. the React `<UploadButton>`): [sdk/README.md](./sdk/README.md).

---

## Configuration

| Env | Default | Notes |
|---|---|---|
| `APP_ENV` | `development` | `production` requires `MASTER_KEY`; warns on empty CORS allow-list |
| `PORT` / `BIND_ADDRESS` | `5100` / `0.0.0.0` | Public/edge listener |
| `INTERNAL_PORT` / `INTERNAL_BIND_ADDRESS` | `5101` / `127.0.0.1` | Internal listener — keep private |
| `ALLOWED_ORIGINS` | _(empty)_ | CSV of allowed CORS origins; empty = allow-all (dev only) |
| `RATE_LIMIT_PER_SECOND` / `RATE_LIMIT_BURST` | `50` / `100` | Per-client-IP rate limit on the public plane; `0` disables |
| `TRUST_FORWARDED_FOR` | `false` | Trust `X-Forwarded-For`/`X-Real-IP` for the client IP — enable **only** behind a trusted proxy |
| `ALLOW_PRIVATE_OUTBOUND` | `false` | Allow outbound webhook / download-auth calls to private/loopback/link-local targets (disables the SSRF guard) |
| `DATABASE_URL` | `…@localhost:5433/bytehangar` | Postgres |
| `DB_MAX_CONNECTIONS` / `DB_MIN_CONNECTIONS` | `10` / `0` | Postgres pool size bounds |
| `DB_ACQUIRE_TIMEOUT_SECS` | `30` | Max wait to acquire a pooled connection |
| `STORAGE_BACKEND` | `local` | `local` or `s3` |
| `DATA_ROOT` | `./data` | Local-disk root |
| `S3_BUCKET` / `S3_REGION` / `S3_ENDPOINT` / `S3_ACCESS_KEY_ID` / `S3_SECRET_ACCESS_KEY` / `S3_FORCE_PATH_STYLE` | — / `us-east-1` / — / — / — / `false` | S3-compatible backend (set endpoint + path-style for MinIO/R2) |
| `MAX_UPLOAD_BYTES` | `52428800` | Inviolable global ceiling |
| `BLOB_ALLOWED_CONTENT_TYPES` | _(empty)_ | Master content-type allowlist (CSV). Empty = `image/png,jpeg,webp,gif` + `application/pdf`; `*` = allow all except the inviolable executable/active-content denylist |
| `ADMIN_TOKEN` | _(empty)_ | Bootstrap admin token; empty = provisioning disabled |
| `MASTER_KEY` | _(empty)_ | Encrypts tenant secrets at rest (AES-256-GCM). **Required in production** |
| `MASTER_KEY_PREVIOUS` | _(empty)_ | Old key accepted for decrypt during a [key rotation](./docs/OPERATIONS.md#master-key-rotation) |
| `SIGNED_URL_TTL_SECONDS` / `PUBLIC_BASE_URL` | `300` / _(empty)_ | Signed download URLs |
| `GC_INTERVAL_SECONDS` | `0` | Built-in GC scheduler interval; `0` disables (run GC via the endpoint/cron instead) |
| `GC_RETENTION_SECONDS` | `86400` | Trash window — only GC files soft-deleted at least this long ago |
| `IMAGE_RENDER_CONCURRENCY` | `0` | Max concurrent image-variant renders (CPU-bound); `0` = auto (CPU count) |

---

## Image transforms

Register **named transform presets** per policy; reference one by name on download and
the server renders it lazily (pure-Rust — no libvips) then caches it as a content-addressed
sibling blob reclaimed by GC with the original. Presets are the render-DoS guard: only the
named, bounded outputs you define can ever be produced.

```ts
await storage.registerCatalog([{
  key: "avatar", category: "avatars", maxSizeBytes: 5_000_000,
  allowContentTypes: ["image/png", "image/jpeg", "image/webp"],
  transforms: {
    thumb:  { w: 256, h: 256, fit: "cover", fmt: "webp", q: 80 },
    banner: { w: 1200, fit: "inside", fmt: "jpeg", q: 82 },
  },
}]);

// private: fold the variant into the signed URL
const { url } = await storage.signDownload(fileRef, { variant: "thumb" });
// public: just add it to the file URL
client.fileUrl(tenantId, fileRef, { variant: "thumb" });
```

- **fit**: `cover` (scale + center-crop), `inside` (fit within, keep aspect), `fill` (stretch).
- **fmt**: `jpeg`, `png`, `webp` (`q` applies to jpeg). Give at least one of `w`/`h`.
- The variant is part of the signed surface, so a signature for `thumb` can't fetch `banner` or the original.

---

## Security model

- **Grants** are HMAC-signed (per-tenant secret), short-lived, and **single-use** (nonce consumed transactionally) — a client can only perform an upload your backend authorized, within bounds it can't change.
- **Global caps** are inviolable: `MAX_UPLOAD_BYTES` and path-safe categories — enforced regardless of what a request claims.
- **Content types**: the allowlist is **configurable** (`BLOB_ALLOWED_CONTENT_TYPES`; default images + pdf, `*` = allow-all) but sits behind an **inviolable denylist** that always blocks executables **and** active/render-unsafe types (`text/html`, `image/svg+xml`, `*/javascript`, xhtml/xml) — checked against both the sniffed and the declared type. All downloads send `X-Content-Type-Options: nosniff`.
- **Downloads**: public files served unsigned; private files need a **signed URL** or approval from the tenant's **download-auth callback** (the server forwards the requester's `Authorization`/`Cookie`). Responses carry `Cache-Control` + `ETag` and honor conditional `If-None-Match` GETs (multi-value + weak validators → `304`); public files are cached `immutable`.
- **Secrets at rest**: tenant signing + webhook secrets are AES-256-GCM encrypted when `MASTER_KEY` is set (required in production).
- **Multi-tenant isolation**: per-tenant keys + secrets; every blob path and signature is tenant-scoped.
- **Webhooks** are HMAC-signed (`x-bytehangar-signature: sha256=…`) and **durable** (persisted in the same transaction as the event, then retried with backoff). Delivery is **at-least-once** — dedupe on event + file_ref.
- **Rate limiting**: built-in per-client-IP token bucket on the public `/v1` plane (`RATE_LIMIT_*`), keyed on the socket peer by default; set `TRUST_FORWARDED_FOR=true` only behind a trusted proxy. TLS and global/L7 DDoS protection still belong at the reverse proxy / ingress.
- **SSRF guard**: outbound calls to tenant-controlled URLs (webhook + download-auth callback) reject private / loopback / link-local / cloud-metadata targets and don't follow redirects; `ALLOW_PRIVATE_OUTBOUND=true` opts out for trusted internal use.
- **Edge hardening**: admin token compared in constant time (min length enforced in production); suspended tenants blocked on the public plane; the download `disposition` is part of the signed URL.
- **Request IDs**: every request gets a server-assigned `x-request-id`, echoed on the response and in logs.

---

## Operations

- `GET /health` — liveness; `GET /ready` — readiness (checks Postgres).
- `GET /metrics` (internal plane) — Prometheus counters (uploads, downloads, bytes, deletes).
- `POST /internal/v1/gc` (admin) — reclaim blobs for soft-deleted files (dedup-safe). Run on a schedule (cron), **or** set `GC_INTERVAL_SECONDS>0` to use the **built-in GC scheduler** (single-flight via an advisory xact lock) and skip the cron entirely. `GC_RETENTION_SECONDS` sets the trash window.
- `POST /internal/v1/reconcile` (admin) — reclaim **orphan blobs**: bytes with no `files`/`file_variants` row (e.g. an upload that wrote the blob then crashed before its DB row). Conservative (only deletes blobs older than `grace_seconds`, default 3600); supports `dry_run`. Run occasionally and after a restore.
- `POST /internal/v1/admin/rotate-secrets` (admin) — re-encrypt all tenant secrets under the current `MASTER_KEY` (see [key rotation](./docs/OPERATIONS.md#master-key-rotation)).
- **Audit log**: admin/provisioning actions (tenant / key / quota / webhook / download-auth / secret-rotation) are written to the `audit_log` table for traceability.
- **Backup, disaster recovery, and routine maintenance**: see the [operations runbook](./docs/OPERATIONS.md) — Postgres is the source of truth; the blob store must never be restored *behind* it.
- **Graceful shutdown** on SIGINT/SIGTERM drains in-flight requests (and the webhook worker).
- Server **auto-migrates** on boot.

---

## Development

```bash
cargo test  --manifest-path server/Cargo.toml          # unit tests (no DB)
cargo clippy --manifest-path server/Cargo.toml --all-targets -- -D warnings
bash scripts/run-e2e.sh                                 # full e2e (needs compose up)
```

CI (GitHub Actions) runs clippy + tests + release build, the SDK build, and the e2e on both backends.

---

## License

[MIT](./LICENSE).
