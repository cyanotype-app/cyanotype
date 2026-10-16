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
| Object storage | `aws-sdk-s3` (R2, B2, MinIO, AWS) |
| Image processing | `image`, `fast_image_resize`, `libheif-rs`, `rawler` |
| Video (post-MVP) | `ffmpeg` subprocess |
| EXIF | `kamadak-exif` / `nom-exif`, `exiftool` fallback |
| ML | `ort` + `tokenizers`, CLIP/SigLIP-class model |
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
| 0 | Cargo workspace, Docker Compose (Postgres + MinIO), CI, migrations, auth | done (scaffold) |
| 1 | Presigned upload, dedupe, asset CRUD, basic grid UI | backend done; grid UI done (login/register, infinite-scroll grid with ThumbHash placeholders, lightbox); browser upload done via a temporary backend proxy (`PUT /assets/uploads/{id}/parts/{n}`, bounded to 3 buffered parts) because Linode has no CORS; follow-up: switch back to direct presigned PUTs once storage supports CORS |
| 2 | Worker, job queue, EXIF, thumbnails, ThumbHash, HEIC | done: queue; metadata (EXIF, sha256 verify) + thumbnail/preview/ThumbHash for JPEG/PNG/WebP, HEIC/HEIF/AVIF (libheif 1.23.1, statically linked) and camera RAW (embedded-preview first, capped rawler develop fallback); magic-byte routing; shared memory budget. Follow-ups: DCT-scaled JPEG and streamed HEIC decoding, colour-profile conversion for wide-gamut HEIC, HDR gain maps |
| 3 | Virtualized timeline, lightbox, albums, sharing links | done: lightbox; photo management done (multi-select, trash with undo, restore, delete forever, empty trash, automatic purge after `TRASH_RETENTION_DAYS`; `docs/library.md`). albums done (create/edit/delete, add/remove, cover, add-to-album from the library and lightbox; `docs/library.md`). RAW + JPEG pairs done (a camera's RAW and JPEG/HEIC show as one photo; `docs/library.md`). Virtualized timeline done (month/day headers, only rows near the viewport in the DOM, per-day counts so the whole library has an exact scrollbar, year/month scrubber that jumps to any date without loading what is before it, favorites and the Favorites view, download of originals by presigned attachment URL; `docs/library.md`). Share links done (public links to an album or a set of photos with expiry, password, download policy and revocation; GPS never leaves: derived WebP carry no metadata, downloads are a metadata-free JPEG copy unless the owner includes originals; `docs/library.md`). |
| 4 | CLIP semantic search and filters | |
| 5 | Map view and offline reverse geocoding | done: clustered photo map (MapLibre GL, `/map`, a tab beside All photos and Favorites) with a viewport photo list; server-side grid clustering behind `GET /assets/map`; Protomaps PMTiles basemap served same-origin through the assets service (the bucket has no CORS; `scripts/fetch-basemap.sh` installs it); offline reverse geocoding (`geocode` job over a bundled GeoNames snapshot loaded by `cyanotype migrate`, place shown in the lightbox). Follow-ups: place subtitles in the timeline, search by place (milestone 4); `docs/library.md`, `docs/deploy.md` |
| 6 | Rust CLI bulk importer and deploy scripts | deploy half done: single-VPS compose, GHCR publish, deploy workflow, backups (`docs/deploy.md`); importer done: `cyanotype login/import/status`, API keys (`docs/import.md`) |

Non-goals for MVP: native mobile apps, face recognition, video transcoding, live photos, partner sharing.

## Deployment decisions

- One Linode VPS (shared CPU, 4 GB, us-ord) with docker compose, not Kubernetes. Object storage is Linode (us-ord-10); Datadog site us5.
- Images are built once in CI (bake), published to GHCR (private) and only pulled on the server. Deploys run from a GitHub Environment (`dev`) over SSH or manually with `just deploy`.
- Public-facing hardening: `REGISTRATION_MODE` (default `first_user`), `Secure` session cookie, per-IP rate limit on login/register.
- Postgres backed up nightly to the bucket; the bucket has versioning on. Known gap: Linode returns 501 for `PutBucketCors`, so browsers upload through the assets service proxy (temporary) instead of directly.

## Requirements carried through every milestone

- Multi-user schema (`owner_id` everywhere) even while the UI is single-user.
- Auth: argon2, HttpOnly session cookies, API keys for CLIs, OIDC later. Per-tenant S3 prefixes, short presign TTLs, GPS stripped on shared links (done, `docs/library.md`).
- Memory discipline: stream everything, DCT-scaled JPEG decode, semaphores around heavy jobs. Today: a shared byte budget (`WORKER_MEMORY_BUDGET_MB`) refuses jobs that cannot fit the container limit, and HEIC and JPEG are decoded at full size then resized. **Follow-up:** DCT-scaled JPEG decode and tiled/streamed HEIC decode would cut the per-megapixel cost 4-8x and lift the ~40 MP HEIC / ~95 MP JPEG ceilings (`docs/build.md`, "Memory budget").
- `reindex` command that rebuilds the DB from the bucket (done: `cyanotype reindex`, [`recovery.md`](recovery.md)).
- `tracing` + job/upload metrics + dashboards: done for Datadog (DogStatsD from the worker and assets service, dashboard and monitors as code in `infra/datadog/`, [`observability.md`](observability.md)). Workers wake on Postgres LISTEN/NOTIFY instead of polling (done; 30 s fallback poll). Follow-ups: tracing (OTLP), a Postgres/Caddy Agent integration, per-user import ETA.
- Idempotent, versioned jobs so models can be swapped and re-run.

## Open decisions

1. Library size and RAW usage.
2. ~~Frontend: SvelteKit or React~~ decided: React + Vite (`apps/web`).
3. Mobile: PWA + CLI uploader for v1, or native auto-upload sooner.
