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
- Migrations run in exactly one place: `cyanotype migrate` (a one-shot compose step). Services never migrate on boot.

## Layout

```
apps/auth      cyanotype-auth     :3001  register / login / logout / me
apps/assets    cyanotype-assets   :3002  presigned upload flow, asset reads
apps/worker    cyanotype-worker          metadata + thumbnail jobs
apps/cli       cyanotype (cyanotype-cli) `migrate`, `login`, `import`, `status`
apps/web       cyanotype-web      :8080  React + Vite UI (login, library grid, lightbox), served by Caddy
crates/cyanotype-core      config, db, storage, job queue, telemetry (logging)
crates/cyanotype-service   shared HTTP plumbing: AuthUser extractor, ApiError, request id + request logging
infra/caddy    gateway: Caddyfile (local :3000), Caddyfile.prod (real hostname, automatic HTTPS), shared gateway.caddy
infra/backup   nightly Postgres backup image (pg_dump + rclone -> bucket)
```

## Status

Milestone 0/1 scaffold: workspace, migrations, auth (argon2 + HttpOnly session cookies + API keys),
and the presigned multipart upload flow. Milestone 2 in progress: the worker handles metadata and
thumbnails for JPEG/PNG/WebP. The first web UI slice (sign in, photo grid, lightbox) is in `apps/web`;
uploads still go through the CLI. See the plan in `docs/` for what comes next.

## Web UI

`apps/web` is a React + TypeScript + Vite single-page app (pnpm, React Router, plain CSS, Prussian-blue accent
with light and dark themes). It is served from the same origin as the API: the gateway sends `/auth/*`,
`/assets/*` and `/healthz` to the APIs and everything else to the `web` container, so the session cookie
needs no CORS. SPA routes (`/login`, `/register`, `/library`) therefore must not start with `/auth` or `/assets`.

```sh
just up        # the stack (gateway on :3000)
just web-dev   # Vite dev server on :5173, proxying /auth, /assets, /healthz to :3000
just web-check # tsc + eslint + vitest
just web-build # production build into apps/web/dist
```

See `docs/build.md` for the image and the API client.

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
`images` (build all five images), `backup-now`, `deploy`, `datadog-up`, `clean-docker`.

When running from source, call the services directly (`:3001` for `/auth/*`, `:3002` for `/assets/*`)
unless you run them in compose. Images are per-service Dockerfile targets (`auth|assets|worker|cli`) built
from one shared builder; see [`docs/build.md`](docs/build.md).

Logs are JSON on stdout (`LOG_FORMAT=pretty` for local reading). Datadog Agent collection is an opt-in
compose profile; see [`docs/observability.md`](docs/observability.md).

### Upload flow

```
POST /assets/check     {sha256, size_bytes, filename, content_type}
  -> {status: "exists", asset_id}                      # dedupe hit
  -> {status: "upload", upload_id, part_size, parts: [{number, url}], uploaded: [{number, etag}]}
     # `parts` still to PUT; `uploaded` = parts an interrupted earlier attempt already stored (resume)
     # optional request field: file_modified_at (RFC 3339), the timeline fallback when EXIF has no capture time
PUT  <each part url>   (raw bytes, straight to S3; keep the ETag headers)
POST /assets/complete  {upload_id, parts: [{number, etag}]}
  -> {asset_id}        # row inserted, metadata + thumbnail jobs enqueued
GET  /assets?limit=&cursor=<opaque>
  -> {items: [...], next_cursor: <opaque>|null, next_before: <rfc3339>|null}
     # keyset pages in (sort_time DESC, id DESC) order: pass next_cursor as `cursor`; no gaps or repeats when
     # assets share a sort time. `before` / `next_before` are DEPRECATED (strict timestamp, can skip ties)
     # and kept only for older web builds. An invalid cursor is a 400.
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
`Authorization: Bearer <api key>` instead. Security knobs (all in [`docs/deploy.md`](docs/deploy.md)):
`REGISTRATION_MODE=open|first_user|closed` (default `first_user`: only the first account can register),
`COOKIE_SECURE` (default true; local compose sets false), and a per-IP rate limit on login/register (429).

## Importing a library (CLI)

Browser uploads are blocked on some S3 providers (no bucket CORS), so a library is loaded with the CLI:

```sh
cyanotype login --server https://photos.example.com     # email + password -> stores an API key (mode 600)
cyanotype import --dry-run ~/Pictures                    # preview
cyanotype import ~/Pictures                              # resumable; re-run to continue or to top up
cyanotype status
```

It walks directories, skips sidecars and junk, hashes (cached), lets the server dedupe, and streams the rest to
S3 in parallel parts. JPEG, PNG, WebP, HEIC, AVIF, TIFF, DNG, the common RAW formats and mov/mp4/m4v are
imported as originals; thumbnails for HEIC/RAW arrive with the worker's decoder support. API keys are managed
at `/auth/api-keys` (session only) and in the web UI. Full guide: [`docs/import.md`](docs/import.md);
`just import <paths>` runs it from source.

## Deploying

One Linode VPS with `docker compose`: `docker-compose.prod.yml` (GHCR images, automatic HTTPS, memory limits,
nightly backups to the bucket), a publish workflow that pushes images to GHCR, and a deploy workflow plus
`just deploy`. The full runbook, backups/restore and the go-live checklist are in [`docs/deploy.md`](docs/deploy.md).

## Licence

AGPL-3.0-or-later. See `LICENSE`.
