# Contributing to ByteHangar

Thanks for your interest in contributing! ByteHangar is a Rust core server plus an isomorphic TypeScript SDK. This guide covers the commands you need to build, test, and validate a change locally so it lands green in CI.

## Prerequisites

- **Rust** — the toolchain is pinned in [`rust-toolchain.toml`](./rust-toolchain.toml) to **1.94** (with `clippy` and `rustfmt`). If you use `rustup`, the correct toolchain is selected automatically.
- **Node.js 20+** and npm — for the SDK.
- **Docker + Docker Compose** — the end-to-end harness needs a local Postgres and MinIO, started via `docker compose`.

## Server (Rust)

All server commands run from the `server/` directory:

```bash
cd server

# Unit tests (no database required)
cargo test

# Lint — warnings are errors, matching CI
cargo clippy --all-targets -- -D warnings

# Optional: keep formatting clean
cargo fmt
```

## SDK (TypeScript)

```bash
cd sdk
npm ci
npm run build          # tsc — typechecks and emits dist/
```

## End-to-end harness

The e2e script provisions a tenant and runs the full flow (provision → catalog → grant → upload → download → dedup → GC) against a real server and database. Bring up the dependencies first:

```bash
# From the repo root: start Postgres + MinIO (host ports 5433 / 9100)
docker compose up -d --wait

# Local-disk backend
bash scripts/run-e2e.sh

# S3 / MinIO backend
STORAGE_BACKEND=s3 S3_ENDPOINT=http://localhost:9100 S3_BUCKET=bytehangar \
  S3_ACCESS_KEY_ID=bytehangar S3_SECRET_ACCESS_KEY=bytehangar-secret \
  S3_FORCE_PATH_STYLE=true PORT=5181 bash scripts/run-e2e.sh
```

> If the MinIO bucket does not exist yet, create it once (see the `bytehangar` bucket setup in [`.github/workflows/ci.yml`](./.github/workflows/ci.yml)).

## CI

Every push and pull request runs, via [GitHub Actions](./.github/workflows/ci.yml):

- `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo build --release` (in `server/`)
- the SDK build (`npm ci && npm run build` in `sdk/`)
- the e2e harness on **both** the local-disk and S3 backends
- `cargo audit` for advisories (non-blocking)

Please make sure clippy, tests, and the SDK build pass locally before opening a PR.

## Pull requests

- Keep PRs focused; one logical change per PR.
- Write clear, conventional-ish commit messages (e.g. `fix: …`, `feat: …`, `docs: …`).
- Update docs (`README.md`, `sdk/README.md`, env tables) when behavior or configuration changes.
- Make sure CI is green — clippy, tests, SDK build, and e2e.
- Fill out the [pull request template](./.github/PULL_REQUEST_TEMPLATE.md), including the testing you ran.

> A `Co-Authored-By` trailer is **not** required for external contributions — feel free to omit it.

## Reporting security issues

Do **not** report vulnerabilities through public issues or PRs. See [SECURITY.md](./SECURITY.md) for private disclosure instructions.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](./LICENSE).
