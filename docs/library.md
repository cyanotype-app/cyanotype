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
