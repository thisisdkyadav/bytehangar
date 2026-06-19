// Public types shared by the server and client SDK entry points.
// (No runtime code, no secrets — safe to import from either side.)

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
  createdAt: string;
  deletedAt: string | null;
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

/** Error thrown by both SDK halves when the server returns a non-2xx envelope. */
export class ByteHangarError extends Error {
  readonly status: number;
  constructor(message: string, status: number) {
    super(message);
    this.name = "ByteHangarError";
    this.status = status;
  }
}
