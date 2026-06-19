// End-to-end integration test — dogfoods the SDK against a running server + DB.
//
//   BYTEHANGAR_URL=http://localhost:5180 ADMIN_TOKEN=e2e-admin-token node scripts/e2e.mjs
//
// Exercises: provision (admin) -> catalog (idempotent) -> grant -> upload ->
// single-use enforcement -> signed download round-trip -> dedup -> content-type
// enforcement -> usage -> quota enforcement -> delete. Backend-agnostic (works
// against the local-disk or S3 driver, whichever the server is configured with).

import { ByteHangarServer } from "../sdk/dist/server/index.js";
import { ByteHangarClient } from "../sdk/dist/client/index.js";

const BASE = process.env.BYTEHANGAR_URL ?? "http://localhost:5180";
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
  console.log(`ByteHangar e2e against ${BASE}\n`);

  // --- provision (admin) ---
  const admin = new ByteHangarServer({ baseUrl: BASE, apiKey: "", adminToken: ADMIN });
  const tenant = await admin.createTenant(`e2e-${Date.now()}`);
  check("createTenant returns id", typeof tenant.id === "string" && tenant.id.length > 0);
  const created = await admin.createKey(tenant.id, "e2e-key");
  check("createKey returns plaintext key", created.key?.startsWith("bh_"));

  const storage = new ByteHangarServer({ baseUrl: BASE, apiKey: created.key });
  const client = new ByteHangarClient({ baseUrl: BASE });

  // --- catalog (idempotent) ---
  const policies = [
    { key: "img", category: "images", maxSizeBytes: 1024 * 1024, allowContentTypes: ["image/png"] },
    { key: "blob", category: "blobs", maxSizeBytes: 50 * 1024 * 1024, allowContentTypes: [] },
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
  const dlUrl = signed.url.startsWith("http") ? signed.url : BASE + signed.url;
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
  const bigUrl = signedBig.url.startsWith("http") ? signedBig.url : BASE + signedBig.url;
  const dlBig = Buffer.from(await (await fetch(bigUrl)).arrayBuffer());
  check(
    "large download byte-exact (6 MiB)",
    dlBig.length === big.length && Buffer.compare(dlBig, big) === 0,
  );

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

  console.log(`\n${passed} passed, ${failed} failed`);
  process.exit(failed ? 1 : 0);
}

main().catch((err) => {
  console.error("\nE2E ERROR:", err);
  process.exit(1);
});
