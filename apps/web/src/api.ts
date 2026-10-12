// Hand-written client for the Cyanotype API (same origin, session cookie). The types mirror the Rust
// structs in apps/auth/src/routes.rs and apps/assets/src/routes.rs; generating this from a utoipa
// OpenAPI document is a planned follow-up (see docs/build.md).

/** GET /auth/me */
export interface Me {
  id: string;
  email: string;
}

/** An entry of GET /auth/api-keys. The secret is never listed. */
export interface ApiKey {
  id: string;
  name: string;
  /** Non-secret start of the key, to tell keys apart. */
  prefix: string;
  created_at: string;
  last_used_at: string | null;
}

/** POST /auth/api-keys: `key` is the plaintext secret, returned only once. */
export interface CreatedApiKey extends Omit<ApiKey, "last_used_at"> {
  key: string;
}

/** Fields shared by every asset representation (`AssetSummary` in the assets service). */
export interface AssetSummary {
  id: string;
  original_name: string;
  content_type: string;
  size_bytes: number;
  width: number | null;
  height: number | null;
  /** RFC 3339: taken_at, falling back to created_at. */
  sort_time: string;
}

/** An entry of GET /assets. `thumbhash`/`thumb_url` are null until the worker has produced the derived images. */
export interface AssetListItem extends AssetSummary {
  /** Base64 ThumbHash. */
  thumbhash: string | null;
  /** Presigned, short-lived. */
  thumb_url: string | null;
}

/** GET /assets */
export interface AssetList {
  items: AssetListItem[];
  /** Pass as `before` for the next page; null on the last page. */
  next_before: string | null;
}

/** GET /assets/{id}. The URLs are presigned and short-lived. */
export interface AssetDetail extends AssetSummary {
  thumbhash: string | null;
  original_url: string;
  thumb_url: string | null;
  preview_url: string | null;
}

/** A non-2xx response (or a network failure, status 0). `message` is the server's `{"error": ...}` text when present. */
export class ApiError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  headers.set("Accept", "application/json");
  if (init.body !== undefined) headers.set("Content-Type", "application/json");

  let res: Response;
  try {
    res = await fetch(path, { ...init, headers, credentials: "include" });
  } catch {
    throw new ApiError(0, "Cannot reach the server. Check your connection and try again.");
  }

  if (res.status === 204) return undefined as T;

  const text = await res.text();
  let body: unknown;
  try {
    body = text ? JSON.parse(text) : undefined;
  } catch {
    body = undefined; // e.g. an HTML error page from a proxy
  }

  if (!res.ok) {
    const msg =
      typeof body === "object" && body !== null && "error" in body && typeof body.error === "string"
        ? body.error
        : `Request failed (${res.status})`;
    throw new ApiError(res.status, msg);
  }
  return body as T;
}

const json = (body: unknown): RequestInit => ({ method: "POST", body: JSON.stringify(body) });

export const api = {
  me: () => request<Me>("/auth/me"),
  login: (email: string, password: string) => request<{ id: string }>("/auth/login", json({ email, password })),
  /** 403 when registration is closed (REGISTRATION_MODE=closed, or first_user with an owner already present). */
  register: (email: string, password: string) => request<Me>("/auth/register", json({ email, password })),
  logout: () => request<void>("/auth/logout", { method: "POST" }),
  listApiKeys: () => request<{ items: ApiKey[] }>("/auth/api-keys"),
  createApiKey: (name: string) => request<CreatedApiKey>("/auth/api-keys", json({ name })),
  revokeApiKey: (id: string) => request<void>(`/auth/api-keys/${encodeURIComponent(id)}`, { method: "DELETE" }),
  listAssets: (opts: { before?: string | null; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.limit !== undefined) q.set("limit", String(opts.limit));
    if (opts.before) q.set("before", opts.before);
    const qs = q.toString();
    return request<AssetList>(`/assets${qs ? `?${qs}` : ""}`);
  },
  getAsset: (id: string) => request<AssetDetail>(`/assets/${encodeURIComponent(id)}`),
};

export type Api = typeof api;
