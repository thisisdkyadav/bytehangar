# @bytehangar/sdk

Isomorphic SDK for ByteHangar. Two entry points with a hard secret boundary:

- `@bytehangar/sdk/server` — Node. Holds the tenant API key. Registers the catalog, mints grants, manages files. **Never import from browser code.**
- `@bytehangar/sdk/client` — browser. Holds no secrets; uploads with a short-lived grant your backend minted.
- `@bytehangar/sdk/shared` — shared types only.

## Install

```bash
npm install @bytehangar/sdk
```

## 1. Register your catalog at app boot (server)

```ts
import { ByteHangarServer } from "@bytehangar/sdk/server";

const storage = new ByteHangarServer({
  baseUrl: process.env.BYTEHANGAR_URL!,   // e.g. http://localhost:5100
  apiKey: process.env.BYTEHANGAR_KEY!,    // tenant API key
});

await storage.registerCatalog([
  { key: "profile-image", category: "profile-images", maxSizeBytes: 500 * 1024,
    allowContentTypes: ["image/png", "image/jpeg"] },
  { key: "id-document", category: "id-documents", maxSizeBytes: 10 * 1024 * 1024,
    allowContentTypes: ["application/pdf"] },
]);
```

## 2. Mint a grant for an authorized client (server)

```ts
// Express — read the policy + run your own authz, then mint:
app.post("/uploads/profile", async (req, res) => {
  const { token } = await storage.createGrant("profile-image");
  res.json({ token });
});
```

Attach attribution metadata when you mint — it's baked into the grant, so the
client can't tamper with it, and it surfaces on the resulting `FileRecord` so
apps can attribute and filter uploads:

```ts
const { token } = await storage.createGrant("profile-image", {
  metadata: {
    actorId: req.user.id,          // who uploaded
    actorRole: req.user.role,      // in what capacity
    sourceService: "admin-portal", // which app/surface
    entityHint: `student:${studentId}`, // what it's attached to
  },
});
```

Or, for any fetch-based framework (Next.js App Router, Remix, Hono, Bun, Deno,
Workers), use the one-liner helper:

```ts
import { createGrantRoute } from "@bytehangar/sdk/server";

export const POST = createGrantRoute(storage, {
  resolvePolicy: () => "profile-image",
  authorize: (req) => myAuth(req), // strongly recommended
});
```

## 3. Upload directly from the browser (client)

```ts
import { ByteHangarClient } from "@bytehangar/sdk/client";

const client = new ByteHangarClient({ baseUrl: "https://files.example.com" });

const { token } = await (await fetch("/uploads/profile", { method: "POST" })).json();
const result = await client.upload(token, file, {
  onProgress: (pct) => console.log(`${pct}%`),
});
// result.fileRef -> persist this against the user
```

## 4. Serve a download (server)

```ts
const { url } = await storage.signDownload(fileRef, { expiresInSeconds: 600 });
// redirect the browser to `url`, or return it as an <img src>
```

## 5. Soft delete and restore (server)

`deleteFile` is a soft delete: the row is tombstoned and the blob is held until
garbage collection reclaims it. Until then you can undo:

```ts
await storage.deleteFile(fileRef);   // tombstone now
await storage.restoreFile(fileRef);  // undo, within the GC retention window
```

`restoreFile(fileRef)` brings a soft-deleted file back to live. It is
quota-gated (restoring counts against your usage again, so it can fail if you're
over quota) and throws `ByteHangarError` with `.status === 410` if GC has
already reclaimed the blob — past that window the file is gone for good.

## The `FileRecord` shape

`getFile`, `listFiles`, and `restoreFile`'s underlying record all return a
`FileRecord`. Beyond the storage facts, it carries the attribution metadata you
passed to `createGrant` and the soft-delete tombstone:

```ts
interface FileRecord {
  id: string;
  tenantId: string;
  fileRef: string;
  policyKey: string;
  category: string;
  originalName: string;
  storedKey: string;
  contentType: string;
  sizeBytes: number;
  checksumSha256: string;
  visibility: string;

  // attribution — from the grant's `metadata` (null if not supplied)
  actorId: string | null;
  actorRole: string | null;
  sourceService: string | null;
  entityHint: string | null;

  createdAt: string;        // ISO-8601
  deletedAt: string | null; // ISO-8601 once soft-deleted, else null
}
```

`listFiles` returns only live files (those with `deletedAt === null`), newest
first, and accepts `{ limit, offset, category }`.

## React (`@bytehangar/sdk/react`)

A hook + drop-in button built on the client. `getGrant` calls your own backend
(which mints the grant with `@bytehangar/sdk/server`).

```tsx
import { UploadButton, useByteHangarUpload } from "@bytehangar/sdk/react";

const getGrant = async () =>
  (await (await fetch("/uploads/profile", { method: "POST" })).json()).token;

// Drop-in button:
<UploadButton
  baseUrl="https://files.example.com"
  getGrant={getGrant}
  accept="image/*"
  onComplete={(r) => console.log(r.fileRef)}
/>;

// Or the hook for custom UI:
function Avatar() {
  const { upload, status, progress } = useByteHangarUpload({
    baseUrl: "https://files.example.com",
    getGrant,
  });
  return <input type="file" onChange={(e) => e.target.files?.[0] && upload(e.target.files[0])} />;
}
```

React is an optional peer dependency — only needed if you import `/react`.

## Notes

- Success responses are the raw resource JSON; errors are `{ success:false, message, ... }` and surface as a thrown `ByteHangarError` (with `.status`).
- This is one package with separate `./server` and `./client` entry points. The client entry imports nothing from the server entry, so bundlers never pull the API key into a browser bundle.
- The server enforces a configurable content-type allowlist: an upload is accepted only if its type is permitted by the server's master allowlist and by the policy's own `allowContentTypes` (when set). Disallowed types are rejected at upload — the client `upload` throws a `ByteHangarError`.
- Downloads carry `Cache-Control` and an `ETag`, so a conditional GET (`If-None-Match`) returns `304 Not Modified` and browsers/CDNs can cache served files.
