# ByteHangar — Plan

> Open-source, self-hostable file-storage product + SDK. Think "UploadThing / Cloudinary, but open-source, self-hosted, and pluggable on your own backend." Rust core, Postgres metadata, pluggable blob backends (local disk or S3-compatible), an isomorphic TypeScript SDK.

Status: **planning → scaffolding**. This document is the single source of truth for decisions. Update it as decisions change.

---

## 1. Vision & positioning

- A complete, end-to-end storage system you can **run on your own server** — not tied to any vendor cloud.
- The differentiator vs existing OSS: not another object store (MinIO/SeaweedFS/Garage already do bytes). ByteHangar is the **control plane + SDK** that sits on top: typed upload policies, signed grants, multi-tenancy, usage metering, and a great developer SDK. It can use local disk **or** S3-compatible storage as the byte backend.
- **Backend is always in the path.** Unlike pure presigned-direct-to-S3, the ByteHangar server always mediates uploads/downloads (even with the S3 backend). This is a deliberate choice for uniform validation/scanning/transforms and a single security model. Trade-off: the server carries bandwidth, so the data plane must stream efficiently (Rust's strength).
- First tenant: **HMS** (the hostel-management system). ByteHangar lives in its **own standalone repo** (`/home/devesh/code/bytehangar`), consumed by HMS like any other tenant.

### Prior art (integrate, don't rebuild)
- Blob stores (use as backends): **MinIO**, SeaweedFS, Garage, AWS S3, Cloudflare R2, Backblaze B2.
- Upload/resumable: **tus protocol** (adopt for resumable uploads in v2 rather than inventing).
- Closed competitors we're an OSS alternative to: UploadThing, Cloudinary, Supabase Storage.

---

## 2. Non-goals (core stays a primitive)

- **No billing/pricing/invoicing in core.** Billing is a *separate wrapper* that consumes ByteHangar as a tenant (see §9). Core only emits usage events + enforces a configured quota number.
- No reactive/real-time DB features (this is why we did **not** pick Convex — it's an app platform, not an infra metadata store).
- Not a CDN (but downloads can later redirect to a CDN/S3 signed URL for read scale).
- Not a general media-processing suite on day one (hooks designed in; transforms come later).

---

## 3. Core architecture — the spine

Two independent storage concerns, **each behind a driver interface**:

### 3.1 Metadata store → Postgres
File records, tenants, API keys/secrets, the policy catalog, usage counters/events, audit, upload-session/nonce state. Small rows, transactional, queried constantly. **Postgres** (see §11 for the Mongo-vs-Postgres rationale). Optional embedded **SQLite** driver later for trivial single-node self-host.

### 3.2 Blob backend → pluggable driver
```
trait BlobBackend {
    put(key, stream) -> ()        // streaming write
    get(key) -> stream            // streaming read
    delete(key) -> ()
    stat(key) -> BlobStat
}
impls: LocalDisk | S3Compatible (S3, MinIO, R2, B2)
```
- Self-hosters configure `LocalDisk` or point at their own MinIO.
- The SaaS configures `S3`.
- **Same metadata layer, same grant/catalog logic, same SDK — only the driver changes.** This is the single most important abstraction in the product; everything else is backend-agnostic.
- Because the backend is always in the path, the client SDK always uploads to **our** edge; the driver decides where bytes land. (Optional future: download path may return a signed-S3/CDN redirect for read scale while uploads still proxy.)

### 3.3 Control plane vs data plane
- **Control plane** (Postgres-backed): register catalog, mint grants, manage tenants/keys, validate, audit, emit usage. CPU-light, consistency-heavy.
- **Data plane** (blob-backed): byte ingress/egress. Bandwidth-heavy, stateless.
- They scale independently. With the S3 backend the data plane is light; with local disk it carries the bytes.

---

## 4. The request model — Catalog → Grant → Enforcement

Three layers; together they make "no possibility of misuse" actually true:

1. **Catalog** (boot-time, app → server, persisted): the SDK registers a tenant's policy set — categories, size limits, allowed content types. Idempotent upsert on every app boot. Stored per tenant in Postgres.
2. **Grant** (per-upload, app backend → client, signed): when a user wants to upload, the *app's* backend checks *its own* authz, picks a policy, and asks ByteHangar to mint a **short-lived signed upload token** scoped to `{tenant, category, max_size, content_type, expiry, nonce}`. The client receives only this grant — never keys, never raw config.
3. **Enforcement** (server): on upload, verify the grant signature + single-use nonce, then enforce `grant ∩ catalog policy ∩ global hard caps`. Trust nothing from the client except the unforgeable token.

Downloads mirror this: signed, time-limited download URLs (HMAC), already the proven pattern.

---

## 5. Multi-tenancy (first-class, not cosmetic)

Single instance serves many apps. Isolation is mandatory and by construction:
- **Per-tenant credentials:** each tenant gets its own **API key** (server-to-server) *and* its own **signing secret**. Never one global key. Tenant A cannot mint grants or sign URLs valid for Tenant B.
- **Tenant in every path and every signature:** disk/S3 key = `{tenant}/{category}/{yyyy}/{mm}/{shard}/{file_id}.{ext}`; `tenant_id` on every metadata row; signatures use the tenant's secret → cross-tenant forgery impossible.
- **Catalog + grants + usage all keyed by tenant.**
- Default isolation: shared Postgres (tenant_id column + row scoping) + shared blob backend (tenant-prefixed keys). Per-tenant bucket/DB only if a tenant demands hard isolation.
- **Quotas:** per-tenant `limit` enforced inline at upload (see §9).

---

## 6. Security model

- **Inviolable, caller-independent caps** (always hold, protect the server): global `MAX_UPLOAD_BYTES`, master content-type allowlist (no executables ever), path-component sanitization (`^[a-z0-9-]+$` for category; reject `..`/slashes), magic-byte sniffing (`infer`).
- **Delegated, caller-supplied limits** (per-category size/type): taken on trust from an authenticated tenant, can only ever be *stricter* than global caps.
- **Grants:** HMAC-signed (tenant secret), short TTL, **single-use nonce** (consumed transactionally in Postgres to prevent replay).
- **Two route planes** (see §8): internal (key-auth) vs public/edge (grant/URL-signature-auth, CORS-enabled).
- **API keys** stored hashed (argon2/bcrypt); secrets shown once on creation.
- Optional later: virus scan (ClamAV) and EXIF strip as inline data-plane steps; per-file private/public ACL with app-callback authorization for downloads.

---

## 7. Tech stack

| Concern | Choice | Notes |
|---|---|---|
| Server language | **Rust** | Best-in-class streaming, safety, single binary, great self-host story; ideal since backend is always in byte path |
| HTTP | **Axum + Tokio + Tower** | Streaming bodies, middleware, backpressure |
| Metadata DB | **Postgres** | via `sqlx` (async, no ORM). Runtime queries first; `sqlx` offline/macros once schema stabilizes |
| Blob backends | **LocalDisk** + **S3-compatible** | S3 via `aws-sdk-s3` (works with MinIO/R2/B2 via endpoint override) |
| Cache (optional) | **Redis** | Hot catalog + file-record cache, nonce/rate-limit; not required for single-node |
| Hashing | `sha2` (checksums, content-address dedup), `argon2` (api keys) | |
| Signing | `hmac` + `sha2` | grants + download URLs |
| MIME sniff | `infer` | magic-byte content-type detection |
| IDs | `uuid` v7 (time-sortable) | file_id, tenant_id, etc. |
| Migrations | `sqlx migrate` (SQL files) | |
| SDK | **TypeScript** | `@bytehangar/server` (Node) + `@bytehangar/client` (browser); framework adapters later |
| Dev infra | docker-compose: Postgres + MinIO | local dev/test |

---

## 8. Route exposure (internal vs public)

Two route classes; exposure controlled at the **network layer**, not via VM-topology logic:

- **Internal** (`/internal/v1/*`): register catalog, mint grants, server-side fetch/delete, tenant/key admin. Auth: tenant API key (`x-bytehangar-key`) or admin key.
- **Public/edge** (`/v1/*`): direct upload with a grant token, signed download. Internet-facing, CORS-enabled, token/signature-auth.

**Deployment model:** two listeners — an **internal port** (bind to localhost/private interface when co-located; private network when split across VMs) and a **public port** (bind `0.0.0.0`, edge routes only). Co-located → don't expose internal port. Split VMs → internal port on a private network, still key-protected. App stays topology-agnostic; a reverse proxy/ingress handles the rest.

---

## 9. Billing seam (mechanism vs policy)

Billing is a **separate wrapper** (closed/SaaS), a *consumer* of ByteHangar like any tenant. The seam:

```
ByteHangar  --usage events-->  Billing wrapper     (what happened: tenant, op, bytes, count, ts)
Billing wrapper  --set quota-->  ByteHangar         (what's allowed: per-tenant limit number)
ByteHangar enforces the number inline; never knows price/plan
```

- **Core owns:** append-only `usage_events` + per-tenant `usage_counters` (live totals, needed locally for inline quota enforcement) + quota **enforcement**.
- **Billing wrapper owns:** plans, pricing, metering→invoice, payments, dashboards, durable billing ledger; and **sets** each tenant's quota via the admin API based on plan.
- Enforcement is local (can't call billing on every upload). Core keeps *enough* usage to enforce; wrapper keeps *billing-grade* history by consuming the event stream.
- This keeps one core serving HMS (no billing), self-hosters (no billing), and the SaaS (billing wrapper) with no forks.

---

## 10. SDK design (isomorphic TypeScript)

One product, **two entry points**, and the line between them is a **security boundary**:

- `@bytehangar/server` (Node): holds the tenant key + signing secret; registers the catalog at boot; mints grants; server-side fetch/delete.
- `@bytehangar/client` (browser): **zero secrets**; takes a grant from your backend; performs the direct upload to the edge; builds/uses signed download URLs.

**Hard rule:** the client export must not transitively import anything holding a secret, or bundlers leak it. Shared *types* and *policy names* may be common; secrets never. Framework adapters (React `<UploadButton>`/hooks, Next.js route handler, Express middleware) grow on top of these two.

---

## 11. Database choice rationale (Mongo vs Postgres)

Decision: **Postgres.** Reasoning (DB stores *metadata*, not bytes):
- **Point reads** (file by id/ref): tie — both sub-ms, index-bound; network/serialization dominate.
- **Transactional integrity** (nonce single-use, atomic quota counter + enforce, catalog version swaps): **Postgres native + cheap**; Mongo's multi-doc txns are heavier and discouraged → would force correctness-vs-speed trade-offs exactly where correctness is the feature.
- **Relational tenancy** (tenant→keys→files→versions→usage→audit, joins): native to Postgres; Mongo forces denormalization / slow `$lookup`.
- **Flexible metadata:** Postgres **JSONB** matches Mongo's flexibility without losing relational power.
- Engine is **not the bottleneck** at this scale anyway (blob I/O + network dominate; hot reads cached in Redis). Choose on correctness/modeling → Postgres.
- Scale headroom: partition `files` by tenant/time from the start; vertical first, then Citus/partitioning if a SaaS tenant gets huge.

---

## 12. Postgres schema (initial)

```
tenants(id pk, name, status, signing_secret_enc, quota_bytes, used_bytes, created_at)
api_keys(id pk, tenant_id fk, name, key_hash, role[admin|app], last_used_at, created_at, revoked_at)
policies(id pk, tenant_id fk, key, category, max_size_bytes, allow_content_types[], created_at, UNIQUE(tenant_id,key))
catalog_versions(tenant_id fk, version, hash, applied_at)         -- idempotent boot registration
files(id pk uuid, tenant_id fk, file_ref unique, policy_key, category, original_name,
      stored_key, content_type, size_bytes, checksum_sha256, actor_id, actor_role,
      source_service, entity_hint, created_at, deleted_at)         -- soft delete
file_versions(id pk, file_id fk, version, stored_key, size_bytes, checksum, created_at)  -- v2
upload_grants(nonce pk, tenant_id fk, policy_key, max_size, content_type, expires_at, consumed_at)  -- single-use
usage_events(id pk, tenant_id fk, op[upload|egress|delete], bytes, count, created_at)  -- append-only
usage_counters(tenant_id pk, used_bytes, object_count, updated_at)  -- live, for enforcement
audit_log(id pk, tenant_id fk, actor, action, target, before jsonb, after jsonb, created_at)
```
Notes: content-addressable dedup via `checksum_sha256` (identical bytes stored once, refcounted — v2). `stored_key` = blob backend key.

---

## 13. API surface (initial)

**Internal (`/internal/v1`, key-auth):**
- `PUT  /catalog` — register/replace tenant policy catalog (idempotent, versioned)
- `POST /grants` — mint a signed upload grant
- `GET  /files/:file_ref` — metadata
- `GET  /files/:file_ref/content` — server-side stream (S2S)
- `POST /files/:file_ref/sign` — mint signed download URL
- `DELETE /files/:file_ref` — soft delete
- `GET  /usage` — tenant usage (for billing wrapper)
- admin: `POST /tenants`, `POST /tenants/:id/keys`, `PATCH /tenants/:id/quota`

**Public/edge (`/v1`, grant/signature-auth, CORS):**
- `POST /upload` — direct browser upload with a grant token (multipart/stream)
- `GET  /files/:file_ref` — download via signed URL
- `GET  /health`

---

## 14. Grant token format

Compact signed token (HMAC-SHA256 with tenant secret), e.g. `bh1.<base64url(payload)>.<sig>`:
```json
{ "t": "<tenant_id>", "p": "<policy_key>", "cat": "<category>",
  "max": 1048576, "ct": ["image/png","image/jpeg"],
  "n": "<nonce-uuid>", "exp": 1718900000 }
```
Server verifies sig → checks `exp` → consumes `n` transactionally (reject if already consumed) → enforces `max`/`ct` ∩ catalog ∩ global caps during the streamed upload.

---

## 15. Roadmap / phasing

**Phase 1 — Foundations (MVP core):**
- Cargo scaffold, config, error envelope, HTTP skeleton (internal + public planes), health.
- Postgres schema + migrations; `BlobBackend` trait + **LocalDisk** + **S3** drivers.
- Tenants/keys, catalog registration, grant mint+verify (nonce single-use), streamed upload with enforcement, signed download, soft delete.
- Content-addressed dedup, usage events + counters + inline quota enforcement.
- docker-compose (Postgres + MinIO).

**Phase 2 — SDK + DX:**
- `@bytehangar/server` + `@bytehangar/client` (secret boundary), one React upload component, one server handler. → usable open-source UploadThing.

**Phase 3 — Scale/UX:**
- tus resumable uploads, private files + app-callback download auth, admin dashboard, webhooks, Prometheus metrics, hard-delete GC/orphan reconcile, file versioning.

**Phase 4 — SaaS:**
- Hosted control plane on S3/R2; billing wrapper consuming usage events; per-tenant onboarding.

---

## 16. Repo structure (target)

```
bytehangar/
├── plan.md
├── README.md
├── LICENSE                 # OSS (Apache-2.0 or MIT — TBD)
├── docker-compose.yml      # postgres + minio (dev)
├── server/                 # Rust core
│   ├── Cargo.toml
│   ├── migrations/         # sqlx SQL migrations
│   └── src/
│       ├── main.rs
│       ├── config.rs
│       ├── error.rs
│       ├── http/           # router, internal/, public/, middleware
│       ├── domain/         # models, dtos
│       ├── catalog/  grants/  files/  tenants/  usage/   # modules (capability-first)
│       ├── blob/           # trait + local + s3
│       ├── db/             # pool, repositories
│       └── crypto/         # signing, hashing
└── sdk/                    # TypeScript (Phase 2)
    ├── package.json        # @bytehangar/* (workspaces)
    ├── server/             # @bytehangar/server
    ├── client/             # @bytehangar/client
    └── shared/             # shared types/policy schema
```

---

## 17. Decisions log (locked)

- **Product:** open-source, self-hostable storage system + SDK ("open-source UploadThing"). New standalone repo; HMS is first tenant.
- **Name:** ByteHangar. Crate `bytehangar`; npm `@bytehangar/server`, `@bytehangar/client`.
- **Server language:** Rust (Axum/Tokio).
- **Metadata DB:** Postgres (SQLite embedded driver optional later). Not Mongo, not Convex.
- **Blob backends:** Local **and** S3-compatible, both **backend-mediated** (server always in path; no pure client→S3 presign in core).
- **Multi-tenant:** yes, first-class (per-tenant key + secret, path/signature scoping, quotas).
- **Request model:** Catalog (boot) → Grant (per-upload, signed, single-use) → Enforcement (server).
- **Billing:** out of core; a separate wrapper/consumer. Core emits usage events + enforces a configured quota.
- **Route planes:** internal (key) vs public/edge (grant/URL signature); exposure at network layer (two listeners).
- **SDK:** isomorphic TS, hard secret boundary between server/client entry points.

---

## 18. Open questions / deferred

- License: Apache-2.0 vs MIT (lean Apache-2.0 for patent grant).
- Redis required vs optional for single-node (lean optional; in-process cache fallback).
- Download scaling: when (if) to allow signed-S3/CDN redirect for reads despite "backend in path".
- Dedup refcounting + hard-delete GC semantics (Phase 3).
- tus integration approach (adopt library vs implement subset).
- Per-tenant hard isolation tier (bucket/DB-per-tenant) — only on demand.
