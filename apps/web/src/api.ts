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

/** The other file of a RAW + JPEG pair: the RAW stacked behind a visible photo. */
export interface PairedFile {
  id: string;
  original_name: string;
  content_type: string;
  size_bytes: number;
  kind: "raw";
}

/** An entry of GET /assets. `thumbhash`/`thumb_url` are null until the worker has produced the derived images. */
export interface AssetListItem extends AssetSummary {
  /** Base64 ThumbHash. */
  thumbhash: string | null;
  /** Presigned, short-lived. */
  thumb_url: string | null;
  /** The RAW file stacked behind this photo; empty or absent for a single file (older servers omit it). */
  paired?: PairedFile[];
}

/** GET /assets */
export interface AssetList {
  items: AssetListItem[];
  /** Opaque keyset cursor: pass as `cursor` for the next page; null on the last page. Absent on older servers. */
  next_cursor?: string | null;
  /** Deprecated: timestamp for `before`. Only used when the server sends no `next_cursor`. */
  next_before: string | null;
}

/** An entry of GET /assets/trash: a list item plus when it was trashed and when it will be deleted for good. */
export interface TrashItem extends AssetListItem {
  /** RFC 3339. */
  deleted_at: string;
  /** RFC 3339: deleted_at + the server's retention period. */
  purge_at: string;
}

/** GET /assets/trash */
export interface TrashList {
  items: TrashItem[];
  next_cursor: string | null;
  /** Everything in the trash, not just this page. */
  total: number;
  retention_days: number;
}

/** Most ids the server accepts per bulk request; longer lists are sent in several requests. */
export const MAX_BULK_IDS = 1000;

/** An album a photo is in (GET /assets/{id}). */
export interface AlbumRef {
  id: string;
  title: string;
}

/** The thumbnail shown for an album: its chosen cover, else its most recent photo. */
export interface AlbumCover {
  id: string;
  thumbhash: string | null;
  thumb_url: string | null;
}

/** An entry of GET /assets/albums and the body of GET /assets/albums/{id}. Counts and dates skip trashed photos. */
export interface Album {
  id: string;
  title: string;
  description: string | null;
  photo_count: number;
  /** Null while the album is empty. */
  cover: AlbumCover | null;
  /** RFC 3339 times of the earliest and latest photo; null while empty. */
  first_at: string | null;
  last_at: string | null;
  created_at: string;
  updated_at: string;
}

/** PATCH /assets/albums/{id}: omitted fields stay; `null` clears the description or the chosen cover. */
export interface AlbumPatch {
  title?: string;
  description?: string | null;
  cover_asset_id?: string | null;
}

/** GET /assets/{id}. The URLs are presigned and short-lived. */
export interface AssetDetail extends AssetSummary {
  /** The albums this photo is in. Absent on older servers. */
  albums?: AlbumRef[];
  /** The RAW stacked behind this photo. Absent on older servers. */
  paired?: PairedFile[];
  thumbhash: string | null;
  original_url: string;
  thumb_url: string | null;
  preview_url: string | null;
}

/** POST /assets/check body. `transport: "proxy"` asks for same-origin part URLs (browsers cannot PUT to S3 cross-origin). */
export interface CheckRequest {
  sha256: string;
  size_bytes: number;
  filename: string;
  content_type?: string;
  /** RFC 3339 (File.lastModified). */
  file_modified_at?: string;
  transport?: "direct" | "proxy";
}

export interface PartUrl {
  number: number;
  url: string;
}

export interface DonePart {
  number: number;
  etag: string;
}

/** POST /assets/check. `parts` are still to PUT; `uploaded` were stored by an earlier attempt (resume). */
export type CheckResponse =
  | { status: "exists"; asset_id: string }
  | { status: "upload"; upload_id: string; part_size: number; parts: PartUrl[]; uploaded: DonePart[] };

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

/** Pull the server's `{"error": ...}` text out of a failed response body, if there is one. */
function errorMessage(text: string, status: number): string {
  try {
    const body: unknown = JSON.parse(text);
    if (typeof body === "object" && body !== null && "error" in body && typeof body.error === "string") return body.error;
  } catch {
    /* not JSON */
  }
  return `Request failed (${status})`;
}

/**
 * PUT one part to a (same-origin) proxy URL with upload progress. XHR, because fetch has no upload progress.
 * Resolves with the part's ETag; rejects with ApiError (status 0 = network failure or abort).
 */
function putPart(
  url: string,
  body: Blob,
  opts: { onProgress?: (loaded: number) => void; signal?: AbortSignal } = {},
): Promise<{ etag: string }> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("PUT", url);
    xhr.withCredentials = true;
    xhr.responseType = "text";
    xhr.setRequestHeader("Content-Type", "application/octet-stream");
    xhr.setRequestHeader("Accept", "application/json");
    xhr.upload.onprogress = (e) => opts.onProgress?.(e.loaded);
    xhr.onerror = () => reject(new ApiError(0, "Cannot reach the server. Check your connection and try again."));
    xhr.onabort = () => reject(new ApiError(0, "Upload canceled."));
    xhr.ontimeout = () => reject(new ApiError(0, "The upload timed out."));
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        try {
          const b: unknown = JSON.parse(xhr.responseText);
          if (typeof b === "object" && b !== null && "etag" in b && typeof b.etag === "string") {
            resolve({ etag: b.etag });
            return;
          }
        } catch {
          /* fall through */
        }
        reject(new ApiError(xhr.status, "Unexpected response from the server."));
      } else {
        reject(new ApiError(xhr.status, errorMessage(xhr.responseText, xhr.status)));
      }
    };
    if (opts.signal) {
      if (opts.signal.aborted) return reject(new ApiError(0, "Upload canceled."));
      opts.signal.addEventListener("abort", () => xhr.abort(), { once: true });
    }
    xhr.send(body);
  });
}

/** POST `ids` to a bulk endpoint in chunks of MAX_BULK_IDS; resolves with the total number of items affected. */
async function bulk(path: string, ids: readonly string[]): Promise<{ count: number }> {
  let count = 0;
  for (let i = 0; i < ids.length; i += MAX_BULK_IDS) {
    const r = await request<{ count: number }>(path, json({ ids: ids.slice(i, i + MAX_BULK_IDS) }));
    count += r.count;
  }
  return { count };
}

export const api = {
  me: () => request<Me>("/auth/me"),
  login: (email: string, password: string) => request<{ id: string }>("/auth/login", json({ email, password })),
  /** 403 when registration is closed (REGISTRATION_MODE=closed, or first_user with an owner already present). */
  register: (email: string, password: string) => request<Me>("/auth/register", json({ email, password })),
  logout: () => request<void>("/auth/logout", { method: "POST" }),
  listApiKeys: () => request<{ items: ApiKey[] }>("/auth/api-keys"),
  createApiKey: (name: string) => request<CreatedApiKey>("/auth/api-keys", json({ name })),
  revokeApiKey: (id: string) => request<void>(`/auth/api-keys/${encodeURIComponent(id)}`, { method: "DELETE" }),
  listAssets: (opts: { cursor?: string | null; before?: string | null; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.limit !== undefined) q.set("limit", String(opts.limit));
    if (opts.cursor) q.set("cursor", opts.cursor);
    else if (opts.before) q.set("before", opts.before);
    const qs = q.toString();
    return request<AssetList>(`/assets${qs ? `?${qs}` : ""}`);
  },
  /** Move photos to the trash (reversible). `count` is how many actually moved. */
  trashAssets: (ids: readonly string[]) => bulk("/assets/trash", ids),
  /** Bring photos back from the trash. */
  restoreAssets: (ids: readonly string[]) => bulk("/assets/restore", ids),
  /** Delete trashed photos forever. Not reversible. */
  purgeAssets: (ids: readonly string[]) => bulk("/assets/purge", ids),
  /** Delete everything in the trash forever. Not reversible. */
  emptyTrash: () => request<{ count: number }>("/assets/trash/empty", { method: "POST" }),
  listTrash: (opts: { cursor?: string | null; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.limit !== undefined) q.set("limit", String(opts.limit));
    if (opts.cursor) q.set("cursor", opts.cursor);
    const qs = q.toString();
    return request<TrashList>(`/assets/trash${qs ? `?${qs}` : ""}`);
  },
  listAlbums: () => request<{ items: Album[] }>("/assets/albums"),
  getAlbum: (id: string) => request<Album>(`/assets/albums/${encodeURIComponent(id)}`),
  /** Create an album, optionally with photos (lists longer than the server's bulk cap are added in further requests). */
  createAlbum: async (input: { title: string; description?: string; asset_ids?: readonly string[] }): Promise<Album> => {
    const ids = input.asset_ids ?? [];
    const album = await request<Album>(
      "/assets/albums",
      json({ title: input.title, description: input.description, asset_ids: ids.slice(0, MAX_BULK_IDS) }),
    );
    if (ids.length <= MAX_BULK_IDS) return album;
    await bulk(`/assets/albums/${encodeURIComponent(album.id)}/assets`, ids.slice(MAX_BULK_IDS));
    return request<Album>(`/assets/albums/${encodeURIComponent(album.id)}`);
  },
  updateAlbum: (id: string, patch: AlbumPatch) =>
    request<Album>(`/assets/albums/${encodeURIComponent(id)}`, { method: "PATCH", body: JSON.stringify(patch) }),
  /** Deletes the album only; its photos stay in the library. */
  deleteAlbum: (id: string) => request<void>(`/assets/albums/${encodeURIComponent(id)}`, { method: "DELETE" }),
  listAlbumAssets: (id: string, opts: { cursor?: string | null; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.limit !== undefined) q.set("limit", String(opts.limit));
    if (opts.cursor) q.set("cursor", opts.cursor);
    const qs = q.toString();
    return request<AssetList>(`/assets/albums/${encodeURIComponent(id)}/assets${qs ? `?${qs}` : ""}`);
  },
  /** Add photos to an album. `count` is how many were new to it (already-present photos are skipped). */
  addToAlbum: (id: string, ids: readonly string[]) => bulk(`/assets/albums/${encodeURIComponent(id)}/assets`, ids),
  /** Take photos out of an album; they stay in the library. */
  removeFromAlbum: (id: string, ids: readonly string[]) =>
    bulk(`/assets/albums/${encodeURIComponent(id)}/assets/remove`, ids),
  /** Separate a RAW + JPEG pair into two photos (either file's id); they are not paired again automatically. */
  unpair: (id: string) => request<void>(`/assets/${encodeURIComponent(id)}/unpair`, { method: "POST" }),
  getAsset: (id: string) => request<AssetDetail>(`/assets/${encodeURIComponent(id)}`),
  /** Dedupe + part URLs. Defaults to the proxy transport, the only one a browser can use. */
  checkUpload: (req: CheckRequest) => request<CheckResponse>("/assets/check", json({ transport: "proxy", ...req })),
  /** `url` is a `parts[].url` from `checkUpload` (relative, same origin). */
  putPart,
  completeUpload: (uploadId: string, parts: DonePart[]) =>
    request<{ asset_id: string }>("/assets/complete", json({ upload_id: uploadId, parts })),
};

export type Api = typeof api;
