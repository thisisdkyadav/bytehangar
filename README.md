# ByteHangar

Open-source, self-hostable file storage + SDK — an alternative to UploadThing/Cloudinary that runs on **your** server, with **your** choice of byte backend (local disk or any S3-compatible store).

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
| `DATABASE_URL` | `…@localhost:5433/bytehangar` | Postgres |
| `STORAGE_BACKEND` | `local` | `local` or `s3` |
| `DATA_ROOT` | `./data` | Local-disk root |
| `S3_BUCKET` / `S3_REGION` / `S3_ENDPOINT` / `S3_ACCESS_KEY_ID` / `S3_SECRET_ACCESS_KEY` / `S3_FORCE_PATH_STYLE` | — | S3-compatible backend (set endpoint + path-style for MinIO/R2) |
| `MAX_UPLOAD_BYTES` | `52428800` | Inviolable global ceiling |
| `ADMIN_TOKEN` | _(empty)_ | Bootstrap admin token; empty = provisioning disabled |
| `MASTER_KEY` | _(empty)_ | Encrypts tenant secrets at rest (AES-256-GCM). **Required in production** |
| `SIGNED_URL_TTL_SECONDS` / `PUBLIC_BASE_URL` | `300` / _(empty)_ | Signed download URLs |

---

## Security model

- **Grants** are HMAC-signed (per-tenant secret), short-lived, and **single-use** (nonce consumed transactionally) — a client can only perform an upload your backend authorized, within bounds it can't change.
- **Global caps** are inviolable: a master content-type allowlist (no executables), `MAX_UPLOAD_BYTES`, and path-safe categories — enforced regardless of what a request claims.
- **Downloads**: public files served unsigned; private files need a **signed URL** or approval from the tenant's **download-auth callback** (the server forwards the requester's `Authorization`/`Cookie`).
- **Secrets at rest**: tenant signing + webhook secrets are AES-256-GCM encrypted when `MASTER_KEY` is set (required in production).
- **Multi-tenant isolation**: per-tenant keys + secrets; every blob path and signature is tenant-scoped.
- **Webhooks** are HMAC-signed (`x-bytehangar-signature: sha256=…`).
- **Rate limiting / TLS** are delegated to your reverse proxy / ingress (nginx, Cloudflare, API gateway) — run the public plane behind one.

---

## Operations

- `GET /health` — liveness; `GET /ready` — readiness (checks Postgres).
- `GET /metrics` (internal plane) — Prometheus counters (uploads, downloads, bytes, deletes).
- `POST /internal/v1/gc` (admin) — reclaim blobs for soft-deleted files (dedup-safe). Run on a schedule (cron).
- **Graceful shutdown** on SIGINT/SIGTERM drains in-flight requests.
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
