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
{"scope": "library" | "favorites" | "trash" | {"album": "<id>"} | {"person": "<id>"} | {"trash_batch": "<time>"}, "except": ["<id>", ...]}
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
| `GET /assets/timeline[?favorite=true]` | `{total, days: [{day: "YYYY-MM-DD", count}]}`, newest first, owner-scoped. The timeline view counts one photo per stack while stacks are collapsed (`?stacks=`, "Stacks in the timeline"). |
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

# Downloading as a ZIP

## What the user sees

- **An album** has **Download** in its header (smart albums too: what the rules match now). **Selecting** photos (in the
  library, an album, search, a person's page, a tag) adds **Download** to the selection bar. **Select all** downloads
  every photo of the view (minus any you un-ticked) without the page ever listing them.
- The button asks the server what the archive will hold, says "Downloading 12 files (48 MB) as a ZIP" and starts an
  ordinary browser download. The browser shows its own progress (the response announces its exact size) and saves the
  file; the page stays where it is. If there is nothing to download, or the selection is too big, a message appears
  instead of a failed download.
- A **share page** shows **Download all** when the owner allows downloads for the link (see "Share links" below).

## What is in the archive

- The **originals** of your live photos, byte for byte, under their **original file names** (folders and `..` are
  stripped; characters Windows cannot unpack become `_`). Names that repeat, ignoring case, become `IMG_0001 (2).jpg`,
  `IMG_0001 (3).jpg`. Edits (rotate, crop) are not applied: for those the viewer's "Download edited" stays.
- A **RAW + JPEG pair is both files** (`IMG_0007.JPG` and `IMG_0007.CR3`): the single-photo download offers the pair as
  a choice, an archive of your library should hold everything. Each entry's time is the photo's capture time, else the
  file's own modification time.
- The same rules as listing: only your own photos; never trashed or being-purged ones (the Trash scope is for restoring,
  not downloading); hidden photos never through an album or a scope, and by explicit ids only while the Hidden view is
  unlocked in that session (an API key never unlocks it). The file is named after the album (`Alpha trip.zip`), after the
  page's `name` for a selection, or `cyanotype-photos.zip`.

## How it works

The assets service reads each original from the bucket and relays it chunk by chunk into a **store-only** ZIP (JPEG, HEIC
and RAW do not compress), so memory stays at one chunk whatever the album's size and the response starts at once (the first
entry's header is sent before its bytes are fetched). Entries are written the streaming way (a data descriptor after each
file), so the archive's exact length is known up front and sent as `Content-Length`. **ZIP64** is used for any file of
4 GiB or more and for archives past 4 GiB or 65,534 files. If an original cannot be read half way (it is missing from the
bucket, or its size is not what the database says) the connection is aborted rather than ended, so a broken archive is
never mistaken for a whole one; the cause is in the service log.

The browser receives the response itself: the page submits a hidden form (`POST /assets/zip`, session cookie, same origin;
the gateway's `form-action 'self'` allows it) and the `attachment` response goes to disk. Nothing is built in the page's memory.

## API (assets service, owner-scoped)

| Request | Answer |
|---|---|
| `POST /assets/zip/plan` | `{files, bytes, name}`: what the archive would hold. `404` when nothing qualifies, `413` over the file limit. |
| `POST /assets/zip` | The archive (`application/zip`, attachment, `Content-Length`). |
| `GET /assets/albums/{id}/zip` | The archive of one album: a plain link, and the call for API keys with the `read` scope. |

The POST body is a bulk selector (see "Bulk actions on a whole view": `ids` up to 1000, or a `scope` with `except`) and an
optional `name`; it is JSON, or a form with that JSON in a `selection` field. A request with a `read` key can use the
`GET`; the `POST` needs the `full` scope like any other write route.

Limits: at most **50,000 files** per archive (`413`: download it in parts), **16 archives streaming at once** per assets
process (`429` with `Retry-After`). An archive of your own photos has no size limit.

Metrics: `cyanotype.assets.zip_downloads` (tag `source`: `album`, `scope`, `ids`); the service logs each archive's file count
and size (never names or ids of photos).

## Tests

`cargo test` (the route tests need `DATABASE_URL` and the `S3_*` variables, a real bucket): the writer (reads back, ZIP64
end record, exact predicted length, DOS time clamping, safe unique names), the archive of an album (valid ZIP, `unzip -t`,
bytes equal to the originals, deduplicated names, path-like names, capture times, file name), pairs, form and JSON bodies,
scopes with `except`, the plan, smart albums, owner isolation, trashed, purging and hidden photos (locked and unlocked),
the file limit, that the body is streamed in small pieces (3 x 8 MiB: first frame is a header, no frame above 1 MiB) and
that a missing original aborts it. `cargo test -p cyanotype-assets zip64_over_4_gib -- --ignored` writes a real 4 GiB+ entry
and has `unzip -t` read it (about a minute; needs 4 GiB of disk). Web (vitest): the request body, the form post, the button
and its refusals, the share page's wait-and-follow. Browser: `e2e/zip-layout.spec.ts` (album header, selection bar, share
page at 320 to 414 px and 1280 px, light and dark; the plan and the post are really requested).

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
- **Download all** (when the owner allows downloads) is one streamed ZIP of the link's downloadable photos (below).

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
| `GET .../zip/plan` | `{files, bytes, pending, skipped, name, max_files, max_bytes}`: what "Download all" would hold. Queues the download copies that are missing (`pending` of them); the page asks again until it is `0`. |
| `GET .../zip` | One streamed ZIP of the photos the link offers for download (the same ones as the per-photo downloads, nothing else): the location-free JPEG copies under their original names (an edited photo: its edited render), or, with "include originals", the untouched files, a RAW + JPEG pair as both. Photos not offered (HEIC, RAW, ... by default) are left out and counted in `skipped`. `403` when downloads are off, `409` while copies are still being made, `413` over the limits, `429` over the rate limit. Hidden and trashed photos are never in it. |
| `GET .../items/{id}/download?file=` | `307` to a presigned attachment URL (one minute). `403` when downloads are off, `409` while the copy is being made, `404` for anything that is not offered (a RAW without "include originals", HEIC by default, another photo). |

**Download all limits** (a link has no account to charge, so one request is bounded): at most **1,000 files and 4 GiB**
per archive (`413`, "download them one by one"), **6 archive requests a minute per visitor address** (the plan and the archive
each count once, so about 3 downloads a minute; `429` with `Retry-After`), and **4 archives streaming at once** per
assets process, apart from the owner's own 16 (`429`). The token authorises exactly the photos the link shows: its album
(or selection) as listed right now, minus hidden and trashed ones. The same `no-store`, no-referrer and robots headers apply.
The metrics are `cyanotype.shares.downloads` and `cyanotype.shares.zip_downloads`.

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
  the query (`skip=date`, `skip=place` or `skip=people` in the URL; the people the owner has named are read out of it too while People is
  on, `docs/faces.md`, "People in search, albums and bulk actions"). Only places in the library count ("in Tokyo" is just words if no
  photo was taken there), and words that are also ordinary words ("nice", "reading", "may", "victoria") are never a place
  or a date without the right context ("in Nice", "May 2023"). The rules are below.
- **Filters** (button under the header; a bottom sheet on a phone, a dialog on a wide screen): year or a date range,
  favorites, camera, place (city, region or country), file type (JPEG, HEIC, RAW, other), has a location, album, and with People on
  (`docs/faces.md`) the people who must all be in the photo. The
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
  "-" that closes it again). This is in text searches and in "More like this", and in the timeline as stacks (see "Stacks in the
  timeline" at the end); the albums and a filter-only search list every photo. See "Bursts" below.
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
- A filter-only search is **not** collapsed. The timeline collapses the same groups as **stacks** ("Stacks in the timeline" at the end),
  and search now uses the stack rule too: a photo the owner took out of a burst is its own result, and a stack made by hand is one result.

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
| `GET /assets/search?q=&from=&to=&favorite=&camera=&lens=&place=&people=&type=&album=&has_location=&limit=&cursor=&weaker=&skip=&cutoff=` | Ranked results for `q` (the strong tier; at most 300 altogether, best first) or, without `q`, the photos matching the filters, newest first. Every parameter is optional; `from`/`to` are inclusive UTC days (`2024-03-01`), `camera` and `place` are the facet values below (case-insensitive; `place` matches a city, region, country or country code), `type` is `jpeg`, `heic` (also HEIF and AVIF), `raw` or `other`, `limit` defaults to 60 (at most 200). `weaker=true` also lists the weaker tier (continue with `weaker.cursor`); `skip=date`, `skip=place`, `skip=people` or several (`date,place`) does not read that kind of thing out of `q`; `people` is a comma-separated list of person ids (at most 10, all of them in the photo; People only, ignored while it is off); `cutoff=false` returns the whole ranked list. Items have the shape of `GET /assets` plus `score` (cosine similarity, null without `q`), `probability` (the model's calibrated estimate, SigLIP 2 only) and `tier` (`strong`/`weak`). The response has `next_cursor`, `ranked`, `total` (first page of a filter-only search), and with `q`: `strong_count`, `weaker` (`{count, cursor}` or null), `parsed` (`{text, chips}`), `model` (which model answered) and `reindexing` (null, or how far the configured model is while an older one answers). `400` for a bad date, type, album id, cursor or a `q` over 200 characters; `503` when the model is not installed and `q` is set. |
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

# Editing a photo

Fix a wrong date, set or correct a location, add a caption: for one photo from its details panel, for a selection from the
library's selection bar (*Edit*: move all dates by an offset, one location for all, one caption for all). **The original in the
bucket is never touched** and a download is byte-identical to what was uploaded; share links' JPEG copies carry no caption.

## What the user sees

- **Panel (one photo):** *Adjust date and time* (a date-time field and a time zone: UTC-12:00 to UTC+14:00 in half hours, or
  "Unknown"), *Edit location* / *Add location* (tap the map or drag the pin, pick one of **your own places** from a search box,
  or type coordinates) and *Add a caption* / *Edit caption*. An edited value is marked "Edited", and each editor says what the
  file itself says and has **Revert to original**. A caption also shows under the photo in the viewer.
- **Selection (many photos, up to 1000):** *Edit* opens a choice of the three. A date change is a **shift** ("1 day 2 hours
  earlier"): every photo moves by the same amount and keeps its own time zone, so a camera whose clock was wrong is fixed for a
  whole trip at once. Revert works on all of them too.
- The effective values are what everything uses: the timeline (the photo moves to its new day; the library refreshes), the map,
  date filters, the place line (the place is forgotten and looked up again for new coordinates), bursts, and the day the date
  link jumps to. Text search finds a photo by the words of its caption (below).

## How it works

- **Where the edit lives.** `assets.taken_at`, `latitude` and `longitude` stay the *effective* columns, so nothing that reads them
  changed. An edit overwrites them and keeps what the file said in `taken_at_exif`, `latitude_exif`, `longitude_exif` (set when
  the photo is first edited, cleared on revert); `date_edited` / `location_edited` say which parts are edited,
  `taken_offset_edit` the zone chosen for an edited time, `caption` the caption and `edited_at` when it last changed (migration
  0015; new columns with constant defaults, so no rows are rewritten). The worker's metadata job writes the file's values to
  `*_exif` instead of over the effective ones while a part is edited, so a re-run (a migration, a `reindex`) never undoes a
  correction.
- **Where the edit survives: the bucket.** Before the database changes, the service writes the photo's whole edit state to
  `edits/{user}/{hash[0:2]}/{hash}.json` (named like the original: by owner and content hash, which a rebuilt database keeps,
  unlike asset ids): `{v, updated_at, taken_at?, taken_offset?, latitude?, longitude?, caption?}`; a part that is not edited is
  absent. A photo whose edits are all reverted keeps an empty object (a tombstone). `cyanotype reindex` reads `edits/` and applies
  every object to the photo with that owner and hash when the row has no edits or older ones (`edited_at`), see
  `docs/recovery.md`. I chose a per-photo object over adding the edits to the daily manifests because the manifests are up to a
  day old (an edit made after the last one, then a lost database, would be gone), while an object is as new as the edit. The cost
  is one small PUT per edited photo (a bulk edit is up to 1000, written 8 at a time; `failed` lists the ids to retry).
  Permanently deleting a photo deletes its edits object with all its versions: a caption does not outlive its photo.
- **Side effects.** A new location clears the place and queues the `geocode` job again. A new time marks the photo and the rest of
  its old burst "to be computed" and queues a regroup at the photo (its new neighbourhood) and at another member of the old
  burst; the sweep covers a lost job.
- **Rules as everywhere:** only your own photos that are not in the trash and not the hidden RAW of a pair (any other id counts as
  `missing`); idempotent (the same request again is `unchanged` and writes nothing).
- **Captions in search.** With a text query, photos whose caption contains **every** word of it (case-insensitive, as part of a
  word; words under three letters and a few stop words are ignored; at most 8 words) come **first**, newest first, at most 50,
  each its own strong result with no score, then the model's ranking follows (a photo that is both is shown once, in front).
  They are *in front of* the ranking, not blended into it: the model's cut-off is relative to its own best match, so mixing
  caption hits in as "perfect scores" would push every real semantic match into the weaker tier. The request's filters apply to
  them; a caption match needs no embedding, so it works while the library is still indexing. It is a substring match, not a
  stemmed or typo-tolerant one (modest on purpose).
- **Not done:** clearing a location (hide it without replacing it), captions in the grid, bulk edit from the search, album and map
  pages (the selection bar of the library has it; the viewer's panel edits one photo anywhere), editing the hidden RAW of a pair
  (it keeps its own EXIF), a caption in share links.

## API (assets service, owner-scoped)

| Request | Effect |
|---|---|
| `POST /assets/edits {ids \| scope+except, date?, location?, caption?}` | `date`: `{local: "2023-07-04T13:05:09", offset: "+02:00" \| null}` (set; no offset = zone unknown, read as UTC), `{shift_seconds}` (move each photo) or `{revert: true}`; `location`: `{latitude, longitude}` or `{revert: true}`; `caption`: `{text}` (empty clears) or `{revert: true}`. At most 1000 photos (a scope that resolves to more: `400`). Response `{updated, unchanged, missing, failed: [ids]}`. `400` for nothing to change, a bad date, zone, coordinate or a caption over 2000 characters. |
| `GET /assets/places?q=` | The places of your own photos (city, region, country, mean `latitude`/`longitude`, `count`), most photographed first, at most 20; `q` matches the start of a word of the city, region or country. |
| `GET /assets/{id}/details` | Gains `caption` and `edits: {date_edited, location_edited, original_date, original_location}` (what the file says, for Revert); `date` carries the zone of an edited time. `GET /assets/{id}` gains `caption`. |

## Tests

- `crates/cyanotype-core/src/edits.rs` unit tests (the object's shape and round trip, validation, newer layouts and garbage refused),
  `keys.rs` (the edits key), `tests/reindex.rs` (a rebuilt database: orphans recreated, then their edits restored, the dry run, objects
  that are not valid or have no photo, idempotence).
- `apps/worker` (`handlers.rs`): the metadata job keeps an edited date and location and records the file's own next to them, and an unedited
  photo takes the file's values.
- `apps/assets/src/routes/edits_tests.rs` (Postgres + an S3 API): override, revert part by part, the object in the bucket, zone-less
  edits, bulk shift (zones kept, fallback times, shifting back restores), one location and caption for all, owner isolation (another
  user's, trashed, hidden RAW and scopes write nothing), validation, bursts and geocoding re-queued, caption search (every word, others'
  captions and the trash excluded, filters, paging through the caption matches), places, **reindex restoring edits onto a database from
  before them** (also reverts, and an idempotent second run), and the purge deleting the object and its versions. `routes/edits.rs` unit
  tests (local time parsing, the request forms, bad requests).
- Web: `editText.test.ts`, `EditDialog.test.tsx` (date and zone, an unknown zone, a shift, revert, location from the map, a place or typed
  coordinates, caption, errors and retrying only the failed photos), `LightboxEdit.test.tsx` (the editors from the panel, refresh after a
  save, the caption under the photo, Escape order), and the edit states of `e2e/mobile-overflow.spec.ts` (the three editors in the viewer,
  the selection bar's Edit, its menu and the shift dialog) at 320 to 1280 px, light and dark.

# Hidden, archive, memories and recently added

Four small library features (migration 0020): photos you can **hide** behind your password, an **archive** for photos you keep
but do not want in the timeline, **On this day**, and a **Recently added** order. Everything about hidden photos is enforced by
the server, in every query that can return a photo; the web app only decides what to draw.

## What the user sees

- **Hidden.** *Hide* in the viewer's menu and in the selection bar (*Unhide* inside the Hidden view). A hidden photo is in no
  other view: not the timeline, Favorites, Recently added, the Archive, search results and their facets and place suggestions, the
  map, albums (it stays a member: the album says "N hidden" to its owner and never uses it as the cover), memories, "More like
  this", burst covers, day counts and the scrubber, and **never in a share link**, even one made from an album it is in. *More*
  (the fourth pill of the library header, "..." on a narrow phone) opens *Hidden*, which asks for the **account password**; the
  view then stays open for **10 minutes** with a "Lock now" button, and asks again when the time is up.
- **Archive.** *Archive* in the viewer menu and the selection bar, with an **Undo** toast (a whole-view archive is undone as the batch
  it was, as in the trash). Archived photos leave the timeline, the map and memories, and stay in albums, Favorites, search (with an
  "Archived" badge) and the *Archive* view, where *Restore* puts them back. Search has an **Include archived** filter, on by default
  (off shows a "Without archived" chip).
- **On this day.** A strip at the top of All photos, one card per earlier year ("3 years ago", with the count); a tap opens the page
  with all of them, a section per year. *Select* there, then *Not this one*, takes photos out of memories (with Undo); the strip has
  *Hide* (turn it off; the page has the switch to turn it on again). Nothing for today: **no strip at all**.
- **Recently added.** *More*, *Recently added*: the library by **upload time**, with day headers of the day of upload. The header of
  All photos has a **Date taken | Date added** toggle, remembered per user. (Which import batch a photo came from is not shown: nothing
  records a batch, and grouping by upload time would only guess.)

## How it works

- **Columns.** `assets.hidden`, `assets.archived` (+ `archived_at`, the batch of a bulk archive), `assets.memory_dismissed`,
  `sessions.hidden_unlocked_until`, and the table `user_prefs (memories_enabled, library_sort)`. The visible photo of a RAW + JPEG
  pair carries both flags and the hidden RAW takes the pair's (pairing carries a RAW's own flag to its JPEG, never the other way
  round), so separating a pair keeps the state.
- **One place says which photos a view shows**: `visibility::view_sql`, used by the list *and* by its day counts, so they always
  agree (timeline: not hidden, not archived; favorites: not hidden; archive: archived, not hidden; hidden: hidden, unlock needed).
  Search and everything built on it go through `Filters::push_where`, which always has `NOT a.hidden` (the ranked plans, the
  timeline-order list, caption matches, counts, facets, "More like this", scopes that select by search). Albums, the map, places, the
  trash, shares (`share_public::VISIBLE`) and the album and share counts have the predicate in their own SQL.
- **The unlock lives on the login session** (`sessions.hidden_unlocked_until`). `POST /auth/hidden/unlock {password}` checks the
  password with argon2 (like a login, behind the same per-IP rate limiter; a wrong one is `403 wrong_password`) and sets
  now + 10 minutes on **this session's row**; `GET /auth/hidden` reads it and `DELETE /auth/hidden` closes it. The assets service reads
  the row on every request (`Unlock`): other sessions of the same user, other users and **API keys never have it** (the CLI cannot
  list or open hidden photos). The browser holds nothing that could be forged or extended; a request without the unlock is refused
  `403 hidden_locked`, and a photo's own endpoints (`GET /assets/{id}`, `/details`, `/download`, `/similar`) answer `404` for a
  hidden photo to a locked session.
- **What needs the unlock:** `GET /assets?hidden=true`, `GET /assets/timeline?hidden=true`, `scope: "hidden"` in any bulk request,
  **unhiding**, and reading a hidden photo by id. **Hiding does not** (you can hide from anywhere), nor does trashing, favoriting or
  adding to an album by id (nothing is read). A hidden photo that is trashed is in the trash view, and in "restore everything",
  only for an unlocked session.
- **Shares.** A new link refuses hidden photos (`400` when nothing is left; a mixed selection keeps the visible ones; `scope:
  "hidden"` is refused); an existing link (album or selection) stops showing a photo the moment it is hidden, and shows it again
  when it is unhidden. The owner's link list counts only what visitors can see.
- **Audit log.** Every hide and unhide writes `photos.hide` / `photos.unhide` with `{count}` and **no photo id or name**, so an
  administrator sees that it happened and how many, never what (as on the accounts page, counts only).
- **Which date memories use.** The photo's own local date: `taken_at` read in the zone the camera recorded (the EXIF `OffsetTime*`
  tag; the same rule as the details panel) or, for an edited date, in the zone the edit chose (edits overwrite `taken_at`, so they
  are honoured); with no zone known the clock reads as UTC. A photo without a capture time is never a memory. The server looks one
  UTC window (14 h before to 36 h after midnight) up per earlier year in `assets_timeline_idx`, then does the zone arithmetic on the
  few rows in the windows. The browser sends its own local date (`?date=`), so the strip follows the user's day. Excluded: hidden,
  archived, trashed, dismissed, the RAW of a pair; a burst counts once (a favorite, else its first photo); a 29 February only looks at
  leap years. Dismissing a photo of a burst dismisses the burst.
- **Recently added** is `sort=added` on the same list and day counts: `created_at` instead of the timeline time, with its own
  index. Nothing else changes (selection, "Select all", the scrubber).
- **Recovery.** Hidden and archived are in the daily manifest, one optional field each on an `asset` line (`hidden`, `archived`;
  absent in older manifests, which read as "not hidden"; an older reader skips them), and `reindex` puts them back **in the same
  transaction that recreates the photo's row**, so there is no moment at which a restored photo is visible. A manifest only knows
  what was true when it was written, and hiding is a privacy promise, so a hide, unhide or archive also writes the owner's manifest
  about 30 seconds later (one write for a burst of changes; best effort, the daily pass still runs). The strip and sort
  settings are in the manifest's `prefs` line and memory dismissals are an optional `memory_dismissed` field on the `asset` line
  (written only where set); both come back after a bucket-only rebuild, and dismissing or taking back a memory also schedules the
  manifest write. See `docs/recovery.md`.
- **Indexes** (50,000 photos, one owner, `EXPLAIN ANALYZE`): the timeline's first page and a page at 2012 are an index scan of
  `assets_timeline_idx` (0.3 ms), the same for Recently added (`assets_added_idx`, 0.1 ms), Archive and Hidden (small partial
  indexes, 0.2 to 0.3 ms), Favorites (0.2 ms), memories (20 windows, 0.9 ms). `assets_timeline_idx` is now partial on `NOT hidden`
  and carries `archived` and the three timestamps of the sort expression (`INCLUDE`), so `NOT archived` is evaluated in the index
  and the day counts can be an index-only scan (5 MB at 50,000 photos). The day counts themselves read every photo's day, about 50 ms
  at 50,000 photos before and after (the sort of 46,000 dates, not the scan, is the cost); the web app reads them once per view.

## Not done

Hiding a photo that is in the trash; per-album "hidden" counts for people other than the owner (albums are not shared); a hidden
view inside search; showing the import batch of a photo; memories for photos without a capture time; keeping the strip setting and
dismissals in the bucket.

## API (assets service, owner-scoped; unlock endpoints on the auth service)

| Request | Effect |
|---|---|
| `POST /auth/hidden/unlock {password}` | Open Hidden for this session for 10 minutes: `{unlocked, expires_at}`; `403 wrong_password`; not for API keys. |
| `GET /auth/hidden`, `DELETE /auth/hidden` | Is it open (and until when); close it now. |
| `POST /assets/hide {ids \| scope+except, hidden}` | Hide or unhide (`403 hidden_locked` to unhide while locked). `{count}`. |
| `POST /assets/archive {ids \| scope+except, archived}` | Archive or restore. A scope-based archive also returns `batch`; `{"scope": {"archive_batch": batch}}` restores exactly it. |
| `GET /assets?archived=true`, `?hidden=true`, `?sort=added` | The Archive view, the Hidden view (`403 hidden_locked`), the library by upload time (`sort_time` is then the upload time). `/assets/timeline` takes the same. Items carry `archived` and `hidden`. |
| `GET /assets/search?include_archived=false` | Leave archived photos out (the default keeps them). |
| `GET /assets/memories?date=YYYY-MM-DD&all=true` | `{enabled, day, years: [{year, years_ago, count, items}]}`; the strip form is a few photos per year and empty when the strip is off; `all` is the page. |
| `POST /assets/memories/dismiss {ids, dismissed?}` | "Not this one" (a burst goes along); `dismissed: false` takes it back. |
| `GET\|PUT /assets/prefs` | `{memories_enabled, library_sort: "taken" \| "added", stacks_collapsed}`. |
| Albums | `hidden_count` beside `photo_count`; the album's list leaves hidden photos out. |

## Tests

- `apps/assets/src/routes/visibility_tests.rs` (Postgres): the **every-endpoint test** (a photo that is hidden everywhere it could
  leak from, then 50 requests to the lists, timeline, search in both plans, facets, places, map, albums, shares, public share
  links, similar, memories, details and downloads; neither its id, its caption, its place nor its camera is in any response),
  unlock on the session (locked, unlocked, another session, an API key, another user, expiry, unhide only when open), new shares,
  pairs, the trash, select-all, audit rows with counts only, archive everywhere (and undo by batch), recently added, prefs, and
  memories (local date and zones, edits, exclusions, bursts, dismissals, off, empty, leap days). `timeline_tests.rs`: the EXPLAIN
  check for all five indexes.
- `apps/auth` (`routes.rs`): unlock with the password, wrong password, ten minutes, this session only, API keys refused, lock.
- `crates/cyanotype-core/tests/reindex.rs`: hidden and archived (and pairs) restored from the manifest, in a dry run and an apply,
  idempotently; manifests without the fields hide nothing.
- Web: `visibility.test.tsx` (words, API requests, the strip, sort, archive and hide from the bar and the viewer, undo, the
  Archive and Hidden views, the password prompt, lock and expiry); `e2e/mobile-overflow.spec.ts` ("library views") at 320 to 1280 px,
  light and dark: strip, More menu, selection actions, viewer menu, memories, recent, archive, the unlock prompt with its error,
  the Hidden view, album counts, the search filter.
# Text in photos

What is written in a photo (a sign, a receipt, a screenshot, a page) is read on the server, found by search, shown in the viewer to
copy, and QR codes in it are listed. Nothing leaves the server and no service is called: the model files are baked into the worker.

## What the user sees

- **Search** finds a photo by the words in it, like it finds one by its caption: every word of the query (three letters or more) must be in
  the text, in any case, as part of a word (`minste` finds `WESTMINSTER`), and such photos come **in front of** the model's ranking, after
  the photos whose caption has the words (captions are the owner's own words), newest first within each. Someone else's text never matches.
  Filters apply to them as to every result.
- **Filter "Text in the photo"** in the search filter sheet (`has_text=true|false`; chip "Has text"): only photos with (or without) text; the
  count of photos with text is in the sheet.
- **Viewer, "Text" button** (or `t`): the recognised text, **Copy all**, and **Show on the photo**, which draws a box on every line (and on
  QR codes) over the picture, in percentages of it so it follows any size. It replaces the Info panel while open (one panel at a time) and
  stays open while paging. A photo not read yet says so; a photo with no text says "No text found". The text is machine-read: the panel says
  to check numbers and names.
- **QR codes** are listed with what they say (web address, Wi-Fi network, contact, ...), with **Copy**. A plain `http(s)` address also has an
  **Open example.org** link, which opens only when clicked, in a new tab (`noopener noreferrer nofollow`). Nothing is ever opened for you, and
  `javascript:`, `data:` and everything else is text only. The server decides what is a link (`link` in the API) and the page checks again.

## How it works

- **Job.** An `ocr` job per photo, queued when its `embed` job stores its vector (so text comes after search indexing), at the lowest
  priority; the worker's `ocr` loop takes one only when no thumbnail, metadata, embed, pairing, geocode or takeout job is runnable, and shares the
  embed loop's one-at-a-time permit, so the two ONNX models never work at once. It reads the photo's **`preview.webp`** (at most 1440 px), never
  the original. Models load on the first job and are dropped after 5 quiet minutes (like embedding); each job holds 224 MB of the worker's byte
  budget (`WORKER_MEMORY_BUDGET_MB`) while it runs. `WORKER_OCR=0` turns it off; `WORKER_OCR_THREADS` (default 1).
- **Backfill.** The loop's sweep (at start, then every 15 minutes) queues, at backfill priority, every photo with a preview that the current
  model has not read: an existing library is read in the background, at the pace of what is left over after uploads and embedding (about 1 s a
  typical photo, up to 10 s for a page of dense text, on one thread).
- **Model switch.** `crates/cyanotype-core/src/ocr.rs` has `MODEL` and `VERSION`; `models/ocr.lock` pins the files. A new model or version
  reads the library again, like a new search model; the old text keeps answering until each photo's new text replaces it.
- **Storage.** `asset_text` (migration 0023): the text, the lines with a confidence, a quadrilateral and the words of each, the QR codes. Coordinates
  are 0..1 of the picture. A generated `simple`-configuration `tsvector` (no stemming or stop words, so any script and any mix of languages
  works), a GIN trigram index on the text for part-of-a-word matches (`ILIKE`), and `has_text`. A photo with nothing on it has a row with empty
  text so it is not read again. Rows go with their photo.
- **No manifest.** The text is derived from the preview and re-made by the sweep; it is not state to preserve, so a bucket-only rebuild
  (`cyanotype reindex`) reads the photos again like any derived data.
- **Limits.** No angle classifier: text upside down is not read. Words in scripts the model's dictionary lacks (Cyrillic, Vietnamese
  composed letters, Arabic) come out as look-alike Latin letters or nothing (measured in the bake-off). At most 150 text boxes per photo are read (the
  biggest), so a screenshot of a spreadsheet is read in part. QR codes: found on a clear sticker, a sign or a screen; not on a crumpled receipt or a
  distant, small or damaged one (see the bake-off).

## API (assets service, owner-scoped)

- `GET /assets/{id}/text` -> `{status: "read"|"pending", text, lines: [{t, s, q: [x0,y0,...,x3,y3], w: [{t, q}]}], qr: [{t, q, link?}]}`.
  `pending`: not read yet. 404 for a photo that is not yours or is in the trash. A `read` API key (or a session) may call it.
- `GET /assets/search?...&has_text=true|false`, and `with_text` in `GET /assets/search/facets`.
- `GET /assets/processing` has `text: {read, total, queued, running, failed, model, percent}`, reported apart from `active` (a backfill takes hours
  and is not "processing"), and `ocr` jobs are left out of `by_kind` unless they failed.

## Tests

- `cyanotype-ocr`: geometry (components, hull, rotated rectangle), detection boxes from a probability map, CTC decoding and word spans, batching, the
  dictionary parser, QR reading on synthetic codes at several sizes, turned, in a noisy picture and two in one picture, and which payloads are links.
- The assets service (`text_tests.rs`): the route (pending, read, links only for web addresses, other owners, trash), search order (captions, then
  text, once each), filters, `has_text` and its facet, the sweep and a model switch, the processing report, and `simple`/trigram matching of Cyrillic,
  Vietnamese and parts of numbers. The worker (`ocr.rs`): the stored shape, and a real preview read with the real model when `OCR_TEST_IMAGE` and the
  model files are there.
- Web: `textText.test.ts`, `TextPanel.test.tsx` (copy, QR links, never auto-opened, empty and pending states, boxes), and `e2e/text-layout.spec.ts`
  (the panel, boxes and QR list at 320 to 1280 px, light and dark).

# Star ratings

Every photo can have 0 to 5 stars (migration 0024). A rating is the owner's own mark, set from the viewer, the info panel or in bulk, a
search filter and a facet, and it can be read from the photo's own metadata as a default.

## What the user sees

- **In the viewer:** five stars under the photo, and the keys **0 to 5** (a bare digit; with Ctrl, Alt, Cmd or Shift they are not
  shortcuts). Tapping the star the photo already has clears the rating. The **Info** panel has a *Rating* row with the same stars.
  On a phone the stars are 44 px targets.
- **In bulk:** *Rate* in the selection bar of the library, search results and an album opens a small dialog (5 stars down to *Clear
  rating*); it works on "Select all" (the whole view) like the other bulk actions, and ends in a toast ("12 photos rated 4 stars").
- **Search:** *Rating* in the filter sheet (*4 stars & up*, with how many photos have that many or more), a chip *4 stars & up* under the
  search bar, `rating=` in the address. It combines with every other filter and with a text query. The same filter is in "More like
  this" and in `scope: {"search": {...}}`, so *Select all* over a rating filter selects exactly what it lists.
- A photo from before ratings, or one whose file says nothing, is *Not rated*: 0, the same as a cleared rating.

## How it works

- **Columns.** `assets.rating smallint NOT NULL DEFAULT 0 CHECK (0..5)` and `assets.rating_source` (`NULL` = nobody, `file`, `user`). A
  RAW + JPEG pair carries the rating on its visible photo, as the favorite flag does; the id of the hidden RAW means its primary, and
  pairing hands a RAW's stars to the visible photo when they are higher and the owner has not rated the visible photo.
- **Who set it.** `POST /assets/rating` always writes `rating_source = 'user'` (a deliberate 0 included: it keeps a photo whose file says
  4 stars cleared). The worker's `metadata` job reads the file's rating (below) and writes `rating` and `rating_source = 'file'` **only
  while the source is not `user`**: a re-run, a re-import or a rebuild never overrides the owner. Clearing a rating on an unrated photo
  is not a change and does not mark it as the owner's.
- **The file's rating** (`apps/worker/src/embedded.rs`): `xmp:Rating` (attribute or element; `xap:` too) from the XMP packet, found by
  its APP1 segment in a JPEG and by the text `<x:xmpmeta` in the first 4 MiB of any other container (HEIC, PNG, WebP, TIFF-based RAW);
  else the EXIF `Rating` tag (0x4746) or `RatingPercent` (0x4749: 1, 25, 50, 75, 99). `-1` ("rejected") and anything that is not 0 to 5 is
  no rating; XMP wins over EXIF. There is no XML parser: a packet the scanner does not understand is "no rating", never a failed job.
- **Search.** `rating=N` is `a.rating >= N` in `Filters::push_where`, so it applies in every ranked plan, the timeline-order list,
  counts, "More like this" and the scopes. Hidden photos are never in it (the clause of the base). **Facets** (`GET /assets/search/facets`)
  gain `ratings`: "at least N stars" counts for N = 5 to 1, over the visible library only.
- **Hidden photos.** Rating a hidden photo by id (or `scope: "hidden"`) needs the unlocked Hidden view; a locked session's request for
  a hidden id changes nothing and answers `count: 0`.
- **Recovery.** The manifest's `asset` line gains `rating` (1 to 5, or 0 for a rating cleared on purpose) and `rating_source`; both are
  absent when they say nothing and older readers skip them. `reindex` writes them in the transaction that recreates the photo's row
  (`ratings_restored` in its report). The manifest is rewritten about 30 s after a rating changes (the hide/archive debounce), so a
  bucket-only rebuild loses at most those seconds. A `file` rating is read from the file again by the metadata job in any case.
- **Index** (50,000 photos, one owner, 15 % of them rated, `EXPLAIN ANALYZE`): `assets_rating_idx` is partial (`rating > 0`, live, not a
  RAW half, not hidden) in timeline order and `INCLUDE (rating)`. The first page of "4 stars and up, newest first" is an index scan
  that stops after 61 rows (0.2 ms), the total (7,900 rated photos) a bitmap scan (5 ms), "5 stars and a camera" 17 ms through the
  camera index. The facet counts add five `FILTER` aggregates to the facets' existing full scan of the owner's photos (about 40 ms at
  50,000, as before). No row is rewritten and no job is queued by the migration (constant column defaults).

## API (assets service, owner-scoped)

| Request | Effect |
|---|---|
| `POST /assets/rating {ids \| scope+except, rating}` | 0 to 5 (`400` otherwise). `{count}` of photos that changed; foreign, unknown and trashed ids are ignored. |
| `GET /assets/search?rating=N`, `/assets/{id}/similar?rating=N` | At least N stars (1 to 5; 0 is no filter; anything else `400`). |
| `GET /assets/search/facets` | `ratings: [{value: "5", count}, ...]`, "at least" counts. |
| any list, search result, `GET /assets/{id}` | Items carry `rating`. |

## Tests

- `apps/assets/src/routes/ratings_tests.rs` (Postgres): ids and scopes, idempotence, owner isolation, trashed photos, a RAW id, the
  file-to-owner switch, the filter (≥ N, combined, as a scope, invalid), the facet; `visibility_tests.rs`: a hidden photo's five stars
  are in no filter or facet and it is rated only from an unlocked session.
- `apps/worker`: XMP and EXIF rating parsing (attribute, element, other containers, `-1`, garbage, truncated files) and the metadata
  job (file default, never over the owner's, a cleared rating stays cleared) against Postgres and S3.
- `crates/cyanotype-core/tests/reindex.rs`: ratings and their source come back with a rebuilt database (dry run and apply).
- Web: `ratingText.test.ts`, `components/Rating.test.tsx` (stars, keys, the dialog), filter sheet and URL; `e2e/ratings-layout.spec.ts`
  (Playwright, 320 to 414 px, 1280 px, 360 px dark).

# Tags

Global per-user labels on photos (migration 0025): autocomplete, rename, merge, delete, bulk add and remove, a Tags view and a tag
page, a search filter, tag words in text search, and keywords imported from the files.

## What the user sees

- **Tagging.** *Tag* in the selection bar of the library, search results and albums (also over a whole view) opens a dialog with *Add
  tags* and *Remove tags*: chips plus a field with autocomplete (Enter or a comma adds a name, Backspace takes the last chip back). In
  the viewer's Info panel a *Tags* row shows the photo's tags (a dashed outline marks one that was **imported** from the file), an *x*
  on each removes it, *Add tag* opens the same field.
- **Tags view** (*More*, *Tags*): every tag with its photo count, A to Z or most used, with a filter box. A tag opens its **tag page**: the
  photos (select, rate, tag, *Remove this tag*), *Rename*, *Merge into...* (another tag; every photo gets that one and this one goes),
  *Delete tag* (taken off its photos, the photos stay). Renaming to a name another tag has says so and points at Merge.
- **Search.** *Tags* in the filter sheet (existing tags only, with autocomplete) and, with two or more, *All of them* or *Any of them*. A chip
  (*Tags: beach and sunset*), `tags=beach,sunset&tag_mode=any` in the address. Text search also finds a photo by the words of its tags:
  they lead the results exactly as caption words do (every word must be in the caption or in a tag).
- Names are flat: letters, digits, spaces, symbols, and `/` (so `trip/2024` is one name; nothing treats it as a hierarchy yet). No commas
  (the separator), at most 100 characters, no control characters. Two names are the same tag ignoring case; the first spelling is kept.
- **Imported tags.** IPTC *Keywords* and XMP `dc:subject` of a file become tags marked imported, once per photo: removing one stays removed
  when the metadata job runs again, and adding the same tag yourself makes it yours.

## How it works

- **Tables.** `tags (id, owner_id, name, created_at)` with a unique index on `(owner_id, lower(name))`; `asset_tags (asset_id, tag_id,
  imported, added_at)`, both sides cascading (a purged photo, a deleted tag or account takes its links); `assets.tags_imported_at`. The
  visible photo of a RAW + JPEG pair carries the tags: pairing moves a RAW's onto it, the id of the hidden RAW means its primary.
- **Keywords** (`apps/worker/src/embedded.rs`): IPTC record 2 dataset 25 from the Photoshop segment (UTF-8, else Latin-1) of a JPEG, and the
  entries of `<dc:subject>` in the XMP packet (any container, as for ratings), at most 200 per file, 50 imported. `tags::import_keywords`
  runs in the metadata job under a row lock and sets `tags_imported_at`, so a photo's keywords are read once. A keyword is cleaned into a
  name (commas and control characters become spaces, cut at 100 characters).
- **Filter.** Per tag, `a.id IN (SELECT asset_id FROM asset_tags JOIN tags ... lower(name) = $tag)` (all), one such subquery with
  `= ANY(...)` (any). The join form lets the planner see how many photos the tag has: with the tag id in an `InitPlan` instead, a rare
  tag walked the whole timeline index (570 ms at 50,000 photos).
- **Text search.** `caption_matches` builds, per query word, a materialized set (photos whose caption has it, by `assets_caption_idx`, union
  photos with a tag whose name has it); the first word's set drives a primary-key lookup per candidate and the others filter it.
  Order of what leads the model's ranking: captions and tags (the owner's words), then text read off the picture ("Text in photos"), sharing
  one budget of 50. The has-text and tag filters combine.
- **Hidden photos.** Tagging a hidden photo by id (or `scope: "hidden"`) needs the unlocked Hidden view. A locked session's tag list leaves out
  a tag that only hidden photos have and counts only visible photos; with the view unlocked, hidden photos count. The search filter, the
  caption/tag text matches and the tag lists of `GET /assets/{id}` all go through the base's hidden rules; the every-endpoint test has a tag
  only the hidden photo carries.
- **Audit.** Merge and delete write `tags.merge` / `tags.delete` with a photo count and nothing else (no names).
- **Recovery.** The manifest gains `tag` lines (format 3): id, name, created time, photos by hash (`items` the owner's, `imported` the file's),
  one line per tag so an empty tag survives. Older readers skip them. `reindex` recreates a tag the owner lacks (same id when free) with all its
  photos, and for a tag that exists only tags the photos *it recreated* (so a tag removed since the manifest is not given back); a photo restored
  from a manifest that knows tags gets `tags_imported_at`, so its keywords are not read again. Only the newest manifest's tag lines count (as share
  links do), so a deleted or merged tag does not return from an older file. The manifest is rewritten about 30 s after a change.
- **Indexes** (50,000 photos, 172,000 links, `EXPLAIN ANALYZE`): a rare tag (40 photos) 0.6 ms; a tag on 40 % of the library 74 ms; all of two tags 15 ms;
  any of two 19 ms; with a camera and a rating filter 73 ms (the tag's 20,000 photos are the driver). The tag list with counts (300 tags) 177 ms,
  autocomplete 0.3 ms. Text search leads: a word few photos have 8 ms, a word in the captions of 4 % of the photos 20 ms, a word in nearly every
  tag name about 190 ms (that is every photo; the cost is the count of matches).

## API (assets service, owner-scoped)

| Request | Effect |
|---|---|
| `GET /assets/tags?q=&sort=name\|count&limit=` | `{items: [{id, name, count}]}`; `q` finds names containing it (autocomplete). |
| `POST /assets/tags {name}` | Make a tag, or return the existing one: `{id, name, created}`. |
| `PATCH /assets/tags/{id} {name}` | Rename; `409 tag_exists` with `existing_id` for a name another tag has. |
| `POST /assets/tags/{id}/merge {into}` | Move every photo to `into` and delete the tag: `{into, photos}`. Audited. |
| `DELETE /assets/tags/{id}` | Delete; `204`. Audited. |
| `POST /assets/tags/apply {ids \| scope+except, add, remove}` | At most 20 names each; `{added, removed}` links changed. |
| `GET /assets/search?tags=a,b&tag_mode=all\|any`, `/assets/{id}/similar` | The tag filter (names containing no comma; at most 20). |
| `GET /assets/{id}` | `tags: [{id, name, imported}]`. |

People (faces grouped into named people, behind `FACES_ENABLED`, off by default) has its own routes under `/assets/people` and `/assets/faces`: see `docs/faces.md`, "People".

## Tests

- `apps/assets/src/routes/tags_tests.rs` (Postgres): tags made once whatever the case, bulk apply (ids, scope, pairs, owners, trash), imported
  taken over, list/count/sort/autocomplete (with LIKE characters), rename/merge/delete with owners and audit rows, the filter (all, any, unknown,
  combined, as a scope), tag words in text search, the indexes; `visibility_tests.rs`: a tag only a hidden photo has leaks nowhere and a hidden
  photo is tagged only from an unlocked session.
- `crates/cyanotype-core/tests/tags.rs` (once-per-photo import, pairs, caps, cascades) and `reindex.rs` (tags restored with their ids, imported
  marks, empty tags, never given back to photos that were not recreated); `apps/worker`: IPTC and XMP keyword parsing and the metadata job.
- Web: `tagText.test.ts`, `Tags.test.tsx` (the field, the dialog, the Tags view), filter sheet and URL; `e2e/tags-layout.spec.ts` (Playwright).

# Smart albums

An album defined by saved rules instead of members (migration 0026). It is **live**: nothing is stored about which photos are in it, so
a photo that starts to match appears and one that stops disappears, in the album and in its share link.

## What the user sees

- *Albums*, *New smart album*: a title, **match all / any** over a list of rules, *Include archived photos*, and a **live preview** (how many photos
  match right now and eight of them). Rules: *Date range*, *Place*, *Camera*, *Lens*, *File type*, *Favorites*, *Rating* (N stars and up), *Tags*
  (with all / any), *Location* (has or has none), *Text in the photo* (has or has none), *Caption contains*, and with People on *People* (all of the chosen
  people are in the photo, `docs/faces.md`). Place, camera and lens suggest what the library has.
- **A text query is not a rule**, and the editor says why: a semantic search such as "dog on the beach" ranks by a model, so the album's contents
  would change without a photo changing. The caption rule (a plain substring) and the tag rule are stable.
- In *Albums* a smart album has a **Smart** badge, its rules in words and its live count (`10,000+` for a big one). Its page says *Smart*, shows the
  rules and has **Edit rules** in place of *Add photos*; selecting photos there offers Rate, Tag, Share and the rest, but no *Set as cover* or
  *Remove from album* (the cover is the newest match). A smart album is not offered in *Add to album* or as the album filter of a search.
- **Sharing** works like any album link: the visitor sees what the rules match when they look (the newest 10,000), never a hidden photo, and the
  owner's link list counts the same.

## How it works

- **Storage.** `albums.rules jsonb` (NULL for an ordinary album): `{"match": "all"|"any", "archived": "include"|"exclude", "rules": [...]}`, validated
  and normalised by `routes/smart.rs` (at most 20 rules, strings trimmed and at most 200 characters, dates real, tags 1 to 20 valid names). `archived` is
  a switch for the whole album, not a rule, so "exclude" never becomes "or not archived". A smart album has no `album_assets` rows.
- **Same SQL as search.** Each rule becomes a `search::Filters` holding only that rule; the album is `Filters::smart`, whose conditions are the rules'
  `push_conditions` (the very code of the search filters) joined by `AND` or `OR`, after the usual ownership and visibility terms of `push_where`
  (live, not a RAW half, **not hidden**). So everything that follows search follows here: hidden photos are never in a smart album, its cover or counts,
  its link, or a bulk scope.
- **Listing.** `GET /assets/albums/{id}/assets` of a smart album is the search's timeline-order list (the same cursor and `ListResponse`).
  `GET /assets/albums` and `{id}` give `smart: true`, `rules`, a live `photo_count` (counted up to 10,000, `photo_count_capped` beyond), the oldest and the
  newest photo's time (two timeline-index reads) and the newest photo as the cover. `hidden_count` is 0.
- **Scopes.** `{"album": id}` in a bulk request (select all, rate, tag, share, trash...) resolves a smart album's rules: the photos it matches now.
  Adding or removing photos of a smart album by hand, and choosing a cover, are `400`.
- **Share links.** `share_public::ShareRow::source` resolves the rules when the link is viewed into an array of ids (the newest 10,000), joined to `assets`
  with the usual `VISIBLE` predicate. The owner's list counts through the same filters.
- **Recovery.** The album's manifest line gains `rules` (absent for ordinary albums and in older manifests). `reindex` recreates a missing smart album with
  its rules (`smart albums (saved rules)` in the report) and its share link; there are no members to restore. The manifest is rewritten about 30 s after a
  smart album is made or changed.
- **Indexes** (50,000 photos, `EXPLAIN ANALYZE`): a page of an `AND` of favorite and a rating walks `assets_favorites_idx` (26 ms); an `OR` of favorite,
  rating and a tag walks the timeline index and stops after 61 rows (9 ms); the album card's count is capped at 10,001 rows (26 ms for a rule matching
  20,000 photos, where counting them all took 411 ms); the oldest and newest times are one-row index reads (3 ms each). An `OR` whose branches are all
  indexed becomes a `BitmapOr` of the rating, favorites and camera indexes.
- **Preview.** `POST /assets/albums/preview {rules}` validates, counts (capped) and lists eight photos; nothing is saved.

## API (assets service, owner-scoped)

| Request | Effect |
|---|---|
| `POST /assets/albums {title, rules}` | A smart album (`400` with `asset_ids`, or with a text query as a rule: "its ranking is not stable"). |
| `PATCH /assets/albums/{id} {rules, title...}` | Edit the rules of a smart album (`400` for an ordinary album; no cover). |
| `POST /assets/albums/preview {rules}` | `{count, capped, items, rules}`. |
| `GET /assets/albums`, `/assets/albums/{id}` | `smart`, `rules`, `photo_count`, `photo_count_capped`. |
| `POST /assets/albums/{id}/assets[/remove]` | `400` for a smart album. |

## Tests

- `smart.rs` (unit): normalisation, round trip, the text-query refusal, every kind of nonsense. `smart_tests.rs` (Postgres): every rule against the search filter's
  own results, match all / any and the archived switch, live membership (a photo starts to match, one stops, one is trashed, one hidden, a stranger's),
  the cover and counts and paging, editing and everything a smart album refuses, bulk scopes, the preview, a share link that follows the rules and never
  shows a hidden photo, owner isolation, and the plan; `visibility_tests.rs`: a smart album that matches the hidden photo lists, counts, covers and links
  without it; `key_scope_tests.rs`: the new route.
- `crates/cyanotype-core/tests/reindex.rs`: a smart album's rules and link come back, with no members.
- Web: `smartText.test.ts`, `SmartAlbumEditor.test.tsx` (the editor, the Albums list, the album page); `e2e/smart-layout.spec.ts` (Playwright).

# Near-duplicates

Photos that are the same picture twice (a resized or re-saved copy, an export, a shot repeated moments later) are found in the background and
offered for review: side by side, the best one preselected, the others to the trash after what they carry has been merged onto the one kept
(migration 0027). Exact copies cannot exist (the content hash is unique per owner); this is for re-encodes and near-identical shots.

## What the user sees

- *More*, *Duplicates*: **Find duplicates** (and, for an admin, *Check every account*), a line with the state of the check ("Last checked 3 min ago
  (12,345 photos): 2 groups found."), then the groups. Each photo of a group is a card with its thumbnail (click: look at it full size and
  step through the group), **Keep this one** (the suggested photo is preselected and marked *Suggested*), its **resolution**, **size**
  (with how it compares to the largest), **date**, and how much **metadata** it carries (EXIF fields, location, caption, tags), a heart and the stars.
- **Keep selected, trash N others**: the others go to the trash, the toast says what was merged and has **Undo**. **Not duplicates** dismisses
  the group **permanently**.
- A check also runs by itself a few minutes after the last upload of an import has been embedded; the page follows a running check.

## How it works

- **Finding** (`cyanotype_core::duplicates`, run by the worker's `dup.rs` loop): for each live photo of the owner that has an image vector (not
  the hidden RAW half of a pair, never a trashed one) the HNSW index gives its 12 nearest neighbours, and a pair is a duplicate when the cosine
  reaches a threshold that depends on what else the two share: **the same camera and capture times within 60 s** (0.85), **the same pixel
  dimensions** (0.92), or **neither has a capture time** (a copy that lost its EXIF: 0.94); anything else is not a duplicate. (SigLIP 2; CLIP's are one
  point lower. Calibration, precision and recall: `docs/build.md`, "Near-duplicate threshold".) A pair from **one burst** is never a duplicate (bursts
  have their own grouping), nor a pair the owner dismissed. Pairs join into groups by connected components; a group of more than 50 photos is left out and
  counted (`too_big`). The groups of a scan replace the owner's previous ones; they are derived data (nothing to back up: the next check finds them).
- **Requests** (`duplicate_scans`): *Find duplicates* is a manual request (runs at once); an embedded photo makes an automatic one that waits until
  the owner's uploads have been quiet for 5 minutes (each new embedding moves it back), so one check follows a whole import. A manual request is never
  downgraded by an automatic one, a request made while a check runs stays wanted, a check that died is retried after 10 minutes, a failed one is
  recorded and not retried in a loop. Admins can ask for every account (`POST /assets/admin/duplicates/scan`, audited with a count).
- **Best photo**: most pixels, then most metadata (EXIF fields, plus a place, a capture time, a caption and one per tag), then a favorite, then the
  larger file, then the lowest id.
- **Keeping one** (`POST /assets/duplicates/{id}/keep {keep}`): merged onto the kept photo, additively (nothing it has is taken away): the **favorite**
  flag, the **highest rating** (as the owner's), **albums** (and a cover that was one of the others), **tags** (the owner's mark wins over an imported
  one), **captions** (every distinct one, the kept photo's first, separated by a blank line, cut at 2,000 characters), a **location** and a **capture
  time** only if the kept photo has none. Captions, place and time are written as the photo's **edits** (the object in the bucket first, as every edit is),
  so a rebuild keeps them. Then the others go to the **trash** (a RAW behind a copy goes with it), where **Undo** (or the trash page) restores them: **the
  merge stays after an undo**, on purpose: it only adds to the kept photo, and undoing it would need a copy of what was there. Audited as
  `duplicates.keep` with counts only (no ids, no names). The group is dropped.
- **Hidden photos.** Hidden photos are grouped like any other, but a session that has not unlocked the Hidden view neither sees them in a group nor a
  group with fewer than two visible photos, and *Keep* acts on the visible members only: a locked session never trashes or merges a hidden photo, and
  nothing of a hidden photo (caption, tag, place) is copied onto a visible one. API keys never have the unlock. Another owner's group is a 404.
- **Exclusions**: bursts, hidden photos (unless unlocked), trashed photos, the RAW half of a pair, other users' photos (the index is shared; a
  neighbour that is not the owner's is skipped).
- **Not duplicates** writes every pair of the group to `duplicate_dismissals` (`a < b`), which a check honours and which goes with the photos when
  they are purged. Dismissals are not in the recovery manifest (below).
- **Recovery.** Groups and dismissals are database-only: after a bucket-only rebuild the groups are found again by the first check (an automatic one
  follows the re-embedding), and **dismissed groups come back for review**. The merges that "Keep" made (captions, place and time) are edits in
  the bucket, tags/albums/ratings/favorites are in the manifests, and the trashed photos are in the trash state the manifest carries.
- **Measured** (50,000 photos for one owner plus 10,000 for another, 768-dimension random vectors, so the HNSW graph is as hard as it gets): the
  candidate list 90 ms; 500 photos' twelve neighbours each 1.1 s through `asset_embeddings_siglip2_b16_hnsw`, so a whole scan is about two minutes
  of background database work (the worker holds one connection and nothing else; searches keep their latency); listing groups is a small indexed
  query over `duplicate_groups` and the members' own primary keys.

## API (assets service, owner-scoped)

| Request | Effect |
|---|---|
| `GET /assets/duplicates?limit=&cursor=` | `{groups: [{id, members: [...]}], total, next_cursor, scan}`; each member is a list item plus `exif_fields`, `has_location`, `has_caption`, `tag_count`, `best`. |
| `POST /assets/duplicates/scan` | Check this library now. `{scan: {state: wanted\|running\|idle\|never, ...}}`. |
| `POST /assets/admin/duplicates/scan` | Admins: every account with photos. `{accounts}`. |
| `POST /assets/duplicates/{id}/keep {keep}` | Merge and trash. `{kept, trashed: [ids], merged: {...counts}}`. `400` when `keep` is not a visible member or there is nothing to merge. |
| `POST /assets/duplicates/{id}/dismiss` | "Not duplicates", permanent. `{dismissed}`. |

## Tests

- `crates/cyanotype-core/src/duplicates.rs` (unit): the three rules and what none of them admits, bursts, components. `apps/assets/src/routes/duplicates.rs`
  (unit): the best photo and the merged caption. `duplicates_tests.rs` (Postgres, S3 for the edits): the scan on stored vectors (each rule, the
  exclusions, a rescan, a trashed member), chains and a group that is too big, dismissals (permanent, audited, owner only, cascade), the review
  (resolution, size, metadata, best, paging), keeping one (favorite, rating, albums, cover, tags, captions, location, the edits object, trash and
  the RAW, audit, Undo), what is never overwritten, the request state machine (manual, automatic and quiet, a request during a scan, the admin button),
  and the plan of the neighbour lookup. `visibility_tests.rs`: a group with a hidden member (locked and unlocked lists, a locked keep never trashes or
  merges it); `key_scope_tests.rs`: the routes.
- `cyanotype-clip` `duplicate_bakeoff` (ignored, `just bakeoff`-style): the calibration.
- Web: `dupText` and `Duplicates.test.tsx` (the page, keep and Undo, dismiss, a check with polling, the admin button, paging); `e2e/duplicates-layout.spec.ts`
  (Playwright).

# Rotate and crop

Turn a photo, straighten it and crop it **without touching the file**: the original stays byte for byte what was uploaded, and
the thumbnail, the preview, the search embedding and the "edited" JPEG are made from the original plus a small *recipe*
(migration 0028). It is one more kind of edit (`docs/library.md`, "Editing a photo"): it lives in the same `edits/` object, is
restored by `reindex`, and "Revert to original" puts the picture back.

## What the user sees

- **In the viewer**: the **Crop** icon next to the heart (or the key `e`; `r` opens it already turned a quarter, `c` opens it with
  the crop frame), and a **Picture** row in the Info panel that describes the recipe ("Rotated 90° right, cropped to 1800 × 2000")
  with the same button.
- **The editor** is a full-screen dialog showing the whole upright picture, turned and zoomed the way the worker will render it:
  - **Left / Right** (`r`, Shift+`R`) turn a quarter; the crop turns with the picture.
  - **Straighten**: a slider from -45 to 45 degrees in tenths. The picture is magnified just enough that no empty corner shows.
  - **Crop** (`c`) shows the frame with eight handles (44 px touch targets, `touch-action: none` so a finger drags the frame instead of
    scrolling) and a thirds grid. Drag a handle or the inside, or use the arrow keys on a focused handle (Shift: five times the step).
    The shape chips (Free, Original, 1:1, 4:3, 3:4, 3:2, 2:3, 16:9, 9:16) lock the ratio in pixels, and a locked ratio turns with the picture.
  - **Save** (Enter, when no button has the focus), **Cancel** (Esc), **Reset** (back to what was saved), **Revert to original**
    (shown while the photo has a recipe; saves at once).
- After a save the viewer keeps the old picture under *Updating the picture…* until the worker has rendered the new one (the page
  asks every 1.5 s, at most 40 times), then shows it and tells the page so its tile is refreshed.
- A photo with a recipe says *edited* beside its size, and its tile and the grid use the edited picture's shape.
- **Download**: with a recipe the button opens a small menu, **Download original** (the file, byte for byte) and **Download edited
  (JPEG)**. The edited JPEG is made on demand, so the button says *Preparing the edited photo…* until the worker has written it.
- **Share links** that offer downloads (and not the originals) give the visitor the edited JPEG for an edited photo, whatever the
  original's format (a rotated HEIC is downloadable that way); a link that includes the originals still serves the original files.

## How it works

- **The recipe** (`cyanotype_core::recipe`): `rotate` (0, 90, 180, 270 clockwise), `straighten` (degrees, positive turns the picture
  clockwise) and `crop` (`x`, `y`, `w`, `h` as fractions of the frame, from the top left). The steps, in this order, on the picture
  *as displayed* (the EXIF orientation already applied, which is what `width` / `height` always meant): **turn**, **straighten**
  (same frame size, magnified by `cos a + sin a * max(w/h, h/w)`, the least that covers every corner), **crop** (to the frame those two
  made). A recipe is normalised (a multiple of 90 reduced, the angle to a tenth, a crop clamped and rounded to 4 decimals, a crop
  smaller than 2 % of a side refused) and one that changes nothing is none. A quarter turn and a small tilt commute, so a *relative*
  turn (`{"turn": 90}`, the bulk way) only adds 90 degrees and turns the crop rectangle: `(x, y, w, h)` becomes `(1 - y - h, x, h, w)`.
- **Storage.** `assets.recipe` (jsonb) holds the effective recipe; the bucket's `edits/{user}/{xx}/{hash}.json` gets it first
  (`"recipe": {...}`, an additive field of layout 1: a server that predates it ignores it). `edits::apply` writes the column and the
  sizes, clears the edited copy, and queues the re-render, in one transaction. `reindex` applies it like any other edit, so the restored
  photo is rendered again. A photo whose recipe was reverted keeps an object with no recipe (the tombstone), as before.
- **Sizes.** `width` / `height` become the edited picture's size, so the grid's tile shapes, the viewer's "400 x 300" and the
  review of duplicates follow what the photo looks like; the file's own size is kept in `width_orig` / `height_orig` (the details'
  `file` block and the editor use it). The metadata job writes the file's size to `width_orig` while a recipe exists and fixes
  `width` / `height` up afterwards (`edits::refresh_dims`).
- **The render** (`apps/worker`, `imaging::apply_recipe`, `media::decode_upright`): the thumbnail job reads the recipe, decodes the
  original **at the size the crop needs** (a crop of a quarter of the width is decoded at four times the preview's size, up to 8,192 px;
  JPEG through the DCT-scaled decoder, HEIF and PNG at their reduced decoders, RAW from its preview or development), applies the
  recipe, and makes the thumbnail, the 1,440 px preview and the ThumbHash from that. The memory budget decides the size: the job tries
  the wanted edge, then half of it, down to the plain preview edge, and takes the first whose estimate (the decode plus 8 bytes a
  pixel for the copies the turn, the straighten and the crop make) fits `WORKER_MEMORY_BUDGET_MB`. It also writes
  `derived/{user}/{asset}/uncropped.webp`: the whole upright picture at preview size, because `preview.webp` is already cropped and the
  editor needs the rest (deleted when the recipe is reverted; counted in `derived_bytes`).
- **A re-render is its own job.** The upload's `thumbnail` job is version 1; a recipe change queues version 2 (`jobs::requeue`
  puts a finished job back in the queue). Only version 2 queues the **embedding** again (so search and "similar photos" describe the
  crop), forgets the **text** read off the old picture and reads it again (`ocr::forget_and_requeue`: the boxes were in the old
  picture's coordinates), and removes `uncropped.webp` when there is no recipe any more. A render whose recipe is no longer the one
  on the row (an edit landed meanwhile) is not published; the job fails and runs again against the new recipe, so the newer edit
  is never lost and the older one never shown.
- **The edited copy** (`KIND_EDITED_COPY`, `derived/{user}/{asset}/edited.jpg`): a JPEG at most 4,096 px on the long edge, quality
  90, **no metadata** (no EXIF, no GPS, no camera serial, no embedded thumbnail; the colour profile is not carried either, like
  the thumbnails, so it is the picture as the viewer shows it). It is written on demand: `GET /assets/{id}/download/edited` queues it
  and answers `202 {"pending": true}` until it exists, then `307` to a presigned URL (60 s, saved as `IMG_0042-edited.jpg`); a share
  link's `prepare` queues it too. Every new recipe clears `edited_copy_at`, so a stale copy is never served, and the bytes are in the
  usage counters (`derived_bytes`).
- **Originals** are never read for writing. `GET /assets/{id}/download` is unchanged (the 307 to the file under `originals/`), and
  so is a share link that includes originals.
- **Hidden photos.** Setting or reverting a recipe, the edited download and the details of a hidden photo need the unlocked Hidden
  view, like everything else about it: by id a locked session finds nothing (`missing`), by scope it is refused. (This also closes a
  gap in the older edits: a locked session could change the date or caption of a hidden photo by its id; the edit endpoint now has
  the `NOT hidden OR unlocked` rule for every kind of edit.) API keys never have the unlock.
- **What a copy-and-merge does not do.** "Keep selected" in the duplicate review merges albums, ratings, tags, captions, place and time
  onto the kept photo but **not a recipe**: the copies differ in size and framing, and applying one photo's crop to another is
  more often wrong than right. The kept photo keeps its own recipe; a trashed copy keeps its own and comes back with it.
- **Limits, stated.** A crop of a RAW is rendered from the embedded preview (or the developed image), not at full sensor size; a
  crop of an interlaced PNG or a very large HEIC is limited by the memory budget (the edge is halved until it fits, and the
  result is a smaller render, never a failure unless even the preview does not fit). The edited JPEG has at most 4,096 px on the
  long edge even when the original is larger (the full-resolution edited file is not offered). The straighten is bilinear.
  Videos and Live Photo clips are not touched (the video part of a motion photo stays as uploaded).

## API (assets service, owner-scoped)

| Request | Effect |
|---|---|
| `POST /assets/edits {ids \| scope, recipe}` | `recipe` is `{"rotate": 90, "straighten": -1.5, "crop": {"x","y","w","h"}}` (every part optional; one that changes nothing reverts), `{"turn": 90}` (relative, any multiple of 90) or `{"revert": true}`. Same result shape and rules as the other edits (`updated`, `unchanged`, `missing`, `failed`); a bad recipe is a `400` before anything is written (`422` where the body does not have the shape). |
| `GET /assets/{id}` | adds `recipe`, `rendering` (a thumbnail job is queued or running), `edit_base_url` (the whole upright picture, once rendered) and `file_width` / `file_height`. |
| `GET /assets/{id}/details` | `edits.recipe`, `edits.edited_copy_ready`; `file.width` / `height` are the file's own. |
| `GET /assets/{id}/download/edited` | `202 {"pending": true}` and the job queued, or `307` to the edited JPEG; `400` for a photo with no recipe, `404` for anything that is not the caller's visible, live photo. |
| list items | `edited: true` for a photo with a recipe; `width` / `height` are the edited picture's. |

## Tests

- `recipe.rs` (unit): normalisation and refusals, the output size, the straighten zoom (checked against the corner condition), the
  crop fraction. `edits.rs`: the object with a recipe round-trips, an old object has none, a no-op recipe is a tombstone.
- `imaging.rs` (unit): quarter turns are clockwise (pixels), the crop is cut from the turned frame, a straightened picture shows no
  edge of the original (a black 1 px frame never reaches the corners from -12.5 to 45 degrees) and really leans the right way,
  edited renders have the edited size and carry the whole upright picture for the editor, the export is a bare JPEG within 4,096
  px (transparency goes onto white), a larger decode edge costs more.
- `handlers.rs` (Postgres + S3): metadata keeps the file's size next to the edited one; the thumbnail job renders the crop
  (320 x 240 thumbnail, 1,440 x 1,080 preview from a 3,000 x 2,000 file), writes the editor's picture, queues the embedding and the
  text again; the edited-copy job writes a bare JPEG of the right size; a render of a stale recipe is refused; reverting renders
  the plain picture and removes the editor's picture; an edited copy of an unedited photo is skipped.
- `rotate_tests.rs` (Postgres + S3): the stored recipe, the effective and the file's size, the queued render, the object in
  the bucket, relative turns (and four are the identity, with the crop back where it was), refusals, bulk with other accounts' and
  trashed photos counted missing, `reindex` restoring a recipe and queueing the render, the details and record, and the edited
  download (400, 202 once however often asked, 307 with the right name, the original download untouched, 404s).
  `share_tests.rs`: an edited HEIC and JPEG are pending until the edited copy exists, `prepare` queues it (not the stripped
  original) once, the download is the edited render, originals stay originals. `visibility_tests.rs`: a hidden photo's recipe,
  edited download and details for a locked and an unlocked session; `key_scope_tests.rs`: the route.
- Web: `recipeText` (the geometry shared with the server, dragging with and without a ratio, turning a crop), `RotateCrop`
  (keys, handles by pointer and keyboard, straighten, ratios, save, failure, reset, revert, focus), `LightboxPicture` (opening, the
  keys, saving and waiting for the render, an edited photo's own picture, the edited download), `api` (the redirect that is not
  followed, the request bodies); Playwright `e2e/rotate-layout.spec.ts` (320 to 414, 1280, dark 360: no overflow, handles are 44 px
  touch targets with `touch-action: none`, a touch-pointer drag and a mouse drag move the crop).

# Viewer: fit and zoom

## What the user sees

- **The photo fits.** In the viewer (library, favorites, albums, search, map and the public share page) the photo is as large as the
  area between the bar and the stars allows, with its shape kept, and never runs past the bottom of the screen: phones, a landscape
  phone, a tablet, a desktop, with the Info or Text panel open. It is not drawn at more than twice its own pixels.
- **Zoom the photo, not the page.** Pinch with two fingers, pinch on a trackpad (or hold Ctrl and turn the wheel), or double-tap /
  double-click (back to fit on the next one) to zoom around that point. A plain mouse wheel does nothing, as before. While zoomed,
  one finger or the mouse drags the photo, and it cannot be dragged off the screen. Keys: **+** / **=** zoom in, **-** out, **z**
  between fit and zoomed (**0** to **5** stay the rating; Ctrl with +/- is still the browser's own page zoom). On a computer the
  corner of the photo has **-**, the level (fit = 100 %), **+** and **Fit** buttons; a touch screen shows them once zoomed.
- **Limits.** From fit (the smallest) up to 4 device pixels per picture pixel, at least 2x and at most 8x fit.
- **Zoomed, the gestures that move you away are off:** the swipe up for the Info panel does not fire (the finger pans). It works
  again at fit. Zoom starts over when you go to another photo or close the viewer. The rest of the app, and the page around the
  viewer, zoom as before: only the photo area has `touch-action: none`, and the viewport meta has no `user-scalable` or
  `maximum-scale`.
- **Sharper when it matters.** The viewer shows the ~1,440 px preview. Once the zoom shows the photo at more device pixels than the
  preview has, the original file replaces it, if browsers can show it: an **unedited JPEG, PNG or WebP** (its `original_url`, the
  presigned link the viewer already has, so the same authorization as everything else). HEIC and RAW (browsers cannot show them), an
  edited photo (its original is not the edited picture) and a file not larger than the preview have none: the preview is scaled.
  The swap happens once the original is decoded, in the box the preview made, so nothing jumps. The public share page has no such
  link (its downloads are attachments), so it scales the preview.

## How it works

- Layout: `.lightbox-stage` is a size container (`container-type: size`), so its box comes from the flex column alone, and the photo is
  sized in `cqw` / `cqh` from its shape (`photoFit.ts`: `--ar`, `--cap`). The old grid with an auto row let the picture's own height size
  the row, so `max-height: 100%` had nothing to shrink to and a 1,440 px preview ran past a 720 px screen.
- Zoom: `zoomMath.ts` (pure: limits, clamping, zoom around a point, pinch, steps, the sharper-picture rule) and `usePhotoZoom.ts`
  (Pointer Events and `wheel`; the transform is written straight to the layer, `will-change` only while a gesture runs, and React hears
  only "zoomed" and "past the preview").

## Tests

- Web: `zoomMath` (29 cases). Playwright `e2e/viewer-fit-layout.spec.ts` (the photo inside the stage, filling it, shape kept, never under
  the bar or stars, at 320x568 to 1920x1080, light and dark, with the Info panel, and on the share page) and
  `e2e/viewer-zoom-layout.spec.ts` (ctrl + wheel, double click, keys, buttons, pan clamping, reset, a two-finger pinch and double tap by
  CDP touch events, swipe up off while zoomed, page zoom untouched, and the original requested only past the preview's resolution).

# Slideshow and photo frame

## What the user sees

- **Slideshow.** A **Slideshow** button in the header of an album (smart ones too), the library, Favorites and the Archive, a search
  (filters or words), a person, a tag, "On this day" and a share link's page starts a full-screen slideshow of what that page lists.
  The viewer has **Play from here** (an action of its bar, in the More menu on a phone, or the **S** key): the slideshow starts at the photo on
  show and goes on through the photos the viewer has loaded around it (as listed, oldest first, or shuffled with that photo first); Esc comes
  back to the viewer.
- **Settings** (the gear): time per photo (3, 5, 10 or 20 s), order (as listed = the page's order, newest first; oldest first; shuffle),
  transition (crossfade or none) and whether to start again at the end. They are remembered per browser (`localStorage`, key
  `cyanotype.slideshow`; a browser without storage uses the defaults). A device that asks for reduced motion
  (`prefers-reduced-motion`) always gets a cut, and the transition setting says so.
- **Controls** hide after 3 s without a mouse move, key or tap (a tap on the picture brings them back; they stay while paused, with the
  settings open, or with the keyboard focus on a button). Swipe left and right on a touch screen for next and previous.
- **Keys** (in the slideshow; it keeps every key from the page underneath, so the viewer's shortcuts do not act behind it):

  | key | action |
  |---|---|
  | Space | play / pause |
  | ← → | previous / next photo (also while paused) |
  | F | full screen on / off (where the browser has it) |
  | Esc | close the settings, else the slideshow (a browser in full screen takes the first Esc to leave full screen) |
  | S | in the viewer: play slideshow from here (does not collide with the viewer's I, T, E, R, C, 0 to 5, + - Z, Delete) |

- **Full screen** is requested from the click or key that starts the slideshow. Safari on an iPhone has no full screen for pages: the
  button is not shown and the slideshow fills the window (`position: fixed; inset: 0`). The Screen Wake Lock keeps the screen on while it
  runs (feature-detected; taken again when the tab comes back).
- **Screen readers** hear "Slideshow playing / paused", "Photo 4 of 120" after a manual move, "End of the slideshow" and "Cannot reach the
  server. Trying again."; a photo that changed by itself is not announced.
- A video is never in a slideshow: Cyanotype does not import video, and a photo whose derived images are not ready is skipped.

## The frame (`/frame`)

A wall tablet's view: random photos, each fitted inside the screen over a blurred, darkened copy of itself, changing by themselves.
Sign in on the tablet once (the session lasts 30 days from sign-in, not sliding), open `/frame`, and choose on the setup screen, or give the
choice in the address and bookmark it:

| parameter | meaning |
|---|---|
| `source=library` / `source=favorites` / `album=<id>` | what to show (default of the setup screen: the whole library) |
| `interval=<seconds>` | time per photo, 5 to 3600, default 30 |
| `clock=1` | the time and date, lower left |
| `caption=1` | the photo's caption (if it has one), lower right |
| `setup` | open the setup screen with the current choice |

With no parameters the frame uses what this browser last started with (`localStorage`, `cyanotype.frame`), else shows the setup screen. A tap
opens a small menu (change what is shown, full screen, exit, close; it closes by itself after 10 s; Esc opens and closes it).

- **Which photos**: never hidden ones (the Hidden view is not a source), never trashed ones, a RAW + JPEG pair once; **archived photos are
  left out** (the library and Favorites scopes skip them on the server, an album lists them, so the frame skips them on the client): the
  archive is where a photo is put away on purpose. Shuffle is fair over the whole set: each pass is a fresh server-side shuffle, so every
  photo comes once per pass, and a photo never follows itself.
- **Running for days**: at most two photos (and their two blurred copies) are in the page; the page holds a window of about 60 past and
  a chunk of 40 upcoming list entries; the decoded picture of every photo that has been shown is released; no object URLs are made at all
  (the CSP has no `blob:` in `img-src`, the pictures are plain `<img>` addresses). The preview address is asked for just before a photo is
  due (`GET /assets/previews`, valid for minutes), never kept from a list page, so an address cannot expire on screen. A request that
  never answers is abandoned after 45 s.
- **No network**: the photo on screen stays, a small "Offline. Trying again..." appears, and the retries wait 2, 5, 15, 30, 60 s and then
  5 minutes; the browser's `online` event and the tab coming back retry at once. Photos that fail one by one (an expired link, a deleted
  object) are skipped; three in a row, or any failure while the browser knows it is offline, count as trouble and are not skipped.
- **Session ended** (401 from any call): a "Sign in again" screen with a Sign in button that returns to the frame. A tablet that boots
  before its Wi-Fi shows "Waiting for the network" (it asks `GET /auth/me` every 10 s) instead of claiming to be signed out.

## API (assets service, owner-scoped)

| call | answer |
|---|---|
| `GET /assets/slideshow?scope=<json>&order=listed\|oldest\|shuffle&seed=&limit=&cursor=` | `{items, next_cursor, total}`. `scope` is the JSON of a bulk action's scope (`"library"`, `"favorites"`, `"archive"`, `{"album": id}`, `{"person": id}`, `{"search": {filters}}`, `"hidden"` while unlocked); trash and undo-batch scopes are `400`. Items are the library's list entries. `limit` 1 to 100 (default 40); `cursor` is an offset into the ordered view. `listed` is newest first (the page's order), `oldest` its reverse, `shuffle` orders by `md5(id \|\| seed)` (`seed`: up to 64 letters, digits, `-` or `_`), so one seed gives one order whatever the page size and no photo repeats within a pass. The view is resolved like "Select all" (a smart album by its rules), then ordered and cut in SQL: a page costs one pass over the view (tens of ms for 50,000 photos), and the browser only ever holds the pages it has asked for. `read` and `full` keys. |
| `GET /assets/previews?ids=a,b,c` | `{items: [{id, preview_url, width, height, caption}]}` for up to 24 of the caller's own, live photos that have derived images (hidden ones only while the Hidden view is unlocked); anything else is left out. `preview_url` is the same presigned address `GET /assets/{id}` gives. |

A text search is ranked and cannot be named as a scope, so the slideshow of one reads the search's own pages (as listed: lazily; oldest first or
shuffle: up to 5,000 results read and ordered in the browser). A share link's slideshow does the same through the public items, and asks for each
photo's preview address (`GET .../items/{id}`) just before showing it.

## How it works

- `slideshow/deck.ts`: seeded shuffle (xmur3 + mulberry32 + Fisher-Yates), and `SlideDeck`, the window over a show's passes (peek / commit, so a
  photo that could not be loaded is asked for again, chunks fetched one at a time, no repeats within a pass, a new pass on loop, a bounded
  history). `sources.ts`: scope, paged, items and search sources. `previews.ts`: batched, short-lived preview addresses.
- `slideshow/controller.ts`: the state machine (timing, pause, next/previous, preloading and decoding the next photo before it is shown so there
  is no flash, crossfade or cut, announcements, retry with backoff, request timeout, 401). Plain TypeScript with injected timers and loader.
  `Slideshow.tsx` and `pages/Frame.tsx` only draw its state.
- No new dependency.

## Tests

- Rust (`routes/slideshow_tests.rs`): listed, oldest and shuffle pages join up; a seeded shuffle is stable across page sizes, has no repeats,
  covers the view and differs per seed; other accounts', trashed, archived (library scope), hidden and paired-RAW photos never appear; Hidden only
  when unlocked; album and favorites scopes; bad input; previews for the caller's own rendered photos only. `key_scope_tests` lists both routes.
- Web (vitest): the deck, seeded shuffle fairness, sources, preview cache, settings, the controller with fake timers (timing, pause, next/previous,
  loop, shuffle cycles, reduced motion, announcements, memory, backoff, timeouts, 401), the slideshow component (keys, settings, full screen,
  states), "Play from here", the frame (setup, session, offline, archived, small page, no object URLs, menu).
- Playwright: `e2e/slideshow-layout.spec.ts` and `e2e/frame-layout.spec.ts`, under the gateway's real CSP.

# Stacks in the timeline

Bursts (rapid-fire shots, "Bursts" above) were grouped in search only. A **stack** takes them to the timeline and adds the owner's
own say: photos shown as **one tile, the cover**, with a stack badge and the number of photos. A stack is either an automatic burst
(`assets.burst_id`, found by the worker) or one the owner made by hand (migration 0033). Same idea as stacks in other photo
libraries, with one difference on purpose: what the owner decided is never undone by the worker.

## What the user sees

- **Timeline.** A stack is one tile at the date of its cover, with a badge (two sheets and the count, "Stack of 3 photos" for screen
  readers). A **Stacked / Every photo** switch in the library bar (the setting is remembered per user, default **Stacked**) lists every
  photo instead. The day counts, the scrubber, the paging and "Select all" follow the switch.
- **Which views collapse.** The **timeline** (All, and Recently added) and **search** collapse stacks. **Favorites, the Archive, Hidden,
  albums, the map, People, shares and the trash list every photo**: those views are about the photos themselves (a favorite stack member
  is a favorite on its own, an album is a list somebody made photo by photo, and the album's "Add photos" picker asks for every
  photo), so nothing there is hidden behind a cover. Only photos
  the view lists are in a stack's count: an archived or trashed member leaves the tile's number.
- **The viewer.** Opening a stack's tile shows a strip under the photo: "Photo 3 of 5 in this stack", the stack's photos as a filmstrip
  (the cover is marked), previous and next buttons, and the keys **`[`** and **`]`** (the arrows still move through the timeline, so
  next and previous across the library work as before; the strip is for the stack). The viewer opens on the cover. Every action of the
  viewer (favorite, stars, Info, trash, album, share...) is about the photo on show, not the whole stack.
- **Controls** (in the strip): **Set as stack cover** (the viewer stays; the timeline is read again when it closes, so photos do not move
  under you), **Remove from stack** (the photo leaves, the viewer closes) and **Unstack all** (with Undo, which stacks the same photos
  again by hand). In the selection bar of the timeline, **Stack** makes one stack of the selected tiles (two or more photos; the first
  selected is the cover; not available for "Select all"; the toast's Undo puts every photo back as it was, merged stacks included).
- **Selecting a stack's tile means all of its photos**, like a collapsed burst in search: its label says "Select IMG_1 and 2 more from
  its stack", the bar says "plus 2 more in stacks", and favorite, archive, hide, trash, add to album, share, download, tags, stars
  and edit work on every photo of it. In the viewer an action means the one photo on show.

## What is kept, and what the sweep may do

| Column (migration 0033) | Written by | Meaning |
|---|---|---|
| `burst_id`, `burst_version` | the worker (regroup jobs, the 15-minute sweep) | the automatic burst, as before |
| `stack_id` | the owner (Stack) | the manual stack a photo is in; a random id shared by its members; wins over the burst |
| `stack_unlinked` | the owner (Remove from stack, Unstack all) | never part of an automatic burst again |
| `stack_cover` | the owner (Set as cover, Stack) | the owner's choice of cover (at most one per stack) |

A photo's stack is `coalesce(stack_id, CASE WHEN stack_unlinked THEN NULL ELSE burst_id END)`. The worker only ever writes the first
row, so **a regroup, the sweep or a time change never re-adds a photo the owner removed and never breaks a manual stack** (test:
`what_the_owner_decided_about_a_stack_survives_regroups_and_the_sweep`). The cover is the owner's choice, else the first favorite of
the stack, else its earliest photo, among the photos the view lists. Details:

- **Manual stacks merge.** Stacking a photo that is already in a manual stack brings that whole stack along (trashed members too, so a
  restored photo does not return to a stack that is gone). The other photos of a picked photo's *burst* do not come along on the
  server; the web app sends all of a collapsed burst's photos when its tile is picked, so picking a burst tile stacks the burst.
- **At most 100 photos** in a stack made by hand; **at least two**. **A manual stack never keeps fewer than two live
  photos.** Every path that shrinks one dissolves what is left in the same transaction (`cyanotype_core::stacks`): *Remove from stack*,
  trashing (also through "select all", albums, people... they all go through the trash call), hiding, and a file that becomes the RAW of
  another photo. Dissolving clears `stack_id` and the cover flag on every member, trashed ones included, and does not set
  `stack_unlinked`, so the photos are ordinary again (back in their burst, if they were in one). Migration 0035 did the same once for the
  one-photo stacks that already existed. Archiving does not shrink a stack (archived photos stay members and are listed by the Archive;
  the timeline counts the ones it lists). Permanent delete, the 30-day purge and account deletion only remove rows that were trashed
  first, which has already settled their stacks.
- **Trash.** A trashed photo keeps its stack columns while the stack still has two live photos (restoring it puts it back); when the
  stack falls below two it dissolves completely, so a photo restored later is an ordinary one and never rejoins a stack that is gone.
  **Hiding** takes the photo out of its manual stack for good (unhiding does not put it back); a hidden photo cannot be stacked.
  A RAW that is the hidden half of a pair is never a stack member (its JPEG is).
- **The cover** is never stored for a photo that left: the flag is cleared when the cover is taken out or the stack dissolves, and a
  stack without a flagged cover uses the first favorite, else the earliest photo. A trashed cover keeps its flag while the stack lives
  (it covers again once restored).
- **Undo of "Stack".** The answer of `POST /assets/stacks` lists `previous` (each moved photo's `stack_id` and `stack_cover` before,
  the merged stacks' members and trashed members included); the web toast's *Undo* sends it to `POST /assets/stacks/restore {items}`,
  which writes exactly those values back (only the caller's photos), then dissolves any stack that fell below two meanwhile.
- **Recovery.** The columns are in the database, so a dump keeps them. A rebuild from the bucket alone (`reindex`) restores them too:
  the recovery manifest (format 4) carries them on the photo's line, only where set (`stack`: the id the members share, `stack_unlinked`,
  `stack_cover`), and the **Stacked / Every photo** switch as a `prefs` line. `reindex` writes them in the transaction that recreates the
  photo's row, so the worker's regroup (which writes only `burst_id`) and the sweep cannot undo them (test:
  `stacks_and_the_stacks_switch_come_back_with_a_rebuilt_database`); bursts are found again by the worker. If some of a stack's photos have no original left in the bucket, `reindex` does not recreate a stack
  with fewer than two live photos (they come back unstacked) and gives a stack whose chosen cover is gone a new one (the first favorite
  among its live photos, else the earliest); tests `a_restored_stack_that_lost_photos_is_dropped_or_gets_a_new_cover`. Like every manifest field, they
  are as new as the newest manifest, which a change rewrites within about 30 seconds.
- Search groups by the same rule (`burst_id` in its results is the stack), so a manual stack is one search result and a photo taken out
  of a burst is its own. A search result's cover is still its best-ranked favorite, then its best-ranked photo; the owner's cover
  choice is the timeline's.

## API (assets service, owner-scoped)

| Request | Effect |
|---|---|
| `GET /assets?stacks=collapsed\|expanded`, `GET /assets/timeline?stacks=` | Timeline only (not with `favorite`, `archived`, `hidden` or `bbox`). Absent: the user's setting. Collapsed: one item per stack (its cover), the day counts count tiles. Items in a stack of two or more carry `stack: {size, members}` (`members`: the ids of every listed photo of the stack, the cover included, only while collapsed). |
| `GET /assets/{id}/stack` | `{cover_id, manual, items}`: the stack's photos in capture order as list items. `404` if the photo is not in a stack of two or more (also another account's, and a hidden photo while the Hidden view is locked; every route below answers the same). |
| `POST /assets/stacks {ids}` | Stack photos by hand; the first id is the cover. `{size, cover_id, previous}`. `400` for fewer than two photos or more than 100; foreign, unknown, trashed and hidden ids are ignored. |
| `POST /assets/stacks/restore {items}` | Undo of a stacking: `items` is the `previous` the stacking answered (`[{id, stack_id, stack_cover}]`). Puts each of the caller's photos back; `{count}`. |
| `POST /assets/{id}/stack/cover` | Make the photo its stack's cover. |
| `POST /assets/{id}/stack/remove` | Take the photo out of its stack (it stays out of automatic bursts). |
| `DELETE /assets/{id}/stack` | Dissolve the photo's stack: `{count}`. |
| `GET\|PUT /assets/prefs` | gains `stacks_collapsed` (default true). |

The write routes need a `full` API key (like the other writes). Everything filters on `owner_id`: another account's photo is a `404`, and
a stack never holds photos of two owners.

## How it works

`apps/assets/src/routes/stacks.rs` holds the SQL that says what a stack is (`key_sql`, `in_stack_sql`, `cover_order_sql`) and
`collapse_sql`, the one extra `WHERE` term the list and the day counts share, so they cannot disagree: a photo in a stack is listed only
when it is the cover of the stack's photos of the same view (a correlated subquery that runs only for photos in a stack, through
`assets_stack_idx` and `assets_burst_idx`). The page's stack info is one more query for all of its stacks. The migration adds three
columns with constant defaults, one partial index and a preference column: nothing is rewritten, so it is instant at any size. While
collapsed the day counts are no longer an index-only scan (the filter reads three more columns for the rows that have a stack key); with
"Every photo" they are unchanged.

## Tests

- Rust (`routes/stacks_tests.rs`, Postgres): collapsed list and day counts agree, paging, the setting and `?stacks=`, favorites/archive/
  albums list everything, stacking (cover, merging, limits), cover change, remove and dissolve (and a regroup that does not undo them),
  a manual stack that wins over a burst, search, trashed and hidden members, and **ownership** (every route is a `404` for another
  account, a stacking request mixing accounts only stacks the caller's photos); `key_scope_tests` lists the new routes.
  `crates/cyanotype-core/tests/bursts.rs`: the owner's columns survive `regroup`, the sweep and a time change.
- Web: `stacks.test.ts` (selection, expansion, stepping), `pages/Library.stacks.test.tsx` (badge, switch, viewer strip and keys, cover,
  remove, unstack and Undo, stacking from the selection bar, trashing a stack's tile); Playwright `e2e/stacks-layout.spec.ts` (badge,
  viewer strip, controls, selection bar and the switch at 320 to 1280 px, light and dark).
