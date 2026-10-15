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
