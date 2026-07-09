# Operating ByteHangar

A runbook for self-hosters: backup/restore, the durability model, and routine
maintenance (GC, reconcile, key rotation).

## The durability model (read this first)

ByteHangar has **two stores**, and **Postgres is the source of truth**:

- **Postgres** — all metadata: tenants, policies, `files`, `file_variants`, usage,
  webhook deliveries, audit log. A file *exists* iff it has a row here.
- **Blob store** (local disk or S3) — the raw bytes, addressed by `stored_key`.

An upload **commits the blob first, then the DB row** (in a transaction). So the only
inconsistency a crash can produce is an **orphan blob**: bytes with no DB row. It never
produces the reverse (a DB row with no bytes) for a successful upload. Orphans are
harmless (they just waste space) and are cleaned by `reconcile` (below).

Implication for backups: **the blob store may be slightly *ahead* of the DB (extra
orphan blobs), but must never be *behind*** — a DB row pointing at a missing blob makes
that file return `404`/`410` forever. So: **back up the DB at least as frequently as the
blob store, and restore the blob store to a point at or after the DB snapshot.**

## Backup

- **Postgres**: `pg_dump` on a schedule, or continuous archiving (WAL-G / pgBackRest)
  for point-in-time recovery. This is the critical backup — losing it loses the catalog.
- **Blob store**:
  - *S3*: enable **bucket versioning** + optionally cross-region replication. That's your
    blob backup; no separate job needed.
  - *Local disk*: snapshot or `rsync` the `DATA_ROOT` directory. It only ever appends
    new keys and deletes reclaimed ones, so an incremental copy is cheap. Skip `*.part`
    files (in-progress writes).

## Restore (disaster recovery)

1. **Restore Postgres first** (it's the source of truth).
2. **Restore the blob store** to a snapshot **at or after** the DB snapshot's time. If
   the blob store is older than the DB, some files will reference missing bytes.
3. Point the server at both, start it (it auto-migrates — a no-op on a restored DB).
4. **Reconcile** to remove orphan blobs the DB no longer knows about (the
   `Content-Type: application/json` header is **required** — without it the body is
   ignored and `dry_run` silently defaults to `false`, i.e. it deletes):
   ```bash
   # preview first
   curl -XPOST $INTERNAL/internal/v1/reconcile \
        -H "x-bytehangar-admin: $ADMIN_TOKEN" -H "Content-Type: application/json" \
        -d '{"dry_run":true}'
   # then delete
   curl -XPOST $INTERNAL/internal/v1/reconcile \
        -H "x-bytehangar-admin: $ADMIN_TOKEN" -H "Content-Type: application/json" \
        -d '{}'
   ```
5. Spot-check: any DB row whose blob is missing (DB ahead of blobs) will `404` on
   download — that file is unrecoverable without an older-enough blob backup. There is no
   automatic detection for this; it's why step 2's ordering matters.

## Routine maintenance

### Garbage collection (soft-deleted files)
Deletes are soft (tombstoned) and held for `GC_RETENTION_SECONDS` (default 24h) so they
can be restored. GC reclaims the blob once no live file references it (dedup-safe) and
purges tombstones past the retention window — including their rendered image variants.
- **Automatic**: set `GC_INTERVAL_SECONDS>0` (built-in scheduler, single-flight).
- **Manual/cron**: `POST /internal/v1/gc` (optionally `{"older_than_seconds":N}`).

### Reconcile (orphan blobs)
`reconcile` lists the blob store and deletes any blob no `files`/`file_variants` row
references — the crash-mid-upload leftovers GC can't see. Run it occasionally (e.g.
weekly) and always after a restore. It's a full store listing, so it's heavier than GC
— schedule it off-peak, not every minute.

> ⚠️ **The blob store (S3 bucket / `DATA_ROOT`) must be DEDICATED to this ByteHangar
> instance.** reconcile treats *every* unreferenced object as an orphan, so pointing it
> at a bucket shared with other applications will delete their objects. Give ByteHangar
> its own bucket/prefix.

It is safe to run against a live server: two guards protect concurrent uploads — a
`grace_seconds` window (default 3600, floored at 60) that skips recently-written blobs,
and a per-key DB re-check immediately before each delete. Prefer the default grace.

### Master key rotation
Tenant signing/webhook secrets are encrypted at rest with `MASTER_KEY` (AES-256-GCM).
To rotate without downtime:
1. Deploy with the **new** key in `MASTER_KEY` and the **old** key in
   `MASTER_KEY_PREVIOUS` (both are accepted for decrypt; new writes use the new key).
2. Re-encrypt every tenant's secrets under the new key:
   ```bash
   curl -XPOST $INTERNAL/internal/v1/admin/rotate-secrets -H "x-bytehangar-admin: $ADMIN_TOKEN"
   ```
3. Once it succeeds for all tenants, deploy again with `MASTER_KEY_PREVIOUS` removed.

## Monitoring

- `GET /health` — liveness. `GET /ready` — readiness (checks Postgres).
- `GET /metrics` (internal plane) — Prometheus counters (uploads, downloads, bytes,
  deletes). Alert on `/ready` failing and on error-rate spikes.
- The `audit_log` table records admin/provisioning actions (tenant/key/quota/webhook/
  download-auth/secret-rotation) for traceability.

## Two planes — keep the internal one private

The **public plane** (`PORT`, default 5100) serves browser uploads + signed downloads and
is internet-facing. The **internal plane** (`INTERNAL_PORT`, default 5101) carries
provisioning, grants, catalog, GC/reconcile/rotate, and S2S — it must **not** be exposed
publicly. Restrict it at the network/orchestration layer.
