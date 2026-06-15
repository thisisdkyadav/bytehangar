// @bytehangar/sdk/client — browser-side SDK. Holds NO secrets: it only ever
// carries a short-lived grant token your backend minted (via @bytehangar/sdk/server).

import { ByteHangarError, type UploadResult } from "../shared/index.js";

export interface ByteHangarClientOptions {
  /** Server base URL, e.g. "https://files.example.com" (trailing slash trimmed). */
  baseUrl: string;
  /** Override fetch (tests). Defaults to global fetch. */
  fetch?: typeof fetch;
}

export interface UploadOptions {
  /** Override the file name recorded server-side. */
  fileName?: string;
  /** Force a content type (otherwise the Blob's type / server sniffing is used). */
  contentType?: string;
  /** Upload progress 0..100. Uses XMLHttpRequest when available. */
  onProgress?: (percent: number) => void;
  /** Abort the upload. */
  signal?: AbortSignal;
}

export class ByteHangarClient {
  private readonly baseUrl: string;
  private readonly f: typeof fetch;

  constructor(options: ByteHangarClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, "");
    this.f = options.fetch ?? fetch;
  }

  /**
   * Upload a file directly to the storage server using a grant token obtained
   * from your backend. The grant encodes the policy, size cap, and expiry — the
   * client cannot exceed them.
   */
  async upload(grant: string, file: Blob, opts: UploadOptions = {}): Promise<UploadResult> {
    const url = `${this.baseUrl}/v1/upload`;
    const fileName = opts.fileName ?? (file as File).name ?? "upload";
    const blob =
      opts.contentType && opts.contentType !== file.type
        ? new Blob([file], { type: opts.contentType })
        : file;

    const form = new FormData();
    form.append("file", blob, fileName);

    // Use XHR when progress is requested and available (fetch lacks upload progress).
    if (opts.onProgress && typeof XMLHttpRequest !== "undefined") {
      return uploadWithProgress(url, grant, form, opts);
    }

    const res = await this.f(url, {
      method: "POST",
      headers: { "x-bytehangar-grant": grant },
      body: form,
      signal: opts.signal,
    });
    if (!res.ok) {
      throw await toError(res);
    }
    return toUploadResult(await res.json());
  }
}

function uploadWithProgress(
  url: string,
  grant: string,
  form: FormData,
  opts: UploadOptions,
): Promise<UploadResult> {
  return new Promise<UploadResult>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", url);
    xhr.setRequestHeader("x-bytehangar-grant", grant);

    if (opts.onProgress) {
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable) {
          opts.onProgress!(Math.round((event.loaded / event.total) * 100));
        }
      };
    }
    if (opts.signal) {
      opts.signal.addEventListener("abort", () => xhr.abort(), { once: true });
    }

    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        try {
          resolve(toUploadResult(JSON.parse(xhr.responseText)));
        } catch {
          reject(new ByteHangarError("invalid response", xhr.status));
        }
      } else {
        let message = xhr.statusText || `HTTP ${xhr.status}`;
        try {
          const env = JSON.parse(xhr.responseText) as { message?: string };
          if (env?.message) message = env.message;
        } catch {
          // keep statusText
        }
        reject(new ByteHangarError(message, xhr.status));
      }
    };
    xhr.onerror = () => reject(new ByteHangarError("network error", 0));
    xhr.onabort = () => reject(new ByteHangarError("aborted", 0));
    xhr.send(form);
  });
}

function toUploadResult(d: any): UploadResult {
  return {
    fileRef: d.file_ref,
    contentType: d.content_type,
    size: d.size,
    originalName: d.original_name ?? null,
    deduplicated: d.deduplicated,
  };
}

async function toError(res: Response): Promise<ByteHangarError> {
  let message = res.statusText || `HTTP ${res.status}`;
  try {
    const env = (await res.json()) as { message?: string };
    if (env?.message) message = env.message;
  } catch {
    // non-JSON error body
  }
  return new ByteHangarError(message, res.status);
}
