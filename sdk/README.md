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
// In your own route, AFTER you've authenticated + authorized the user:
app.post("/uploads/profile", async (req, res) => {
  const { token } = await storage.createGrant("profile-image");
  res.json({ token });
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

## Notes

- Success responses are the raw resource JSON; errors are `{ success:false, message, ... }` and surface as a thrown `ByteHangarError` (with `.status`).
- This is one package with separate `./server` and `./client` entry points. The client entry imports nothing from the server entry, so bundlers never pull the API key into a browser bundle.
