# Cyanotype

An open-source, self-hosted photo library. S3 is the source of truth, Postgres is the
only other dependency, and the backend is a handful of small stateless Rust services in one repo.

- **Originals are immutable and content-addressed** under `originals/{user}/{hash[0:2]}/{hash}.{ext}`.
  The database can be rebuilt by re-scanning the bucket.
- **Bytes never touch the API.** Clients upload and download straight to S3 with presigned URLs.
- **`cyanotype-auth`** (`/auth/*`) and **`cyanotype-assets`** (`/assets/*`) are separate HTTP services,
  each with its own binary, port and config, behind a Caddy gateway on `:3000`, so clients see one origin.
  **`cyanotype-worker`** runs the processing pipeline off a Postgres job queue (`FOR UPDATE SKIP LOCKED`).
  All are stateless, scale horizontally and are safe to kill. They share the one Postgres (separate tables).
- Migrations run in exactly one place: `cyanotype-cli migrate` (a one-shot compose step). Services never migrate on boot.

## Layout

```
apps/auth      cyanotype-auth     :3001  register / login / logout / me
apps/assets    cyanotype-assets   :3002  presigned upload flow, asset reads
apps/worker    cyanotype-worker          metadata + thumbnail jobs
apps/cli       cyanotype-cli             `migrate`
apps/web       (reserved for the SvelteKit frontend)
crates/cyanotype-core      config, db, storage, job queue, telemetry (logging)
crates/cyanotype-service   shared HTTP plumbing: AuthUser extractor, ApiError, request id + request logging
infra/caddy    gateway Caddyfile
```

## Status

Milestone 0/1 scaffold: workspace, migrations, auth (argon2 + HttpOnly session cookies + API keys),
and the presigned multipart upload flow. Milestone 2 in progress: the worker handles metadata and
thumbnails for JPEG/PNG/WebP. See the plan in `docs/` for what comes next.

## Development

Tasks run through [`just`](https://github.com/casey/just) (`cargo install just --locked`, `brew install just`,
or a release binary). `just` lists every recipe.

```sh
just up        # whole stack in containers (Postgres, MinIO, migrate, auth, assets, worker, gateway) at http://localhost:3000
just e2e       # register -> check -> PUT -> complete -> worker -> GET, through the gateway
just down
```

Or run the binaries from source against the compose infrastructure:

```sh
cp .env.example .env     # just loads .env automatically
just dev                 # compose Postgres + MinIO, migrate, then auth :3001, assets :3002, worker
```

Before pushing: `just check` (fmt-check + clippy `-D warnings` + tests; set `DATABASE_URL` to a Postgres,
e.g. the compose one, to include the DB integration tests). Other recipes: `fmt`, `lint`, `test`, `migrate`,
`images` (build all four images), `datadog-up`, `clean-docker`.

When running from source, call the services directly (`:3001` for `/auth/*`, `:3002` for `/assets/*`)
unless you run them in compose. Images are per-service Dockerfile targets (`auth|assets|worker|cli`) built
from one shared builder; see [`docs/build.md`](docs/build.md).

Logs are JSON on stdout (`LOG_FORMAT=pretty` for local reading). Datadog Agent collection is an opt-in
compose profile; see [`docs/observability.md`](docs/observability.md).

### Upload flow

```
POST /assets/check     {sha256, size_bytes, filename, content_type}
  -> {status: "exists", asset_id}                      # dedupe hit
  -> {status: "upload", upload_id, part_size, parts: [{number, url}]}
PUT  <each part url>   (raw bytes, straight to S3; keep the ETag headers)
POST /assets/complete  {upload_id, parts: [{number, etag}]}
  -> {asset_id}        # row inserted, metadata + thumbnail jobs enqueued
GET  /assets?limit=&before=<rfc3339>
  -> {items: [...], next_before: <rfc3339>|null}       # pass next_before as `before` for the next page
GET  /assets/{id}
  -> {..., original_url, thumb_url, preview_url}       # presigned GETs; thumb/preview are null
                                                       # until the worker has produced them
```

The worker (`cyanotype-worker`) reads EXIF/dimensions, verifies the stored sha256, and writes
320px / 1440px WebP derivatives plus a ThumbHash for JPEG, PNG and WebP originals. Other types
are skipped (job marked done) until HEIC/RAW support lands. Tunables: `WORKER_HEAVY_CONCURRENCY`
(default 2) and `WORKER_MAX_ORIGINAL_BYTES` (default 200 MiB).

All URLs above are served through the gateway on `:3000`, unchanged by the service split.

Auth: `POST /auth/register` / `POST /auth/login` set a session cookie; CLIs send
`Authorization: Bearer <api key>` instead.

## Licence

AGPL-3.0-or-later. See `LICENSE`.
