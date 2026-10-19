# Cyanotype: MVP Plan

See the project README for the elevator pitch. This file tracks milestones and decisions.

## Guiding principles

- **S3 is the source of truth.** Originals are immutable and content-addressed; Postgres can be rebuilt by re-scanning the bucket.
- **Bytes never touch the API.** Presigned URLs for upload and download. (Temporary exception: browser uploads are relayed part by part through the assets service until the bucket allows CORS; see `docs/import.md`.)
- **Small stateless services from one repo:** `auth`, `assets` and `worker` binaries (`apps/`), shared libraries in `crates/`, a Caddy gateway in front. Each service has its own port and only the config it needs; all scale horizontally and are safe to kill.
- **One migration owner:** `cyanotype-cli migrate` is the only thing that applies migrations (a one-shot compose step); services never migrate on boot.
- **Structured logs to stdout:** JSON with Datadog reserved attributes, collected by the Datadog Agent (opt-in compose profile). See `docs/observability.md`.
- **Postgres is the only other dependency:** data, pgvector search, and the job queue. No Redis, no Python ML sidecar.

## Stack

| Layer | Choice |
|---|---|
| HTTP | `axum` + `tokio`; OpenAPI via `utoipa` -> typed TS client |
| Gateway | Caddy (`infra/caddy/Caddyfile`): `/auth/*` -> auth, `/assets/*` -> assets |
| Logging | `tracing` -> JSON on stdout -> Datadog Agent |
| DB | Postgres + `sqlx` + pgvector |
| Object storage | `aws-sdk-s3` (R2, B2, SeaweedFS, AWS) |
| Image processing | `image`, `fast_image_resize`, `libheif-rs`, `rawler` |
| Video (post-MVP) | `ffmpeg` subprocess |
| EXIF | `kamadak-exif` / `nom-exif`, `exiftool` fallback |
| ML | `ort` (ONNX Runtime, static) + `tokenizers`; Google SigLIP 2 base/16 (Apache 2.0; 4-bit image tower, int8 text tower), OpenAI CLIP ViT-B/32 (int8) as the alternative |
| Frontend | React + Vite (TypeScript, pnpm, React Router), PWA-installable; static build served by Caddy |
| Map | MapLibre GL + Protomaps PMTiles in the bucket |

## S3 layout

```
originals/{user_id}/{hash[0:2]}/{hash}.{ext}   # immutable
derived/{user_id}/{asset_id}/thumb.webp
derived/{user_id}/{asset_id}/preview.webp
derived/{user_id}/{asset_id}/share.jpg         # on demand: the JPEG original without metadata, for share-link downloads
```

Deleting is two-stage: `DELETE`/`POST /assets/trash` only sets `assets.deleted_at` (objects untouched, so restore is instant and the trash view keeps its thumbnails); permanent deletion removes every **version and delete marker** of the original and of everything under `derived/{user_id}/{asset_id}/` (the bucket is versioned, so a plain delete would keep the bytes), then the rows. See `docs/library.md`.

## Milestones

| # | Scope | Status |
|---|---|---|
| 0 | Cargo workspace, Docker Compose (Postgres + SeaweedFS), CI, migrations, auth | done (scaffold) |
| 1 | Presigned upload, dedupe, asset CRUD, basic grid UI | backend done; grid UI done (login/register, infinite-scroll grid with ThumbHash placeholders, lightbox); browser upload done via a temporary backend proxy (`PUT /assets/uploads/{id}/parts/{n}`, bounded to 3 buffered parts) because Linode has no CORS; follow-up: switch back to direct presigned PUTs once storage supports CORS |
| 2 | Worker, job queue, EXIF, thumbnails, ThumbHash, HEIC | done: queue; metadata (EXIF, sha256 verify) + thumbnail/preview/ThumbHash for JPEG/PNG/WebP, HEIC/HEIF/AVIF (libheif 1.23.1, statically linked) and camera RAW (embedded-preview first, capped rawler develop fallback); magic-byte routing; shared memory budget. Follow-ups: colour-profile conversion for wide-gamut HEIC, HDR gain maps; DCT-scaled JPEG, tile-streamed HEIC grid and row-streamed PNG decoding are done (see Memory discipline) |
| 3 | Virtualized timeline, lightbox, albums, sharing links | done: lightbox; photo management done (multi-select, trash with undo, restore, delete forever, empty trash, automatic purge after `TRASH_RETENTION_DAYS`; `docs/library.md`). albums done (create/edit/delete, add/remove, cover, add-to-album from the library and lightbox; `docs/library.md`). RAW + JPEG pairs done (a camera's RAW and JPEG/HEIC show as one photo; `docs/library.md`). Virtualized timeline done (month/day headers, only rows near the viewport in the DOM, per-day counts so the whole library has an exact scrollbar, year/month scrubber that jumps to any date without loading what is before it, favorites and the Favorites view, download of originals by presigned attachment URL; `docs/library.md`). Share links done (public links to an album or a set of photos with expiry, password, download policy and revocation; GPS never leaves: derived WebP carry no metadata, downloads are a metadata-free JPEG copy unless the owner includes originals; `docs/library.md`). |
| 4 | CLIP semantic search and filters | done: natural-language search ("dog on the beach", "red car at night") and structured filters (dates, favorites, camera, place, file type, location, album) over the whole library, entirely on the server. OpenAI CLIP ViT-B/32 (MIT) as 8-bit ONNX, run in-process with `ort` + `tokenizers` (`crates/cyanotype-clip`): the worker embeds each photo's `preview.webp` in a low-priority background `embed` job (behind thumbnails; backfill of an existing library by migration 0012), the assets service embeds each query, vectors live in pgvector (`halfvec(512)`, HNSW cosine) and the model files are baked into the images (checksums pinned in `models/clip.lock`). Web: search field in the library header, results grid, filter chips and sheet, recent searches, "still indexing" state. Jobs gained a `priority`. `docs/library.md` ("Search"), `docs/build.md` ("Semantic search model") |
| 4b | Search v2: SigLIP 2, a calibrated cut-off, dates and places in the query | done: SigLIP 2 B/16 replaces CLIP as the default after a bake-off on 209 CC0 photos (P@5 0.98 against 0.92, usable in Spanish, German and French: `docs/build.md`), `SEARCH_MODEL` picks the model, vectors of several models live in one table with one HNSW index each (migration 0013) and the previous model answers while a library is re-indexed (`docs/library.md`, "Changing the search model"). Results are cut into strong matches and "Show weaker matches" by the model's own probability, a relative rule and a gap rule; a query nothing answers returns nothing strong. Dates ("June 2023", "summer 2022", "last year") and the user's own places ("in Lisbon", "sunset Maui") are read out of the words as removable chips. The 8-bit export of the SigLIP 2 image tower loses too much and is not used (4-bit instead); the text tower's embedding table is split into a side file to fit 512m. Follow-ups: ONNX Runtime >= 1.31 maps the side file (text tower to about 120 MB), idle-unloading the text tower |
| 5 | Map view and offline reverse geocoding | done: clustered photo map (MapLibre GL, `/map`, a tab beside All photos and Favorites) with a viewport photo list; server-side grid clustering behind `GET /assets/map`; Protomaps PMTiles basemap served same-origin through the assets service (the bucket has no CORS; `scripts/fetch-basemap.sh` installs it); offline reverse geocoding (`geocode` job over a bundled GeoNames snapshot loaded by `cyanotype migrate`, place shown in the lightbox). Follow-ups: place subtitles in the timeline, search by place (milestone 4); `docs/library.md`, `docs/deploy.md` |
| 5b | Photo details | done: Apple Photos style details for the open photo: Info button, `i` key or a swipe up on a phone; date with the photo's own UTC offset (and "time zone unknown" when EXIF has none), place with a small non-interactive map, GPS altitude and direction, camera and lens, the exposure line (`ISO 100 · 26 mm · ƒ/1.8 · 1/120 s · +0.3 ev`), 35 mm equivalent, flash, white balance, metering, programme, files of a RAW + JPEG pair, resolution, colour space, software, albums, burst position. Derived on the server from the stored EXIF dump by `GET /assets/{id}/details`, so no reprocessing and no migration; camera, lens, place and date link to the search, the timeline and the map; the search gained a lens filter. Share links expose none of it. Follow-up: the ICC profile name (needs the metadata job re-run). `docs/library.md` ("Photo details") |
| 5c | Edit date, location, caption | done: from the details panel (one photo) or the library's selection bar (many): adjust the date and time with its zone (for a selection, shift all by an offset), set or fix the location (map, your own places, coordinates), add a caption; "Revert to original". The original is never touched; the effective `taken_at`/`latitude`/`longitude` columns are overwritten (so timeline, map, filters, bursts, geocoding follow) with the file's values kept beside them, and each photo's edits are written first to `edits/{user}/{xx}/{hash}.json` in the bucket, which `cyanotype reindex` restores from (migration 0015, no rows rewritten). Text search finds photos by their caption words, in front of the model's ranking. `docs/library.md` ("Editing a photo"), `docs/recovery.md` |
| 7a | Admin portal: roles and invites | done: accounts have a `role` (admin or member; the earliest account is promoted by migration 0016, the first account of a fresh instance is made admin by registration); `REGISTRATION_MODE=invite` (the new default) lets later accounts register only with a single-use, expiring invite link made by an admin on the new `/admin` area (hidden from members, enforced by the server for login sessions only); invite tokens are stored as sha256 and a link is shown once (re-issuing replaces it); optional preset limits are stored for the limits work; every admin action is written to the new `audit_log` table. `docs/admin.md`, `docs/deploy.md` |
| 7b | Admin portal: usage and accounts | done: per-account usage (originals, trash, photos, RAW, measured thumbnail sizes) kept current by triggers on `assets` (migration 0017) and reconciled nightly by the worker with drift logged and sent as metrics; an upload log for the last 24 hours and 30 days; Settings > Storage for the user; Admin > Accounts (counts and sizes only, never photos) with server totals and an on-demand bucket check that runs as a background `admin_tasks` job. `docs/admin.md` |
| 7c | Admin portal: limits | done: per-account storage quota (trash counts until purged), rolling daily upload cap and largest file, each optional (NULL = unlimited; existing accounts and the admin stay unlimited, migration 0018); a server default for new accounts and invite presets; enforced at upload creation under a per-account lock with unfinished uploads reserved, and again as parts arrive (the proxy pins every part, `complete` refuses parts that add up to more than was declared); 413/429 with stable codes and a reset time; web uploader and `cyanotype import` stop cleanly with the reason; meters and 80/95 % warnings on Settings > Storage and in the upload tray; admins edit limits on the accounts page, audited. `docs/admin.md` |
| 6 | Rust CLI bulk importer and deploy scripts | deploy half done: single-VPS compose, GHCR publish, deploy workflow, backups (`docs/deploy.md`); importer done: `cyanotype login/import/status`, API keys (`docs/import.md`) |

Non-goals for MVP: native mobile apps, face recognition, video transcoding, live photos, partner sharing.

## Deployment decisions

- One Linode VPS (shared CPU, 4 GB, us-ord) with docker compose, not Kubernetes. Object storage is Linode (us-ord-10); Datadog site us5.
- Images are built once in CI (bake), published to GHCR (private) and only pulled on the server. Deploys run from a GitHub Environment (`dev`) over SSH or manually with `just deploy`.
- Public-facing hardening: `REGISTRATION_MODE` (default `invite`; `first_user`, `open` and `closed` still work), `Secure` session cookie, per-IP rate limit on login/register.
- Postgres backed up nightly to the bucket; the bucket has versioning on. Known gap: Linode returns 501 for `PutBucketCors`, so browsers upload through the assets service proxy (temporary) instead of directly.

## Requirements carried through every milestone

- Multi-user schema (`owner_id` everywhere) even while the UI is single-user.
- Auth: argon2, HttpOnly session cookies, API keys for CLIs, OIDC later. Per-tenant S3 prefixes, short presign TTLs, GPS stripped on shared links (done, `docs/library.md`).
- Memory discipline: stream everything, DCT-scaled JPEG decode, semaphores around heavy jobs. Done: a shared byte budget (`WORKER_MEMORY_BUDGET_MB`) refuses jobs that cannot fit the container limit; JPEGs are decoded through the IDCT at 1/2, 1/4 or 1/8 size, HEIC grids (iPhone photos) tile by tile, and non-interlaced PNGs row by row, so peak memory no longer grows with the image (a 100 MP JPEG: 336 to 21 MB; a 48 MP iPhone HEIC: 285 to 35 MB; measurements and the new ceilings in `docs/build.md`, "Memory budget"). Photos refused before this are requeued by migration 0010. **Follow-ups:** streamed WebP and interlaced PNG (no row interface in the decoders), tile-wise decode of HEIC grids with alpha, a smaller decode for single-image HEIC/AVIF (libheif has no reduced-size decode; about 90 MP 8-bit is the ceiling), progressive JPEGs above about 150 MP (coefficients are held in full).
- `reindex` command that rebuilds the DB from the bucket (done: `cyanotype reindex`, [`recovery.md`](recovery.md)).
- `tracing` + job/upload metrics + dashboards: done for Datadog (DogStatsD from the worker and assets service, dashboard and monitors as code in `infra/datadog/`, [`observability.md`](observability.md)). Workers wake on Postgres LISTEN/NOTIFY instead of polling (done; 30 s fallback poll). Follow-ups: tracing (OTLP), a Postgres/Caddy Agent integration, per-user import ETA.
- Idempotent, versioned jobs so models can be swapped and re-run.

## Open decisions

1. Library size and RAW usage.
2. ~~Frontend: SvelteKit or React~~ decided: React + Vite (`apps/web`).
3. Mobile: PWA + CLI uploader for v1, or native auto-upload sooner.
