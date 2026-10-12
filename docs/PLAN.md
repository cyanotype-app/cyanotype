# Cyanotype: MVP Plan

See the project README for the elevator pitch. This file tracks milestones and decisions.

## Guiding principles

- **S3 is the source of truth.** Originals are immutable and content-addressed; Postgres can be rebuilt by re-scanning the bucket.
- **Bytes never touch the API.** Presigned URLs for upload and download.
- **Two stateless binaries** (`api`, `worker`) from one Cargo workspace. Both scale horizontally, both safe to kill.
- **Postgres is the only other dependency:** data, pgvector search, and the job queue. No Redis, no Python ML sidecar.

## Stack

| Layer | Choice |
|---|---|
| HTTP | `axum` + `tokio`; OpenAPI via `utoipa` -> typed TS client |
| DB | Postgres + `sqlx` + pgvector |
| Object storage | `aws-sdk-s3` (R2, B2, MinIO, AWS) |
| Image processing | `image`, `fast_image_resize`, `libheif-rs`, `rawler` |
| Video (post-MVP) | `ffmpeg` subprocess |
| EXIF | `kamadak-exif` / `nom-exif`, `exiftool` fallback |
| ML | `ort` + `tokenizers`, CLIP/SigLIP-class model |
| Frontend | SvelteKit, PWA-installable |
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
| 1 | Presigned upload, dedupe, asset CRUD, basic grid UI | backend done; UI pending |
| 2 | Worker, job queue, EXIF, thumbnails, ThumbHash, HEIC | queue done; metadata (EXIF, sha256 verify) + thumbnail/preview/ThumbHash handlers done for JPEG/PNG/WebP; HEIC and RAW pending |
| 3 | Virtualized timeline, lightbox, albums, sharing links | |
| 4 | CLIP semantic search and filters | |
| 5 | Map view and offline reverse geocoding | |
| 6 | Rust CLI bulk importer and deploy scripts | |

Non-goals for MVP: native mobile apps, face recognition, video transcoding, live photos, partner sharing.

## Requirements carried through every milestone

- Multi-user schema (`owner_id` everywhere) even while the UI is single-user.
- Auth: argon2, HttpOnly session cookies, API keys for CLIs, OIDC later. Per-tenant S3 prefixes, short presign TTLs, GPS stripped on shared links.
- Memory discipline: stream everything, DCT-scaled JPEG decode, semaphores around heavy jobs.
- `reindex` command that rebuilds the DB from the bucket.
- `tracing` + Prometheus metrics + job dashboards.
- Idempotent, versioned jobs so models can be swapped and re-run.

## Open decisions

1. Library size and RAW usage.
2. Frontend: SvelteKit (leaning) or React.
3. Mobile: PWA + CLI uploader for v1, or native auto-upload sooner.
