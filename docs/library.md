# Library: trash, restore and permanent deletion

## What the user sees

- **Select** (library toolbar) turns on selection mode: click or tap to select, Shift-click for a range, long-press on
  a touch screen to start selecting, "Select all" (everything in the current view: All photos, Favorites, an album or the Trash, also the photos that are not loaded; the count comes from the view's total) and "Clear". Deselecting a photo after Select all keeps the rest selected. Escape clears the
  selection, a second Escape leaves selection mode. The sticky bar shows the count and **Move to trash**; the Delete
  key asks first and then does the same.
- In the photo viewer, **Move to trash** (or Delete) removes the current photo and shows the next one.
- Moving to trash is reversible, so there is no confirmation: the photos leave the grid immediately (and come back if
  the server refuses) and a toast says "N moved to trash" with **Undo**.
- **Trash** (top bar) lists trashed photos with "Deleted N days ago, gone in M days". Select and **Restore**, or
  **Delete forever** (confirmation, cannot be undone), or **Empty trash** (confirmation with the count).
- Photos in the trash for more than `TRASH_RETENTION_DAYS` (default 30) are deleted permanently by the assets service.

Uploading a photo that is in the trash puts the same asset back in the library (it keeps its thumbnails and
metadata). If that photo is being permanently deleted at that moment the upload is refused with `409` and a message
to try again in a minute; once the deletion has finished the same file uploads as a new photo.

## API (assets service, owner-scoped, idempotent)

| Request | Effect |
|---|---|
| `DELETE /assets/{id}` | Move one photo to the trash (`204`; `404` if not a live photo of yours). |
| `POST /assets/trash {"ids": [...]}` | Move photos to the trash. |
| `POST /assets/restore {"ids": [...]}` | Back to the library. Not possible once a purge is queued. |
| `GET /assets/trash?limit=&cursor=` | Trashed photos, newest deletion first (`deleted_at DESC, id DESC`, keyset `next_cursor`). Same fields as `GET /assets` plus `deleted_at`, `purge_at`; the response also has `total` and `retention_days`. |
| `POST /assets/purge {"ids": [...]}` | Delete trashed photos forever (photos not in the trash are ignored). |
| `POST /assets/trash/empty` | Delete everything in the trash forever. |

### Bulk actions on a whole view

Every bulk endpoint (`POST /assets/trash`, `restore`, `purge`, `favorite`, `albums/{id}/assets` and
`albums/{id}/assets/remove`) accepts, instead of `ids`, a **scope** and an optional `except` list:

```json
{"scope": "library" | "favorites" | "trash" | {"album": "<id>"} | {"trash_batch": "<time>"}, "except": ["<id>", ...]}
```

The scope names the photos the matching view lists (hidden RAW files are not part of it, so a RAW + JPEG pair is one
photo and acts as one). It is resolved to ids inside the operation's transaction, for the caller's own photos only.
Sending both `ids` and `scope`, or `except` without a scope, is a `400`; `except` has at most 20,000 ids; another
user's album is `404`. A scope-based trash answers `{"count", "batch"}`: `batch` is the transaction time, and
`{"scope": {"trash_batch": batch}}` restores exactly what that request trashed (the Undo toast uses it). The web app
asks first ("Move 48,213 photos to trash?") before trashing a whole view.

The bulk endpoints take at most 1000 ids (`400` beyond that) and answer `{"count": n}`, the number of photos that
actually changed. Unknown ids, other people's ids and photos in the wrong state are not errors, they just do not count,
so a repeated request returns `0`.

## How permanent deletion works

1. **Mark.** `POST /assets/purge`, `POST /assets/trash/empty` and the janitor (for trash older than the retention
   period) set `assets.purging_at`. The photo disappears from the trash and can no longer be restored. The API
   wakes the janitor so the work starts at once.
2. **Sweep** (the janitor in the assets service, hourly, on demand, and again after 5 minutes if anything failed).
   For each marked photo it removes **every version and delete marker** of the original (exact key) and of everything
   under `derived/{user_id}/{asset_id}/` (thumbnail, preview and whatever later stages add), then deletes the `jobs`,
   `asset_embeddings` and `assets` rows in one transaction. If storage fails the row stays marked and the next sweep
   tries again; deleting what is already gone is not an error. Failures are logged (`purge failed; will retry next
   sweep`). It is safe to run next to the API and several assets instances can sweep at once.
3. **The worker leaves it alone.** Jobs for a photo that is being purged are skipped, and a thumbnail job that was
   already rendering when the purge started removes what it wrote. Photos that are only in the trash keep being
   processed normally, so their thumbnails exist in the trash and after a restore.

### Versioned buckets

The production bucket has versioning on. A plain `DeleteObject` there only adds a delete marker and the bytes stay
(and stay billed), so purging uses `ListObjectVersions` and `DeleteObjects` with each `VersionId`, markers included,
in batches of at most 1000. If the store rejects `ListObjectVersions` (HTTP 501 / `NotImplemented`) the listing falls
back to a plain `ListObjectsV2` and deletes without version ids; if the store rejects the bulk delete, objects are
deleted one by one. On a bucket without versioning, ordinary deletes are all that happens.

Things purging does **not** touch: Postgres backups under `backups/postgres/` (see `docs/deploy.md`), and any bucket
lifecycle rules. A photo that was already included in an older database dump will reappear as a row if that dump is
restored, but its objects are gone.

## Configuration

`TRASH_RETENTION_DAYS` (assets service; default `30`; a whole number from 1 to 36500; invalid values stop the
service at startup). Changing it applies to everything already in the trash: `purge_at` is always
`deleted_at + retention`.

## Tests

- `cargo test` (DB-backed tests run when `DATABASE_URL` is set): trash, restore, listing and pagination, owner
  isolation, bulk limits, purge and empty, the sweep with a failing and a recovering store, retention, re-upload of
  a trashed photo, a photo mid-purge, worker skips.
- `crates/cyanotype-core/tests/storage.rs` runs against a real S3 API when `S3_ENDPOINT` etc. are set: old versions
  and delete markers are removed, only exact keys, more than 1000 versions.
- `scripts/e2e.sh` uploads, trashes, restores and purges through the gateway and checks that the objects are gone;
  with `E2E_S3_ENDPOINT`, `E2E_S3_BUCKET`, `E2E_S3_ACCESS_KEY` and `E2E_S3_SECRET_KEY` (a throwaway local bucket with
  versioning enabled; never a real one) it also lists versions and markers and expects none.

# Albums

Albums are named, owner-scoped collections of photos. An album references photos; it never owns them.

## What the user sees

- **Albums** (top bar, `/albums`): cards with cover, title, photo count and date range, most recently updated first,
  and **New album**.
- An album page (`/albums/:id`) shows the title and description (**Edit**), the photos in the library grid and
  lightbox, **Select** (**Remove from album**, **Set as cover** with exactly one photo selected), **Add photos** (a
  picker over the whole library using the same selection bar) and **Delete album** (confirmation: the photos stay in
  the library).
- **Add to album** in the library's selection bar and in the lightbox opens a dialog with a searchable album list and
  **New album** (creates it with the photos). A toast says "Added N to <album>"; photos already in the album are not
  counted. The lightbox lists the albums the current photo is in.

## API (assets service, owner-scoped)

Mounted under `/assets/albums` because the gateway sends only `/auth/*` and `/assets/*` to the APIs (`/albums` is the
web app's page).

| Request | Effect |
|---|---|
| `GET /assets/albums` | `{items}`: each album with `title`, `description`, `photo_count`, `cover` (`id`, `thumbhash`, `thumb_url`; null when empty), `first_at`/`last_at`, `created_at`, `updated_at`; sorted by `updated_at DESC`. |
| `POST /assets/albums {title, description?, asset_ids?}` | Create (`201`, returns the album). Title 1-200 chars, description up to 5000 (blank = none), at most 1000 `asset_ids` (only your live photos are added). |
| `GET /assets/albums/{id}` | One album, same shape. |
| `PATCH /assets/albums/{id} {title?, description?, cover_asset_id?}` | Omitted fields stay; `null` clears the description or the chosen cover. The cover must be a live photo in the album (`400`). |
| `DELETE /assets/albums/{id}` | Deletes the album and its memberships only (`204`). |
| `GET /assets/albums/{id}/assets?limit&cursor` | The album's photos, same item shape, ordering (`coalesce(taken_at, file_modified_at, created_at) DESC, id DESC`) and keyset pagination as `GET /assets`. |
| `POST /assets/albums/{id}/assets {ids}` | Add; idempotent; unknown, foreign, trashed and already-present ids are skipped. `{count}` = newly added. At most 1000 ids. |
| `POST /assets/albums/{id}/assets/remove {ids}` | Remove (photos stay in the library). `{count}` = memberships removed. |

`GET /assets/{id}` also returns `albums: [{id, title}]`. Another user's album or an unknown id is always `404`.

The cover is the chosen photo when it is still in the album and not trashed, otherwise the most recent photo. The date
range is the min/max of the same timeline time as the library. Adding, removing or editing bumps `updated_at`.

## Albums and the trash

Trashing a photo keeps its membership: it disappears from album views, counts, covers and date ranges, and comes back
when restored (also as the chosen cover). Purging deletes the asset row, and the `ON DELETE CASCADE` on
`album_assets.asset_id` drops the membership (an explicit cover falls back through `ON DELETE SET NULL`). Removing the
chosen cover from an album clears the choice. Migration `0004_albums.sql`; the primary key `(album_id, asset_id)`
serves "photos of an album" (sorted in the query by the timeline expression) and `album_assets_asset_idx` serves
"albums containing this photo".

## Tests

`cargo test` (DB-backed, needs `DATABASE_URL`): CRUD and validation, owner isolation, add/remove idempotency and
caps, pagination with tied times, trash/restore/purge interplay, cover fallback, list ordering and the asset's album
list. Web: API calls, the add-to-album flow (library and lightbox), albums and album pages.

# RAW + JPEG pairs

A camera that shoots RAW + JPEG (or an iPhone with ProRAW: DNG + HEIC) produces two files per shot. Cyanotype keeps
both, but shows them as one photo.

## What the user sees

- The JPEG/HEIC/HEIF/AVIF is the photo: its thumbnail and preview are what the library, albums and trash show. The
  RAW is hidden from the grids, counts and timeline. Tiles of a pair carry a small **RAW** badge.
- The photo viewer (library, Favorites and album pages) lists **RAW + JPEG** (or **RAW + HEIC**, ...) with both file names and sizes and an **Unpair**
  button. Unpairing makes the two independent photos (the RAW appears in the library as its own tile) and is
  remembered: they are never paired again automatically.
- Moving a pair to the trash, restoring it, deleting it forever, and adding it to an album all work on the photo as a
  whole and count it once. Uploading either file again (it is still deduplicated by sha256) revives a trashed pair.
- Order does not matter: the RAW may be uploaded before or after the JPEG, in the same or another upload.

## Model

Both files stay ordinary assets: own row, own S3 original (the source of truth; nothing is ever merged or deleted by
pairing), own derived objects, own sha256 dedupe. `assets.primary_id` on the RAW points at its visible primary
(`NULL` = a normal visible photo; migration `0005_raw_jpeg_pairs.sql`). A partial unique index allows at most one RAW
per primary; hidden files are excluded with `primary_id IS NULL` in the library list, count, trash list, album views
and the timeline index. Pairs the user separated are recorded in `asset_pair_rejections (raw_id, primary_id)`.

I chose a nullable self-reference over a `stacks` table because a pair is always exactly two files, the primary is
the row every existing query, album membership and cursor already uses, and no join is needed on the hot list path.
(A group of three or more files would call for a stack table.)

## Matching rule

Two files of the same owner pair when one is a RAW (`cr2 cr3 nef arw dng raf orf rw2 pef srw ...`, the list the
worker also routes to its RAW pipeline) and the other a JPEG/HEIC/HEIF/AVIF, both live (not in the trash, not being
deleted), neither already paired, not separated by the user, and:

1. **Same original filename stem**, case-insensitive: the name without directory and last extension
   (`IMG_1234.CR3` and `img_1234.jpg`). No suffix patterns (`IMG_1234 (1).JPG`, `-edit`) are handled.
2. **Same capture time**: `taken_at` equal within +/- 1 second. If several fit, the closest wins (ties: lowest id).
3. If either `taken_at` is missing: equal camera make and model (both present, case-insensitive) pairs them, but only
   when exactly one candidate fits. More than one is ambiguous and nothing is paired. A capture time that is present
   on both sides but differs by more than a second never falls back to the camera.

Two JPEGs, two RAWs, PNG/WebP/TIFF/video and files of different owners never pair; one RAW never pairs with more than
one primary and vice versa.

## When pairing runs

- **New uploads:** at the end of the worker's metadata job (that is when `taken_at` and the camera are known), in the
  same job, so a failure retries the job. Whichever half finishes last finds the other. A trashed counterpart is not
  paired; restoring a trashed file pairs it then.
- **Concurrency:** one transaction takes `pg_advisory_xact_lock` on (owner, stem), re-reads and locks the candidate
  rows, links, and moves album memberships and cover choices of the RAW onto the primary. Unpairing takes the same lock.
  Two workers finishing the two halves at the same moment serialise; the second sees `Already`.
- **Existing library (backfill):** the migration inserts one `pair` job per live photo with finished metadata
  (`ON CONFLICT DO NOTHING`, so re-running adds nothing). The worker drains them in the background after deploy; each
  job is an indexed lookup (`assets_owner_stem_idx` on `asset_stem(original_name)`) and does nothing for files that are
  neither RAW nor JPEG/HEIC. Nothing downloads from S3, nothing needs the CLI, and pairing is idempotent so the jobs
  can be retried freely. For a library of tens of thousands of photos the migration is a few hundred milliseconds
  (index builds take a brief write lock on `assets`) and the drain takes seconds to a few minutes.

## Trash, purge and albums

- Trash, restore and purge (API, the janitor's retention sweep, "empty trash") change both rows in one transaction, so
  the pair is never half-trashed. The purge sweep deletes both originals and both derived prefixes with the existing
  version-aware machinery; if storage fails, both rows stay marked and are retried.
- Albums reference the primary only: adding a RAW's id is skipped, the cover can only be the primary, counts and date
  ranges count the pair once. When a pair forms, memberships and a cover choice of the RAW move to the primary.
  After an unpair the RAW is a normal photo again and is in no album.

## API

| Request | Effect |
|---|---|
| `GET /assets`, `GET /assets/trash`, `GET /assets/albums/{id}/assets` | Items gain `paired: [{id, original_name, content_type, size_bytes, kind: "raw"}]` (empty for a single file). Hidden RAW files are not items. |
| `GET /assets/{id}` | Also returns `paired`. |
| `POST /assets/{id}/unpair` | Separate the pair `id` belongs to (either file). `204`; `404` if it is not a pair of yours. |

Bulk trash/restore/purge counts are photos: a pair is `1`.

## Tests

`cargo test` (DB-backed): the pure matching rule (stems, case, +/- 1 s, missing time, camera fallback, ambiguity,
RAW-only, two JPEGs); pairing in either order, trash/restore/purge/expiry of a pair, albums, unpair persistence,
re-upload and restore, backfill idempotency, concurrent pairing. Web: badge, viewer info, Unpair.

# Timeline, favorites and downloads

## What the user sees

- **Date-grouped library.** Photos sit under month headers ("October 2026") and day sub-headers ("Wed, Oct 14"), newest
  first (`coalesce(taken_at, file_modified_at, created_at) DESC, id DESC`, as before). Days are UTC calendar days of
  that time, the same bucketing on server and client, so a photo never changes day between the counts and the list.
- **Virtualized.** Only the rows near the viewport exist in the page (about 20 to 40 tiles on a phone or desktop, a
  few hundred at most), however large the library is. Scrolling, selection, shift-range, select all, the lightbox and
  uploads work as before; a tile whose photo has not arrived yet is a grey placeholder, and photos arrive with their
  ThumbHash as before.
- **Scrubber.** A thin rail on the right edge lists the years (and a tick per month) of the whole library. Drag the
  handle (mouse, touch or pen) and a bubble names the month under it ("October 2022"); let go and the page shows that
  month. Tap a year to go to its first month, tap anywhere else on the rail to go to that point. With the keyboard
  the handle is a slider: Up/Down move a month, PageUp/PageDown a year, Home/End the ends. The rail is not shown for a
  library that fits on a couple of screens. It is 26px wide (30px on touch screens) and leaves its own gutter, so it
  never covers tiles.
- **Favorites.** A heart on each tile (on hover on desktop, always while favorited; on touch screens it only marks
  favorites, to avoid accidental taps), a heart in the photo viewer, and **Favorite / Unfavorite** in the selection bar
  (it unfavorites when every selected photo is already a favorite). **Favorites** (`/favorites`, a tab next to **All
  photos** at the top of the library, not a new top-bar button, so the phone nav keeps fitting at 320px) shows only
  favorites in the same timeline. In that view a bulk Unfavorite removes the photos; a single heart keeps the tile
  until the page is reloaded so an accidental click can be undone. Albums have hearts too.
- **Download.** The photo viewer's **Download** saves the original file under its original name. For a RAW + JPEG pair
  it opens a chooser with **Download JPEG** (or HEIC/AVIF) and **Download RAW** and the sizes. (This replaces the old
  "Original" link, which opened the file in a tab.)

## How it works

**Layout from counts.** `GET /assets/timeline` returns `{total, days: [{day, count}]}`, newest day first, for photos
that are not trashed and not hidden RAW files (`?favorite=true` for favorites). From that the web app builds rows of
fixed height: a month header (52px), a day header (34px) and `ceil(count / columns)` rows of square tiles per day.
Because every height is known, the page is exactly as tall as the library needs, and scrollbar and scrubber positions
are stable before any photo is loaded. (The query selects only the timeline expression, so it is an index-only scan of
`assets_timeline_idx`, or `assets_favorites_idx` for favorites; a test asserts the plan. About 50,000 photos over ten
years give a few thousand rows of JSON.)

**Windowed virtualization.** `@tanstack/react-virtual` (pinned `3.14.13`) renders the visible rows plus 3 rows of
overscan, with the page itself as the scroll container (a window virtualizer), so sticky bars, browser scroll
restoration and touch scrolling are untouched. Columns follow the width (3 below 400px, 4 up to 600px, then as many
tiles of at least 152px/192px as fit).

**Photos by global index.** The position of a photo in the whole library (newest = 0) is the sum of the counts before
its day plus its place within the day. The web app keeps loaded photos in a map keyed by that index. For the photos on
screen (plus 60 ahead) it fetches the first missing one's page: `GET /assets?before=<end of that photo's day>&limit=120`
starts exactly at the top of that day, so a jump to any month fetches one page there and nothing before it; the
response's `next_cursor` is remembered for the following page. Each stored page is checked against the counts (every
photo must fall on the day its index says); if not (photos were added or removed in between) the counts are read again.
Jumping into the middle of a very large day walks from the top of that day, one page at a time. While the scrubber is
being dragged nothing is fetched; it is fetched when the handle is let go.

**Bounded memory.** Beyond 2,400 loaded photos those farther than 1,000 from the viewport are dropped (the selection is
kept) and fetched again if scrolled back to, which also refreshes their short-lived thumbnail URLs. "Select all"
therefore means everything currently loaded, as before.

**Changes.** Moving to trash removes the photos locally and closes the gaps (later photos move up, day counts shrink);
anything that adds photos (an upload finishing, Undo, unpair) re-reads the counts and the page on screen and swaps both
in at once, so nothing flickers; while selecting, that waits for selection mode to end.

## API

| Request | Effect |
|---|---|
| `GET /assets/timeline[?favorite=true]` | `{total, days: [{day: "YYYY-MM-DD", count}]}`, newest first, owner-scoped. |
| `GET /assets?before=<RFC 3339>&limit=` | A page starting at a date: photos strictly older than `before` (the keyset position for "the end of that day"); continue with `next_cursor`. Existing parameter, now covered by tests with the day counts. |
| `GET /assets?favorite=true` | Favorites only (also combines with `before` / `cursor`). Items of every listing (library, trash, albums) and `GET /assets/{id}` carry `favorite`. |
| `POST /assets/favorite {ids, favorite}` | Set or clear the flag. `{count}` = photos that changed; unknown, foreign and trashed ids are ignored; at most 1000 ids; a hidden RAW's id means its primary. Idempotent. |
| `GET /assets/{id}/download[?file=<id>]` | `307` to a presigned S3 URL (valid 60 seconds, `Cache-Control: no-store`) that signs `response-content-disposition: attachment; filename="<original name>"` (names with quotes, slashes, control characters or non-ASCII get a sanitised `filename` plus an RFC 5987 `filename*`). `file` is the id of the RAW of a pair (from `paired`). `404` for another user's photo, a trashed one, a RAW as a photo of its own, or a file that is not part of the photo. |

**Downloads need no bucket CORS.** The viewer's Download is a plain `<a href="/assets/{id}/download">`: a top-level
navigation to the API (session cookie, same origin) that is redirected to the bucket, which answers with an
`attachment` disposition, so the browser saves the file and the page stays where it is. Nothing uses `fetch()`/XHR
against the bucket (that is what would need CORS), the bytes never pass through the API, and the URL is signed when the
link is clicked, so a page left open for hours never holds an expired one. The browser test serves the redirect target
from another origin that sends no CORS headers and checks the requests were navigations.

## Model

Migration `0006_favorites.sql`: `assets.favorite boolean NOT NULL DEFAULT false` (instant: constant default) and the
partial index `assets_favorites_idx (owner_id, timeline expression DESC, id DESC) WHERE favorite AND deleted_at IS NULL
AND primary_id IS NULL`. For a RAW + JPEG pair the visible primary carries the flag; when a pair forms, a flag the RAW
had on its own moves to the primary. Trashing keeps the flag (restoring brings the photo back as a favorite).

## Tests

- `cargo test` (DB-backed): day counts (exclude trashed photos and hidden RAWs, owner isolation, file-mtime and
  created-at fallbacks, favorites only, empty library, index-only plan on both indexes); the `before` position and
  cursor continuation without gaps, with favorites and other owners; favorites (bulk, idempotent, owner isolation,
  trashed, trash/restore, albums, pairs through either id, flag handover when pairing, 1000 cap, auth); downloads
  (redirect, signed attachment disposition, 60 second TTL, RAW of a pair, wrong file, hidden RAW, other user, trashed,
  restored, escaping); the disposition header builder.
- Web (vitest): grouping, layout, row/offset/month lookup, index arithmetic, page placement and staleness, eviction,
  removal, lightbox run, scrubber mapping and mark thinning; the scrubber (drag, tap, year jump, keyboard, follows the
  page); the library (headers, virtualized tile count and paging by scroll, lightbox across page boundaries, hearts,
  bulk favorite, Favorites view, selection and trash with the new store, uploads); lightbox heart and downloads.
- Browser (`pnpm test:mobile`, Playwright with a mocked API): `e2e/timeline.spec.ts` builds a 20,000-photo library over
  seven years and checks the number of tile cells in the DOM at the top, after scrubber jumps (drag, year tap, end,
  start) and while scrolling (at most 300 allowed; about 20 to 40 observed), that a jump issues at most a few list
  requests and starts at the target date, favorites and downloads (the redirect target is a separate origin without
  CORS headers). `e2e/mobile-overflow.spec.ts` also covers the headers, scrubber (resting and dragged), selection bar
  with the new button, favorites view and the pair download chooser at 320 to 414px and 1280px with no sideways scroll.

# Processing indicator (after a big import)

## What the user sees

While the worker is still making thumbnails and reading metadata, a slim banner sits under the Library tabs:
"Processing 1,234 photos..." with a progress bar (the share of the library that already has its thumbnail; an
indeterminate bar when only metadata/pairing tasks remain). It polls every 3 s while work is going on, then backs off
(10 s, 20 s, up to 60 s) and stops while the tab is hidden. It disappears when nothing is queued, running or failed.

If jobs failed (corrupt file, a photo too large for the worker's memory budget, ...) a "N failed" button opens a panel
with the latest 20 (file name, step, attempts, the error, shortened) and a **Retry failed** button that requeues them
all with a fresh set of attempts. The banner is a full-width row, not a fifth top-bar button, so it fits 320 to 414 px.

## API (assets service, owner-scoped)

| Request | Result |
|---|---|
| `GET /assets/processing` | `{active, queued, running, failed, by_kind: [{kind, queued, running, failed}], total_photos, without_thumbnail, failures: [{job_id, asset_id, original_name, kind, last_error, attempts}]}`. `without_thumbnail` counts photos with a thumbnail job queued or running (files the worker skips, such as videos, never count). `failures` is the newest 20; `last_error` is cut at 200 characters. |
| `POST /assets/processing/retry` | Requeue the caller's `failed` jobs (attempts reset to 0). `{requeued: n}`. Other users' jobs are never touched. |

Photos queued for permanent deletion are ignored; trashed photos still count (the worker processes them too).

## Tests

`cargo test` (DB-backed): counts per kind and state, truncation, owner isolation, finished and purging photos ignored,
retry requeues only the caller's failed jobs. Web (vitest): idle (renders nothing), progress, polling and back-off,
a failed poll keeps the last status, the failures panel, retry (and its error). Browser: `e2e/mobile-overflow.spec.ts`
renders the banner, the open panel (with a very long file name and error) and the state after a retry at 320 to 414 px
and 1280 px.

# Map and places

## What the user sees

- **Map** is the third tab beside "All photos" and "Favorites" (`/map`; three pills fit at 320 px, so the top bar
  gained no button). It shows the user's geotagged photos on a map: a single photo is a round thumbnail, photos that
  are close together are a count bubble. Tapping a bubble zooms to the photos in it; tapping a thumbnail opens the
  photo in the same lightbox as the library (favorite, download, add to album, trash with undo).
- Under the map on a phone, beside it on a wide screen, a list shows the photos in the part of the map on screen,
  newest first, loading more as you scroll. Panning or zooming starts the list again for the new area.
- Hidden RAW files and trashed photos are not on the map, exactly as in the library.
- The lightbox caption shows where a photo was taken ("Paris, Île-de-France, France") once the worker has looked it up.
- Without a basemap installed (docs/deploy.md, "Map") the markers sit on a plain background with a notice. Without
  WebGL the map is replaced by a notice and the list of all geotagged photos. The credits (OpenStreetMap contributors,
  Protomaps, GeoNames) are on the map.
- A library without GPS photos shows an explanation instead of an empty map.

## API (assets service, owner-scoped)

- `GET /assets/map?bbox=west,south,east,north&zoom=Z[&favorite=true]`: clusters for the box. `west > east` (or longitudes
  beyond +-180, which a map scrolled past the date line produces) means the box crosses the antimeridian; a box 360
  degrees or wider is the world. Response: `{clusters: [{id, lat, lon, count, bounds, thumbhash, thumb_url}], total, truncated}`.
  `bounds` (`[west, south, east, north]` of the cluster's photos) is null for a single photo; `thumbhash`/`thumb_url` are
  set for single photos only (clusters are drawn as bubbles). `total` is the number of photos in the box.
- `GET /assets?bbox=west,south,east,north[&cursor=...&limit=...]`: the same list as the library, restricted to geotagged
  photos in the box, with the usual keyset `next_cursor`.
- `GET /assets/{id}` gained `place`: `{city, region, country, country_code}` or null.
- `GET /assets/tiles` (which basemap archives are installed) and `GET /assets/tiles/{world,detail}.pmtiles` (range reads).

## How clustering works

Photos are grouped in SQL into a fixed grid in degrees whose cell size halves with every zoom level: a cell is
`360 / 2^(floor(zoom) + 3)` degrees, which is 64 to 128 screen pixels on a 512 px tile map. The grid is anchored at
the date line and equator, so clusters stay put while panning, split cleanly when zooming in, and a cluster never
straddles the antimeridian. Each cluster returns its count, the mean position, and the bounds of its photos (a click
fits them; photos that share one spot zoom in instead). A response holds at most 3000 cells (it is bounded by the
screen, a few hundred in practice); more sets `truncated`.

Why clusters instead of sending every point: 50,000 photos are about 3.5 MB of JSON before any thumbnail URL, re-sent
on each pan, and the browser would have to cluster them. The query is served by the partial index `assets_geo_idx`
(`(owner_id, latitude, longitude)` over live, visible, geotagged photos only, so it stays small for a library where
few photos have GPS). Measured on PostgreSQL 17 (docker, warm cache) with 50,000 geotagged photos, 70% concentrated
around 40 cities and 30% scattered worldwide, the worst case in which every photo has a position:

| request | rows read | time |
|---|---|---|
| clusters, whole world (zoom 0, 32 cells) | 50,000 (sequential scan) | ~130 ms |
| clusters, whole world at zoom 6 (14,000 cells, capped to 3000) | 50,000 | ~90 ms |
| clusters, a city-sized box (zoom 10, 211 cells) | 1,251 (index) | ~5 ms |
| viewport list, 60 photos of a city-sized box | 1,200 (index, top-N sort) | ~4 ms |
| viewport list, whole world | 60 (timeline index) | <1 ms |

The expensive requests are the zoomed-out ones, which the first view of the map makes once; panning around a city is
a few milliseconds.

## Places (offline reverse geocoding)

A `geocode` job (version 1) turns a photo's coordinates into `place_city`, `place_region`, `place_country` and
`place_country_code` on the asset. No external service is called.

- **Data**: GeoNames `cities1000` (171,306 populated places with more than 1000 inhabitants), `admin1CodesASCII` and
  `countryInfo`, licensed CC BY 4.0, https://www.geonames.org/ . GeoNames files are rebuilt daily and are not
  versioned, so a snapshot is committed instead of downloaded at deploy time: `apps/cli/data/places.tsv.gz` (2.6 MB,
  header records the snapshot date and source checksums), built by `scripts/build-places-dataset.sh`.
- **Loading**: the snapshot is compiled into the CLI image and `cyanotype migrate` loads it into the `places` table
  (about 22 MB with its index in Postgres, a few seconds) whenever its version differs from the one in
  `place_dataset`; otherwise it is a no-op. `migrate` runs on every deploy, so nothing is done by hand and offline
  deploys work. A new version replaces the table and queues the geocode job again for every geotagged photo.
- **Lookup**: a latitude/longitude box around the photo (50 km, widened for latitude, wrapped across the antimeridian,
  all longitudes near the poles) is read from the btree `places_lat_lon_idx` and ranked by great-circle distance; the
  nearest place within 50 km wins (ties go to the larger place). Farther than that (open sea, polar regions, deserts) a
  photo has no place rather than a misleading one. The nearest place decides the country, so a photo just across a
  border gets the neighbouring country's name; the place can be a small village. No PostGIS or extension is needed;
  the lookup is a few milliseconds and runs in the worker as a database-only job (no memory budget, no download).
- **When it runs**: the metadata job queues it when a photo has coordinates (and clears the place when a re-run finds
  none); migration `0008` queued it for every photo that already had coordinates, like the pairing backfill.
  Re-queuing is idempotent (`UNIQUE (kind, asset_id, version)`). While the dataset is not loaded the job fails and
  retries instead of recording "no place".
- Worker memory is unchanged: the dataset lives in Postgres, not in the worker.

## Tests

- DB integration tests (`apps/assets/src/routes/map_tests.rs`, `crates/cyanotype-core/tests/places.rs`,
  `apps/cli/src/places.rs`): cluster counts, bounds and zoom splitting, owner/trash/hidden-RAW filtering, malformed
  boxes, boxes across the antimeridian (three spellings), the viewport list's keyset pagination, nearest place within
  range, across a border, in the open sea, at the poles and across the date line, the geocode job body, backfill
  idempotency, dataset reloads, and the real bundled dataset (Paris, San Francisco, Basel/Saint-Louis, across the date line, ocean).
- `apps/assets/src/routes/tiles_tests.rs` (needs the S3 API like the upload tests): range relay, ETag/If-Match, caps,
  and that only the two archive names are served.
- Web: `geo.test.ts`, `MapCanvas.test.tsx` (MapLibre stand-in), `MapPage.test.tsx`; Playwright
  `mobile-overflow.spec.ts` covers the map page at phone widths.


# Share links

A share link gives anyone who has it a read-only view of an album or of a handful of photos, without an account.
`https://<host>/s/<token>` opens the public viewer.

## What the owner sees

- **Share** in the library's selection bar, on an album page (the whole album, or the selected photos), and in the
  photo viewer. The dialog asks for an optional title, an expiry (1 day, 1 week, 1 month, never), an optional password
  and the download policy, then shows the link and copies it.
- **Shared links** (`/shared`, reached from the Albums page so the top bar keeps its four buttons at 320 px) lists every
  link with what it shares, its state (expired, password, originals), views and its address, with **Copy link**,
  **Open**, **Edit** (title, expiry, password, downloads) and **Revoke** (confirmation).
- An **album link** follows the album live: photos added later appear, trashed photos disappear (and come back when
  restored), and deleting the album revokes the link. A **selection link** is a fixed set of photos; trashed or purged
  photos disappear from it. A RAW + JPEG pair counts as one photo.

## What a visitor sees

A minimal page: the title (the link's own, else the album's), the number of photos, the expiry, a grid of thumbnails and
a viewer with the preview-size image. Nothing else: no owner name or email (accounts have no display name, and the
email is never sent), no file names, no camera data, no location, no links into the app, and no ids except the random
ids of the shared photos themselves. The page is `noindex` three ways (`<meta name="robots">`, `X-Robots-Tag` on the
page and on every API answer) and sends no `Referer`.

- **Password links** show only a password form until the right password is given. The server then sets an `HttpOnly`,
  `SameSite=Lax` cookie that lives six hours and is sent only to that one link's API path (and `Secure` when
  `COOKIE_SECURE` is true, as for sessions). The cookie holds a random value; the database keeps its hash. Changing or
  removing the password ends every unlock.
- **Unknown, revoked and expired** links are the same `404` with the same body. A visitor cannot tell them apart.
- **Downloads** (when the owner allows them) are plain links that redirect to a presigned attachment URL valid for one
  minute; the bucket has no CORS policy, so these are navigations, never `fetch()`. For a RAW + JPEG pair both files
  are offered only when the owner chose to include originals.

## Privacy: no location leaves with a share

| What a visitor can fetch | What it contains |
|---|---|
| Thumbnails and previews (`thumb.webp`, `preview.webp`) | Pixels only. The worker re-encodes to WebP from decoded pixels and writes no EXIF, XMP or ICC chunk. A worker test feeds a JPEG with a GPS position, camera make and serial number through the pipeline and asserts that the RIFF chunks are image data only, that none of those strings occur, and that an EXIF reader finds nothing. |
| **Download, default** | A JPEG original is served as `derived/{user}/{asset}/share.jpg`: the same compressed image data, byte for byte (no re-encode), with every metadata segment removed (EXIF including GPS, serial numbers, embedded thumbnail and maker notes; XMP; IPTC/Photoshop; comments; MPF; any vendor APPn segment; anything stored after the end-of-image marker such as a motion-photo video) except the ICC colour profile, the Adobe colour marker and a freshly written EXIF block that holds only the orientation. A test builds a JPEG with location in the EXIF, XMP, IPTC, a comment, an MPF block and trailing data and asserts that none of it survives and that the pixels are identical. |
| Download of **HEIC, RAW, PNG, WebP, AVIF** by default | Not offered. Their metadata cannot be stripped safely, so a share never serves them unless the owner opts in below. |
| Download with **Include originals (contains location)** | The untouched files, with GPS, camera and everything else. The dialog and the links list say so, in a warning. Off by default, and it cannot be on while downloads are off. |

The copy is made by the worker (`share_copy` job), not on request and not for every upload: a visitor who opens a JPEG
in the viewer asks for it (`POST .../prepare`), the viewer shows "Preparing download..." and polls until the link
appears. It is written once per photo, lives under the photo's `derived/` prefix (so a purge removes it) and is
regenerable. Photos nobody downloads never cost storage.

## Token storage and revocation

The token is 24 random bytes (192 bits) from the OS-seeded CSPRNG, URL-safe base64, 32 characters, stored as-is in a
unique index. The alternative, storing only a hash, would mean a link can be shown once and never copied again; the
list page and **Copy link** depend on seeing it. Plaintext is acceptable here because everything the token opens is
already something the owner chose to publish, a password link still needs its (argon2id-hashed) password, and anyone
who can read the table can already read every photo's metadata and S3 key (database backups live in the bucket the
originals live in, so they share a trust domain). Revoking deletes the row, and every public request reads the row, so
a revoked link answers `404` on the next request; nothing is cached (`Cache-Control: no-store`).

## Rate limits and lockout

All per client IP (the same token-bucket limiter as login and register, in `cyanotype-service::ratelimit`; the client
address follows `TRUSTED_PROXIES` exactly as in the auth service; IPv6 by /64):

| Budget | Default | Spent by |
|---|---|---|
| Public requests | 120 burst, 120/min | every request under `/assets/public/s/` |
| Dead tokens | 10 burst, 10/min | requests for a token that is unknown, revoked or expired (`429` after that, even for other paths) |
| Passwords | 5 burst, 5/min | each password attempt, right or wrong |

Per link, ten wrong passwords in a row (from any addresses) lock it for ten minutes (`429` with `Retry-After`, even for
the right password); a right password or the owner changing the password clears it. Limits are in memory per assets
instance.

## API

Owner (assets service, signed in; another user's link or an unknown id is `404`):

| Request | Effect |
|---|---|
| `POST /assets/shares {album_id \| asset_ids, title?, expires_at?, password?, allow_download?, include_originals?}` | Create (`201`, with the `token`). Exactly one of `album_id` and `asset_ids` (1 to 1000, your live photos; a RAW's id means its JPEG). The expiry must be in the future, the password 4 to 200 characters, the title at most 200. |
| `GET /assets/shares` | `{items}`: `id`, `kind`, `album`, `title`, `token`, `has_password`, `allow_download`, `include_originals`, `expires_at`, `expired`, `item_count` (what a visitor sees now), `view_count`, `last_viewed_at`, times. Newest first. |
| `GET\|PATCH\|DELETE /assets/shares/{id}` | One link; edit `title`, `expires_at` (`null` = never), `password` (`null` removes), `allow_download`, `include_originals`; revoke. |

Public (no login; the gateway sends `/assets/*` to the assets service, `/s/*` to the web app):

| Request | Answer |
|---|---|
| `GET /assets/public/s/{token}` | Info (`title`, `kind`, `item_count`, `allow_download`, `include_originals`, `expires_at`); `{needs_password: true, unlocked: false}` for a locked password link. Counts as a view (metric `cyanotype.shares.views`, log line with `share_id`). |
| `POST .../unlock {password}` | `204` + cookie, `401` wrong password, `429` limited or locked. |
| `GET .../items?limit&cursor` | `{items, next_cursor}`: `id`, `width`, `height`, `sort_time`, `thumbhash`, `thumb_url`, `preview_url` (presigned, 10 minutes), `downloads: [{id, label, size_bytes}]`, `download_pending`. Same order and keyset paging as the library. |
| `GET .../items/{id}` | One item, same shape. |
| `POST .../items/{id}/prepare` | `202 {pending}`: queue the download copy if it is needed. |
| `GET .../items/{id}/download?file=` | `307` to a presigned attachment URL (one minute). `403` when downloads are off, `409` while the copy is being made, `404` for anything that is not offered (a RAW without "include originals", HEIC by default, another photo). |

Migration `0009_shares.sql` (`shares`, `share_assets`, `share_unlocks`, `assets.share_copy_at`).

## Logs

Share events log `share_id` and, for downloads, the asset id; never the token, a password or a presigned URL. The
assets service logs the route template, and the gateway's access and error logs blank the token (`docs/observability.md`).

## Tests

`cargo test` (DB-backed, `DATABASE_URL`): create, list, edit and revoke, owner isolation, validation, tokens,
album-live against selection-fixed, trash, restore and purge, paging, someone else's photos never shown, identical `404`s,
rate limits and lockout, the password flow and cookie, downloads on and off and with originals, pairs, a JPEG with a
position, a camera and an owner behind it that exposes none of them, view counts and headers. Worker: the JPEG stripper
and the WebP-has-no-metadata test. Web (vitest): the share dialog, the links page, the public viewer. Browser:
`e2e/mobile-overflow.spec.ts` renders the dialog, the links page and the public viewer (grid, viewer with downloads,
password form, unavailable) at 320 to 414 px and 1280 px.

# Search

Natural-language search ("dog on the beach", "red car at night", "perro en la playa", "birthday cake", "Maui summer 2022")
over the whole library, plus structured filters, with nothing leaving the server: the model runs inside the worker and the assets service, there is no external
API and no sidecar. Design and measurements are here; the model, its licence and sizes are in `docs/build.md` ("Semantic
search model").

## What the user sees

- A **search field** in the library header, next to All photos / Favorites / Map. On a phone it is an icon at the end of
  that row that expands into a full-width field (the tabs step aside, a back arrow closes it), so the row never holds more
  than three tabs and one icon, and fits 320 px. Recent searches (kept in this browser only) drop down under the field.
- **Enter** goes to `/search?q=...`: results in the library's own grid (lightbox, favorites, select, add to album, share,
  trash all work), best match first, "N matches". A text query always has *some* ranking, so the search **cuts the list where
  the matches stop**: it shows the **strong matches** (the photos the model is sure about and that are close to the best one),
  and when there are more, a **"Show weaker matches (N)"** button continues right after them with the next tier, which is
  looser (at most 300 results altogether). A query nothing in the library answers ("a giraffe") shows "No close matches"
  with that button, instead of a screenful of the nearest photos ("Changing the search model" below says how the cut-off
  was tuned, and what it can and cannot do for each model).
- **Dates and places in the words.** "sunset Maui", "dog in Lisbon", "beach June 2023", "snow last year" become **chips**
  ("2023 x", "Maui x") above the results, and what is left of the words is what the model searches for; the chips are
  filters like the ones in the Filters sheet. A wrong guess is one click: the x on a chip searches again with those words left in
  the query (`skip=date` or `skip=place` in the URL). Only places in the library count ("in Tokyo" is just words if no
  photo was taken there), and words that are also ordinary words ("nice", "reading", "may", "victoria") are never a place
  or a date without the right context ("in Nice", "May 2023"). The rules are below.
- **Filters** (button under the header; a bottom sheet on a phone, a dialog on a wide screen): year or a date range,
  favorites, camera, place (city, region or country), file type (JPEG, HEIC, RAW, other), has a location, album. The
  choices come from what the library has, with counts. Each active filter is a chip with an x; filters work alone (then
  they list photos newest first, with no limit) or together with the text.
- While the worker is still embedding the library for the first time search says **"Still indexing 12,345 photos: results may
  be incomplete"** with a progress bar; photos that are not indexed yet are simply not text results (they still match
  filters). "Nothing matched" says so too. After a **model change** the library keeps answering with the old model until the
  new one has indexed 95 %, and the page says **"Re-indexing with the improved search model: 40 %"** instead.
- **Similar photos.** The lightbox has a **Similar photos** button (an icon next to the heart on a phone, where the bar has no
  room for its words) that opens `/search?similar=<id>`: the library's photos that look like that one, best first, in the same
  grid, with a chip showing the source photo's thumbnail (its x ends the mode) and the same Filters. Like a text search it
  shows the strong matches and offers **"Show weaker matches (N)"**. A photo the worker has not embedded yet says **"This photo
  is still being indexed"** instead. See "More like this" below.
- **Bursts.** Rapid-fire shots of one moment collapse into one result: a tile with a **"+N"** badge (N more photos behind it). Tapping
  the badge opens the burst inline (its photos appear right behind the cover, set apart by an accent edge; the badge turns into a
  "-" that closes it again). This is in text searches and in "More like this", not in the timeline, the albums, or a filter-only
  search, which list every photo. See "Bursts" below.
- **Select all** on a filter-only search selects every match (a server-side scope, like the other views). On a text search
  it selects the results that are loaded (at most 300): a ranked list has no stable "everything" for the server to
  re-resolve, so those actions send explicit ids. **A collapsed burst is one tile that stands for all of its photos**: tapping it
  (or long-pressing it) selects every photo of the burst, its label says "Select IMG_1 and 3 more from its burst", and the
  selection bar says how many photos are hidden in collapsed bursts; the count, "Select all" and the actions (favorite, album,
  share, trash) work on photos, so selecting a collapsed burst and moving it to the trash trashes all of its photos. Open
  the burst first to pick its photos one by one.

Privacy: queries are never logged (the request log has the route template and no query string, the gateway's redacts it,
the metrics carry only `mode`, `plan` and `outcome` tags), the recent searches live in the browser's `localStorage`, and
no byte of a photo or a query goes to a third party.

## How it works

```
upload -> metadata -> thumbnail ──> embed (worker, background) ──> asset_embeddings (halfvec, one HNSW index per model)
                       writes preview.webp   reads preview.webp                          ▲
query ──> understand dates/places ──> assets service: tokenizers + ONNX text tower ──> vector ── nearest ──┘ (+ filters)
                                                             └─> strong / weaker tiers cut from the ranked list
```

- **Image side (worker).** The thumbnail job enqueues an `embed` job for the photo at priority 0. The embed job reads the
  photo's `preview.webp` (1440 px, already oriented; never the original, so there is no big decode, no RAW development and
  nothing to refuse for memory), preprocesses it as the model's own processor does (SigLIP 2: the whole image squashed to
  224 x 224, bilinear, pixels to -1..1; CLIP: shorter side to 224, centre crop; `docs/build.md`), runs the image tower and stores
  a unit vector with the model id and version. Each photo is its own run of the model (never batched with others: the 8-bit
  graph's activation scale is per batch, so a vector would depend on the photos embedded with it). It runs in its own loop, takes up to 4 jobs at a time (their
  previews download concurrently while the model runs) and **only when no thumbnail, metadata, pairing or
  geocode job is runnable**, so an upload's thumbnails are never queued behind the backfill. One ONNX session with
  `WORKER_EMBED_THREADS` (default 1) threads, loaded on the first job and dropped after five quiet minutes. Its resident
  memory (about 150 MB for either model) is not part of the decode budget; the worker's container limit includes it (`docs/build.md`), and the
  `embed.model_loaded` and `worker.rss_bytes` gauges show it against that limit.
- **Versioned and idempotent.** `jobs.version` of an embed job is the model's `version()` (unique across models, so a new
  model's jobs do not collide with the old model's finished ones) and `asset_embeddings` keeps `(model, version)` with every
  vector. At start and every 15 minutes the worker queues an embed job (priority 10) for every visible photo with a preview that
  has no vector for the *configured* model and version, so changing `SEARCH_MODEL` (or a model's `version()` in
  `crates/cyanotype-clip`) re-embeds the library with no migration. The sweep also fills in photos whose job was lost, whose
  RAW partner was unpaired, or that were embedded after a failure. A new vector replaces the same model's old one; **other
  models' vectors stay** until every photo has one from the configured model, and then the worker deletes them
  ("Changing the search model" below). Search only reads vectors of the model it is using, so two vector spaces never mix.
- **Job priority.** `jobs.priority` (smallint, default 0, lower first) orders the claim: `ORDER BY priority, run_after, id`
  with the index `jobs_claim_idx (kind, priority, run_after, id) WHERE state = 'queued'`. Migration 0012 backfills one embed
  job per existing photo at priority 10 (`ON CONFLICT DO NOTHING`, so running it twice adds nothing). LISTEN/NOTIFY is
  unchanged.
- **Text side (assets service).** `tokenizers` turns the query into the model's tokens (SigLIP 2: Gemma's, lower-cased,
  padded to 64; short queries get "a photo of a" in front, which is what makes the calibrated cut-off work: below), the ONNX
  text tower embeds it (about 150 ms for SigLIP 2 on this host, 10 ms for CLIP), and the vector is compared with the photos'. The
  tower loads on the first search and stays loaded (one at a time: during a model swap the service holds the tower of the model
  that is answering, not both); a 256-entry LRU keeps recent queries (case and spacing do not matter), so paging, changing a
  filter or retyping costs nothing. A server without the model files answers searches with `503` and keeps every filter
  working.
- **Storage.** `asset_embeddings(asset_id, model, version, embedding halfvec)`, primary key `(asset_id, model)`, `ON DELETE
  CASCADE` (purging a photo takes its vector with it). The column has no fixed dimension (CLIP B/32 is 512, SigLIP 2 base is
  768); **each model has its own HNSW index**, a partial expression index `((embedding::halfvec(N)) halfvec_cosine_ops)
  WHERE model = '<id>'` (`m = 16`, `ef_construction = 64`, cosine; migration 0013). An index only ever holds one model's
  vectors, its size is checked when a row is written (a vector of the wrong size for its model is refused), and a new
  model is one more `CREATE INDEX ... WHERE model = ...` in a migration. `halfvec`, not `vector`: half the size (1.5 KB a row
  at 768 dimensions, so a row stays inline in its page instead of being TOASTed, and the index is half as large), and fp16's three
  digits are far finer than the gaps between neighbours.

### Changing the search model

`SEARCH_MODEL` (`siglip2-b16`, the default, or `clip-b32`) names the model the worker embeds with and searches prefer. Changing
it, or a deploy that changes the default, needs no migration step and no downtime:

1. The worker's sweep queues an embed job for every photo without a vector from the configured model (priority 10, behind
   thumbnails and uploads). Photos uploaded meanwhile are embedded first (priority 0).
2. **Searches keep using the previous model's vectors** for each user until the configured model has indexed at least
   `SEARCH_SWITCH_PERCENT` (default 95) of that user's photos, and the assets image keeps the previous model's text tower
   for that (CLIP's is 66 MB; only one tower is in memory at a time). If the previous model's files are not installed, the new
   model answers at once, with whatever it has indexed so far.
3. While the old model answers, `GET /assets/search` says so (`reindexing: {model, indexed, total, percent,
   switch_at_percent}`, and `model` is the one that answered), `GET /assets/processing` has `embedding.reindexing`,
   `percent` and `serving_model`, and the page shows "Re-indexing with the improved search model: 40 %".
4. At the threshold the new model takes over (the stragglers are not text results until embedded). When **every** photo has a
   vector from the configured model, the worker's sweep deletes the vectors of the models it replaced (`prune_superseded`) and
   the old index empties.

A rollback is the same in reverse: set `SEARCH_MODEL` back; the old vectors are still there until step 4 has happened.
Embedding job versions are unique across models, so going back and forth never reuses a finished job by mistake.

### Which results are shown: the strong and the weaker tier

A ranked list always has an end only because it is cut. `crates/cyanotype-clip/src/cutoff.rs` cuts two tiers from the 300
best-first cosines (rules and numbers per model; tuning results in `docs/build.md`, "The cut-off"):

- **strong**, the default: (SigLIP 2) the model's own probability `sigmoid(scale x cos + bias)` (the checkpoint's `logit_scale`
  and `logit_bias`, 112.67 and -16.77) is at least 0.3 %; **and** the cosine is within 0.06 of the best photo's; **and** the list
  stops before the largest drop between neighbours if that drop is at least 0.02 (never within the first 3 results).
  The probability is tiny for keyword queries even when the answer is right (the model was trained on captions), which is why the
  floor is 0.3 % and not 50 %, and why the query gets "a photo of a" in front. (CLIP) only the relative and gap rules, within 0.03
  and a 0.015 drop: CLIP's softmax-trained cosines mean different things for different queries (0.30 is a great "dog" and a
  poor "dog on the beach"), so no absolute number separates a match from a non-match, and a CLIP library answers a query
  nothing matches with its nearest photos. This is a documented limitation of CLIP, and a reason SigLIP 2 is the default.
- **weaker**, on request (`weaker=true`): everything else down to a looser floor (SigLIP 2: probability 0.1 % and within 0.12;
  CLIP: within 0.04).

Pagination is stable across the tiers: the cursor carries the position, the `k` of the HNSW plan and where the tiers end, computed
once from the whole list on the first page. The response says how many strong matches there are (`strong_count`) and, when
the weaker ones were not asked for, how many exist (`weaker.count`) and the cursor that continues right after the last strong
one (`weaker.cursor`, set once a page has reached it; with no strong matches `weaker=true` alone lists the weaker ones).
`cutoff=false` returns the whole ranked list with every item's tier, for tooling and tests.

### More like this

`GET /assets/{id}/similar` ranks the owner's photos by cosine similarity to one photo's **own stored vector**, with the model
that answers text searches for that user right now (`SearchState::serving`: the configured model once it covers
`SEARCH_SWITCH_PERCENT` of the library, until then the one it replaces). Vectors of two models are never compared: the source's
vector is read for the serving model and version, and only photos with a vector of that model can be in the list. If the source
has none (just uploaded and still in the embed queue; or, during a model change, embedded only by the model that is not serving
yet) the answer is **409** "this photo is still being indexed" and the web app shows that state. The query runs the same way as
a text search (`search.rs`'s plans: exact distance when at most 8,000 photos pass the filters, else the model's partial HNSW
index with the index expression and predicate repeated as migration 0013 requires; the same cursor, depth of 300 and tiers),
so it takes the same time as one (the text model is not involved, which saves its ~10 ms).

Left out: the source itself, trashed photos, other users' photos, and the hidden RAW half of a pair (the source's RAW partner,
if it has one: a RAW + JPEG pair is one photo, `assets.primary_id`). The filters are those of `GET /assets/search` (dates, place,
camera, favorites, album, file type, has location); there is no text. The response is a search response: `score` (cosine
similarity to the source, about 0.4 to 0.97), `tier`, `strong_count`, `weaker`, `model`, `reindexing`, plus `similar_to`; `probability`
is null, because the sigmoid head of SigLIP 2 reads image-text cosines.

**The cut-off is its own** (`ImageRules` in `cutoff.rs`, `Model::image_cutoff_rules`). Photo-to-photo cosines are on another
scale than text-to-photo ones (a query's good match is 0.1 to 0.2 for SigLIP 2 and 0.3 for CLIP; two photos of one kind of thing are
0.6 to 0.9), and there is no per-query probability, so the text rules do not carry over. Unlike a text query's cosine, a photo-to-photo
cosine means about the same for every source, so both models get an absolute floor, plus a distance below the best neighbour
(so a source with a near-duplicate does not list loose matches as its equals) and the same gap guard:

| Model | Strong: cosine at least | within of the best | Gap | Weaker: cosine at least | within of the best |
|---|---|---|---|---|---|
| SigLIP 2 | 0.76 | 0.10 | 0.05 | 0.70 | 0.20 |
| CLIP | 0.74 | 0.10 | 0.05 | 0.68 | 0.20 |

Calibrated on the 209-photo evaluation set (`similar_bakeoff` in `tests/bakeoff.rs`, run by `just bakeoff`): every photo is the
source once, the other 208 are ranked by cosine and a result is relevant when it has the source's category (there are only 9
other photos of the same category, which caps recall, and categories are broad: a "dog" photo is relevant to any dog photo
whatever the scene, and a photo of a different category that looks alike counts as wrong, so these precisions are floors for
what a person would call similar). Cosines of same-category pairs have a median of 0.68 (SigLIP 2) / 0.66 (CLIP), of other-category pairs
0.52 / 0.48, and their 99th percentile is 0.70 / 0.68; the best neighbour of a photo has a median of 0.80 / 0.82.

| List (209 sources) | Precision | Recall | Photos per source | Sources with nothing |
|---|---|---|---|---|
| SigLIP 2, top 5 | 0.700 | 0.390 | 5.0 | 0 % |
| SigLIP 2, top 10 | 0.542 | 0.605 | 10.0 | 0 % |
| **SigLIP 2, strong tier** | **0.922** | 0.196 | 1.9 | 35 % |
| SigLIP 2, strong + weaker | 0.669 | 0.396 | 5.3 | 15 % |
| CLIP, top 5 | 0.660 | 0.369 | 5.0 | 0 % |
| CLIP, top 10 | 0.518 | 0.578 | 10.0 | 0 % |
| **CLIP, strong tier** | **0.919** | 0.224 | 2.2 | 30 % |
| CLIP, strong + weaker | 0.668 | 0.411 | 5.5 | 12 % |

The strong tier is deliberately strict (precision 0.92 against 0.54 for a plain top 10): in a set of one-off photos a third of
the sources have no close relative, and the page then says "No close matches" with the weaker button instead of ten loosely
related photos. A real library has bursts, duplicates and trips, so the strong tier is longer there than on this set; the
numbers are the floor for how few it gives. The sweep behind the choice (floor x distance, 55 cells per model) is printed by
the test.

### Bursts

Rapid-fire shots of the same moment (a held shutter button, a burst mode) are grouped so that a search shows one result for
them instead of ten near-identical photos in a row. The grouping is **stored** (`assets.burst_id`, migration 0014) and computed in
the background, not at query time.

**What a burst is** (`crates/cyanotype-core/src/bursts.rs`): a chain of one owner's visible photos taken with the same camera
(`camera_label`), in capture-time order (`taken_at`), where each photo is
- at most **2 s** after the one before it (capture times are whole seconds on most cameras, so this is "the same or the next
  second"), **and**
- **visually near-identical** to it: the cosine of their image embeddings is at least the model's threshold (SigLIP 2 0.91, CLIP
  0.87, `Model::burst_rules` in `cyanotype-clip`, `burst.rs`),
and a chain is cut where it would span more than **30 s** or hold more than **40 photos**, counting from its start. Only photos with
a capture time, a camera and a vector are ever grouped; trashed photos and the hidden RAW half of a pair are not (a RAW + JPEG pair
is one photo). **No vector yet: not grouped.** A time-and-camera-only fallback with a stricter gap was considered and left out:
two shots two seconds apart of different things are common, and "the same thing" is exactly what the embedding says; a photo
is grouped as soon as it is embedded (the embed job queues its regroup), so the cost is a short delay, and a library with
embedding turned off is not grouped at all (search does not work then either).

**Calibration** of the visual threshold (`burst_bakeoff` in `tests/bakeoff.rs`, run by `just bakeoff`). There is no real burst data
in the evaluation set (209 unrelated photos) and none could be taken from a real library, so a burst is **simulated**: every third photo
(70) gets 5 more frames with a camera move (shift and zoom up to 2, 5 or 10 % of the frame, in random directions) and an exposure drift
(up to 4, 10 or 20 % of full scale), each frame moved independently from the original; consecutive frames are therefore further
apart than frames of a real burst, whose subject moves less than a whole frame. They are compared with all 21,736 pairs of different
photos. Cosines of two different photos: median 0.48 (CLIP) / 0.52 (SigLIP 2), 99th percentile 0.77 / 0.77, 99.9th 0.92 / 0.93;
consecutive burst frames: median 0.95 to 0.98, 1st percentile 0.81 to 0.92 even for a 10 % move.

| min cosine | CLIP: different photos merged | CLIP: burst frames kept (2 / 5 / 10 % move) | SigLIP 2: different photos merged | SigLIP 2: burst frames kept |
|---|---|---|---|---|
| 0.80 | 0.74 % | 100 / 100 / 99.4 % | 0.75 % | 100 / 100 / 100 % |
| 0.85 | 0.44 % | 100 / 99.4 / 98.3 % | 0.50 % | 100 / 100 / 100 % |
| **0.87** (CLIP) | **0.30 %** | **100 / 99.1 / 96.9 %** | 0.44 % | 100 / 100 / 100 % |
| 0.89 | 0.17 % | 99.7 / 98.0 / 96.3 % | 0.34 % | 100 / 100 / 100 % |
| **0.91** (SigLIP 2) | 0.12 % | 99.4 / 96.3 / 91.1 % | **0.22 %** | **100 / 100 / 99.1 %** |
| 0.93 | 0.07 % | 98.0 / 93.4 / 78.6 % | 0.11 % | 99.7 / 100 / 97.4 % |

The thresholds keep at least 99 % of the frames of a 5 % camera move and merge 0.2 to 0.3 % of arbitrary pairs of different photos
(some of those are near-duplicates in the set: it holds a few photos that are the same picture twice, and many that are the same
kind of scene). That share is an upper bound for the real risk, because a pair is only judged when the same camera took both
within two seconds. CLIP's cosines are lower for the same change, so its threshold is lower. Real bursts differ (a moving subject,
a pan); the limits that keep a wrong merge cheap are the 2 s gap, the 30 s span and the 40 photos, and the expanded burst shows
every photo, so a wrong merge costs a tap.

**How it is computed and kept right.** A `burst` job regroups the neighbourhood of one photo from scratch: it loads the owner's visible
photos of that camera around the photo's capture time (a 2-minute window, doubled while a chain reaches its edge, so a chain is never
cut by the window and the cut points do not depend on which photo asked), judges each close pair by the first model that has a vector for
both photos (the configured model, then the one it replaces while a library is re-indexed), cuts the chains and writes `burst_id` (the
earliest member's id, on every member of a group of two or more; NULL otherwise) and `burst_version` where they differ. It is idempotent
and safe to run twice or at once (runs for one owner take an advisory lock). Jobs are queued by:
- the worker after it **embeds** a photo (so grouping follows indexing, including a model change: each new vector regroups its
  neighbourhood, and a pair is judged by the new model as soon as both photos have its vector);
- a **trigger** on `assets` when `deleted_at` or `primary_id` changes (migration 0014): a photo trashed, restored, paired as a
  RAW or unpaired. The job runs for the photo itself even when it is hidden now, which is how the photos around a removed one regroup
  without it. Purging needs nothing (the photo was trashed first);
- the worker's **sweep** (with the embedding sweep, every 15 minutes): every visible photo with a capture time, a camera and a vector whose
  `burst_version` is not `bursts::VERSION` (never computed, a rules bump, lost jobs) gets a job at backfill priority. This is how an
  existing library is grouped: **the migration itself writes no rows** (two nullable columns and two partial indexes; the window index is built over the existing photos, 3 MB at 50,000), and a library of 50,000 photos is
  grouped in the background over the following minutes. Jobs are one row per event (versions from a sequence) so a regroup can be
  queued while another runs; done ones are deleted after a day.

**Measured** (`burst_performance_on_50k_photos`, ignored, same synthetic library as the search numbers plus 3 near-identical shots a
third of a second apart for each of 5,000 photos: 65,000 photos, 20,009 of them in bursts afterwards; release build, busy shared host,
unconstrained Postgres): the sweep queued a job per photo in about 0.6 s (the migration itself writes no rows; the whole `migrate` run, the 3 MB window index included, took a couple of seconds at this size); a regroup takes **p50 1.8 ms, p95 3.7 ms** (a window of a handful of photos, one query for the window and one for the pairs), and the
whole library was grouped in 139 s on one thread (49,806 regroups ran; 15,194 jobs found their photo already grouped by an earlier job and
skipped). Search itself, with the whole 300-photo ranked list now fetched and grouped on every page instead of a page of 60: text
search "no filter" p50 225 ms (221 ms before, same session, 50,000 photos; the p95 of that run, 711 ms, was a load spike on the host),
More like this "no filter" p50 141 ms / p95 176 ms (138 / 166 before): no measurable change beyond the host's noise. Memory: the
response carries the members of every burst on the page (list items, at most 40 per burst), a few KB per burst; nothing is resident.

**In search and "More like this"** (`apps/assets/src/routes/group.rs`). The whole ranked list (at most 300 photos) is grouped in Rust
after ranking and before the tiers are cut, so everything after it counts **groups**:
- a group ranks by its **best photo**, and its tier is that photo's tier; its members are listed whatever their own scores;
- the **cover** is the best-ranked favorite of the group if it has one, else its best-ranked photo; `score` is the cover's;
- `strong_count`, `weaker.count`, `limit` and the cursor count groups. Pages are cut from the grouped list (the cursor carries
  the `k` that built the list, the position in groups and where the tiers end), so a burst is never split across pages;
- only photos that are **in the result** are in its burst: with a filter (say favorites) a burst shows only the members that pass it,
  and a burst with one photo left is a result of its own;
- each item with a burst has `burst: {size, members}`: `size` counts the cover, `members` are the other photos as list items with
  their `score`, best first (so the web app opens a burst without another request). At most 40 photos make a burst.
- **"More like this" leaves out the source photo's own burst** (the other shots of that moment are not "more like this"; you can
  see them next to it in the library). Other bursts in its results collapse like anywhere else. A photo whose burst has not been
  computed yet is just the source.
- A filter-only search and the timeline are **not** collapsed (the timeline stays unchanged; collapsing it is a possible follow-up).

### What is read out of the words

`apps/assets/src/query_parse.rs` takes dates and the user's own places out of `q` before it is embedded. Everything
is UTC and unit tested, including the ambiguous words:

| Words | Becomes |
|---|---|
| `2023`, `in 2023`, `from 2023` | that calendar year (1900 up to this year; `2027` stays words) |
| `June 2023`, `jun 2023`, `June of 2023`, `2023-06` | that month; `5 June 2023`, `June 5th 2023`, `2023-06-05`: that day; `2019-2021`: those years |
| `summer 2022`, `fall of 2021`, `winter 2022` | the season with its year, **northern hemisphere**: spring Mar-May, summer Jun-Aug, autumn/fall Sep-Nov, winter Dec-Feb, and `winter 2022` starts in December 2022 |
| `last year`, `this year`, `last month`, `this month`, `last week`, `this week` (Monday to Sunday), `yesterday`, `today`, `last summer`, `this autumn` | the calendar range; `last <season>` is the latest finished one |
| `in Lisbon`, `at Maui`, `near Paris`, a place at the end (`sunset Maui`) | the place filter, **only if the library has a photo geocoded there** (city, region or country, case-insensitive; country codes never) |
| `may`, `march`, `june`, `summer`, `fall`, `last`, `this` alone; `nice`, `reading`, `victoria`, `jersey`, `turkey`... without "in" | stay words: a month or season needs its year, and a place that is also a common word needs a preposition (`in Nice`) |

A filter chosen in the Filters sheet wins over words of the same kind (the words stay in the query). If only filler words
("photos", "pictures", "from", "in") are left, the search is a filter-only search (the timeline of the filter, newest first,
selectable as a scope). The response's `parsed` has `text` (what was searched for) and `chips` (`kind`, `label`, `matched`,
`from`/`to` or `value`/`place_kind`).

### The query

Both plans compute cosine distance between the query vector and the vectors of the caller's own live, visible photos
(`owner_id`, `deleted_at IS NULL`, `primary_id IS NULL`: no trash, no hidden RAW halves of pairs), rank the best 300, and
page them with a keyset cursor over `(distance, id)`, so a page boundary inside a run of ties loses and repeats nothing.

- **At most 8,000 photos pass the filters** (a small library, an album, a month, one camera): *exact* distance to every
  candidate, from the candidates' primary-key lookups (hash joins are switched off for the statement, because the planner's
  habit of reading the whole embeddings table for an album of 40 photos is the slow case). Complete and about 50 ms at
  8,000.
- **More than that** (a big library, however it is filtered): the nearest *k* vectors straight from the HNSW index
  (`hnsw.ef_search = 400` and `hnsw.iterative_scan = relaxed_order`, so the index keeps producing past `ef_search` when
  *k* is larger), then the filters on their photos. *k* starts at 450 without a filter and, with one, at 1.5 x 300 divided by
  the smallest share of vectors that can pass (more than 8,000 do: 2,800 at 50,000 photos). If the filters still leave fewer
  than 300 and the index has more, *k* grows fourfold (to at most 20,000) and the step runs again; the cursor carries the final
  *k*, so every page ranks the same list. Written as two steps because a join with a filter makes the planner *guess* how many
  rows pass, and a wrong guess silently turns the index scan into a scan of the whole table (measured: 15 ms became 300 ms).
- The queries are built per request and run as unnamed prepared statements: a cached statement is planned once with
  placeholders, and that generic plan was 20x slower than the plan for the actual dates.
- The file-type and camera filters read stored generated columns (`assets.file_ext`, the extension of the original's name, and
  `assets.camera_label`, the camera as the facets name it, with an index on its lowercase form): computing them per row
  cost hundreds of milliseconds at 50,000 photos. `raw` means a RAW file *or* a photo with a RAW
  paired behind it, so one JPEG + RAW pair counts under both `jpeg` and `raw`.

### Measured (50,000 photos, one owner)

`scripts/search-perf-seed.sql` builds a synthetic library (50,000 photos of one owner, 400 topics of clustered 512-d unit
vectors, 8 years of dates, 3 cameras, 40 places, GPS on half, JPEG/HEIC/RAW, 10 % favorites, an album of 40) and
`search_performance_on_50k_photos` (ignored; `PERF_DATABASE_URL=... cargo test --release -p cyanotype-assets
search_performance -- --ignored --nocapture`) times the whole request path of `GET /assets/search` (query vector in, the
60 items with their thumbnail URLs out; the text model is not in this number, it adds ~10 ms) for 40 queries each, on
`pgvector/pgvector:pg17` (pgvector 0.8.7) in a container limited to 768 MB with the production flags
(`shared_buffers=192MB`), after a `VACUUM ANALYZE`, on this 4-core host shared with other work. Query vectors are means of
40 random photos (a point between photos, as a text query is; a photo's own vector is an easier case for latency and a
noisy one for recall, because its topic-mates are near-ties).

| Search (ranked, 60 results) | Photos passing | Plan | p50 | p95 |
|---|---|---|---|---|
| no filter | 50,000 | HNSW | 42 ms | 64 ms |
| favorites | 5,000 | exact | 44 ms | 53 ms |
| one year | ~6,000 | exact | 60 ms | 72 ms |
| camera + year | ~2,000 | exact | 41 ms | 60 ms |
| a city | 1,250 | exact | 100 ms | 130 ms |
| an album of 40 | 40 | exact | 8 ms | 9 ms |
| favorites + has location | 5,000 | exact | 52 ms | 64 ms |
| has a location | 25,000 | HNSW | 81 ms | 96 ms |
| JPEG only | 25,000 | HNSW | 66 ms | 84 ms |
| HEIC only | 12,500 | HNSW | 85 ms | 105 ms |
| one camera | 16,700 | HNSW | 87 ms | 98 ms |

Everything is under 130 ms at p95 before the text model, so a text query answers in about 50 to 150 ms (the model adds ~10 ms,
nothing on a cached query), well inside the 300 ms target. These are warm numbers: with the Postgres cache cold (right after a
restart, or after the host's page cache was flushed) the first queries read the 65 MB index and 56 MB of vectors from disk
and took seconds (the first run of this benchmark after a big compile: p95 484 ms, worst 4.5 s), then settled. Sizes at
50,000 photos: embeddings table 56 MB, primary key 4 MB, HNSW index 65 MB (125 MB together; `assets` itself is 24 MB). Building
the index by inserting 50,000 vectors one at a time took about 5 minutes on this host (what the worker's backfill does as it
goes). Iterations that shaped the plan (each measured on this data): a join with a filter let the planner pick a whole-table
scan (300 ms); a cached generic plan for a date range was 20x slower than the plan for the actual dates; the extension
regex per row (400 ms) and the camera label per row (200 ms) became stored columns; a second HNSW pass when the first `k`
was too small cost as much as a generous first `k`.

**More like this** (`similar_performance_on_50k_photos`, ignored; same data and command pattern, 40 random photos as the source, 60
results, the whole request path including the uncached model-coverage count that production caches for 30 s): no filter (HNSW)
p50 138 ms / p95 166 ms, favorites (exact) 153 / 173, one camera (HNSW, filtered) 192 / 208, one year (exact) 152 / 197. These were
run on a busy shared host with an unconstrained Postgres container, where the text search above measured 221 / 250 ms for "no
filter": slower than the table, because of the host, and in the same range as a text query (no text model, and one primary-key
lookup of the source's vector more). The ranking query is the same, so the plans, recall and index sizes above apply.
Memory: nothing new is resident (no model is loaded for it, no new index; one 3 KB vector is read per request).

HNSW recall against the exact ranking (`ef_search = 400`, iterative scan, 40 queries): top 10 **0.997**, top 60 **0.998**,
top 300 **0.977** without a filter; 1.000 / 1.000 / 1.000 with a 50 % filter and with a 33 % filter.
(`ef_search` 40, pgvector's default, gave 0.83 / 0.89 / 0.96 on the same data and 0.46 / 0.46 / 0.57 for in-distribution
queries: the setting matters; 400 costs a few ms more than 100.) The synthetic data is easier or harder than real photos in
ways that are unknown (real recall with the real model on a real library is not measured here).

## API (assets service, owner-scoped)

| Request | Effect |
|---|---|
| `GET /assets/search?q=&from=&to=&favorite=&camera=&lens=&place=&type=&album=&has_location=&limit=&cursor=&weaker=&skip=&cutoff=` | Ranked results for `q` (the strong tier; at most 300 altogether, best first) or, without `q`, the photos matching the filters, newest first. Every parameter is optional; `from`/`to` are inclusive UTC days (`2024-03-01`), `camera` and `place` are the facet values below (case-insensitive; `place` matches a city, region, country or country code), `type` is `jpeg`, `heic` (also HEIF and AVIF), `raw` or `other`, `limit` defaults to 60 (at most 200). `weaker=true` also lists the weaker tier (continue with `weaker.cursor`); `skip=date`, `skip=place` or `skip=date,place` does not read that kind of thing out of `q`; `cutoff=false` returns the whole ranked list. Items have the shape of `GET /assets` plus `score` (cosine similarity, null without `q`), `probability` (the model's calibrated estimate, SigLIP 2 only) and `tier` (`strong`/`weak`). The response has `next_cursor`, `ranked`, `total` (first page of a filter-only search), and with `q`: `strong_count`, `weaker` (`{count, cursor}` or null), `parsed` (`{text, chips}`), `model` (which model answered) and `reindexing` (null, or how far the configured model is while an older one answers). `400` for a bad date, type, album id, cursor or a `q` over 200 characters; `503` when the model is not installed and `q` is set. |
| `GET /assets/{id}/similar?from=&to=&favorite=&camera=&place=&type=&album=&has_location=&limit=&cursor=&weaker=&cutoff=` | "More like this": the caller's photos most similar to this one (see above), the strong tier by default, `weaker=true` for the weaker one too (continue with `weaker.cursor`), the filters of search, no text. Response as search's, plus `similar_to`. `404` when the photo is not the caller's, is in the trash or is the hidden half of a pair; `409` when it has no vector yet for the model that serves searches; `400` for a bad filter or cursor. |
| (search and similar results) | Items of a ranked list may carry `burst` (`{size, members: [{...list item, score}]}`), see "Bursts". `strong_count`, `weaker.count`, `limit` and the cursors count groups (a burst is one). The `search` bulk scope is unchanged: it only exists for filter-only searches, which are never collapsed, and a ranked list's actions send the ids of the photos selected, members of collapsed bursts included. |
| `GET /assets/search/facets` | What the filters can be set to, with counts over the whole library: `total`, `favorites`, `with_location`, `types`, `cameras` (top 50), `places` (top 120, each with its `kind`), `years`. |
| `POST /assets/favorite`, `trash`, `restore`, albums, shares with `{"scope": {"search": {"camera": "...", ...}}, "except": [...]}` | The bulk endpoints accept the filters of a filter-only search as a scope (never `q`). |
| `GET /assets/processing` | Gains `embedding`: `indexed`, `total` (visible live photos with a preview), `queued`, `running`, `failed`, and for the configured model `model`, `percent`, `reindexing` (an older model answers meanwhile), `switch_at_percent` and `serving_model`. Indexing does not count towards `active`, `queued` or `running`, so the library page's "Processing..." banner is not on for hours after a model change; failed embed jobs do show in `failed` and `failures` and "Retry failed" requeues them. |

## Configuration

| Variable | Service | Default | Meaning |
|---|---|---|---|
| `SEARCH_MODEL` | worker, assets | `siglip2-b16` | The model to embed with and search with (`siglip2-b16` or `clip-b32`); an unknown name stops the service at start instead of silently re-indexing with something else. |
| `SEARCH_SWITCH_PERCENT` | assets | `95` | While a model change re-indexes, the previous model answers until the new one covers this share of a user's photos. |
| `SEARCH_MODEL_DIR` | worker, assets | `/opt/cyanotype/models` (where the images put it) | Directory with one sub-directory per model (`siglip2`, `clip`). Without the configured model's files the worker skips embedding (a warning at start) and the assets service answers searches with 503. |
| `WORKER_EMBED` | worker | `1` | `0` turns embedding off. |
| `WORKER_EMBED_THREADS` | worker | `1` | Intra-op threads of the image session. 2 roughly doubles the backfill speed and takes the second core of a 2 vCPU host from everything else. |
| `SEARCH_THREADS` | assets | `2` | Intra-op threads of the text session. |

## Tests

- `crates/cyanotype-clip/tests/model.rs`, for each model: preprocessing compared as a tensor with Pillow's (a 16 x 16 grid per
  channel; CPU independent), token ids of the Python `tokenizers` reference (CLIP's truncation to 77; SigLIP 2's lower-casing,
  truncation to 63 + `<eos>` and padding to 64, with Spanish, French and German queries), text and image embeddings against
  reference vectors made by one-off scripts (`tests/fixtures/make_reference.py`, `make_reference_siglip2.py`) *and* against the
  fp32 graph of the same export (the CPU-independent anchor), that threads and batching do not change a vector, the query cache,
  the split text model giving the same vectors, and a **quality smoke test**: 20 CC0 photos from Wikimedia Commons
  (`tests/fixtures/images/SOURCES.json`), two queries each, the right photo must be in the top 3 of 20 (recall printed), and for
  SigLIP 2 that absent subjects come back with no strong match. `src/cutoff.rs`, `src/split.rs` and `src/lib.rs` have unit tests
  (tiers, the gap rule, the splitter, the query template, calibration). They need the model files (`SEARCH_MODEL_DIR`); without
  them they skip, and in CI (`CYANOTYPE_REQUIRE_MODEL=1`) a missing model is a failure.
- `crates/cyanotype-clip/tests/bakeoff.rs` (ignored; `just bakeoff`): the model comparison of `docs/build.md` on the 209-photo
  evaluation set (`scripts/fetch-eval-set.sh`, `tests/eval/`), printed as tables.
- `crates/cyanotype-core/tests/embeddings.rs` (real Postgres with pgvector): schema (untyped `halfvec`, one partial HNSW index
  per model, pgvector >= 0.8), two models in the table and each search using its own index, a vector of the wrong size for its
  model refused, storing and replacing vectors, pruning the replaced model, the sweep (which photos it queues, idempotent, a
  version bump or a new model id re-queues everything, failures are left alone), migration 0012's backfill and its idempotency,
  claim order by priority, embeds waiting for foreground work, progress counts, and which model serves while another indexes
  (`embeddings.rs` unit tests).
- `apps/worker/src/embed.rs` tests (Postgres + S3 + the model): embed jobs write vectors and a text query's nearest vector is
  the right photo, hidden RAWs / photos being deleted / photos without a preview are skipped, a corrupt preview fails
  permanently and a missing one retries, an older model's vector stays until every photo has the new one and is then removed,
  and a thumbnail job queues its embed job at priority 0 which then reads the preview the thumbnail wrote.
- `apps/assets/src/routes/search_tests.rs` (Postgres, fake text model): ranking and scores, every filter alone and combined
  with text, file types incl. RAW pairs, albums, owner isolation on both plans, trash / purging / hidden RAWs excluded,
  unindexed or other-model vectors ignored, keyset pagination without gaps or repeats on both plans (including ties), the
  300 depth limit, plan choice, filter-only totals and the `search` scope, facets, validation and the 503.
  `search_performance_on_50k_photos` (ignored) is the measurement above.
- `apps/assets/src/routes/search_v2_tests.rs`: the strong and weaker tiers for both models (probability, no-strong-match
  queries, the `weaker.cursor` continuing after the last strong match, one-result pages across the tiers), dates and places read
  out of the query and turned off again, places from the caller's own photos only, the model swap (the old model answers
  until the new one has 95 %, then the new one; a fresh library; a missing old model), and `apps/assets/src/query_parse.rs`
  unit tests for the parser (years, months, days, seasons, relative dates, the ambiguous words, multi-word places).
- `apps/assets/src/routes/similar_tests.rs`: ranking by the photo's own vector, the source left out on both plans, other people's
  photos / the trash / purging / the hidden RAW left out and a 404 for a photo that is not the caller's, each filter, one-result
  paging, the strong and weaker tiers (and the cursor between them), the 409 for a photo without a vector (and for an old
  version of it), and the model switch (CLIP answers with CLIP's vectors only until SigLIP 2 covers the library; a photo that only the
  new model has embedded is "still being indexed"). `cutoff.rs` has unit tests for the image rules.
- `crates/cyanotype-core/tests/bursts.rs` (Postgres): chains by gap, camera, owner and vector, ties and the burst id, the model fallback
  (the configured model decides when it has both vectors, else the older one), regrouping after trash, restore and pairing (the trigger and
  the neighbours), a chain longer than the first window cut the same way from either end, idempotence, the sweep (queued, failed, done,
  a rules bump), the embed hand-off, and pruning finished jobs; `bursts.rs` unit tests for the grouping rules (gap, span, size, the visual
  test only asked when time allows). `apps/assets/src/routes/burst_tests.rs`: a burst as one result at its best photo's place, tiers and pages
  counting groups, a favorite as the cover, filters and trashed photos applied to the members, "More like this" leaving out the
  source's burst, filter-only searches never collapsed; `group.rs` unit tests for the collapse.
- Web: `burst.test.ts` (flattening, what a click selects, patching and removing photos inside bursts), `selection.test.ts` (group
  selection), `Grid.test.tsx`, `Search.test.tsx` (the badge, opening a burst, selecting a collapsed burst and the note, one photo at a
  time once open, Select all with members) and the burst states of `e2e/mobile-overflow.spec.ts`.
- Web: `searchText.test.ts`, `SearchBar.test.tsx`, `FilterSheet.test.tsx`, `Search.test.tsx` (the weaker tier button, no close
  matches, More like this (chip, filters beside it, select-all of the loaded photos, the not-indexed state), parsed chips and removing one, select-all over a parsed filter-only search, the re-index note), and
  `e2e/mobile-overflow.spec.ts` (bar collapsed and expanded, recent searches, results, selection, the filter sheet with
  long names, chips, parsed chips with a very long place, the weaker tier offered and shown, no close matches, no results,
  indexing, re-indexing, unavailable, intro, and the similar-photos page with its chip, weaker tier, lightbox button and not-indexed state) at 320 to 1280 px, light and dark.

# Photo details

Apple Photos style details for the photo in the lightbox: when and where it was taken, with what, and what the file is.
The panel is derived from what is already stored (the asset's columns and the EXIF dump in `assets.exif`), so **photos
imported long ago need no reprocessing** and there is no migration.

## What the user sees

- **Open it** with the Info button in the lightbox's bar, the `i` key, or (on a phone) **a swipe up on the photo**. On a wide
  screen it is a column beside the photo; on a narrow one the photo moves up and the details sit under it (a header with a
  grip: drag it down, or tap *Close info*, to put it away). It stays open while paging with the arrows, and shows the next
  photo's details. **Escape** closes the panel first (a second one closes the lightbox); the browser's **Back** button (or a
  phone's back gesture) does the same, through one history entry that the panel takes when it opens and gives back when it
  closes (also when the lightbox closes with it open, or a link in it is followed).
- **Rows, only when the photo has the information:**
  - *Date*: weekday, date and time in the photo's own time zone when EXIF recorded one (`UTC+02:00`), otherwise "Time zone
    unknown" (the time is then the camera's clock as it was written). A photo with no capture time says its date is the
    file's modification date or the day it was added. The date is a link that scrolls the library to that day.
  - *Location*: the place line (city, region, country: a link to the search in that place), the coordinates, GPS altitude
    (`38 m above sea level`) and the direction the camera faced (`132° SE`, with "(magnetic)" when the camera did not say
    true north), and a **small map with a pin** (not interactive: a tap opens `/map` centred there). It is the map page's
    own style and basemap, so its tiles come same origin through the assets service; without a basemap the pin sits on the
    plain background, and without WebGL the map is left out and the coordinates remain.
  - *Camera* and *Lens* (links to the search filtered by that camera, or that lens).
  - *Exposure* in the familiar line `ISO 100 · 26 mm · ƒ/1.8 · 1/120 s · +0.3 ev` (only the parts the photo has), the
    *35 mm equivalent* when the EXIF has `FocalLengthIn35mmFilm` and it differs from the focal length, and the secondary rows
    *Flash* (fired or not, with the mode), *White balance*, *Metering* and *Exposure program*.
  - *File(s)*: name, format (`HEIC`, `JPEG`, `CR3 (RAW)`...) and size of every file (a RAW + JPEG pair lists both), then
    *Resolution* (`4032 × 3024 · 12 MP`), *Colour space* (`sRGB`, or `Uncalibrated` when the EXIF says so: the ICC profile's
    name is not read, so a Display P3 photo shows as uncalibrated) and *Software*.
  - *Albums* (links) and *Burst* (`3 of 8 in a burst`, in capture order).
- Camera, lens and place link to `/search?camera=` / `lens=` / `place=`; the date to `/library?day=YYYY-MM-DD`; the map to
  `/map?lat=&lon=`. The search gained a **lens filter** (and `lenses` in the facets, a *Lens* field in the filter sheet and a
  chip) so that a lens link is a filter like the others.

## How it works

`GET /assets/{id}/details` (a separate endpoint, so the viewer's detail request stays as light as it was) reads one row and
formats it on the server (`apps/assets/src/details.rs`, pure functions with unit tests), so the web only lays the text out:

- **The dump.** `assets.exif` holds tag names to display strings from the primary IFD with its Exif and GPS sub-IFDs
  (`"FNumber": "1.8"`, `"ExposureTime": "1/120"`, `"Flash": "fired, return light detected"`, `"GPSAltitudeRef": "above sea level"`).
  JPEG, HEIC (the "Exif" item) and TIFF-based RAW land in that shape; a RAW whose EXIF the reader cannot open keeps rawler's own
  serialisation (snake_case keys, rationals as `{n, d}`, enums as numbers), which the readers accept too. Unreadable values
  (`unknown`, `[reserved ...]`, `0`) leave their row out.
- **Formatting.** Shutter: `1/120 s` below a third of a second (the camera's `1/119.99999` rounds to 120), decimals from there
  (`0.3 s`, `2 s`). ƒ-number: one decimal, none for whole stops (`ƒ/1.78` reads `ƒ/1.8`, `ƒ/4`). Exposure bias: always signed
  (`+0.3 ev`, `−0.7 ev` with a real minus sign, `0 ev`), thirds rounded to one decimal.
- **The time zone.** `taken_at` is a `timestamptz`: the worker stores the instant only (a missing `OffsetTime` is read as
  UTC). The offset therefore comes from the dump: the first date tag the worker would have used, with its own `OffsetTime*`
  tag, and only when the two give back exactly the stored instant. The weekday and the clock follow the photo's own zone; the
  day the *timeline* files the photo under stays the UTC day (`date.timeline_day`), which is what the date link jumps to.
- **Rules as the detail endpoint:** the caller's own photo that is not in the trash (a hidden RAW answers as it does there);
  another user's photo or an unknown id is a `404`. Share links never use this: no public endpoint reads the dump, and a
  shared photo's derived images and JPEG copies carry no metadata (see "Share links").
- **Not done:** an ICC profile name (it is not stored; extracting it would need the metadata job re-run for every photo),
  lens make, serial numbers, maker notes.

## API (assets service, owner-scoped)

| Request | Effect |
|---|---|
| `GET /assets/{id}/details` | `{id, date: {instant, local, offset, zone_known, weekday, basis, timeline_day}, location: {latitude, longitude, place, altitude, altitude_m, direction, direction_deg} \| null, camera: {label, make, model, lens} \| null, exposure: {line, iso, focal_length, focal_length_35mm, aperture, shutter, bias, flash, white_balance, metering, program} \| null, file: {name, format, raw, content_type, size_bytes, width, height, megapixels, color_space, software}, files: [{id, name, format, raw, size_bytes}], albums: [{id, title}], burst: {position, size} \| null}`. Text values are formatted and null when unknown. `404` for a photo that is not the caller's live photo. |
| `GET /assets/search?lens=` and the other places that take the search filters (`similar`, the `search` scope of the bulk endpoints) | Photos whose EXIF lens model equals `lens` (case-insensitive). `GET /assets/search/facets` gains `lenses` (top 50 lens models with counts). No index: the filter reads `lens_model` of the owner's photos like the place filter does of its columns. |

## Tests

- `apps/assets/src/details.rs` unit tests: shutter fractions and decimals, ƒ-number rounding, exposure bias sign and rounding,
  the 35 mm equivalent (shown only when it adds something, `0` and `unknown` ignored), missing and unreadable fields, every
  exposure-time form, GPS altitude (above and below sea level) and direction (true and magnetic), rawler's own dump, flash
  wording, colour space and software, the photo's own zone, weekday and timeline day across midnight UTC, an unknown zone,
  an offset that disagrees with the stored instant, fallback dates, file formats and megapixels.
- `apps/assets/src/routes/details_tests.rs` (Postgres): a fully described photo, a bare one (no rows), coordinates without a
  place, a RAW + JPEG pair listing both files, burst position in capture order (and a trashed member leaving the count),
  owner isolation (404 and no leak in the body), the trash, signed out, and the detail endpoint unchanged. `search_tests.rs`
  covers the lens filter and the `lenses` facet.
- Web: `detailsText.test.ts`, `DetailsPanel.test.tsx` (every row, hidden rows, the pair, every link's target, retry, focus),
  `LightboxInfo.test.tsx` (button, `i`, Escape order, Back, the history entry given back, paging, retry, a dialog over the
  lightbox, the swipe: up, a quick flick, a mouse, slow/short/downward/sideways drags, a pinch, a zoomed page, a button,
  and down on the header), `PlaceMap.test.tsx`, plus `timeline.test.ts` (`dayTop`), `searchText.test.ts` and
  `FilterSheet.test.tsx` (the lens).
- `e2e/mobile-overflow.spec.ts`: the panel with every row at its widest (very long camera, lens, place, software, file and
  album names, a RAW pair, a burst) and with none but the file, scrolled to its end, closed again with Escape and `i`, at 320 to
  1280 px, light and dark. `e2e/timeline.spec.ts`: a real touch swipe up (Chromium touch events) and the Info button open the panel,
  Back closes only the panel, and the date link scrolls a 400-photo library to the day it names.
