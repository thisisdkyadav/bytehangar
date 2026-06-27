# ByteHangar quickstart (< 5 minutes)

Run a real ByteHangar server and exercise the full flow — **provision → grant →
upload → download** — using only the published [`@bytehangar/sdk`](https://www.npmjs.com/package/@bytehangar/sdk).

**No Rust toolchain needed.** You need:

- Docker + Docker Compose
- Node.js 18+ (for the built-in `fetch` / `Blob`)

The server runs from the prebuilt image `ghcr.io/thisisdkyadav/bytehangar:latest`
with the **local-disk** storage backend, so there's no MinIO/S3 to set up. Ports
are non-default (Postgres `5434`, public `5200`, internal `5201`) to avoid clashing
with other local services.

---

## 1. Start the server

```bash
docker compose up -d
```

This brings up Postgres and the ByteHangar server (which auto-migrates the DB on
boot). Wait until the server reports healthy:

```bash
# Either watch the health status...
docker compose ps

# ...or poll the public health endpoint until it answers:
until curl -fsS http://localhost:5200/health >/dev/null 2>&1; do sleep 1; done
echo "server is up"
```

## 2. Install the SDK

```bash
npm install
```

## 3. Run the quickstart

```bash
node index.mjs
```

## What success looks like

```
ByteHangar quickstart

  provisioned tenant   id=…
  created API key      bh_xxxxxx…
  registered catalog   version=1
  minted upload grant  bh1.xxxxxxx…
  uploaded file        ref=… (9 bytes)
  downloaded 9 bytes — byte-for-byte match

SUCCESS — provision -> grant -> upload -> download round-trip works.
```

The script ends with a non-zero exit code if anything fails, so it's safe to use
in CI as a smoke test.

---

## How it maps to the two planes

| SDK | Talks to | Host URL | Used for |
|---|---|---|---|
| `ByteHangarServer` | internal plane | `http://localhost:5201` | provisioning, catalog, grants, signed downloads (holds secrets — server-side only) |
| `ByteHangarClient` | public/edge plane | `http://localhost:5200` | uploading with a grant, fetching downloads (no secrets) |

In a real deployment the **internal** plane stays private (never internet-facing)
and only the **public** plane is exposed.

## Tear down

```bash
# Stop the containers but keep the data:
docker compose down

# Stop and delete the Postgres + blob volumes too (full reset):
docker compose down -v
```

## Troubleshooting

- **`node index.mjs` fails to connect** — the server may still be starting. Re-run
  the health poll in step 1, then try again.
- **A host port is already in use** — edit the `ports:` mappings in
  `docker-compose.yml` (left side = host port) and update the matching URLs at the
  top of `index.mjs`.
- **Inspect logs** — `docker compose logs bytehangar`.
