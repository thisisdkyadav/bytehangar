# ByteHangar Roadmap (post-v1.0.0)

> Built from a 5-dimension audit (product, security, scale, DX, code-quality) of the
> shipped v1.0.0 codebase. Deduped to the items below. Effort: S/M/L/XL. Each
> milestone is independently shippable.

## Where v1.0.0 stands
A security-first "open-source UploadThing": catalog→grant→enforcement uploads, streaming
Rust core, local + S3 backends, multi-tenant (encrypted secrets, quotas, usage), dedup,
public/private + callback download auth, dedup-safe GC, durable signed webhooks, list/admin
+ key lifecycle, `/health` `/ready` `/metrics`, request IDs, rate limiting, graceful
shutdown, CI, 22 unit + 37 e2e checks, isomorphic SDK. **Strong control plane; thin on
media features and a few real security/ops gaps.**

---

## 🔴 Security follow-ups — do first (the repo is already public)

- **SSRF on webhook + download-auth callback URLs** [M] — the server makes outbound requests
  to fully tenant-controlled URLs (`webhooks` worker, `files::authorize_via_callback`) with no
  guard. A tenant can point them at `169.254.169.254` (cloud metadata), `localhost`, or private
  CIDRs to exfiltrate creds / pivot. **Fix:** resolve + block private/link-local/loopback ranges,
  restrict scheme to https (allow http only in dev), and disable/limit redirects.
- **Admin-token compare + strength** [S] — compare in constant time; reject weak/short
  `ADMIN_TOKEN` at boot in production.
- **Enforce tenant `suspended` status on the edge** [S] — currently only the internal plane
  checks status; a suspended tenant can still upload/download.
- **Sign the download `disposition`** [S] — it's appended to the signed URL unsigned, so it's
  tamperable (low impact, but it's in the signed surface).

## v1.1 — Harden & unblock (mostly S/M; the credibility release)

**Ops / reliability**
- **Automated internal GC + grant/tombstone pruning scheduler** [M] — GC is a manual endpoint
  today; self-hosters shouldn't need an external cron. Add an internal interval task.
- **Hardened, configurable DB pool** [S] — expose max/min/acquire-timeout; tune for the worker + edge.
- **Config validation on boot** [M] — fail fast on bad S3 creds / unreachable DB / port collisions
  instead of lazy failures.
- **Graceful drain of the webhook worker** [M] — on SIGTERM, finish in-flight deliveries.
- **Dockerfile hardening** [S] — non-root user, `HEALTHCHECK`, `.dockerignore`.
- **Audit-log writes** [M] — the `audit_log` table exists but nothing writes to it; log all
  admin/provisioning actions.

**Quality / CI**
- **DB-backed integration tests** [L] — worker delivery/retry, GC dedup-safety, quota counters,
  nonce single-use, rate-limit deny + 413 + expired-grant. (Only happy-path e2e today.)
- **GC concurrency safety** [M] — make the live-check-then-delete transactional / single-flight.
- **Release automation** [M] — publish the server image to GHCR on tag; `cargo-audit` + dependabot
  + coverage in CI; pin `rust-toolchain.toml`.

**Product unblocks (cheap, high-value)**
- **Configurable content-type allowlist** [S] — the hardcoded 5-type ceiling silently 403s video,
  audio, office docs, archives. Make it config (safe default + inviolable executable denylist).
- **App-supplied upload metadata in the grant** [M] — `actor_id`/`entity_hint`/`source_service`
  columns exist but are never written; let the grant carry metadata so apps can attribute + filter.
- **Soft-delete restore (undelete)** [S] — schema is soft-delete but there's no restore endpoint.
- **Cache-Control / ETag / conditional GET on downloads** [S] — CDN-friendliness, cheap.

**DX**
- **Publish `@bytehangar/sdk` to npm** [S] — publish CI job, `prepublishOnly` guard, scope/org.

## v1.2 — Media & lifecycle (the headline differentiators)

- **On-the-fly image transforms / thumbnails** [L] — `?w=&h=&fit=&fmt=&q=`, lazily rendered +
  content-addressed cached as sibling blobs, transform presets whitelisted per policy (render-DoS
  guard). This is *the* reason teams pick Cloudinary; the blob/dedup/signed-URL plumbing already fits.
- **File-level TTL / expiry + sweep** [M] — `files.expires_at` (+ per-policy default), purged by the
  GC sweep; emit `file.expired`. Pairs with restore.
- **Bulk operations** [M] — batch delete / sign / metadata (SDK loops one-by-one today).
- **Full file search** [M] — filename / content-type / size / date / metadata (only category filter today).
- **Copy / move / rename** [M].
- **Virus-scan (ClamAV) + content-moderation hooks** [L] — inline or post-upload pipeline.

## v2.0 — Scale & resumable

- **Resumable / large-file uploads (tus subset)** [XL] — single multipart POST loses a 2 GB upload on
  a dropped connection and wastes the grant nonce; the S3 backend already does internal multipart but
  exposes none of it. Add `upload_sessions` keyed by the grant nonce; SDK auto-switches above a threshold.
- **Opt-in presigned direct-to-S3 (up/down)** [L] — the throughput/cost ceiling at scale; offer as a mode.
- **Signed-S3 / CDN redirect downloads** [L] — read-scale.
- **Explicit dedup refcount table** [L] — replace recomputed checksum queries; needed before huge tenants.
- **Partition `files` by tenant/time** [L] (and add the planned list-path indexes now).
- **Deeper observability** [M] — latency histograms, error rates, queue-depth/in-flight gauges, OTel export.
- **Move egress metering + tenant lookup off the download hot path** [M].
- **Admin dashboard** [L] · **OpenAPI 3 spec** [M] · **Framework adapters (Next/Express/Fastify)** [M]
  · **`bytehangar` CLI** [M] · **second-language SDK (Go/Python)** [L].

## Backlog (P3)
Per-key pepper for API-key hashing · reduce internal-error message leakage · blob-content
encryption-at-rest (or document it isn't) · drop declared-CT fallback on sniff failure / block SVG
polyglots · master-key rotation + re-encrypt · hard-delete / GDPR right-to-erasure endpoint ·
machine-readable error `code` taxonomy · React drag-and-drop dropzone · quickstart example apps.

---

## Recommended sequence
1. **Security follow-ups** (SSRF first) — small, and the repo is live.
2. **v1.1** — turns a "works" product into a "trustworthy, operable" one; nearly all S/M.
3. **v1.2 image transforms** — the single biggest adoption lever.
4. **v2.0 resumable uploads** — the biggest reliability gap, but XL; do after the base is hardened.
