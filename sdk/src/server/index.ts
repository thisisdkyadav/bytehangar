// @bytehangar/sdk/server — server-side SDK. Holds the tenant API key (and
// optional admin token). NEVER import this from browser code: it carries secrets.

import {
  ByteHangarError,
  type FileRecord,
  type GrantResult,
  type PolicyDefinition,
  type RegisterCatalogResult,
  type SignResult,
  type UsageResult,
} from "../shared/index.js";

export interface ByteHangarServerOptions {
  /** Server base URL, e.g. "http://localhost:5100" (trailing slash trimmed). */
  baseUrl: string;
  /** Tenant API key, sent as `x-bytehangar-key`. */
  apiKey: string;
  /** Optional bootstrap admin token for provisioning endpoints. */
  adminToken?: string;
  /** Override fetch (tests / custom agents). Defaults to global fetch. */
  fetch?: typeof fetch;
}

type Auth = "key" | "admin" | "none";

interface RequestOptions {
  auth?: Auth;
  body?: unknown;
}

export class ByteHangarServer {
  private readonly baseUrl: string;
  private readonly apiKey: string;
  private readonly adminToken?: string;
  private readonly f: typeof fetch;

  constructor(options: ByteHangarServerOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, "");
    this.apiKey = options.apiKey;
    this.adminToken = options.adminToken;
    this.f = options.fetch ?? fetch;
  }

  // -- catalog / grants -----------------------------------------------------

  /** Register (idempotently) this tenant's policy catalog. Call at app boot. */
  async registerCatalog(policies: PolicyDefinition[]): Promise<RegisterCatalogResult> {
    const data = await this.request<any>("PUT", "/internal/v1/catalog", {
      auth: "key",
      body: {
        policies: policies.map((p) => ({
          key: p.key,
          category: p.category,
          max_size_bytes: p.maxSizeBytes,
          allow_content_types: p.allowContentTypes ?? [],
        })),
      },
    });
    return {
      version: data.version,
      changed: data.changed,
      policyCount: data.policy_count,
    };
  }

  /** Mint a short-lived, single-use upload grant to hand to the browser client. */
  async createGrant(
    policyKey: string,
    opts: { expiresInSeconds?: number } = {},
  ): Promise<GrantResult> {
    const data = await this.request<any>("POST", "/internal/v1/grants", {
      auth: "key",
      body: { policy_key: policyKey, expires_in_seconds: opts.expiresInSeconds },
    });
    return { token: data.token, expiresAt: data.expires_at };
  }

  // -- files ----------------------------------------------------------------

  async getFile(fileRef: string): Promise<FileRecord> {
    const data = await this.request<any>(
      "GET",
      `/internal/v1/files/${encodeURIComponent(fileRef)}`,
      { auth: "key" },
    );
    return toFileRecord(data);
  }

  /** Mint a signed download URL for a file. */
  async signDownload(
    fileRef: string,
    opts: { expiresInSeconds?: number; disposition?: "inline" | "attachment" } = {},
  ): Promise<SignResult> {
    const data = await this.request<any>(
      "POST",
      `/internal/v1/files/${encodeURIComponent(fileRef)}/sign`,
      {
        auth: "key",
        body: {
          expires_in_seconds: opts.expiresInSeconds,
          disposition: opts.disposition,
        },
      },
    );
    return { url: data.url, expiresAt: data.expires_at };
  }

  /** Server-to-server: fetch the raw bytes. */
  async getContent(fileRef: string): Promise<ArrayBuffer> {
    const res = await this.f(
      `${this.baseUrl}/internal/v1/files/${encodeURIComponent(fileRef)}/content`,
      { headers: { "x-bytehangar-key": this.apiKey } },
    );
    if (!res.ok) throw await toError(res);
    return res.arrayBuffer();
  }

  async deleteFile(fileRef: string): Promise<void> {
    await this.request<unknown>(
      "DELETE",
      `/internal/v1/files/${encodeURIComponent(fileRef)}`,
      { auth: "key" },
    );
  }

  async getUsage(): Promise<UsageResult> {
    const data = await this.request<any>("GET", "/internal/v1/usage", { auth: "key" });
    return { usedBytes: data.used_bytes, objectCount: data.object_count };
  }

  // -- admin (require adminToken) ------------------------------------------

  async createTenant(name: string): Promise<{ id: string; name: string }> {
    return this.request("POST", "/internal/v1/tenants", { auth: "admin", body: { name } });
  }

  async createKey(
    tenantId: string,
    name: string,
    role: "app" | "admin" = "app",
  ): Promise<{ id: string; key: string }> {
    return this.request("POST", `/internal/v1/tenants/${tenantId}/keys`, {
      auth: "admin",
      body: { name, role },
    });
  }

  async setQuota(tenantId: string, quotaBytes: number): Promise<void> {
    await this.request("PATCH", `/internal/v1/tenants/${tenantId}/quota`, {
      auth: "admin",
      body: { quota_bytes: quotaBytes },
    });
  }

  // -- internals ------------------------------------------------------------

  private async request<T>(
    method: string,
    path: string,
    opts: RequestOptions = {},
  ): Promise<T> {
    const headers: Record<string, string> = {};
    if (opts.auth === "key") headers["x-bytehangar-key"] = this.apiKey;
    if (opts.auth === "admin") {
      if (!this.adminToken) {
        throw new ByteHangarError("adminToken is required for this operation", 0);
      }
      headers["x-bytehangar-admin"] = this.adminToken;
    }
    let body: BodyInit | undefined;
    if (opts.body !== undefined) {
      headers["content-type"] = "application/json";
      body = JSON.stringify(opts.body);
    }

    const res = await this.f(`${this.baseUrl}${path}`, { method, headers, body });
    if (!res.ok) throw await toError(res);
    if (res.status === 204) return undefined as T;
    return (await res.json()) as T;
  }
}

function toFileRecord(d: any): FileRecord {
  return {
    id: d.id,
    tenantId: d.tenant_id,
    fileRef: d.file_ref,
    policyKey: d.policy_key,
    category: d.category,
    originalName: d.original_name,
    storedKey: d.stored_key,
    contentType: d.content_type,
    sizeBytes: d.size_bytes,
    checksumSha256: d.checksum_sha256,
    createdAt: d.created_at,
    deletedAt: d.deleted_at,
  };
}

async function toError(res: Response): Promise<ByteHangarError> {
  let message = res.statusText || `HTTP ${res.status}`;
  try {
    const env = (await res.json()) as { message?: string };
    if (env?.message) message = env.message;
  } catch {
    // non-JSON error body; keep statusText
  }
  return new ByteHangarError(message, res.status);
}
