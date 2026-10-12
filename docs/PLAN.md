# Cyanotype: MVP Plan

See the project README for the elevator pitch. This file tracks milestones and decisions.

## Guiding principles

- **S3 is the source of truth.** Originals are immutable and content-addressed; Postgres can be rebuilt by re-scanning the bucket.
- **Bytes never touch the API.** Presigned URLs for upload and download.
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
```

## Milestones

| # | Scope | Status |
|---|---|---|
| 0 | Cargo workspace, Docker Compose (Postgres + MinIO), CI, migrations, auth | done (scaffold) |
| 1 | Presigned upload, dedupe, asset CRUD, basic grid UI | backend done; grid UI in progress (login/register, infinite-scroll grid with ThumbHash placeholders, lightbox; browser upload deferred: Linode CORS) |
| 2 | Worker, job queue, EXIF, thumbnails, ThumbHash, HEIC | done: queue; metadata (EXIF, sha256 verify) + thumbnail/preview/ThumbHash for JPEG/PNG/WebP, HEIC/HEIF/AVIF (libheif 1.23.1, statically linked) and camera RAW (embedded-preview first, capped rawler develop fallback); magic-byte routing; shared memory budget. Follow-ups: DCT-scaled JPEG and streamed HEIC decoding, colour-profile conversion for wide-gamut HEIC, HDR gain maps |
| 3 | Virtualized timeline, lightbox, albums, sharing links | |
| 4 | CLIP semantic search and filters | |
| 5 | Map view and offline reverse geocoding | |
| 6 | Rust CLI bulk importer and deploy scripts | deploy half done: single-VPS compose, GHCR publish, deploy workflow, backups (`docs/deploy.md`); importer pending |

Non-goals for MVP: native mobile apps, face recognition, video transcoding, live photos, partner sharing.

## Deployment decisions

- One Linode VPS (shared CPU, 4 GB, us-ord) with docker compose, not Kubernetes. Object storage is Linode (us-ord-10); Datadog site us5.
- Images are built once in CI (bake), published to GHCR (private) and only pulled on the server. Deploys run from a GitHub Environment (`dev`) over SSH or manually with `just deploy`.
- Public-facing hardening: `REGISTRATION_MODE` (default `first_user`), `Secure` session cookie, per-IP rate limit on login/register.
- Postgres backed up nightly to the bucket; the bucket has versioning on. Known gap: Linode returns 501 for `PutBucketCors`, so browser direct uploads are blocked until solved.

## Requirements carried through every milestone

- Multi-user schema (`owner_id` everywhere) even while the UI is single-user.
- Auth: argon2, HttpOnly session cookies, API keys for CLIs, OIDC later. Per-tenant S3 prefixes, short presign TTLs, GPS stripped on shared links.
- Memory discipline: stream everything, DCT-scaled JPEG decode, semaphores around heavy jobs. Today: a shared byte budget (`WORKER_MEMORY_BUDGET_MB`) refuses jobs that cannot fit the container limit, and HEIC and JPEG are decoded at full size then resized. **Follow-up:** DCT-scaled JPEG decode and tiled/streamed HEIC decode would cut the per-megapixel cost 4-8x and lift the ~40 MP HEIC / ~95 MP JPEG ceilings (`docs/build.md`, "Memory budget").
- `reindex` command that rebuilds the DB from the bucket.
- `tracing` + Prometheus metrics + job dashboards.
- Idempotent, versioned jobs so models can be swapped and re-run.

## Open decisions

1. Library size and RAW usage.
2. ~~Frontend: SvelteKit or React~~ decided: React + Vite (`apps/web`).
3. Mobile: PWA + CLI uploader for v1, or native auto-upload sooner.
