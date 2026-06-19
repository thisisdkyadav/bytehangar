// End-to-end integration test — dogfoods the SDK against a running server + DB.
//
//   BYTEHANGAR_URL=http://localhost:5180 ADMIN_TOKEN=e2e-admin-token node scripts/e2e.mjs
//
// Exercises: provision (admin) -> catalog (idempotent) -> grant -> upload ->
// single-use enforcement -> signed download round-trip -> dedup -> content-type
// enforcement -> usage -> quota enforcement -> delete. Backend-agnostic (works
// against the local-disk or S3 driver, whichever the server is configured with).

import http from "node:http";

import { ByteHangarServer } from "../sdk/dist/server/index.js";
import { ByteHangarClient } from "../sdk/dist/client/index.js";

const PUBLIC = process.env.BYTEHANGAR_PUBLIC_URL ?? "http://localhost:5180";
const INTERNAL = process.env.BYTEHANGAR_INTERNAL_URL ?? "http://127.0.0.1:5101";
const ADMIN = process.env.ADMIN_TOKEN ?? "e2e-admin-token";

let passed = 0;
let failed = 0;
function check(name, cond) {
  if (cond) {
    passed++;
    console.log(`  ✓ ${name}`);
  } else {
    failed++;
    console.error(`  ✗ ${name}`);
  }
}

// A buffer that `infer` detects as PNG (8-byte signature), made unique via `salt`
// so checksums differ when we want to defeat dedup.
function pngBytes(salt = 0) {
  return Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, salt & 0xff]);
}

async function main() {
  console.log(`ByteHangar e2e — public ${PUBLIC}, internal ${INTERNAL}\n`);

  // --- provision (admin, internal plane) ---
  const admin = new ByteHangarServer({ baseUrl: INTERNAL, apiKey: "", adminToken: ADMIN });
  const tenant = await admin.createTenant(`e2e-${Date.now()}`);
  check("createTenant returns id", typeof tenant.id === "string" && tenant.id.length > 0);
  const created = await admin.createKey(tenant.id, "e2e-key");
  check("createKey returns plaintext key", created.key?.startsWith("bh_"));

  const storage = new ByteHangarServer({ baseUrl: INTERNAL, apiKey: created.key });
  const client = new ByteHangarClient({ baseUrl: PUBLIC });

  // --- plane isolation: internal routes must NOT be on the public port ---
  const probe = await fetch(`${PUBLIC}/internal/v1/usage`);
  check("internal plane not exposed on public port", probe.status === 404);

  // --- catalog (idempotent) ---
  const policies = [
    { key: "img", category: "images", maxSizeBytes: 1024 * 1024, allowContentTypes: ["image/png"] },
    { key: "blob", category: "blobs", maxSizeBytes: 50 * 1024 * 1024, allowContentTypes: [] },
    { key: "pub", category: "public-assets", maxSizeBytes: 1024 * 1024, allowContentTypes: ["image/png"], visibility: "public" },
  ];
  const c1 = await storage.registerCatalog(policies);
  check("registerCatalog changed first time", c1.changed === true && c1.version >= 1);
  const c2 = await storage.registerCatalog(policies);
  check("registerCatalog idempotent (no change)", c2.changed === false && c2.version === c1.version);

  // --- grant + upload ---
  const grant = await storage.createGrant("img");
  check("createGrant returns signed token", grant.token?.startsWith("bh1."));

  const png = pngBytes(1);
  const up = await client.upload(grant.token, new Blob([png], { type: "image/png" }), {
    fileName: "x.png",
  });
  check("upload returns fileRef", typeof up.fileRef === "string" && up.fileRef.length > 0);
  check("upload size matches", up.size === png.length);
  check("upload not deduplicated (first time)", up.deduplicated === false);

  // --- single-use: reusing the same grant must fail ---
  let reuseRejected = false;
  try {
    await client.upload(grant.token, new Blob([png], { type: "image/png" }));
  } catch {
    reuseRejected = true;
  }
  check("grant is single-use (reuse rejected)", reuseRejected);

  // --- signed download round-trips the exact bytes ---
  const signed = await storage.signDownload(up.fileRef);
  const dlUrl = signed.url.startsWith("http") ? signed.url : PUBLIC +signed.url;
  const dl = await fetch(dlUrl);
  const got = Buffer.from(await dl.arrayBuffer());
  check("signed download is 200", dl.status === 200);
  check("downloaded bytes match uploaded", Buffer.compare(got, png) === 0);

  // --- dedup: same bytes under a fresh grant reuse the blob ---
  const grant2 = await storage.createGrant("img");
  const up2 = await client.upload(grant2.token, new Blob([png], { type: "image/png" }), {
    fileName: "y.png",
  });
  check("dedup: identical bytes deduplicated", up2.deduplicated === true);

  // --- content-type enforcement: pdf under a png-only policy is rejected ---
  const grant3 = await storage.createGrant("img");
  let ctRejected = false;
  try {
    await client.upload(grant3.token, new Blob([Buffer.from("%PDF-1.4\n")], { type: "application/pdf" }));
  } catch {
    ctRejected = true;
  }
  check("content-type enforced (pdf rejected under png policy)", ctRejected);

  // --- usage reflects stored objects ---
  const usage = await storage.getUsage();
  check("usage objectCount >= 1", usage.objectCount >= 1 && usage.usedBytes > 0);

  // --- large file: exercises streaming + S3 multipart upload (>5 MiB) ---
  const big = Buffer.alloc(6 * 1024 * 1024);
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).copy(big); // png signature
  const grantBig = await storage.createGrant("blob");
  const upBig = await client.upload(grantBig.token, new Blob([big], { type: "image/png" }), {
    fileName: "big.png",
  });
  check("large upload size matches (6 MiB)", upBig.size === big.length);
  const signedBig = await storage.signDownload(upBig.fileRef);
  const bigUrl = signedBig.url.startsWith("http") ? signedBig.url : PUBLIC +signedBig.url;
  const dlBig = Buffer.from(await (await fetch(bigUrl)).arrayBuffer());
  check(
    "large download byte-exact (6 MiB)",
    dlBig.length === big.length && Buffer.compare(dlBig, big) === 0,
  );

  // --- list endpoints ---
  const allFiles = await storage.listFiles();
  check("listFiles returns live files", allFiles.total >= 3 && allFiles.items.length >= 3);
  const imageFiles = await storage.listFiles({ category: "images" });
  check(
    "listFiles category filter",
    imageFiles.items.length >= 2 && imageFiles.items.every((f) => f.category === "images"),
  );
  const tenantList = await admin.listTenants({ limit: 200 });
  check(
    "listTenants includes our tenant with usage",
    tenantList.items.some((t) => t.id === tenant.id && t.objectCount >= 1),
  );

  // --- visibility: public files served without a signature ---
  const pngPub = pngBytes(7);
  const grantPub = await storage.createGrant("pub");
  const upPub = await client.upload(grantPub.token, new Blob([pngPub], { type: "image/png" }), {
    fileName: "pub.png",
  });
  const pubRes = await fetch(client.fileUrl(tenant.id, upPub.fileRef));
  check("public file served without a signature", pubRes.status === 200);
  check(
    "public file bytes match",
    Buffer.compare(Buffer.from(await pubRes.arrayBuffer()), pngPub) === 0,
  );

  // private file (img policy) without a signature is denied
  const privRes = await fetch(client.fileUrl(tenant.id, up2.fileRef));
  check("private file denied without signature", privRes.status === 401);

  // --- private file via app-callback download auth ---
  const cbServer = http.createServer((req, res) => {
    res.statusCode = req.headers["authorization"] === "Bearer good" ? 200 : 403;
    res.end();
  });
  await new Promise((resolve) => cbServer.listen(0, "127.0.0.1", resolve));
  const cbPort = cbServer.address().port;
  await admin.setDownloadAuthUrl(tenant.id, `http://127.0.0.1:${cbPort}/auth`);

  const grantCb = await storage.createGrant("img");
  const upCb = await client.upload(grantCb.token, new Blob([pngBytes(8)], { type: "image/png" }), {
    fileName: "cb.png",
  });
  const cbUrl = client.fileUrl(tenant.id, upCb.fileRef);
  const okRes = await fetch(cbUrl, { headers: { authorization: "Bearer good" } });
  check("callback authorizes private download (good token)", okRes.status === 200);
  const denyRes = await fetch(cbUrl, { headers: { authorization: "Bearer bad" } });
  check("callback denies private download (bad token)", denyRes.status === 401);
  await admin.setDownloadAuthUrl(tenant.id, null);
  cbServer.close();

  // --- quota enforcement ---
  await admin.setQuota(tenant.id, 1); // 1 byte: any further upload exceeds
  const grantQ = await storage.createGrant("img");
  let quotaRejected = false;
  try {
    await client.upload(grantQ.token, new Blob([pngBytes(2)], { type: "image/png" }));
  } catch (err) {
    quotaRejected = /quota/i.test(String(err.message));
  }
  check("quota enforced", quotaRejected);

  // --- soft delete ---
  await storage.deleteFile(up.fileRef);
  let gone = false;
  try {
    await storage.getFile(up.fileRef);
  } catch {
    gone = true;
  }
  check("deleted file not found", gone);

  // --- GC + dedup refcount safety ---
  // `up` (now deleted) and `up2` (still live) share the same blob via dedup.
  await admin.gc({ olderThanSeconds: 0 });
  const sharedSigned = await storage.signDownload(up2.fileRef);
  const sharedUrl = sharedSigned.url.startsWith("http") ? sharedSigned.url : PUBLIC + sharedSigned.url;
  const sharedRes = await fetch(sharedUrl);
  check("GC keeps a blob still referenced by a live (deduped) file", sharedRes.status === 200);

  // delete the last reference, then GC should reclaim the blob
  await storage.deleteFile(up2.fileRef);
  const gc2 = await admin.gc({ olderThanSeconds: 0 });
  check("GC reclaims the blob after the last reference is deleted", gc2.blobsDeleted >= 1);

  console.log(`\n${passed} passed, ${failed} failed`);
  process.exit(failed ? 1 : 0);
}

main().catch((err) => {
  console.error("\nE2E ERROR:", err);
  process.exit(1);
});
