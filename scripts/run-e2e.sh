#!/usr/bin/env bash
# End-to-end harness: build server + SDK, boot the server (self-migrates), wait
# for health, run the SDK integration test, then tear down.
#
# Prereqs: Postgres reachable at DATABASE_URL (e.g. `docker compose up -d`).
# Usage:   bash scripts/run-e2e.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

export DATABASE_URL="${DATABASE_URL:-postgres://bytehangar:bytehangar@localhost:5433/bytehangar}"
export ADMIN_TOKEN="${ADMIN_TOKEN:-e2e-admin-token}"
export PORT="${PORT:-5180}"
export INTERNAL_PORT="${INTERNAL_PORT:-5101}"
export INTERNAL_BIND_ADDRESS="${INTERNAL_BIND_ADDRESS:-127.0.0.1}"
export MASTER_KEY="${MASTER_KEY:-e2e-master-key-change-me}"
export STORAGE_BACKEND="${STORAGE_BACKEND:-local}"
export DATA_ROOT="${DATA_ROOT:-$ROOT/.e2e-data}"

echo "==> building server"
cargo build --manifest-path "$ROOT/server/Cargo.toml"

echo "==> building sdk"
( cd "$ROOT/sdk" && npm install --silent && npm run build --silent )

echo "==> starting server (port $PORT, backend $STORAGE_BACKEND)"
rm -rf "$DATA_ROOT"
"$ROOT/server/target/debug/bytehangar" >"$ROOT/.e2e-server.log" 2>&1 &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true' EXIT

# wait for both planes to be healthy
if ! curl -s --retry 40 --retry-connrefused --retry-delay 1 "http://localhost:$PORT/health" >/dev/null \
   || ! curl -s --retry 40 --retry-connrefused --retry-delay 1 "http://127.0.0.1:$INTERNAL_PORT/health" >/dev/null; then
  echo "server did not become healthy; log:"; cat "$ROOT/.e2e-server.log"; exit 1
fi
echo "==> server healthy (public $PORT, internal $INTERNAL_PORT)"

echo "==> running e2e"
BYTEHANGAR_PUBLIC_URL="http://localhost:$PORT" \
BYTEHANGAR_INTERNAL_URL="http://127.0.0.1:$INTERNAL_PORT" \
ADMIN_TOKEN="$ADMIN_TOKEN" node "$ROOT/scripts/e2e.mjs"
