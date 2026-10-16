# Library: trash, restore and permanent deletion

## What the user sees

- **Select** (library toolbar) turns on selection mode: click or tap to select, Shift-click for a range, long-press on
  a touch screen to start selecting, "Select all" (everything loaded so far) and "Clear". Escape clears the
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
- The photo viewer lists **RAW + JPEG** (or **RAW + HEIC**, ...) with both file names and sizes and an **Unpair**
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
