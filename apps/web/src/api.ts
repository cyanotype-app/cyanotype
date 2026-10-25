// Hand-written client for the Cyanotype API (same origin, session cookie). The types mirror the Rust
// structs in apps/auth/src/routes.rs and apps/assets/src/routes.rs; generating this from a utoipa
// OpenAPI document is a planned follow-up (see docs/build.md).

import type { CliAuthorizeParams } from "./cliLoginText";
import type { Crop, ServerRecipe } from "./recipeText";

/** GET /auth/me */
export interface Me {
  id: string;
  email: string;
  /** Absent on older servers (treated as member). */
  role?: "admin" | "member";
  /** This account has two-factor authentication on. Absent on older servers. */
  two_factor?: boolean;
  /** The administrator requires 2FA and this session may only enrol (every other call is refused until it does). */
  mfa_enrol_required?: boolean;
}

/** What a correct password returns: a session (the cookie is set), or a challenge to answer with a code. */
export type LoginResult = { id: string; mfa_enrol_required?: boolean } | { mfa_required: true; challenge: string };

/** GET /auth/2fa */
export interface TwoFactorStatus {
  /** The server has AUTH_TOTP_KEY, so enrolment can work. */
  available: boolean;
  enabled: boolean;
  recovery_codes_remaining: number;
  /** The administrator requires 2FA (and this server can enforce it). */
  required: boolean;
}

/** POST /auth/2fa/enrol: shown once, until a code confirms it. */
export interface TwoFactorEnrolment {
  secret: string;
  otpauth_uri: string;
  issuer: string;
  account: string;
}

/** An account in GET /auth/admin/2fa. */
export interface TwoFactorAccount {
  id: string;
  email: string;
  role: "admin" | "member";
  enabled: boolean;
  since: string | null;
  recovery_codes_remaining: number;
}

export interface TwoFactorOverview {
  key_configured: boolean;
  required: boolean;
  enforced: boolean;
  users: TwoFactorAccount[];
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

/** An account's limits; null is unlimited. */
export interface Limits {
  quota_bytes: number | null;
  daily_upload_bytes: number | null;
  max_file_bytes: number | null;
}

/** GET /assets/usage: what the caller stores and uploaded lately. */
export interface MyUsage {
  originals_bytes: number;
  trash_bytes: number;
  /** originals + trash: what counts toward a quota (derived images do not). */
  stored_bytes: number;
  derived_bytes: number;
  /** False while some thumbnails have not been measured yet: `derived_bytes` is then a lower bound. */
  derived_complete: boolean;
  photo_count: number;
  raw_count: number;
  file_count: number;
  trash_count: number;
  uploads: { count_24h: number; bytes_24h: number; count_30d: number; bytes_30d: number };
  /** Absent on older servers (no limits). */
  limits?: Limits;
  /** The rolling day as the daily cap counts it (completed uploads plus unfinished ones). */
  daily?: { used_bytes: number; oldest_rolls_out_at: string | null };
}

/** An entry of GET /assets/admin/accounts: counts and sizes only. */
export interface AdminAccount {
  id: string;
  email: string;
  role: "admin" | "member";
  created_at: string;
  last_active: string | null;
  originals_bytes: number;
  trash_bytes: number;
  derived_bytes: number;
  photo_count: number;
  raw_count: number;
  file_count: number;
  trash_count: number;
  derived_files: number;
  uploads_24h_count: number;
  uploads_24h_bytes: number;
  uploads_30d_count: number;
  uploads_30d_bytes: number;
  albums: number;
  shares: number;
  api_keys: number;
  quota_bytes?: number | null;
  daily_upload_bytes?: number | null;
  max_file_bytes?: number | null;
  /** RFC 3339 when the account was disabled (it cannot sign in and its API keys are refused), else null. */
  disabled_at?: string | null;
  /** The latest deletion of this account, if one was requested: queued, running or failed. */
  deletion?: "queued" | "running" | "failed" | null;
  deletion_error?: string | null;
}

/** A session as listed (never its token). */
export interface SessionInfo {
  id: string;
  created_at: string;
  expires_at: string;
  /** Only in the person's own list: this browser's session. */
  current?: boolean;
}

/** POST /auth/admin/users/{id}/reset-link: the link exists in this response only. */
export interface ResetLink {
  token: string;
  /** `/reset-password?token=<token>`: prefix it with the site origin. */
  path: string;
  email: string;
  expires_at: string;
}

export interface AuditEntry {
  id: number;
  at: string;
  actor_id: string | null;
  actor_email: string | null;
  action: string;
  target_type: string | null;
  target_id: string | null;
  detail: Record<string, unknown>;
}

export interface AuditQuery {
  action?: string;
  actor?: string;
  target?: string;
  /** `YYYY-MM-DD`. */
  from?: string;
  to?: string;
  before?: number;
  limit?: number;
}

/** GET /auth/admin/audit */
export interface AuditPage {
  items: AuditEntry[];
  /** Pass as `before` for the next (older) page; null on the last. */
  next_before: number | null;
  /** Every action name in the log, for the filter. */
  actions: string[];
}

/** GET /assets/admin/health */
export interface Health {
  version: { assets: string; workers: { host: string; version: string | null; reporting: boolean }[] };
  queue: { kind: string; queued: number; running: number; failed: number; oldest_queued_age_secs: number }[];
  stalled_seconds: number;
  failed_jobs: { id: number; kind: string; attempts: number; last_error: string | null; finished_at: string | null }[];
  workers: {
    host: string;
    reporting: boolean;
    at: string;
    version: string | null;
    rss_bytes: number | null;
    budget_used_bytes: number | null;
    budget_total_bytes: number | null;
  }[];
  backup: { prefix: string; count?: number; last_at?: string | null; last_size_bytes?: number | null; error?: string };
  storage: {
    database_bytes: number;
    stored_bytes: number;
    derived_bytes: number;
    bucket_check: { at: string | null; current_bytes: number | null; noncurrent_bytes: number | null; objects: number | null } | null;
  };
}

export type IntegrityKind = "missing_original" | "untracked_object" | "checksum_mismatch";
export type IntegrityAction = "restore_version" | "trash" | "acknowledge" | "restore_photo" | "delete_object";

/** One run of the storage integrity check (GET /assets/admin/integrity). */
export interface IntegrityRun {
  id: number;
  trigger: "schedule" | "manual";
  state: "running" | "done" | "failed";
  started_at: string;
  finished_at: string | null;
  error: string | null;
  objects_listed: number;
  assets_checked: number;
  assets_verified: number;
  bytes_hashed: number;
  missing_originals: number;
  untracked_objects: number;
  checksum_mismatches: number;
  derived_requeued: number;
  findings_resolved: number;
}

export interface IntegritySummary {
  last_run: IntegrityRun | null;
  open: Record<IntegrityKind, number>;
  acknowledged: number;
  coverage: { assets: number; verified: number; oldest_verified_at: string | null };
  task: { state: "queued" | "running"; created_at: string; started_at: string | null } | null;
}

export interface IntegrityFinding {
  id: number;
  kind: IntegrityKind;
  state: "open" | "acknowledged" | "resolved";
  owner_id: string | null;
  owner_email: string | null;
  asset_id: string | null;
  key: string;
  detail: {
    reason?: "size" | "hash" | "no_row" | "duplicate_content" | "unrecognised_key";
    size_bytes?: number;
    actual_bytes?: number;
    expected_sha256?: string;
    actual_sha256?: string;
    sha256?: string;
    previous_versions?: number | null;
    owner_known?: boolean;
  };
  resolution: string | null;
  first_seen_at: string;
  last_seen_at: string;
  resolved_at: string | null;
}

export interface IntegrityFindingsPage {
  items: IntegrityFinding[];
  next_before: number | null;
}

export interface IntegrityQuery {
  kind?: IntegrityKind;
  state?: "open" | "acknowledged" | "resolved";
  before?: number;
}

export interface AdminTask<R = unknown> {
  id: number;
  kind: string;
  state: "queued" | "running" | "done" | "failed";
  result: R | null;
  error: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}

/** The result of a bucket check task. */
export interface BucketReport {
  finished_at: string;
  versioned: boolean;
  objects: number;
  current_bytes: number;
  originals_bytes: number;
  derived_bytes: number;
  manifests_bytes: number;
  edits_bytes: number;
  other_bytes: number;
  noncurrent_versions: number;
  noncurrent_bytes: number;
  delete_markers: number;
  db_originals_bytes: number;
  db_derived_bytes: number;
  db_derived_files: number;
  db_files: number;
  unknown_account_bytes: number;
  mismatched: { user_id: string; email: string; bucket_originals_bytes: number; db_originals_bytes: number }[];
}

/** GET /assets/admin/accounts */
export interface AdminAccounts {
  items: AdminAccount[];
  totals: {
    accounts: number;
    originals_bytes: number;
    trash_bytes: number;
    derived_bytes: number;
    derived_complete: boolean;
    photo_count: number;
    raw_count: number;
    uploads_24h_count: number;
    uploads_24h_bytes: number;
    uploads_30d_count: number;
    uploads_30d_bytes: number;
  };
  /** The last nightly reconcile (null before the first). */
  reconcile: { at: string; users_checked: number; drifted_users: number; drift_bytes: number } | null;
  bucket_check: AdminTask<BucketReport> | null;
  /** The limits accounts created from now on start with. */
  default_limits?: Limits;
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
  /** What the key may do. Keys made before scopes existed are `full`. */
  scope: ApiKeyScope;
  expires_at: string | null;
  expired: boolean;
}

export type ApiKeyScope = "import" | "read" | "full";

/** An API key in an admin's view of an account (GET /auth/admin/users/{id}/api-keys). */
export interface AccountApiKey {
  id: string;
  name: string;
  prefix: string;
  scope: ApiKeyScope;
  created_at: string;
  last_used_at: string | null;
  expires_at: string | null;
}

/** What a command-line login request is, as the server read it (POST /auth/cli/authorize/check, /auth/cli/device/lookup). */
export interface CliRequest {
  /** The device name as the server will store it (cut down to plain text). */
  device: string;
  scope: ApiKeyScope;
  /** Device flow only: seconds the code still works. */
  expires_in?: number;
}

/** POST /auth/api-keys: `key` is the plaintext secret, returned only once. */
export interface CreatedApiKey extends Omit<ApiKey, "last_used_at" | "expired"> {
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
  /** Star rating, 0 (not rated) to 5. Absent on older servers (treated as 0). */
  rating?: number;
  /** In the archive: off the timeline and the map; shown with a badge in search and albums. Absent on older servers. */
  archived?: boolean;
  /** Hidden: only the unlocked Hidden view lists such a photo. Absent on older servers. */
  hidden?: boolean;
  /** Rotated, straightened or cropped; `width` and `height` are then the edited picture's. Absent on older servers. */
  edited?: boolean;
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
  /** The stack this photo is in (two or more photos shown as one tile); only the timeline's list carries it. */
  stack?: StackInfo;
}

/** What a timeline entry says about its stack: how many photos the view lists in it, and, while the stack is collapsed to
 * this tile, their ids (this photo's too), so that selecting the tile means all of them. */
export interface StackInfo {
  size: number;
  members?: string[];
}

/** GET /assets/{id}/stack: the photos of a stack in capture order, and which one is the cover. */
export interface StackDetail {
  cover_id: string;
  /** Made by hand, rather than found by the worker as a burst. */
  manual: boolean;
  items: AssetListItem[];
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

/** One line of recognised text: its words and where they are, all coordinates 0..1 of the picture (x0,y0 ... x3,y3: top-left, top-right, bottom-right, bottom-left). */
export interface PhotoTextLine {
  t: string;
  /** Confidence 0..1. */
  s: number;
  q: number[];
  w: { t: string; q: number[] }[];
}

export interface PhotoTextQr {
  /** What the code says. Shown as text; a web address is offered as a link, never opened. */
  t: string;
  q: number[];
}

/** GET /assets/{id}/text. `pending`: the photo has not been read yet (it is, in the background, after processing). */
export interface PhotoText {
  status: "read" | "pending";
  text: string;
  lines: PhotoTextLine[];
  qr: PhotoTextQr[];
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
  /** Reading text off photos, reported apart from `active` (a backfill takes hours). Absent on older servers. */
  text?: { read: number; total: number; queued: number; running: number; failed: number; model: string; percent: number };
}

/** The structured filters of a search, and the optional natural-language query (`GET /assets/search`). */
export interface SearchParams {
  q?: string;
  /** `YYYY-MM-DD` (UTC), inclusive. */
  from?: string;
  to?: string;
  favorite?: boolean;
  /** Tag names, comma-separated (a tag name has no comma); photos with all of them, or any with `tag_mode: "any"`. */
  tags?: string;
  /** `any`: at least one of `tags`. The default (all of them) is not sent. */
  tag_mode?: "any";
  /** At least this many stars (1 to 5). */
  rating?: number;
  /** `false` leaves archived photos out. The server's default is to include them. */
  include_archived?: boolean;
  /** A camera as `facets().cameras` lists it. */
  camera?: string;
  /** A lens as `facets().lenses` lists it (the EXIF lens model). */
  lens?: string;
  /** A city, region or country name as `facets().places` lists it. */
  place?: string;
  /** Person ids, comma-separated: photos where all of them are together (People only; the server ignores it while People is off). */
  people?: string;
  type?: "jpeg" | "heic" | "raw" | "other";
  /** An album id. */
  album?: string;
  has_location?: boolean;
  /** Only photos with (true) or without (false) text read off them. */
  has_text?: boolean;
  /** Dates, places and people the server should not read out of `q`: `date`, `place`, `people`, or several (what a chip's "x" sets). */
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
  kind: "date" | "place" | "people";
  label: string;
  /** The words of the query it replaced ("in June 2023"). */
  matched: string;
  from?: string;
  to?: string;
  value?: string;
  place_kind?: "city" | "region" | "country";
  /** For `people`: who was read out of the query, in the order written. */
  people?: { id: string; name: string }[];
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
  /** Photos with text read off them. Absent on older servers. */
  with_text?: number;
  /** "At least N stars" counts, 5 down to 1 (levels with no photo are left out). Absent on older servers. */
  ratings?: Facet[];
  types: Facet[];
  cameras: Facet[];
  /** Absent on older servers. */
  lenses?: Facet[];
  places: (Facet & { kind: "country" | "region" | "city" })[];
  years: Facet[];
}

/** One photo of a duplicate group, with what the review compares. */
export interface DuplicateMember extends AssetSummary {
  thumbhash: string | null;
  thumb_url: string | null;
  /** Fields in the file's EXIF dump. */
  exif_fields: number;
  has_location: boolean;
  has_caption: boolean;
  tag_count: number;
  /** The photo to keep unless another is chosen: the largest resolution, then the most metadata, then a favorite. */
  best: boolean;
}

export interface DuplicateGroup {
  id: string;
  members: DuplicateMember[];
}

/** The state of the library's near-duplicate check. */
export interface DuplicateScan {
  state: "never" | "wanted" | "running" | "idle";
  requested_at?: string | null;
  started_at?: string | null;
  finished_at?: string | null;
  groups?: number | null;
  photos?: number | null;
  error?: string | null;
  manual?: boolean;
}

/** GET /assets/duplicates */
export interface DuplicatesPage {
  groups: DuplicateGroup[];
  /** Groups with at least two photos this session may see. */
  total: number;
  next_cursor: string | null;
  scan: DuplicateScan;
}

/** What keeping one photo did: the kept photo, the ids that went to the trash (for Undo) and what was merged onto it. */
export interface DuplicateKept {
  kept: string;
  trashed: string[];
  merged: Record<string, unknown>;
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

/** One rule of a smart album (`smart.rs` in the assets service). */
export type SmartRule =
  | { field: "date"; from?: string; to?: string }
  | { field: "place" | "camera" | "lens"; value: string }
  | { field: "type"; value: "jpeg" | "heic" | "raw" | "other" | string }
  | { field: "favorite"; value: true }
  | { field: "rating"; min: number }
  | { field: "tags"; names: string[]; mode: "all" | "any" }
  | { field: "has_location" | "has_text"; value: boolean }
  | { field: "caption"; text: string }
  /** All of these people are in the photo (People only). */
  | { field: "people"; ids: string[] };

/** The saved definition of a smart album: its rules, how they are joined, and whether archived photos are in. */
export interface SmartRules {
  match: "all" | "any";
  archived: "include" | "exclude";
  rules: SmartRule[];
}

/** An entry of GET /assets/albums and the body of GET /assets/albums/{id}. Counts and dates skip trashed photos. */
export interface Album {
  id: string;
  title: string;
  description: string | null;
  /** A smart album: its photos are what its rules match when it is opened. Absent on older servers. */
  smart?: boolean;
  rules?: SmartRules | null;
  photo_count: number;
  /** A smart album with more matches than are counted (`photo_count` is then 10,000). */
  photo_count_capped?: boolean;
  /** Members that are hidden: still in the album, listed nowhere in it. Absent on older servers. */
  hidden_count?: number;
  /** Null while the album is empty. */
  cover: AlbumCover | null;
  /** RFC 3339 times of the earliest and latest photo; null while empty. */
  first_at: string | null;
  last_at: string | null;
  created_at: string;
  updated_at: string;
}

/** POST /assets/albums/preview: what the rules would match now. */
export interface SmartPreview {
  count: number;
  /** More matches than are counted (`count` is then 10,000). */
  capped: boolean;
  items: AssetListItem[];
  rules: SmartRules;
}

/** PATCH /assets/albums/{id}: omitted fields stay; `null` clears the description or the chosen cover. */
export interface AlbumPatch {
  title?: string;
  description?: string | null;
  cover_asset_id?: string | null;
  /** New rules of a smart album. */
  rules?: SmartRules;
}

/** A face's box: left, top, width, height, as fractions (0 to 1) of the photo's preview image. */
export type FaceBox = [number, number, number, number];

/** The face a person is shown by: the box is in the picture `preview_url` serves (short-lived, like the other presigned URLs). */
export interface PersonCover {
  face_id: string;
  asset_id: string;
  box: FaceBox;
  preview_url: string;
}

/** A person (docs/faces.md, "People"); `name` is null until the owner gives one. Counts are of the photos this session may see. */
export interface Person {
  id: string;
  name: string | null;
  hidden: boolean;
  faces: number;
  photos: number;
  /** "Is this X?" suggestions waiting for an answer (named, shown people only; 0 or absent otherwise). */
  pending?: number;
  cover: PersonCover | null;
}

/** A face a person's cover can be chosen from (GET /assets/people/{id}/faces). */
export interface CoverFace {
  face_id: string;
  asset_id: string;
  box: FaceBox;
  preview_url: string;
}

/** One change to several people at once (POST /assets/people/bulk). */
export type PeopleBulk =
  | { action: "hide" | "show" | "delete"; ids: readonly string[] }
  | { action: "merge"; ids: readonly string[]; into: string; name?: string | null };

/** A face in one photo (GET /assets/{id}/faces); `person` is null for a face nobody has claimed. */
export interface PhotoFace {
  id: string;
  box: FaceBox;
  person: { id: string; name: string | null } | null;
}

/** A detection the owner marked "Not a face" (GET /assets/{id}/faces `dismissed`): kept so it can be brought back. */
export interface DismissedFace {
  id: string;
  box: FaceBox;
}

/** An unassigned face that looks like a person ("Is this X?"). */
export interface FaceSuggestion {
  face_id: string;
  asset_id: string;
  box: FaceBox;
  preview_url: string;
  similarity: number;
}

/** A tag of the signed-in user, with how many photos have it (as this session may see them). */
export interface Tag {
  id: string;
  name: string;
  count: number;
}

/** A tag on one photo; `imported` came from the photo's own keywords, not from the owner. */
export interface PhotoTag {
  id: string;
  name: string;
  imported: boolean;
}

/** GET /assets/{id}. The URLs are presigned and short-lived. */
export interface AssetDetail extends AssetSummary {
  /** The albums this photo is in. Absent on older servers. */
  albums?: AlbumRef[];
  /** The photo's caption, if it has one. */
  caption?: string | null;
  /** The photo's tags, by name. Absent on older servers. */
  tags?: PhotoTag[];
  /** The RAW stacked behind this photo. Absent on older servers. */
  paired?: PairedFile[];
  thumbhash: string | null;
  /** Where it was taken, from the offline places dataset; null without GPS or when nothing is near. Absent on older servers. */
  place?: Place | null;
  original_url: string;
  thumb_url: string | null;
  preview_url: string | null;
  /** The rotate/straighten/crop recipe, null when the picture is shown as the file is. Absent on older servers. */
  recipe?: ServerRecipe | null;
  /** The worker is still rendering the thumbnail and preview (after an edit). */
  rendering?: boolean;
  /** The file's own size in pixels (`width` / `height` are the edited picture's while there is a recipe). */
  file_width?: number | null;
  file_height?: number | null;
  /** The whole upright picture, to work on in the rotate/crop editor (the preview is already cropped); null until rendered. */
  edit_base_url?: string | null;
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
    /** The rotate/straighten/crop recipe. Absent on older servers. */
    recipe?: ServerRecipe | null;
    /** The edited JPEG is ready, so "Download edited" answers at once. */
    edited_copy_ready?: boolean;
  };
}

/** How `POST /assets/edits` changes the date: set it (zone `+02:00`, or null when unknown), move it, or put the file's back. */
export type DateEdit = { local: string; offset: string | null } | { shift_seconds: number } | { revert: true };
export type LocationEdit = { latitude: number; longitude: number } | { revert: true };
export type CaptionEdit = { text: string } | { revert: true };
/** A recipe to set, a relative quarter turn (`turn`: 90, -90, 180; the crop turns with the picture), or the original picture back. */
export type RecipeEdit = { rotate?: number; straighten?: number; crop?: Crop } | { turn: number } | { revert: true };
export interface EditRequest {
  date?: DateEdit;
  location?: LocationEdit;
  caption?: CaptionEdit;
  recipe?: RecipeEdit;
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
  /** Google Takeout metadata for this file: the sidecar's text and the album folder. The server holds it by hash and applies it
   *  as an edit after the photo is processed (docs/import.md). */
  takeout?: { sidecar?: string; album?: string; favorites: boolean };
}

/** POST /assets/takeout/plan: the server's pairing of an export's file names. */
export interface TakeoutPlanItem {
  path: string;
  sidecar: string | null;
  album: string | null;
  /** An edited copy that shares its original's sidecar. */
  edited: boolean;
}

export interface TakeoutPlan {
  photos: TakeoutPlanItem[];
  videos: string[];
  other: string[];
  trashed: string[];
  albums: Record<string, number>;
  unused_sidecars: string[];
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
  /** The server's machine-readable reason (`quota_exceeded`, `daily_cap_exceeded`, ...), when it gave one. */
  readonly code?: string;
  /** The rest of an error body (sizes, `retry_at`), when it had more than a message. */
  readonly details?: Record<string, unknown>;
  constructor(status: number, message: string, code?: string, details?: Record<string, unknown>) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.details = details;
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
    const code =
      typeof body === "object" && body !== null && "code" in body && typeof body.code === "string" ? body.code : undefined;
    throw new ApiError(res.status, msg, code, code ? (body as Record<string, unknown>) : undefined);
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
  | "archive"
  /** The Hidden view: refused unless the session has unlocked it. */
  | "hidden"
  | "trash"
  | { album: string }
  /** One of the owner's people (People only): the photos their page lists. */
  | { person: string }
  | { trash_batch: string }
  | { archive_batch: string }
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
      ? ["from", "to", "favorite", "tags", "tag_mode", "rating", "include_archived", "camera", "lens", "place", "people", "type", "album", "has_location", "has_text"]
      : ["q", "from", "to", "favorite", "tags", "tag_mode", "rating", "include_archived", "camera", "lens", "place", "people", "type", "album", "has_location", "has_text", "skip"]
  ) as readonly (keyof SearchParams)[];
  for (const k of keys) {
    const v = params[k];
    // `favorite: false` means "no filter"; `has_location: false` is a real filter ("photos without a location").
    // `include_archived: true` is the server's default, so only `false` is sent.
    if (v === undefined || v === "" || (k === "favorite" && v === false) || (k === "include_archived" && v === true)) continue;
    q.set(k, String(v));
  }
  if (opts.limit !== undefined) q.set("limit", String(opts.limit));
  if (opts.cursor) q.set("cursor", opts.cursor);
  if (opts.weaker) q.set("weaker", "true");
  return q.toString();
}

/** Which photos a library list or its day counts are about (the server's views). */
export interface LibraryView {
  favorite?: boolean;
  archived?: boolean;
  hidden?: boolean;
  /** `taken` (default) or `added` (upload time). */
  sort?: "taken" | "added";
  /** One person's photos (People): the list and the day counts of `/assets/people/{id}`, nothing else of the view applies. */
  person?: string;
  /** Stacks in the timeline: one tile each (`collapsed`) or every photo. Absent: the user's setting. */
  stacks?: "collapsed" | "expanded";
}

function viewQuery(v: LibraryView, q: URLSearchParams) {
  if (v.stacks && !v.favorite && !v.archived && !v.hidden && !v.person) q.set("stacks", v.stacks);
  if (v.favorite) q.set("favorite", "true");
  if (v.archived) q.set("archived", "true");
  if (v.hidden) q.set("hidden", "true");
  if (v.sort === "added") q.set("sort", "added");
}

/** A photo of "On this day" (GET /assets/memories). */
export interface MemoryYear {
  year: number;
  years_ago: number;
  /** Photos of that year on this day, bursts collapsed (the strip carries only the first few). */
  count: number;
  items: AssetListItem[];
}
export interface Memories {
  /** The strip is switched on in the user's settings. */
  enabled: boolean;
  /** `MM-DD`. */
  day: string;
  years: MemoryYear[];
}

/** The per-user switches (GET/PUT /assets/prefs). */
export interface Prefs {
  memories_enabled: boolean;
  library_sort: "taken" | "added";
  /** Stacks show as one tile in the timeline (default); off lists every photo. */
  stacks_collapsed: boolean;
}

/** Whether this session has the Hidden view open (GET /auth/hidden). */
export interface HiddenState {
  unlocked: boolean;
  expires_at: string | null;
}

/** The order a slideshow runs through a view in: as the page lists it (newest first), oldest first, or shuffled. */
export type SlideOrder = "listed" | "oldest" | "shuffle";

/** GET /assets/slideshow: a page of a view in one order. `next_cursor` is null on the last page. */
export interface SlidePage {
  items: AssetListItem[];
  next_cursor: string | null;
  /** Photos in the whole view. */
  total: number;
}

/** GET /assets/previews: a fresh (short-lived) preview address of a photo about to be shown. */
export interface PreviewInfo {
  id: string;
  preview_url: string;
  width: number | null;
  height: number | null;
  caption: string | null;
}

/** Today's date in the user's own zone, `YYYY-MM-DD` (the day "On this day" looks up). */
export function localToday(now: Date = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;
}

export const api = {
  /** The photos of a bulk-action scope in an order (`seed` fixes a shuffle), `limit` at a time from `cursor`. */
  slideshowPage: (scope: Scope, opts: { order: SlideOrder; seed: string; cursor?: string | null; limit?: number }) => {
    const q = new URLSearchParams({ scope: JSON.stringify(scope), order: opts.order });
    if (opts.seed) q.set("seed", opts.seed);
    if (opts.limit !== undefined) q.set("limit", String(opts.limit));
    if (opts.cursor) q.set("cursor", opts.cursor);
    return request<SlidePage>(`/assets/slideshow?${q}`);
  },
  /** Preview addresses for up to 24 photos; one that is gone, hidden or not rendered yet is left out. */
  previews: (ids: readonly string[]) => request<{ items: PreviewInfo[] }>(`/assets/previews?ids=${ids.join(",")}`),
  /** Open the Hidden view for ten minutes with the account password: 403 `wrong_password` otherwise. */
  unlockHidden: (password: string) => request<HiddenState>("/auth/hidden/unlock", json({ password })),
  hiddenState: () => request<HiddenState>("/auth/hidden"),
  lockHidden: () => request<void>("/auth/hidden", { method: "DELETE" }),
  /** Hide or unhide photos. Unhiding needs the Hidden view unlocked. */
  hideAssets: (target: Target, hidden: boolean) => bulk("/assets/hide", target, { hidden }),
  /** Archive or restore photos. A scope-based archive answers a `batch` for its Undo. */
  archiveAssets: (target: Target, archived: boolean) => bulk("/assets/archive", target, { archived }),
  memories: (opts: { all?: boolean; date?: string } = {}) => {
    const q = new URLSearchParams({ date: opts.date ?? localToday() });
    if (opts.all) q.set("all", "true");
    return request<Memories>(`/assets/memories?${q}`);
  },
  /** "Not this one" (a photo of a burst takes its burst along); `dismissed: false` takes it back. */
  dismissMemories: (ids: readonly string[], dismissed = true) =>
    request<{ count: number }>("/assets/memories/dismiss", json({ ids, dismissed })),
  prefs: () => request<Prefs>("/assets/prefs"),
  setPrefs: (patch: Partial<Prefs>) => request<Prefs>("/assets/prefs", { method: "PUT", body: JSON.stringify(patch) }),
  me: () => request<Me>("/auth/me"),
  login: (email: string, password: string) => request<LoginResult>("/auth/login", json({ email, password })),
  /** The second step of a login with 2FA: a 6-digit code or a recovery code. */
  loginCode: (challenge: string, code: string) =>
    request<{ id: string; recovery_code_used?: boolean; recovery_codes_remaining?: number }>(
      "/auth/login/2fa",
      json({ challenge, code }),
    ),
  twoFactorStatus: () => request<TwoFactorStatus>("/auth/2fa"),
  twoFactorEnrol: () => request<TwoFactorEnrolment>("/auth/2fa/enrol", { method: "POST" }),
  twoFactorConfirm: (code: string) =>
    request<{ enabled: true; recovery_codes: string[] }>("/auth/2fa/confirm", json({ code })),
  twoFactorDisable: (password: string, code: string) =>
    request<{ enabled: false; enrol_required: boolean }>("/auth/2fa/disable", json({ password, code })),
  newRecoveryCodes: (password: string, code: string) =>
    request<{ recovery_codes: string[] }>("/auth/2fa/recovery-codes", json({ password, code })),
  adminTwoFactor: () => request<TwoFactorOverview>("/auth/admin/2fa"),
  adminRequireTwoFactor: (required: boolean) =>
    request<{ required: boolean }>("/auth/admin/2fa/require", { ...json({ required }), method: "PUT" }),
  adminResetTwoFactor: (id: string) =>
    request<{ reset: boolean; had_2fa: boolean }>(`/auth/admin/users/${encodeURIComponent(id)}/2fa/reset`, {
      method: "POST",
    }),
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
  usage: () => request<MyUsage>("/assets/usage"),
  setAccountLimits: (id: string, limits: Limits) =>
    request<Limits>(`/assets/admin/accounts/${encodeURIComponent(id)}/limits`, { method: "PUT", body: JSON.stringify(limits) }),
  setDefaultLimits: (limits: Limits) =>
    request<Limits>("/assets/admin/default-limits", { method: "PUT", body: JSON.stringify(limits) }),
  adminAccounts: () => request<AdminAccounts>("/assets/admin/accounts"),
  /** Queue the bucket listing (the worker runs it). */
  startBucketCheck: () =>
    request<{ task: AdminTask<BucketReport>; already_running: boolean }>("/assets/admin/bucket-check", { method: "POST" }),
  bucketCheck: () => request<{ task: AdminTask<BucketReport> | null }>("/assets/admin/bucket-check"),
  health: () => request<Health>("/assets/admin/health"),
  integrity: () => request<IntegritySummary>("/assets/admin/integrity"),
  runIntegrityCheck: () =>
    request<{ task: { state: string }; already_running: boolean }>("/assets/admin/integrity/run", { method: "POST" }),
  integrityFindings: (q: IntegrityQuery = {}) => {
    const p = new URLSearchParams();
    for (const [k, v] of Object.entries(q)) if (v !== undefined) p.set(k, String(v));
    const qs = p.toString();
    return request<IntegrityFindingsPage>(`/assets/admin/integrity/findings${qs ? `?${qs}` : ""}`);
  },
  /** `confirm: "delete"` is required for `delete_object`. */
  integrityAct: (id: number, action: IntegrityAction, confirm?: "delete") =>
    request<{ changed: boolean; finding: IntegrityFinding }>(
      `/assets/admin/integrity/findings/${id}`,
      json(confirm ? { action, confirm } : { action }),
    ),
  /** Requeue failed jobs of every account (one `kind`, or all). */
  retryAllFailedJobs: (kind?: string) =>
    request<{ requeued: number }>("/assets/admin/jobs/retry", json(kind ? { kind } : {})),
  disableAccount: (id: string) =>
    request<{ disabled: boolean; changed: boolean }>(`/auth/admin/users/${encodeURIComponent(id)}/disable`, { method: "POST" }),
  enableAccount: (id: string) =>
    request<{ disabled: boolean; changed: boolean }>(`/auth/admin/users/${encodeURIComponent(id)}/enable`, { method: "POST" }),
  setAccountRole: (id: string, role: "admin" | "member") =>
    request<{ role: string; changed: boolean }>(`/auth/admin/users/${encodeURIComponent(id)}/role`, json({ role })),
  accountResetLink: (id: string) =>
    request<ResetLink>(`/auth/admin/users/${encodeURIComponent(id)}/reset-link`, { method: "POST" }),
  accountSessions: (id: string) => request<{ items: SessionInfo[] }>(`/auth/admin/users/${encodeURIComponent(id)}/sessions`),
  revokeAccountSessions: (id: string) =>
    request<{ revoked: number }>(`/auth/admin/users/${encodeURIComponent(id)}/sessions`, { method: "DELETE" }),
  /** Step two of deleting an account: `confirmEmail` is the address the admin typed. Runs in the background. */
  deleteAccount: (id: string, confirmEmail: string) =>
    request<{ deleting: boolean; already_requested: boolean }>(
      `/auth/admin/users/${encodeURIComponent(id)}/delete`,
      json({ confirm_email: confirmEmail }),
    ),
  auditLog: (q: AuditQuery = {}) => {
    const p = new URLSearchParams();
    for (const [k, v] of Object.entries(q)) if (v !== undefined && v !== "") p.set(k, String(v));
    const qs = p.toString();
    return request<AuditPage>(`/auth/admin/audit${qs ? `?${qs}` : ""}`);
  },
  mySessions: () => request<{ items: SessionInfo[] }>("/auth/sessions"),
  revokeOtherSessions: () => request<{ revoked: number }>("/auth/sessions/revoke-others", { method: "POST" }),
  revokeAllSessions: () => request<{ revoked: number }>("/auth/sessions/revoke-all", { method: "POST" }),
  revokeSession: (id: string) => request<void>(`/auth/sessions/${encodeURIComponent(id)}`, { method: "DELETE" }),
  resetCheck: (token: string) =>
    request<{ valid: boolean; email?: string; reason?: "unknown" | "used" | "expired" | "disabled" }>(
      "/auth/reset-password/check",
      json({ token }),
    ),
  resetPassword: (token: string, password: string) =>
    request<void>("/auth/reset-password", json({ token, password })),
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
  createApiKey: (name: string, scope: ApiKeyScope = "full", expiresInDays?: number) =>
    request<CreatedApiKey>(
      "/auth/api-keys",
      json(expiresInDays ? { name, scope, expires_in_days: expiresInDays } : { name, scope }),
    ),
  revokeApiKey: (id: string) => request<void>(`/auth/api-keys/${encodeURIComponent(id)}`, { method: "DELETE" }),
  accountApiKeys: (id: string) =>
    request<{ items: AccountApiKey[] }>(`/auth/admin/users/${encodeURIComponent(id)}/api-keys`),
  revokeAccountApiKey: (id: string, keyId: string) =>
    request<void>(`/auth/admin/users/${encodeURIComponent(id)}/api-keys/${encodeURIComponent(keyId)}`, { method: "DELETE" }),
  /** Command-line login in the browser (docs/import.md, "Log in"): the loopback page's request, read by the server. */
  cliAuthorizeCheck: (p: CliAuthorizeParams) => request<CliRequest>("/auth/cli/authorize/check", json(p)),
  /** Allow: `redirect` is the CLI's loopback address with the single-use code. */
  cliAuthorizeApprove: (p: CliAuthorizeParams) => request<{ redirect: string }>("/auth/cli/authorize/approve", json(p)),
  cliAuthorizeDeny: (p: CliAuthorizeParams) => request<{ redirect: string }>("/auth/cli/authorize/deny", json(p)),
  /** The device flow: the code travels in the body, never in an address. */
  cliDeviceLookup: (userCode: string) => request<CliRequest>("/auth/cli/device/lookup", json({ user_code: userCode })),
  cliDeviceApprove: (userCode: string) =>
    request<{ approved: boolean; device: string }>("/auth/cli/device/approve", json({ user_code: userCode })),
  cliDeviceDeny: (userCode: string) =>
    request<{ approved: boolean; device: string }>("/auth/cli/device/deny", json({ user_code: userCode })),
  listAssets: (
    opts: { cursor?: string | null; before?: string | null; limit?: number; bbox?: string } & LibraryView = {},
  ) => {
    const q = new URLSearchParams();
    if (opts.bbox) q.set("bbox", opts.bbox);
    if (opts.limit !== undefined) q.set("limit", String(opts.limit));
    viewQuery(opts, q);
    if (opts.cursor) q.set("cursor", opts.cursor);
    else if (opts.before) q.set("before", opts.before);
    const qs = q.toString();
    const base = opts.person ? `/assets/people/${encodeURIComponent(opts.person)}/photos` : "/assets";
    return request<AssetList>(`${base}${qs ? `?${qs}` : ""}`);
  },
  /** Photos per day, newest first (the library's layout; `favorite` limits it to favorites). */
  timeline: (opts: LibraryView = {}) => {
    const q = new URLSearchParams();
    viewQuery(opts, q);
    const qs = q.toString();
    const base = opts.person ? `/assets/people/${encodeURIComponent(opts.person)}/timeline` : "/assets/timeline";
    return request<Timeline>(`${base}${qs ? `?${qs}` : ""}`);
  },
  /** The geotagged photos in `bbox` (`west,south,east,north`) as markers for a map at `zoom`. */
  mapClusters: (bbox: string, zoom: number, signal?: AbortSignal) =>
    request<MapResponse>(`/assets/map?bbox=${encodeURIComponent(bbox)}&zoom=${zoom}`, { signal }),
  /** Which basemap archives are installed (the map draws a plain background without one). */
  tiles: () => request<TilesInstalled>("/assets/tiles"),
  /** Favorite or unfavorite photos. `count` is how many changed. */
  favoriteAssets: (target: Target, favorite: boolean) => bulk("/assets/favorite", target, { favorite }),
  /** Is People switched on (`FACES_ENABLED`)? The only People route that answers while it is off. */
  peopleStatus: () => request<{ enabled: boolean }>("/assets/people/status"),
  listPeople: () => request<{ items: Person[] }>("/assets/people"),
  getPerson: (id: string) => request<Person>(`/assets/people/${encodeURIComponent(id)}`),
  /** Rename (null or blank clears the name), hide or show, choose the cover face. */
  updatePerson: (id: string, patch: { name?: string | null; hidden?: boolean; cover_face_id?: string }) =>
    request<Person>(`/assets/people/${encodeURIComponent(id)}`, { method: "PATCH", body: JSON.stringify(patch) }),
  deletePerson: (id: string) => request<void>(`/assets/people/${encodeURIComponent(id)}`, { method: "DELETE" }),
  /** Give every face of the person to `into`, then delete the person. */
  mergePerson: (id: string, into: string) =>
    request<{ into: string; faces: number }>(`/assets/people/${encodeURIComponent(id)}/merge`, json({ into })),
  personPhotos: (id: string, opts: { cursor?: string | null; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.limit !== undefined) q.set("limit", String(opts.limit));
    if (opts.cursor) q.set("cursor", opts.cursor);
    const qs = q.toString();
    return request<AssetList>(`/assets/people/${encodeURIComponent(id)}/photos${qs ? `?${qs}` : ""}`);
  },
  /** "Not this person": the person's faces in these photos (or these faces) are taken out and never put back. */
  removeFromPerson: (id: string, what: { asset_ids?: readonly string[]; face_ids?: readonly string[] }) =>
    request<{ removed: number }>(`/assets/people/${encodeURIComponent(id)}/faces/remove`, json(what)),
  /** Hide, show, delete or merge several people, all or nothing. `merge` folds `ids` into `into` and may then set its name. */
  bulkPeople: (what: PeopleBulk) =>
    request<{ done: number; into?: string; faces?: number }>("/assets/people/bulk", json(what)),
  /** The faces a person's cover can be chosen from, best first, a page at a time. */
  personCoverFaces: (id: string, opts: { cursor?: string | null; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.limit !== undefined) q.set("limit", String(opts.limit));
    if (opts.cursor) q.set("cursor", opts.cursor);
    const qs = q.toString();
    return request<{ items: CoverFace[]; next_cursor: string | null }>(
      `/assets/people/${encodeURIComponent(id)}/faces${qs ? `?${qs}` : ""}`,
    );
  },
  personSuggestions: (id: string) => request<{ items: FaceSuggestion[] }>(`/assets/people/${encodeURIComponent(id)}/suggestions`),
  /** This face is that person, or a new one (with an optional name). */
  assignFace: (face: string, to: { person_id: string } | { new: true; name?: string }) =>
    request<{ person: Person | null }>(`/assets/faces/${encodeURIComponent(face)}/assign`, json(to)),
  /** "No, not them": the face is not suggested for the person again. */
  rejectFace: (face: string, personId: string) =>
    request<void>(`/assets/faces/${encodeURIComponent(face)}/reject`, json({ person_id: personId })),
  photoFaces: (asset: string) => request<{ items: PhotoFace[]; dismissed?: DismissedFace[] }>(`/assets/${encodeURIComponent(asset)}/faces`),
  /** "Not a face": the detection is no face at all. It leaves its person; `person_id` is the one it was in (to put it back on undo). */
  markNotAFace: (face: string) => request<{ person_id: string | null }>(`/assets/faces/${encodeURIComponent(face)}/not-a-face`, json({})),
  /** Undo "Not a face": the detection is an ordinary unclaimed face again. */
  unmarkNotAFace: (face: string) => request<void>(`/assets/faces/${encodeURIComponent(face)}/not-a-face`, { method: "DELETE" }),
  /** The user's tags with counts: alphabetical (`name`) or most used first (`count`); `q` finds names containing it. */
  listTags: (opts: { q?: string; sort?: "name" | "count"; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.q) q.set("q", opts.q);
    if (opts.sort) q.set("sort", opts.sort);
    if (opts.limit) q.set("limit", String(opts.limit));
    const qs = q.toString();
    return request<{ items: Tag[] }>(`/assets/tags${qs ? `?${qs}` : ""}`);
  },
  /** Make a tag (the existing one when the name is taken, whatever the case). */
  createTag: (name: string) => request<{ id: string; name: string; created: boolean }>("/assets/tags", json({ name })),
  /** Rename. A name another tag has is a 409 `tag_exists` whose `details.existing_id` is that tag. */
  renameTag: (id: string, name: string) =>
    request<{ id: string; name: string }>(`/assets/tags/${encodeURIComponent(id)}`, { method: "PATCH", body: JSON.stringify({ name }) }),
  /** Give every photo of the tag to `into`, then delete the tag. */
  mergeTag: (id: string, into: string) =>
    request<{ into: string; photos: number }>(`/assets/tags/${encodeURIComponent(id)}/merge`, json({ into })),
  deleteTag: (id: string) => request<void>(`/assets/tags/${encodeURIComponent(id)}`, { method: "DELETE" }),
  /** Add and/or remove tags (by name) on photos; `added` and `removed` count (photo, tag) links that changed. */
  applyTags: async (target: Target, change: { add?: readonly string[]; remove?: readonly string[] }) => {
    const body = { add: change.add ?? [], remove: change.remove ?? [] };
    if (isScope(target)) {
      return request<{ added: number; removed: number }>("/assets/tags/apply", json({ scope: target.scope, except: target.except, ...body }));
    }
    const total = { added: 0, removed: 0 };
    for (let i = 0; i < target.length; i += MAX_BULK_IDS) {
      const r = await request<{ added: number; removed: number }>("/assets/tags/apply", json({ ids: target.slice(i, i + MAX_BULK_IDS), ...body }));
      total.added += r.added;
      total.removed += r.removed;
    }
    return total;
  },
  /** Give photos 0 to 5 stars (0 clears). `count` is how many changed. */
  rateAssets: (target: Target, rating: number) => bulk("/assets/rating", target, { rating }),
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
  /** Make a smart album: its photos are whatever the rules match, looked up each time the album is opened. */
  createSmartAlbum: (input: { title: string; description?: string; rules: SmartRules }) =>
    request<Album>("/assets/albums", json(input)),
  /** How many photos (and which first ones) rules would match now; nothing is saved. */
  previewSmartAlbum: (rules: SmartRules, signal?: AbortSignal) =>
    request<SmartPreview>("/assets/albums/preview", { ...json({ rules }), signal }),
  /** The groups of near-duplicate photos found by the last check, newest first, with the best photo marked. */
  duplicates: (opts: { cursor?: string | null; limit?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.limit !== undefined) q.set("limit", String(opts.limit));
    if (opts.cursor) q.set("cursor", opts.cursor);
    const qs = q.toString();
    return request<DuplicatesPage>(`/assets/duplicates${qs ? `?${qs}` : ""}`);
  },
  /** Check the library for duplicates now (the worker starts within seconds). */
  scanDuplicates: () => request<{ scan: DuplicateScan }>("/assets/duplicates/scan", json({})),
  /** Admins: check every account's library. */
  scanAllDuplicates: () => request<{ accounts: number }>("/assets/admin/duplicates/scan", json({})),
  /** Keep one photo of a group: the others go to the trash and what they carry is merged onto it. */
  keepDuplicate: (group: string, keep: string) =>
    request<DuplicateKept>(`/assets/duplicates/${encodeURIComponent(group)}/keep`, json({ keep })),
  /** "Not duplicates": the group is dismissed for good. */
  dismissDuplicates: (group: string) =>
    request<{ dismissed: number }>(`/assets/duplicates/${encodeURIComponent(group)}/dismiss`, json({})),
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
  /** The photos of the stack `id` is in (404 when it is in none). */
  getStack: (id: string) => request<StackDetail>(`/assets/${encodeURIComponent(id)}/stack`),
  /** Stack photos by hand: the first id is the cover; stacks the photos are already in join the new one. */
  stackAssets: (ids: string[]) =>
    request<{ size: number; cover_id: string }>("/assets/stacks", json({ ids })),
  /** Make `id` the cover of its stack. */
  setStackCover: (id: string) => request<{ cover_id: string }>(`/assets/${encodeURIComponent(id)}/stack/cover`, { method: "POST" }),
  /** Take `id` out of its stack (it stays out of automatic bursts). */
  removeFromStack: (id: string) => request<{ removed: string }>(`/assets/${encodeURIComponent(id)}/stack/remove`, { method: "POST" }),
  /** Dissolve the stack `id` is in. */
  unstack: (id: string) => request<{ count: number }>(`/assets/${encodeURIComponent(id)}/stack`, { method: "DELETE" }),
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
  /**
   * Ask for the edited JPEG of a photo ("Download edited"): `ready` when the worker has written it (then
   * `editedHref(id)` downloads it), `pending` when it was just queued.
   */
  editedStatus: async (id: string): Promise<"ready" | "pending"> => {
    const res = await fetch(editedHref(id), { credentials: "include", redirect: "manual" });
    if (res.status === 202) return "pending";
    // The redirect to the file is not followed (it is another origin): the browser reports it as an opaque redirect.
    if (res.type === "opaqueredirect" || (res.status >= 300 && res.status < 400)) return "ready";
    let message = `Could not prepare the edited photo (${res.status}).`;
    try {
      const body = (await res.json()) as { error?: string };
      if (body.error) message = body.error;
    } catch {
      /* keep the generic message */
    }
    throw new ApiError(res.status, message);
  },
  /** The places the user's own photos are in (`q`: the start of a word of the city, region or country). */
  places: (q = "", signal?: AbortSignal) =>
    request<PlaceHit[]>(`/assets/places${q ? `?${new URLSearchParams({ q })}` : ""}`, { signal }),
  /** What the details panel shows: date, place, camera, exposure, files, albums, burst. */
  getDetails: (id: string) => request<PhotoDetails>(`/assets/${encodeURIComponent(id)}/details`),
  /** The text read off a photo (and its QR codes), for the lightbox's Text panel. */
  getText: (id: string) => request<PhotoText>(`/assets/${encodeURIComponent(id)}/text`),
  /** Dedupe + part URLs. Defaults to the proxy transport, the only one a browser can use. */
  checkUpload: (req: CheckRequest) => request<CheckResponse>("/assets/check", json({ transport: "proxy", ...req })),
  takeoutPlan: (paths: string[]) => request<TakeoutPlan>("/assets/takeout/plan", json({ paths })),
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

/** The edited JPEG of a photo (rotated, straightened, cropped; no metadata). A redirect once the worker has made it. */
export const editedHref = (id: string) => `/assets/${encodeURIComponent(id)}/download/edited`;

/** What `POST /assets/zip/plan` says an archive would hold. */
export interface ZipPlan {
  /** Files, a RAW + JPEG pair counting as two. */
  files: number;
  bytes: number;
  /** The download's file name (`<album>.zip`). */
  name: string;
}

/** Most ids one ZIP request takes; a bigger hand-picked selection is refused client-side (Select all sends none). */
export const MAX_ZIP_IDS = 1000;

/** The request body of the ZIP endpoints for a selection (`name` titles the file, e.g. the album's title). */
export const zipBody = (target: Target, name?: string): Record<string, unknown> => ({
  ...(isScope(target) ? { scope: target.scope, except: target.except } : { ids: target }),
  ...(name ? { name } : {}),
});

export const zipApi = {
  /** Ask what the archive would hold: it fails here (nothing to download, too many files) instead of in a download. */
  plan: (body: Record<string, unknown>) => request<ZipPlan>("/assets/zip/plan", json(body)),
};

export type Api = typeof api;
