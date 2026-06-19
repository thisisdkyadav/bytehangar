// @bytehangar/sdk/react — React adapter. Built on @bytehangar/sdk/client, so it
// holds NO secrets: it gets a grant from your backend via `getGrant`.

import { type ChangeEvent, type ReactNode, useCallback, useRef, useState } from "react";

import { ByteHangarClient } from "../client/index.js";
import type { UploadResult } from "../shared/index.js";

/** Returns a fresh upload grant token (your backend mints it after authz). */
export type GrantProvider = () => Promise<string>;

export interface UseUploadOptions {
  /** Storage server public base URL. */
  baseUrl: string;
  /** Fetch a grant token from your backend. */
  getGrant: GrantProvider;
  onComplete?: (result: UploadResult) => void;
  onError?: (error: Error) => void;
}

export type UploadStatus = "idle" | "uploading" | "success" | "error";

export interface UseUploadResult {
  upload: (
    file: Blob,
    opts?: { fileName?: string; contentType?: string },
  ) => Promise<UploadResult | undefined>;
  status: UploadStatus;
  progress: number;
  error: Error | null;
  result: UploadResult | null;
  reset: () => void;
}

export function useByteHangarUpload(options: UseUploadOptions): UseUploadResult {
  const { baseUrl, getGrant, onComplete, onError } = options;
  const [status, setStatus] = useState<UploadStatus>("idle");
  const [progress, setProgress] = useState(0);
  const [error, setError] = useState<Error | null>(null);
  const [result, setResult] = useState<UploadResult | null>(null);

  const upload = useCallback<UseUploadResult["upload"]>(
    async (file, opts = {}) => {
      setStatus("uploading");
      setProgress(0);
      setError(null);
      setResult(null);
      try {
        const client = new ByteHangarClient({ baseUrl });
        const grant = await getGrant();
        const res = await client.upload(grant, file, { ...opts, onProgress: setProgress });
        setResult(res);
        setProgress(100);
        setStatus("success");
        onComplete?.(res);
        return res;
      } catch (caught) {
        const err = caught instanceof Error ? caught : new Error(String(caught));
        setError(err);
        setStatus("error");
        onError?.(err);
        return undefined;
      }
    },
    [baseUrl, getGrant, onComplete, onError],
  );

  const reset = useCallback(() => {
    setStatus("idle");
    setProgress(0);
    setError(null);
    setResult(null);
  }, []);

  return { upload, status, progress, error, result, reset };
}

export interface UploadButtonProps extends UseUploadOptions {
  /** `accept` attribute for the file input (e.g. "image/*"). */
  accept?: string;
  className?: string;
  /** Custom label. Defaults to a progress-aware label. */
  children?: ReactNode;
}

/** A minimal, unstyled upload button: hidden file input + label. */
export function UploadButton(props: UploadButtonProps): ReactNode {
  const { accept, className, children, ...uploadOptions } = props;
  const { upload, status, progress } = useByteHangarUpload(uploadOptions);
  const inputRef = useRef<HTMLInputElement>(null);

  const onChange = useCallback(
    async (event: ChangeEvent<HTMLInputElement>) => {
      const file = event.target.files?.[0];
      if (file) {
        await upload(file, { fileName: file.name });
      }
      if (inputRef.current) {
        inputRef.current.value = "";
      }
    },
    [upload],
  );

  const label =
    children ?? (status === "uploading" ? `Uploading ${progress}%` : "Upload file");

  return (
    <label className={className} style={{ cursor: "pointer", display: "inline-block" }}>
      <input
        ref={inputRef}
        type="file"
        accept={accept}
        onChange={onChange}
        disabled={status === "uploading"}
        style={{ display: "none" }}
      />
      {label}
    </label>
  );
}
