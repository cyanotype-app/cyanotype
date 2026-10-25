// Client for share links: the owner's calls (signed in, /assets/shares) and the public ones a visitor of /s/<token>
// makes (no login, /assets/public/s/<token>). Types mirror apps/assets/src/routes/shares.rs and share_public.rs.
import { ApiError, type Scope } from "./api";

/** A share link as its owner sees it. */
export interface ShareLink {
  id: string;
  /** `album` follows the album live; `selection` is a fixed set of photos. */
  kind: "album" | "selection";
  album: { id: string; title: string } | null;
  title: string | null;
  /** The secret in the URL: `/s/<token>`. */
  token: string;
  has_password: boolean;
  allow_download: boolean;
  /** Downloads serve the untouched originals, which carry location, instead of a copy without it. */
  include_originals: boolean;
  /** RFC 3339; null = never. */
  expires_at: string | null;
  expired: boolean;
  /** Photos a visitor sees right now (trashed ones are not counted). */
  item_count: number;
  view_count: number;
  last_viewed_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface CreateShare {
  album_id?: string;
  asset_ids?: readonly string[];
  /** "Select all": a view and the photos to leave out; the server resolves the set. */
  scope?: Scope;
  except?: readonly string[];
  title?: string;
  expires_at?: string | null;
  password?: string;
  allow_download: boolean;
  include_originals: boolean;
}

/** PATCH: omitted fields stay; `null` clears the title, the expiry or the password. */
export interface ShareChanges {
  title?: string | null;
  expires_at?: string | null;
  password?: string | null;
  allow_download?: boolean;
  include_originals?: boolean;
}

/** A password link that has not been unlocked in this browser: nothing else is revealed. */
export interface PublicLocked {
  needs_password: true;
  unlocked: false;
}

export interface PublicInfo {
  needs_password: boolean;
  unlocked: true;
  kind: "album" | "selection";
  title: string | null;
  item_count: number;
  allow_download: boolean;
  include_originals: boolean;
  expires_at: string | null;
}

export interface PublicDownload {
  /** Pass to `publicDownloadHref` as `file`. */
  id: string;
  /** JPEG, HEIC, RAW, ... */
  label: string;
  size_bytes: number | null;
}

export interface PublicItem {
  id: string;
  width: number | null;
  height: number | null;
  sort_time: string;
  /** Base64 ThumbHash; null until the photo has been processed. */
  thumbhash: string | null;
  /** Presigned, valid for ten minutes. */
  thumb_url: string | null;
  preview_url: string | null;
  downloads: PublicDownload[];
  /** A JPEG download is offered once the server has made its location-free copy. */
  download_pending: boolean;
}

export interface PublicItems {
  items: PublicItem[];
  next_cursor: string | null;
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
    body = undefined;
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

const enc = encodeURIComponent;
const pub = (token: string) => `/assets/public/s/${enc(token)}`;

export const shareApi = {
  list: () => request<{ items: ShareLink[] }>("/assets/shares"),
  create: (body: CreateShare) => request<ShareLink>("/assets/shares", { method: "POST", body: JSON.stringify(body) }),
  update: (id: string, changes: ShareChanges) =>
    request<ShareLink>(`/assets/shares/${enc(id)}`, { method: "PATCH", body: JSON.stringify(changes) }),
  /** Revoke: the link stops working at once. */
  revoke: (id: string) => request<void>(`/assets/shares/${enc(id)}`, { method: "DELETE" }),

  publicInfo: (token: string) => request<PublicInfo | PublicLocked>(pub(token)),
  publicUnlock: (token: string, password: string) =>
    request<void>(`${pub(token)}/unlock`, { method: "POST", body: JSON.stringify({ password }) }),
  publicItems: (token: string, cursor?: string | null) =>
    request<PublicItems>(`${pub(token)}/items?limit=100${cursor ? `&cursor=${enc(cursor)}` : ""}`),
  publicItem: (token: string, id: string) => request<PublicItem>(`${pub(token)}/items/${enc(id)}`),
  /** What "Download all" would hold; `pending` photos still have their download copy being made (it asks for them). */
  publicZipPlan: (token: string) => request<PublicZipPlan>(`${pub(token)}/zip/plan`),
  /** Ask for the download copy of a JPEG to be made now. */
  publicPrepare: (token: string, id: string) =>
    request<{ pending: boolean }>(`${pub(token)}/items/${enc(id)}/prepare`, { method: "POST" }),
};

/**
 * A plain link that saves a file of a shared photo. The API answers with a redirect to a short-lived presigned URL
 * (a navigation, not fetch(): the bucket has no CORS policy and the bytes never pass through the API).
 */
export const publicDownloadHref = (token: string, id: string, file?: string) =>
  `${pub(token)}/items/${enc(id)}/download${file && file !== id ? `?file=${enc(file)}` : ""}`;

/** All the link's downloadable photos as one ZIP: a plain link, so the browser streams the archive to disk. */
export const publicZipHref = (token: string) => `${pub(token)}/zip`;

export interface PublicZipPlan {
  files: number;
  bytes: number;
  pending: number;
  /** Photos the link does not offer (not JPEG, and the owner did not include originals). */
  skipped: number;
  name: string;
  max_files: number;
  max_bytes: number;
}

export type ShareApi = typeof shareApi;
