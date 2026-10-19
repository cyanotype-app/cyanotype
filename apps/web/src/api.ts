// Hand-written client for the Cyanotype API (same origin, session cookie). The types mirror the Rust
// structs in apps/auth/src/routes.rs and apps/assets/src/routes.rs; generating this from a utoipa
// OpenAPI document is a planned follow-up (see docs/build.md).

/** GET /auth/me */
export interface Me {
  id: string;
  email: string;
  /** Absent on older servers (treated as member). */
  role?: "admin" | "member";
}

/** GET /auth/registration: what the register page may offer. */
export interface RegistrationInfo {
  mode: "open" | "first_user" | "invite" | "closed";
  /** The instance has no account yet: the first one registers freely and becomes the admin. */
  first_account: boolean;
  /** Anyone may register right now without an invite. */
  open: boolean;
  invite_required: boolean;
}

export type InviteStatus = "pending" | "used" | "expired" | "revoked";

/** Limits an invite presets on the new account (null: none). */
export interface InviteLimits {
  quota_bytes: number | null;
  daily_upload_bytes: number | null;
  max_file_bytes: number | null;
}

/** An entry of GET /auth/admin/invites. The token is never listed. */
export interface Invite {
  id: string;
  note: string | null;
  status: InviteStatus;
  created_at: string;
  created_by: string | null;
  expires_at: string;
  revoked_at: string | null;
  used_at: string | null;
  /** The address that used it. */
  used_by: string | null;
  limits: InviteLimits;
}

/** POST /auth/admin/invites and .../reissue: the link exists in this response only. */
export interface IssuedInvite {
  invite: Invite;
  token: string;
  /** `/register?invite=<token>`: prefix it with the site origin. */
  path: string;
}

export interface NewInvite {
  note?: string;
  expires_in_days?: number;
  quota_bytes?: number;
  daily_upload_bytes?: number;
  max_file_bytes?: number;
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
  /** Favorited. Absent on older servers (treated as false). */
  favorite?: boolean;
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

/** One calendar day (UTC) of the library and how many photos it has. */
export interface TimelineDay {
  /** `YYYY-MM-DD`. */
  day: string;
  count: number;
}

/** GET /assets/timeline: per-day photo counts, newest day first. The counts match what `GET /assets` lists. */
export interface Timeline {
  total: number;
  days: TimelineDay[];
}

/** A failed background job (GET /assets/processing). */
export interface ProcessingFailure {
  job_id: number;
  asset_id: string;
  original_name: string;
  /** `metadata`, `thumbnail` or `pair`. */
  kind: string;
  /** Truncated by the server; null if none was recorded. */
  last_error: string | null;
  attempts: number;
}

/** GET /assets/processing: progress of the background work (thumbnails, metadata, pairing) on your own photos. */
export interface Processing {
  /** Something is queued or running. */
  active: boolean;
  queued: number;
  running: number;
  failed: number;
  by_kind: { kind: string; queued: number; running: number; failed: number }[];
  /** Photos in the library. */
  total_photos: number;
  /** Of those, how many still wait for a thumbnail. */
  without_thumbnail: number;
  /** The most recent failures (at most 20); `failed` counts them all. */
  failures: ProcessingFailure[];
  /** Search indexing, reported apart from `active` (it can take hours for a big library). Absent on older servers. */
  embedding?: {
    indexed: number;
    total: number;
    queued: number;
    running: number;
    failed: number;
    /** The configured model ("siglip2-b16") and how much of the library it has indexed. Absent on older servers. */
    model?: string;
    percent?: number;
    /** An older model still answers searches while the configured one indexes the library ("re-indexing with the improved model"). */
    reindexing?: boolean;
    /** The percentage at which the configured model takes over. */
    switch_at_percent?: number;
    serving_model?: string;
  };
}

/** The structured filters of a search, and the optional natural-language query (`GET /assets/search`). */
export interface SearchParams {
  q?: string;
  /** `YYYY-MM-DD` (UTC), inclusive. */
  from?: string;
  to?: string;
  favorite?: boolean;
  /** A camera as `facets().cameras` lists it. */
  camera?: string;
  /** A lens as `facets().lenses` lists it (the EXIF lens model). */
  lens?: string;
  /** A city, region or country name as `facets().places` lists it. */
  place?: string;
  type?: "jpeg" | "heic" | "raw" | "other";
  /** An album id. */
  album?: string;
  has_location?: boolean;
  /** Dates and places the server should not read out of `q`: `date`, `place` or `date,place` (what a chip's "x" sets). */
  skip?: string;
  /** "More like this": list photos similar to this photo (`GET /assets/{id}/similar`) instead of searching for `q`. */
  similar?: string;
}

/** A search result: a list item plus the similarity to the query (null for filter-only searches). */
export interface SearchItem extends AssetListItem {
  score: number | null;
  /** The model's own estimate that the photo matches (SigLIP 2 only). Small for keyword queries even when right. */
  probability?: number | null;
  /** `strong` is what the search shows; `weak` only after "Show weaker matches". */
  tier?: "strong" | "weak" | null;
  /** Rapid-fire shots of one moment collapsed into this result; this photo is the cover. Absent for a photo on its own. */
  burst?: Burst;
}

/** The other photos of a burst that are in the results (best first); `size` counts the cover too. */
export interface Burst {
  size: number;
  members: BurstMember[];
}
export interface BurstMember extends AssetListItem {
  score: number | null;
}

/** What the server read out of the query besides the words to search for. */
export interface ParsedChip {
  /** The `skip` value that turns it off. */
  kind: "date" | "place";
  label: string;
  /** The words of the query it replaced ("in June 2023"). */
  matched: string;
  from?: string;
  to?: string;
  value?: string;
  place_kind?: "city" | "region" | "country";
}

/** GET /assets/search */
export interface SearchResult {
  items: SearchItem[];
  next_cursor: string | null;
  /** Ranked by similarity to the query (at most a few hundred, best first), or newest first. */
  ranked: boolean;
  /** Photos matching the filters; only for the first page of a filter-only search. */
  total?: number | null;
  /** Ranked: how many strong matches the whole result has. */
  strong_count?: number | null;
  /** Ranked, without `weaker`: weaker matches exist behind the strong ones. `cursor` continues right after the last strong match. */
  weaker?: { count: number; cursor: string | null } | null;
  /** With a query: what was read out of it (dates, places) and what is left to search for. */
  parsed?: { text: string; chips: ParsedChip[] } | null;
  model?: string | null;
  /** An older model answers while the configured one indexes the library. */
  reindexing?: { model: string; indexed: number; total: number; percent: number; switch_at_percent: number } | null;
  /** "More like this": the photo the list is similar to. */
  similar_to?: string | null;
}

export interface Facet {
  value: string;
  count: number;
}

/** GET /assets/search/facets: what the filters can be set to, with counts over the whole library. */
export interface Facets {
  total: number;
  favorites: number;
  with_location: number;
  types: Facet[];
  cameras: Facet[];
  /** Absent on older servers. */
  lenses?: Facet[];
  places: (Facet & { kind: "country" | "region" | "city" })[];
  years: Facet[];
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
  /** The photo's caption, if it has one. */
  caption?: string | null;
  /** The RAW stacked behind this photo. Absent on older servers. */
  paired?: PairedFile[];
  thumbhash: string | null;
  /** Where it was taken, from the offline places dataset; null without GPS or when nothing is near. Absent on older servers. */
  place?: Place | null;
  original_url: string;
  thumb_url: string | null;
  preview_url: string | null;
}

/** Where a photo was taken: the nearest populated place (GeoNames), looked up offline. */
export interface Place {
  city: string;
  region: string | null;
  country: string;
  country_code: string | null;
}

/** GET /assets/{id}/details (the photo details panel). Every part is absent or null when the photo has nothing to show. */
export interface PhotoDetails {
  id: string;
  date: {
    /** RFC 3339, UTC. */
    instant: string;
    /** `YYYY-MM-DDTHH:MM:SS`: the wall-clock time in the photo's own zone (UTC when the zone is unknown). */
    local: string;
    /** `+02:00`; null when EXIF did not record a time zone. */
    offset: string | null;
    zone_known: boolean;
    weekday: string;
    /** `taken` is the camera's clock; `file` and `added` are the fallbacks for a photo without one. */
    basis: "taken" | "file" | "added";
    /** `YYYY-MM-DD` (UTC) the timeline files the photo under. */
    timeline_day: string;
  };
  location: {
    latitude: number;
    longitude: number;
    place: Place | null;
    altitude: string | null;
    altitude_m: number | null;
    direction: string | null;
    direction_deg: number | null;
  } | null;
  camera: { label: string | null; make: string | null; model: string | null; lens: string | null } | null;
  exposure: {
    line: string | null;
    iso: string | null;
    focal_length: string | null;
    focal_length_35mm: string | null;
    aperture: string | null;
    shutter: string | null;
    bias: string | null;
    flash: string | null;
    white_balance: string | null;
    metering: string | null;
    program: string | null;
  } | null;
  file: {
    name: string;
    format: string;
    raw: boolean;
    content_type: string;
    size_bytes: number;
    width: number | null;
    height: number | null;
    megapixels: string | null;
    color_space: string | null;
    software: string | null;
  };
  /** The photo's own file first, then the RAW of a pair. */
  files: { id: string; name: string; format: string; raw: boolean; size_bytes: number }[];
  albums: AlbumRef[];
  burst: { position: number; size: number } | null;
  /** The photo's caption. Absent on older servers. */
  caption?: string | null;
  /** What was edited and what the file itself says (for "Revert to original"). Absent on older servers. */
  edits?: {
    date_edited: boolean;
    location_edited: boolean;
    original_date: PhotoDetails["date"] | null;
    original_location: { latitude: number; longitude: number } | null;
  };
}

/** How `POST /assets/edits` changes the date: set it (zone `+02:00`, or null when unknown), move it, or put the file's back. */
export type DateEdit = { local: string; offset: string | null } | { shift_seconds: number } | { revert: true };
export type LocationEdit = { latitude: number; longitude: number } | { revert: true };
export type CaptionEdit = { text: string } | { revert: true };
export interface EditRequest {
  date?: DateEdit;
  location?: LocationEdit;
  caption?: CaptionEdit;
}

/** POST /assets/edits. `failed` are the ids that could not be written (retry only those: a shift would move the others twice). */
export interface EditResult {
  updated: number;
  unchanged: number;
  missing: number;
  failed: string[];
}

/** An entry of GET /assets/places: a place the user's own photos are in, with a point to put a photo on. */
export interface PlaceHit {
  city: string;
  region: string | null;
  country: string;
  country_code: string | null;
  latitude: number;
  longitude: number;
  count: number;
}

/** One marker of GET /assets/map: a photo (count 1) or a cluster of photos in one grid cell. */
export interface MapCluster {
  /** The photo when `count` is 1; otherwise an arbitrary photo of the cluster (not meant to be shown). */
  id: string;
  lat: number;
  lon: number;
  count: number;
  /** `[west, south, east, north]` of the cluster's photos; null for a single photo. */
  bounds: [number, number, number, number] | null;
  /** Single photos only (null for clusters, and until the worker has produced the derived images). */
  thumbhash: string | null;
  /** Presigned; same conditions as `thumbhash`. */
  thumb_url: string | null;
}

/** GET /assets/map */
export interface MapResponse {
  clusters: MapCluster[];
  /** Photos in the box. */
  total: number;
  /** Too many cells matched and some were dropped; zoom in. */
  truncated: boolean;
}

/** GET /assets/tiles: which basemap archives the server has. */
export interface TilesInstalled {
  world: boolean;
  detail: boolean;
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

/**
 * What a bulk action applies to: explicit ids, or "everything in this view except ..." resolved by the server, so
 * "Select all" works on photos the browser never loaded. `trash_batch` is what a scope-based trash returns, to undo it.
 */
export type Scope =
  | "library"
  | "favorites"
  | "trash"
  | { album: string }
  | { trash_batch: string }
  /** Every photo a filter-only search lists (no text query: a ranked search selects what it loaded). */
  | { search: Omit<SearchParams, "q"> };
export interface ScopeTarget {
  scope: Scope;
  except: readonly string[];
}
export type Target = readonly string[] | ScopeTarget;
export const isScope = (t: Target): t is ScopeTarget => !Array.isArray(t);

/** The result of a bulk call; `batch` identifies a scope-based trash (pass `{ scope: { trash_batch } }` to undo). */
export interface BulkResult {
  count: number;
  batch?: string;
}

/**
 * POST a target to a bulk endpoint. Ids go in chunks of MAX_BULK_IDS and resolve with the total number of items
 * affected; a scope is one request (the server does the work in one transaction).
 */
async function bulk(path: string, target: Target, extra: object = {}): Promise<BulkResult> {
  if (isScope(target)) {
    return request<BulkResult>(path, json({ scope: target.scope, except: target.except, ...extra }));
  }
  let count = 0;
  for (let i = 0; i < target.length; i += MAX_BULK_IDS) {
    const r = await request<{ count: number }>(path, json({ ids: target.slice(i, i + MAX_BULK_IDS), ...extra }));
    count += r.count;
  }
  return { count };
}

/** The query string of `GET /assets/search`: only the fields that are set. */
export function searchQuery(
  params: SearchParams,
  opts: { cursor?: string | null; limit?: number; weaker?: boolean } = {},
): string {
  const q = new URLSearchParams();
  // A fixed order, so the same search is always the same URL (cache keys, tests).
  // (`similar` is part of the path of "More like this", and that endpoint takes neither `q` nor `skip`.)
  const keys = (
    params.similar
      ? ["from", "to", "favorite", "camera", "lens", "place", "type", "album", "has_location"]
      : ["q", "from", "to", "favorite", "camera", "lens", "place", "type", "album", "has_location", "skip"]
  ) as readonly (keyof SearchParams)[];
  for (const k of keys) {
    const v = params[k];
    // `favorite: false` means "no filter"; `has_location: false` is a real filter ("photos without a location").
    if (v === undefined || v === "" || (k === "favorite" && v === false)) continue;
    q.set(k, String(v));
  }
  if (opts.limit !== undefined) q.set("limit", String(opts.limit));
  if (opts.cursor) q.set("cursor", opts.cursor);
  if (opts.weaker) q.set("weaker", "true");
  return q.toString();
}

export const api = {
  me: () => request<Me>("/auth/me"),
  login: (email: string, password: string) => request<{ id: string }>("/auth/login", json({ email, password })),
  /** 403 when registration is closed (REGISTRATION_MODE=closed, or first_user with an owner already present). */
  register: (email: string, password: string, invite?: string) =>
    request<Me>("/auth/register", json(invite ? { email, password, invite } : { email, password })),
  registrationInfo: () => request<RegistrationInfo>("/auth/registration"),
  /** Whether an invite token can still be used (always 200). */
  checkInvite: (token: string) =>
    request<{ valid: boolean; reason?: "unknown" | "used" | "revoked" | "expired"; expires_at?: string }>(
      "/auth/invites/check",
      json({ token }),
    ),
  listInvites: () => request<{ items: Invite[] }>("/auth/admin/invites"),
  createInvite: (body: NewInvite) => request<IssuedInvite>("/auth/admin/invites", json(body)),
  /** A fresh link for an unused invite; the previous link stops working. */
  reissueInvite: (id: string, expiresInDays?: number) =>
    request<IssuedInvite>(
      `/auth/admin/invites/${encodeURIComponent(id)}/reissue`,
      json(expiresInDays === undefined ? {} : { expires_in_days: expiresInDays }),
    ),
  revokeInvite: (id: string) => request<void>(`/auth/admin/invites/${encodeURIComponent(id)}`, { method: "DELETE" }),
  logout: () => request<void>("/auth/logout", { method: "POST" }),
  listApiKeys: () => request<{ items: ApiKey[] }>("/auth/api-keys"),
  createApiKey: (name: string) => request<CreatedApiKey>("/auth/api-keys", json({ name })),
  revokeApiKey: (id: string) => request<void>(`/auth/api-keys/${encodeURIComponent(id)}`, { method: "DELETE" }),
  listAssets: (
    opts: { cursor?: string | null; before?: string | null; limit?: number; favorite?: boolean; bbox?: string } = {},
  ) => {
    const q = new URLSearchParams();
    if (opts.bbox) q.set("bbox", opts.bbox);
    if (opts.limit !== undefined) q.set("limit", String(opts.limit));
    if (opts.favorite) q.set("favorite", "true");
    if (opts.cursor) q.set("cursor", opts.cursor);
    else if (opts.before) q.set("before", opts.before);
    const qs = q.toString();
    return request<AssetList>(`/assets${qs ? `?${qs}` : ""}`);
  },
  /** Photos per day, newest first (the library's layout; `favorite` limits it to favorites). */
  timeline: (opts: { favorite?: boolean } = {}) =>
    request<Timeline>(`/assets/timeline${opts.favorite ? "?favorite=true" : ""}`),
  /** The geotagged photos in `bbox` (`west,south,east,north`) as markers for a map at `zoom`. */
  mapClusters: (bbox: string, zoom: number, signal?: AbortSignal) =>
    request<MapResponse>(`/assets/map?bbox=${encodeURIComponent(bbox)}&zoom=${zoom}`, { signal }),
  /** Which basemap archives are installed (the map draws a plain background without one). */
  tiles: () => request<TilesInstalled>("/assets/tiles"),
  /** Favorite or unfavorite photos. `count` is how many changed. */
  favoriteAssets: (target: Target, favorite: boolean) => bulk("/assets/favorite", target, { favorite }),
  /** Move photos to the trash (reversible). `count` is how many actually moved. */
  trashAssets: (target: Target) => bulk("/assets/trash", target),
  /** Bring photos back from the trash. */
  restoreAssets: (target: Target) => bulk("/assets/restore", target),
  /** Delete trashed photos forever. Not reversible. */
  purgeAssets: (target: Target) => bulk("/assets/purge", target),
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
  addToAlbum: (id: string, target: Target) => bulk(`/assets/albums/${encodeURIComponent(id)}/assets`, target),
  /** Take photos out of an album; they stay in the library. */
  removeFromAlbum: (id: string, target: Target) =>
    bulk(`/assets/albums/${encodeURIComponent(id)}/assets/remove`, target),
  /** Separate a RAW + JPEG pair into two photos (either file's id); they are not paired again automatically. */
  unpair: (id: string) => request<void>(`/assets/${encodeURIComponent(id)}/unpair`, { method: "POST" }),
  /** Natural-language and/or filtered search; `cursor` from the previous page's `next_cursor`. */
  search: (params: SearchParams, opts: { cursor?: string | null; limit?: number; weaker?: boolean } = {}) =>
    request<SearchResult>(
      params.similar
        ? `/assets/${encodeURIComponent(params.similar)}/similar?${searchQuery(params, opts)}`
        : `/assets/search?${searchQuery(params, opts)}`,
    ),
  searchFacets: () => request<Facets>("/assets/search/facets"),
  processing: () => request<Processing>("/assets/processing"),
  retryFailedJobs: () => request<{ requeued: number }>("/assets/processing/retry", { method: "POST" }),
  getAsset: (id: string) => request<AssetDetail>(`/assets/${encodeURIComponent(id)}`),
  /** Edit the date, location or caption of photos (at most 1000; a scope that resolves to more is refused). */
  editAssets: (target: Target, edit: EditRequest) =>
    request<EditResult>(
      "/assets/edits",
      json(isScope(target) ? { scope: target.scope, except: target.except, ...edit } : { ids: target, ...edit }),
    ),
  /** The places the user's own photos are in (`q`: the start of a word of the city, region or country). */
  places: (q = "", signal?: AbortSignal) =>
    request<PlaceHit[]>(`/assets/places${q ? `?${new URLSearchParams({ q })}` : ""}`, { signal }),
  /** What the details panel shows: date, place, camera, exposure, files, albums, burst. */
  getDetails: (id: string) => request<PhotoDetails>(`/assets/${encodeURIComponent(id)}/details`),
  /** Dedupe + part URLs. Defaults to the proxy transport, the only one a browser can use. */
  checkUpload: (req: CheckRequest) => request<CheckResponse>("/assets/check", json({ transport: "proxy", ...req })),
  /** `url` is a `parts[].url` from `checkUpload` (relative, same origin). */
  putPart,
  completeUpload: (uploadId: string, parts: DonePart[]) =>
    request<{ asset_id: string }>("/assets/complete", json({ upload_id: uploadId, parts })),
};

/**
 * Where to point a plain link to save an original. The API answers with a redirect to a short-lived presigned URL
 * whose response headers say `attachment; filename="<original name>"`, so the browser downloads straight from the
 * bucket (a navigation, not fetch(): no CORS policy needed, no bytes through the API). `file` is the id of the RAW
 * of a pair (from `paired`); leave it out for the photo's own file.
 */
export const downloadHref = (id: string, file?: string) =>
  `/assets/${encodeURIComponent(id)}/download${file ? `?file=${encodeURIComponent(file)}` : ""}`;

export type Api = typeof api;
