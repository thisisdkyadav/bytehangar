// ByteHangar quickstart — provision -> catalog -> grant -> upload -> download.
//
// Run the server first:  docker compose up -d   (see README.md)
// Then:                   npm install && node index.mjs
//
// This mirrors the proven end-to-end flow but stays short and commented. It uses
// the PUBLISHED npm package @bytehangar/sdk — no Rust toolchain required.
//
//   ByteHangarServer (server-side)  -> internal plane :5201 (provisioning, grants)
//   ByteHangarClient (browser-side) -> public plane   :5200 (upload, download)

import { ByteHangarServer } from "@bytehangar/sdk/server";
import { ByteHangarClient } from "@bytehangar/sdk/client";

// Match docker-compose.yml: host-mapped ports + the demo ADMIN_TOKEN.
const INTERNAL_URL = "http://localhost:5201"; // internal plane (keep private in prod)
const PUBLIC_URL = "http://localhost:5200"; // public/edge plane (internet-facing)
const ADMIN_TOKEN = "quickstart-admin-token";

function assert(cond, message) {
  if (!cond) throw new Error(`Assertion failed: ${message}`);
}

async function main() {
  console.log("ByteHangar quickstart\n");

  // (a) PROVISION — admin token unlocks the provisioning endpoints. We create a
  //     demo tenant and an API key, then register the tenant's upload policy.
  const admin = new ByteHangarServer({
    baseUrl: INTERNAL_URL,
    apiKey: "", // admin calls don't need a tenant key
    adminToken: ADMIN_TOKEN,
  });

  const tenant = await admin.createTenant(`quickstart-${Date.now()}`);
  console.log(`  provisioned tenant   id=${tenant.id}`);

  const created = await admin.createKey(tenant.id, "quickstart-key");
  assert(created.key?.startsWith("bh_"), "createKey returns a bh_ plaintext key");
  console.log(`  created API key      ${created.key.slice(0, 10)}…`);

  // The "storage" client acts AS the tenant (sends the tenant API key). It mints
  // grants, signs downloads, and does server-to-server file ops.
  const storage = new ByteHangarServer({
    baseUrl: INTERNAL_URL,
    apiKey: created.key,
  });

  // Register the catalog idempotently — call this at app boot. One policy here:
  // PNG images up to 1 MiB.
  const catalog = await storage.registerCatalog([
    {
      key: "image",
      category: "images",
      maxSizeBytes: 1024 * 1024,
      allowContentTypes: ["image/png"],
    },
  ]);
  console.log(`  registered catalog   version=${catalog.version}`);

  // (b) GRANT — mint a short-lived, single-use upload token for ONE upload, with
  //     metadata that gets persisted on the resulting file record.
  const grant = await storage.createGrant("image", {
    metadata: { actorId: "demo-user", entityHint: "quickstart" },
  });
  assert(grant.token?.startsWith("bh1."), "createGrant returns a signed token");
  console.log(`  minted upload grant  ${grant.token.slice(0, 12)}…`);

  // (c) UPLOAD — the browser-side client uploads using ONLY the grant token (no
  //     secrets). We build a tiny in-memory PNG (8-byte signature) so the server's
  //     content-type sniffing accepts it under the png-only policy.
  const client = new ByteHangarClient({ baseUrl: PUBLIC_URL });
  const pngBytes = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x42]);
  const upload = await client.upload(grant.token, new Blob([pngBytes], { type: "image/png" }), {
    fileName: "hello.png",
  });
  assert(typeof upload.fileRef === "string" && upload.fileRef.length > 0, "upload returns a fileRef");
  assert(upload.size === pngBytes.length, "uploaded size matches the input");
  console.log(`  uploaded file        ref=${upload.fileRef} (${upload.size} bytes)`);

  // (d) DOWNLOAD — mint a signed URL for the (private) file and fetch it back,
  //     asserting the bytes round-trip exactly.
  const signed = await storage.signDownload(upload.fileRef);
  const downloadUrl = signed.url.startsWith("http") ? signed.url : PUBLIC_URL + signed.url;
  const res = await fetch(downloadUrl);
  assert(res.status === 200, `signed download returned ${res.status}`);
  const downloaded = new Uint8Array(await res.arrayBuffer());

  const roundTrips =
    downloaded.length === pngBytes.length && downloaded.every((b, i) => b === pngBytes[i]);
  assert(roundTrips, "downloaded bytes match the uploaded bytes");

  // (e) SUCCESS.
  console.log(`  downloaded ${downloaded.length} bytes — byte-for-byte match\n`);
  console.log("SUCCESS — provision -> grant -> upload -> download round-trip works.");
}

main().catch((err) => {
  console.error("\nQUICKSTART FAILED:", err.message ?? err);
  console.error("\nIs the server up?  docker compose ps   (expect bytehangar healthy)");
  process.exit(1);
});
