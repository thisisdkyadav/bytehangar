# ByteHangar

Open-source, self-hostable file-storage product + SDK — an open-source alternative to UploadThing/Cloudinary that runs on **your** server, with **your** choice of byte backend (local disk or any S3-compatible store).

- **Rust** core (Axum/Tokio) — backend always in the byte path, streaming-first.
- **Postgres** metadata store.
- **Pluggable blob backends:** local disk or S3-compatible (S3, MinIO, R2, B2).
- **Multi-tenant** with per-tenant keys/secrets, typed upload policies, signed grants, usage metering.
- **Isomorphic TypeScript SDK** (`@bytehangar/server` + `@bytehangar/client`).

See [plan.md](./plan.md) for the full architecture and decisions.

## Status

Early scaffold (Phase 1 — foundations). Not yet usable.

## Dev quickstart

```bash
# bring up Postgres + MinIO
docker compose up -d

# run the server (serves /health without a DB; lazy-connects on first query)
cargo run --manifest-path server/Cargo.toml
```

License: TBD (leaning Apache-2.0).
