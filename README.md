# Cyanotype

An open-source, self-hosted photo library. S3 is the source of truth, Postgres is the
only other dependency, and the whole backend is two stateless Rust binaries.

- **Originals are immutable and content-addressed** under `originals/{user}/{hash[0:2]}/{hash}.{ext}`.
  The database can be rebuilt by re-scanning the bucket.
- **Bytes never touch the API.** Clients upload and download straight to S3 with presigned URLs.
- **`cyanotype-api`** serves HTTP; **`cyanotype-worker`** runs the processing pipeline off a
  Postgres job queue (`FOR UPDATE SKIP LOCKED`). Both scale horizontally and are safe to kill.

## Status

Milestone 0/1 scaffold: workspace, migrations, auth (argon2 + HttpOnly session cookies + API keys),
and the presigned multipart upload flow. See the plan in `docs/` for what comes next.

## Development

```sh
docker compose up -d            # Postgres (pgvector) + MinIO, bucket created with versioning on
cp .env.example .env
set -a; . ./.env; set +a
cargo run -p cyanotype-cli -- migrate
cargo run -p cyanotype-api      # http://localhost:3000
cargo run -p cyanotype-worker
```

### Upload flow

```
POST /assets/check     {sha256, size_bytes, filename, content_type}
  -> {status: "exists", asset_id}                      # dedupe hit
  -> {status: "upload", upload_id, part_size, parts: [{number, url}]}
PUT  <each part url>   (raw bytes, straight to S3; keep the ETag headers)
POST /assets/complete  {upload_id, parts: [{number, etag}]}
  -> {asset_id}        # row inserted, metadata + thumbnail jobs enqueued
```

Auth: `POST /auth/register` / `POST /auth/login` set a session cookie; CLIs send
`Authorization: Bearer <api key>` instead.

## Licence

AGPL-3.0-or-later. See `LICENSE`.
