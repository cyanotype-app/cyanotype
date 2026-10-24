# Importing a library with the CLI

`cyanotype import` is the way to load a large library (the web UI also uploads, see "Browser upload" below).
It streams files from disk straight to the presigned S3 URLs; bytes never pass through the API. It is built for big libraries (100k+ files, hundreds of GB): it
is resumable, idempotent and uses a small, fixed amount of memory.

## 1. Install

Your Cyanotype server hands out the program itself. On macOS and Linux (x86_64 and aarch64):

```sh
curl -fsSL https://photos.example.com/cli/install.sh | sh                  # ~/.local/bin
curl -fsSL https://photos.example.com/cli/install.sh | sh -s -- --system   # /usr/local/bin (asks for sudo if needed)
```

On Windows (PowerShell, 64-bit):

```powershell
irm https://photos.example.com/cli/install.ps1 | iex
```

Use your own server's address (the page Settings, "API keys", shows these commands for your server and your OS). The
script picks the right build for your system, downloads it and `SHA256SUMS` from the same server, checks the checksum (and
the signature when `ssh-keygen` is installed), installs it, warns if the folder is not on your `PATH` and prints the next
step, `cyanotype login <your server>`. It changes nothing else.

Keep it current with `cyanotype update` (`--check` only reports; `--version X` installs a version the server offers). The
CLI also mentions a newer version at most once a day after `login`, `import` and `status`
(`CYANOTYPE_NO_UPDATE_CHECK=1` turns that off), and `import` refuses a CLI older than the server's minimum with a message
saying to update. How the downloads are built and signed: [`releases.md`](releases.md).

From source (a developer, or a platform without a build):

```sh
cargo install --path apps/cli --locked      # installs the `cyanotype` binary
# or from a checkout: cargo run --release -p cyanotype-cli -- <command>   (also: `just import <paths>`)
```

A source build includes the operator commands (`migrate`, `reindex`, `manifest`) that the downloadable build leaves out,
and reports itself as a development build (`0.0.1-dev`), which `cyanotype update` will not overwrite.

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

### Key scopes and expiry

Every API key has a **scope**, and optionally an expiry (Settings > API keys, or `POST /auth/api-keys {name, scope, expires_in_days}`):

| Scope | May do |
|---|---|
| `import` | Upload and see status: `/assets/check`, the upload parts, `/assets/complete`, `/assets/count`, `/assets/processing`, `/auth/me`. Cannot list, view, download, change or delete anything. |
| `read` | Everything a `GET` reaches (library, search, downloads). Cannot upload or change anything. |
| `full` | Everything a session can do except what is session-only: managing keys, two-factor authentication, sessions and the whole admin portal (those refuse every key). |

`cyanotype login` makes an **`import`** key by default; ask for another with `--scope read` or `--scope full`. Keys that existed before
scopes are `full` with no expiry. An expired key is refused like a revoked one (401) and is listed as expired. The scope is checked
by the server on every route (a key with too small a scope gets `403` with `code: insufficient_scope`), not by the client. If your
account uses two-factor authentication, `cyanotype login` asks for the code after the password (`--code`, or a recovery code).

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

## Account limits

An administrator may give an account a storage quota (the trash counts until it is emptied), a daily upload cap and a largest
file ([`admin.md`](admin.md), "Limits"). The import honours them without retrying forever:

- **Quota or daily cap reached** (HTTP 413 `quota_exceeded`, 429 `daily_cap_exceeded`): the import stops starting new files (parts
  already on their way finish), counts the rest as "not started" and prints why, in the server's words, plus what to do. For the
  daily cap it says when uploads can resume (`Run the same command again after 2026-06-02T03:04:05Z ...`); for the quota, to free up
  space or ask the administrator. The exit status is non-zero. Run the same command again later: files already uploaded are
  recognised and skipped, so it carries on where it stopped. A 429 that is *not* a limit (busy server) is still retried with backoff.
- **A file over the largest-file limit** (413 `file_too_large`, or `file_exceeds_daily_cap`): that file is reported as failed with
  the reason and the import goes on with the others.

## RAW and HEIC

Everything is imported as an immutable original with a vendor-specific MIME type (for example
`image/x-canon-cr3`, `image/x-nikon-nef`, `image/heic`; unknown extensions fall back to
`application/octet-stream`). Thumbnails, previews and metadata depend on the worker: today it processes JPEG,
PNG and WebP; HEIC and RAW decoding is landing in a separate change. Files the worker cannot decode are kept as
originals and simply have no thumbnail yet. Once that support lands, queue the metadata and thumbnail jobs
again for the existing assets; nothing needs to be re-uploaded.

## Browser upload (temporary proxy)

The library page has **Upload photos** and **Upload folder** buttons, and accepts files or folders dropped
anywhere on the page. Browsers cannot PUT to the bucket cross-origin (Linode Object Storage in us-ord-10
answers 501 to `PutBucketCors`), so the web UI sends each part **through the assets service**:

1. The browser hashes the file (SHA-256, incrementally, in a Web Worker with `hash-wasm`, so a 100 MB RAW
   does not freeze the page or sit in memory) and calls `POST /assets/check` with `"transport": "proxy"` and
   `file_modified_at` from `File.lastModified`. `transport` defaults to `"direct"` (presigned S3 URLs), which is
   what the CLI uses and sees no change.
2. With `proxy`, the response has the same shape, but each `parts[].url` is a same-origin path,
   `/assets/uploads/{upload_id}/parts/{n}` (the upload id is percent-encoded). The resume path honours
   `transport` too, so `uploaded` is unchanged.
3. The browser `PUT`s each missing 16 MiB part (`File.slice`) to that URL with the session cookie or an API key,
   `Content-Length` set and the body being the raw bytes. The service checks the upload belongs to the caller
   (404 otherwise, same as unknown), `1 <= n <= n_parts`, and that `Content-Length` equals the expected size of
   that part (400 if missing or different; 413 if above the part size), relays the part to S3 `UploadPart` and
   answers `{"etag": "..."}` (502 if S3 fails). Then `POST /assets/complete` as before.

Files are filtered by extension, not MIME type (browsers report an empty type for RAW): jpg, jpeg, png, webp,
heic, heif, avif, dng, cr2, cr3, nef, nrw, arw, srf, sr2, raf, orf, rw2, rwl, pef, srw, 3fr, iiq, erf, kdc,
mrw, x3f, crw. Hidden and OS files (`.DS_Store`, `._*`, `Thumbs.db`, `__MACOSX`) are skipped. The queue runs
2 files at a time with 3 parts in flight overall, retries a failed part up to 4 times with backoff (network
errors, 408/429/5xx), shows per-file status and an overall summary in a tray that survives page navigation
within the app, and asks for confirmation if you close the tab mid-upload. Closing it anyway is safe: opening the
same files later resumes from the parts S3 already has.

**Memory bound.** A part is buffered in the assets service before `UploadPart` (a replayable body the SDK can
sign and retry; streaming a non-replayable body to S3 is awkward over plain http). A process-wide semaphore
allows 3 proxied parts at once, so the proxy path holds at most 3 x 16 MiB = 48 MiB, well inside the assets
container's `mem_limit: 256m`. Further parts wait for a permit before their body is read, and a part body must
arrive within 120 s once it has one.

**Limits.** The gateway sets no request body limit for `/assets` and does not buffer request bodies. Cloudflare's
free plan caps request bodies at 100 MB, so 16 MiB parts are fine behind it. Throughput is bounded by the
assets service; for big archives use the CLI.

**Temporary.** The proxy exists only because the bucket cannot get a CORS policy. Once storage moves to a
CORS-capable bucket, the web UI can switch to `"transport": "direct"` and PUT presigned URLs itself; the proxy
route can then be removed.

## Moving from Google Photos

Google Takeout gives you your photos as `.zip` files, each photo next to a small JSON "sidecar" with what Google Photos knew about it
(when it was taken, where, its caption, whether it is a favorite) and one folder per album. Cyanotype imports the photos and applies
that metadata, from the browser or the command line.

**Get the export.** At takeout.google.com choose only *Google Photos* (all albums), export as `.zip`, download every part and
**unzip all of them into one folder** (the parts of one export belong together: a photo and its sidecar can land in different zips).

**In the browser** (Settings > Google Photos): choose the unzipped folder (or drop it on the page). The browser sends only the
*names* of the files to the server, which pairs each photo with its sidecar and says what would happen: how many photos, how many have
their details, which albums would be made, what is skipped (videos, since Cyanotype does not import video; the export's trash folder;
other files), with two options, **Import favorites** and **Create albums from folders**. Nothing is uploaded until you press Import.
Then the photos go up with the ordinary resumable upload (same hash check: a photo already in the library is not uploaded again).
Dropping the `.zip` files themselves is **not supported**: reading a multi-gigabyte zip piece by piece in a browser needs a streaming
unzip library, and the unzipped folder works everywhere; the page says so if you try.

**On the command line:** `cyanotype import --takeout <folder>` (`--takeout-no-albums`, `--takeout-no-favorites`, `--dry-run`). It
uses the same matcher, so both give the same answer. An `import`-scoped key is enough.

### What is applied

| Takeout says | Cyanotype does |
|---|---|
| `photoTakenTime` | the capture time, as an **edit** over the file's own EXIF: revertible with "Revert to original", the file's value kept beside it, written to the bucket like any edit (`docs/library.md`, "Editing a photo"). Not when the two differ only by a time zone (a camera's local time without a zone against Google's true instant: replacing it would shift the displayed time by hours), and never over a date you edited yourself. |
| `geoData` (else `geoDataExif`) | the location, as an edit: when the file has none, or Google Photos has it more than about 100 m elsewhere (you placed it there). `0, 0` means no location and is ignored. |
| `description` | the caption, when the photo has none. |
| `favorited` | a favorite (only ever turned on). |
| the folder | an album with the folder's name (found by title, or made). Takeout repeats a photo in its year folder and in each album: it ends up in every album it was in. Folders named `Photos from 2019` (and their translations) are not albums; neither are `Archive` or the export's own folders. |
| `archived` | the photo is archived (off the timeline and map, still in albums and search, with its paired RAW), as the Archive action does. Only ever turned on. |
| `trashed`, the Trash folder | not imported. |

### How sidecars are matched

By name, in the folder of the photo, with the rules Takeout is known to follow (checked against the current export format and its
reported oddities: [ente#4953](https://github.com/ente/ente/issues/4953),
[GooglePhotosTakeoutHelper#353](https://github.com/TheLastGimbus/GooglePhotosTakeoutHelper/issues/353),
[ExifTool forum](https://exiftool.org/forum/index.php?topic=17536.0)):

- `IMG_1234.JPG` has `IMG_1234.JPG.json`, or since late 2024 `IMG_1234.JPG.supplemental-metadata.json` (the newer one wins if both exist).
- The sidecar's name is cut to **46 characters** before `.json`: that can bite into `.supplemental-metadata` (`IMG.jpg.supplemen.json`,
  `IMG.jpg.s.json`, even `IMG.jpg..json`) or into a long photo name itself (the sidecar is then the first 46 characters of the name).
  Counted in characters, not bytes.
- A second file with the same name is `IMG_1234(1).JPG`, whose sidecar is `IMG_1234.JPG(1).json` (or `...supplemental-metadata(1).json`):
  the counter moves behind the extension. A counter is never matched to the sidecar without one.
- An edited copy `IMG_1234-edited.jpg` (also `-bearbeitet`, `-modifié`, `-editado` and other languages) has no sidecar and shares the
  original's, including when the original had another extension (an edited HEIC is exported as a JPEG) and no other file could be it.
- A different letter case between the photo and its sidecar still matches.
- `metadata.json`, `print-subscriptions.json`, `shared_album_comments.json` and `user-generated-memory-titles.json` are not sidecars.

The matcher is one Rust crate (`crates/cyanotype-takeout`) used by both the command line and the server (the browser asks the server
for the pairing with `POST /assets/takeout/plan`, names only, nothing stored). One implementation, because the rules are the part that
goes wrong and a second copy in another language would drift; the cost is one request per 100,000 names. The cases are in a table
test (`names.rs`), plus a synthetic export that the browser test uploads. A photo whose sidecar cannot be found imports as it is.
Not done: matching by the `title` field inside a sidecar when the names do not match (it would mean reading every sidecar before the
preview).

### How the metadata gets applied (and why closing the tab is safe)

The sidecar's text and the album go with the photo's `POST /assets/check` (`takeout` field: `{sidecar, album, favorites}`; an
`import`-scoped key may send it). The server **holds it by the photo's content hash** (`takeout_imports`, migration 0022) before
anything else happens, and a `takeout` job applies it **after the photo's metadata job has read the file's own EXIF**, so the edit is
made over the real values. That is why a closed tab, a stopped `cyanotype import` or a slow worker cannot lose it, and why it works for
photos that were already in the library (the check call queues the job at once). A sweep once a minute catches photos whose own
processing failed; metadata for a photo that never arrives is dropped after 30 days. Two copies of one photo (year folder and album)
merge: albums are added, the first readable sidecar stays.

**Re-running is idempotent.** The same export again uploads nothing new, creates no second album and changes nothing already applied;
changed metadata is applied once. Nothing is ever removed (a photo moved to another folder is in both albums).

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
