# Importing a library with the CLI

Browser uploads are not possible against some S3 providers (Linode Object Storage in us-ord-10 has no bucket
CORS), so `cyanotype import` is the way to load photos. It streams files from disk straight to the presigned
S3 URLs; bytes never pass through the API. It is built for big libraries (100k+ files, hundreds of GB): it
is resumable, idempotent and uses a small, fixed amount of memory.

## 1. Install

```sh
cargo install --path apps/cli --locked      # installs the `cyanotype` binary
# or from a checkout: cargo run --release -p cyanotype-cli -- <command>   (also: `just import <paths>`)
```

## 2. Create an API key and log in

Either let the CLI do it with your email and password:

```sh
cyanotype login --server https://photos-dev.cyanotype.app
# Email: you@example.com
# Password: ********        (use --password-stdin or CYANOTYPE_PASSWORD for scripts)
```

This logs in, asks the server for a new API key (named `cli on <hostname>`, or `--name`), throws the session
away and stores the key. Or create the key yourself in the web UI (Settings, "API keys": the key is shown
once, copy it) or with `POST /auth/api-keys {"name": "..."}` while logged in, then:

```sh
cyanotype login --server https://photos-dev.cyanotype.app --api-key cyk_...
```

The server URL and key are written to `$XDG_CONFIG_HOME/cyanotype/config.toml` (default
`~/.config/cyanotype/config.toml`, mode 600). `CYANOTYPE_SERVER` and `CYANOTYPE_API_KEY` override the file,
which is handy in CI. `cyanotype status` shows the server, the account and the asset count. Revoke a key
(Settings, "API keys", or `DELETE /auth/api-keys/{id}`) and it stops working immediately. API keys cannot
create, list or revoke keys; only a login session can.

## 3. Import

```sh
cyanotype import ~/Pictures/2024 /mnt/card/DCIM            # directories are walked recursively
cyanotype import --dry-run ~/Pictures                       # what would be sent; no login, no network
cyanotype import --jobs 8 --part-jobs 4 ~/Pictures          # more parallelism on a fat pipe
cyanotype import -v --exclude 'Exports/**' --include '*.cr3' ~/Pictures
```

What is picked up:

| | |
|---|---|
| Images | jpg jpeg png webp heic heif avif tif tiff dng |
| RAW | cr2 cr3 nef nrw arw srf sr2 raf orf rw2 pef srw x3f 3fr iiq rwl |
| Videos | mov mp4 m4v (as originals only), unless `--no-videos` / `--videos=false` |
| Never | sidecars (`.xmp`, `.aae`, `.thm`), dotfiles and dot-directories, OS junk (`Thumbs.db`, `desktop.ini`, `__MACOSX`, `$RECYCLE.BIN`, `@eaDir`, ...), empty files, anything else |

Videos are on by default because the goal is a complete library: phones write Live Photo clips and camera
cards hold video next to the stills, and leaving them behind makes the import look lossy. They are stored as
originals; the worker does not make thumbnails for them yet. Symbolic links are skipped unless
`--follow-symlinks`. Extensions are matched case-insensitively.

`--include GLOB` narrows the selection (a file must match at least one); `--exclude GLOB` removes files and
prunes directories. Globs are case-insensitive and are tested against the file name, the path relative to the
directory you gave, and the full path (`*.cr3`, `Exports/**`, `Screenshots`).

For each file the CLI computes the sha256 (streamed, 1 MiB at a time), asks the server (`POST /assets/check`)
and skips it if the server already has that content. Otherwise it uploads the file in 16 MiB parts to the
presigned URLs and finishes with `POST /assets/complete`. The stored original keeps its filename, and the
file's modification time is sent as a hint (`file_modified_at`); the timeline uses it when the photo has no
EXIF capture time. Implausible mtimes (before 1990, or in the future) are not sent.

Parallelism: `--jobs` files are uploaded at once (default 4) and `--part-jobs` parts of each file (default 4);
hashing and checking run ahead of the uploads. Parts are streamed from disk by byte range in 256 KiB chunks,
so memory use is roughly `jobs x part-jobs x 256 KiB` plus the file list, whatever the file size. Network
errors, 5xx and 429 are retried with exponential backoff (0.5 s up to 30 s, five or six attempts); a refused or
expired presigned URL triggers a fresh `check`, which hands back new URLs.

Progress is a bar on stderr (files, bytes, rate, ETA; hidden when stderr is not a terminal; `-v` prints a line
per file). At the end you get a summary: uploaded, already present, failed (with reasons) and what was skipped
locally and why. The exit status is non-zero if anything failed, and the failed paths with reasons are written
to `cyanotype-import-failures.txt` (`--failures-file`).

## Resuming and re-running

Run the same command again. It is idempotent, and only what is missing is uploaded:

- **Hash cache.** `(absolute path, size, mtime) -> sha256` is kept in `$XDG_CACHE_HOME/cyanotype/hashes.jsonl`
  (append-only JSON lines; `--cache-file` to move it, `--rehash` to ignore it). Resuming a 100k-file library
  re-reads nothing that is unchanged, so a full re-run is one `check` request per file. Delete the file at any
  time; the cost is a re-hash.
- **Server dedupe.** Files are identified by content hash, so renamed or duplicated files are sent once, and
  files already in the library are skipped (`exists`).
- **Interrupted uploads.** `Ctrl-C` once stops starting new work and lets in-flight parts finish; twice quits
  at once; `kill -9` is fine too. The server keeps the multipart upload and its stored parts; the next `check`
  for the same content resumes that upload: it returns the parts already stored (verified by size) plus fresh
  URLs for the rest, so only the missing parts are sent. Uploads idle for more than 7 days are aborted by a
  sweep in the assets service (which frees the storage that unfinished parts would keep billing for).

## RAW and HEIC

Everything is imported as an immutable original with a vendor-specific MIME type (for example
`image/x-canon-cr3`, `image/x-nikon-nef`, `image/heic`; unknown extensions fall back to
`application/octet-stream`). Thumbnails, previews and metadata depend on the worker: today it processes JPEG,
PNG and WebP; HEIC and RAW decoding is landing in a separate change. Files the worker cannot decode are kept as
originals and simply have no thumbnail yet. Once that support lands, queue the metadata and thumbnail jobs
again for the existing assets; nothing needs to be re-uploaded.

## Server-side details

- `POST /assets/check` accepts an optional `file_modified_at` (RFC 3339), stored on the upload and then the
  asset. The timeline orders by `coalesce(taken_at, file_modified_at, created_at)`.
- Its `upload` reply carries `uploaded: [{number, etag}]` (parts to reuse) next to `parts` (parts to send).
  `POST /assets/complete` takes every part, old and new.
- `GET /assets/count` returns `{"count": N}` (used by `cyanotype status`).
- API keys: `POST /auth/api-keys {name}` returns `{id, name, prefix, created_at, key}` once; the key is
  `cyk_` plus 64 hex characters (32 random bytes) and only its sha256 is stored. `GET /auth/api-keys` lists
  `{id, name, prefix, created_at, last_used_at}`; `DELETE /auth/api-keys/{id}` revokes. All three require a
  login session (cookie), never an API key.

## Troubleshooting

- `unauthorized`: the key was revoked or the server is wrong; run `cyanotype login` again.
- `storage refused the part ... SignatureDoesNotMatch` / repeated `403`: the presigned URLs are for a host the
  CLI cannot use as signed. The server's `S3_PUBLIC_ENDPOINT` must be the address clients reach.
- Cloudflare in front of the API: the API calls are small; the part uploads go straight to object storage and
  do not pass through it.
- Slow first run: it reads every byte once to hash it. Later runs use the cache.
