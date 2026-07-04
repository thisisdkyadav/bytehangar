// Public types shared by the server and client SDK entry points.
// (No runtime code, no secrets — safe to import from either side.)

/** How an image is fit into the requested box. */
export type TransformFit = "cover" | "inside" | "fill";
/** Output format for a rendered variant (pure-Rust encoders). */
export type TransformFormat = "jpeg" | "png" | "webp";

/** A named image transform preset. At least one of `w`/`h` is required. */
export interface TransformPreset {
  /** Target width (px). */
  w?: number;
  /** Target height (px). */
  h?: number;
  /** Fit mode; defaults to "cover" (scale + center-crop). */
  fit?: TransformFit;
  /** Output format. */
  fmt: TransformFormat;
  /** JPEG/WebP quality 1..=100 (ignored for PNG). */
  q?: number;
}

/** A policy the app registers at boot. The enforceable upload rule. */
export interface PolicyDefinition {
  /** Stable identifier the app refers to, e.g. "profile-image". */
  key: string;
  /** Path-safe storage category (`^[a-z0-9-]+$`), e.g. "profile-images". */
  category: string;
  /** Per-policy size cap (bytes); may only be stricter than the server's global cap. */
  maxSizeBytes: number;
  /** Allowed content types; empty/omitted => any type the master allowlist permits. */
  allowContentTypes?: string[];
  /** "public" (served without a signature) or "private" (default). */
  visibility?: "public" | "private";
  /**
   * Named image transform presets, e.g.
   * `{ thumb: { w: 256, h: 256, fit: "cover", fmt: "webp", q: 80 } }`.
   * Reference a preset on download via `signDownload(ref, { variant: "thumb" })`
   * or `client.fileUrl(t, ref, { variant: "thumb" })`. Rendered lazily + cached.
   */
  transforms?: Record<string, TransformPreset>;
}

export interface RegisterCatalogResult {
  version: number;
  changed: boolean;
  policyCount: number;
}

export interface GrantResult {
  /** The signed, single-use token to hand to the browser client. */
  token: string;
  /** ISO-8601 expiry. */
  expiresAt: string;
}

export interface UploadResult {
  fileRef: string;
  contentType: string;
  size: number;
  originalName: string | null;
  deduplicated: boolean;
}

export interface FileRecord {
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
  actorId: string | null;
  actorRole: string | null;
  sourceService: string | null;
  entityHint: string | null;
  createdAt: string;
  deletedAt: string | null;
}

/** App-supplied attribution metadata for an upload (carried in the grant). */
export interface UploadMetadata {
  actorId?: string;
  actorRole?: string;
  sourceService?: string;
  entityHint?: string;
}

export interface SignResult {
  url: string;
  expiresAt: string;
}

export interface UsageResult {
  usedBytes: number;
  objectCount: number;
}

export interface TenantSummary {
  id: string;
  name: string;
  status: string;
  quotaBytes: number;
  createdAt: string;
  usedBytes: number;
  objectCount: number;
}

export interface Page<T> {
  items: T[];
  total: number;
  limit: number;
  offset: number;
}

export interface ApiKeySummary {
  id: string;
  name: string;
  role: string;
  createdAt: string;
  lastUsedAt: string | null;
  revokedAt: string | null;
}

/** Error thrown by both SDK halves when the server returns a non-2xx envelope. */
export class ByteHangarError extends Error {
  readonly status: number;
  constructor(message: string, status: number) {
    super(message);
    this.name = "ByteHangarError";
    this.status = status;
  }
}
