# Build and automation

## Layout

One `Dockerfile`, one shared compile:

```
chef      rust + cargo-chef + C/C++ toolchain (build-essential, cmake; libwebp and libheif are built from source
          and linked statically) + libde265-dev, libdav1d-dev (the HEVC and AV1 decoders libheif calls)
planner   cargo chef prepare   -> recipe.json (changes only when manifests / Cargo.lock change)
builder   cargo chef cook      -> dependencies (cached layer)
          cargo build --release --workspace --locked   -> all four binaries, compiled once
clip-vision, clip-text   the CLIP model files (scripts/fetch-models.sh, sha256 pinned in models/clip.lock): image tower for the worker, text tower + tokenizer for assets
runtime   debian:trixie-slim, ca-certificates, non-root user
auth | assets | worker | cli   runtime + a single binary each (the --target); worker also gets libde265-0, libdav1d7, libjpeg62-turbo, libopenjp2-7 and the image tower; assets gets the text tower
```

`docker buildx bake` (group `default` in `docker-bake.hcl`) builds the four targets together; BuildKit
sees the same `builder` stage in all of them and runs it once. `docker compose build` uses the same
`target:` per service, so it shares the stage too. Previously each image compiled the whole workspace
in parallel, which made a cold build take about 25 minutes.

## Rust toolchain pin

`rust-toolchain.toml` pins the exact Rust version (rustfmt + clippy, minimal profile) for local builds and CI
(`actions-rust-lang/setup-rust-toolchain` reads it), so a new Rust release cannot break an unrelated PR with new
lints. The `Dockerfile` pins the matching `RUST_IMAGE` tag (cargo-chef on trixie); `.dockerignore` keeps
`rust-toolchain.toml` out of the image build context, and CI (`scripts/check-rust-pin.sh`) fails if the two versions
differ. The workspace `rust-version` (MSRV) is separate and unchanged.

To bump: edit `channel` in `rust-toolchain.toml` and the `RUST_IMAGE` tag in the `Dockerfile` together (check the tag
exists, e.g. `docker manifest inspect lukemathwalker/cargo-chef:<tag>`), run `just check`, and fix any new lints in
the same PR.

## Worker system dependencies (HEIC/AVIF and RAW)

HEIC/HEIF/AVIF decoding uses libheif through `libheif-rs` with its `embedded-libheif` feature: the
libheif sources vendored in the `libheif-sys` crate are compiled by cmake during `cargo build` and linked
**statically**. The version is therefore pinned by `Cargo.lock` (`libheif-rs =3.0.0` -> `libheif-sys
5.3.1+1.23.1` = **libheif 1.23.1**; a unit test asserts it). Why this and not Debian's libheif:

- Debian ships an older libheif (bookworm 1.15.1, trixie 1.19), older than the `libheif-rs` API we use (1.17+), and the system library
  would put the HEIC decoder behind a moving apt version and separate `libheif-plugin-*` packages.
- One version everywhere: developer machines, CI and the image build the same libheif.
- The runtime image needs **no libheif packages and no plugins** (plugin loading is compiled out). It
  only needs the codec libraries libheif links dynamically: `libde265-0` (HEVC, ~0.5 MB) and
  `libdav1d7` (AV1/AVIF, ~0.7 MB), plus `libjpeg62-turbo` and `libopenjp2-7` (decoders libheif picks up from the builder image). The worker binary grows by about 2 MB for libheif.

Build-time needs: `cmake`, a C++ compiler, `libde265-dev`, `libdav1d-dev` (and `pkg-config`). On Debian or
Ubuntu: `sudo apt-get install cmake pkg-config libde265-dev libdav1d-dev`. The first libheif build takes a
few minutes; the cargo-chef `cook` layer and CI's rust-cache keep it out of the normal loop. CI installs
the same packages in the Rust job (`.github/workflows/ci.yml`).

HEVC (HEIC) is patent-encumbered in some jurisdictions; libde265 is LGPL-3. Both are dynamically linked
and unmodified.

RAW decoding uses `rawler` (pure Rust, from dnglab, no system dependencies). No `exiftool` or other
subprocess is shipped; see "Supported formats" below for what that means in practice.

### Supported formats and how they are handled

| Kind | Detected by | Metadata | Thumbnail and preview |
|---|---|---|---|
| JPEG | magic bytes | EXIF via kamadak-exif; size from the header | DCT-scaled decode: the IDCT produces 1/2, 1/4 or 1/8 of the size (the smallest that still covers the 1440 px preview), then resize and orient by EXIF. Images too small to benefit are decoded whole. |
| PNG | magic bytes | EXIF via kamadak-exif; size from the header | rows are decoded one at a time and averaged down as they arrive (interlaced PNGs are decoded whole), then resize and orient by EXIF |
| WebP | magic bytes | EXIF via kamadak-exif; size from the header | full decode, resize, orient by EXIF (the decoder has no row or scaled interface) |
| HEIC / HEIF / AVIF | `ftyp` brands (`heic`, `mif1`, `avif`, ...) | EXIF from the HEIF Exif item; size is the display size | grid images (iPhone and most phones: independently coded tiles, usually 512x512) are decoded tile by tile through libheif's tile API and averaged down as they arrive; anything else (single-image HEIC, AVIF, images with alpha) is decoded whole by libheif and read straight from its plane. irot/imir are applied by libheif; the EXIF orientation is ignored (never applied twice). 10/12-bit is down-converted. HDR gain maps are ignored (the SDR base image is used). Only the primary image is decoded (no burst or sequence frames). |
| RAW (CR2, CR3, NEF, NRW, ARW, SRF/SR2, RAF, ORF, RW2/RWL, PEF, DNG, SRW, 3FR, IIQ, ERF, KDC, MRW, X3F, CRW, ...) | CR3 `ftyp crx`, RAF/CRW/MRW/X3F/RW2/ORF magics, TIFF magic with a non-.tif name | EXIF from the TIFF container, else from the embedded JPEG (RAF, RW2), topped up from rawler (CR3 and others). Size is the real sensor size (rawler, else RW2 sensor tags, else the camera's EXIF dimensions, else the largest embedded JPEG), never the preview's | the embedded full-size JPEG preview (found by scanning for well-formed JPEG streams, so it works across vendors), oriented once. Only if there is no usable preview: rawler's develop pipeline, pixel-capped (see below). |

Routing is by content: the worker downloads the original and sniffs it, so an
`application/octet-stream` upload of a `.cr3` works. Clearly unrelated content types (video, PDF, ...)
are skipped without a download; a file that is not a supported image after sniffing is skipped too (job
marked done, not failed). Corrupt or unsupported-but-recognised files (a RAW from a camera rawler does not
know and with no JPEG preview, say) fail permanently with a clear message, never a retry loop.

Known limits: HEIC/AVIF colour profiles (Display P3 and friends) are not converted, so wide-gamut photos
look slightly flat; gain-map HDR is not applied; RAW embedded previews are used as shot (no
colour management and no crop). For cameras that rawler does not know, orientation comes from the EXIF in
the file and the preview is trusted as is.

### Memory budget

The container limit for the worker is `mem_limit: 1280m` (`docker-compose.prod.yml`; 1024m before semantic search, which
adds the CLIP image tower: about 150 MB resident while its session is loaded, outside the budget below, so the limit rose by
256 MB, see "Semantic search model"). Two mechanisms keep
decoding inside it, and a job that cannot fit is **refused with a permanent failure** (`jobs.last_error`
says why), not attempted:

1. `WORKER_HEAVY_CONCURRENCY` (default 2) bounds how many jobs run.
2. `WORKER_MEMORY_BUDGET_MB` (default 640) is a byte budget shared by all jobs (`apps/worker/src/budget.rs`).
   A job first reserves `2 x original + 48 MB` while it downloads (the S3 body is buffered and then
   copied into one buffer), then keeps `original + 48 MB` (3x the original for a RAW, whose header read can
   transiently decompress sensor data), and the thumbnail job then reserves
   `+ estimated decode peak` once the header says how big the image is. Requests queue first come first
   served. A job whose own need exceeds the whole budget fails permanently.

The decode estimate depends on the path (`apps/worker/src/media.rs` picks it; the `jobs.decode_path` metric says which ran),
and is measured with `apps/worker/src/memcal.rs`
(`CYANOTYPE_MEMCAL_FILE=<file> cargo test -p cyanotype-worker --release memcal -- --nocapture`, one process per file;
`CYANOTYPE_MEMGEN=...` writes synthetic JPEGs and HEIC grids, see the test's doc comment):

| Path | Estimate | Measured (peak RSS growth) |
|---|---|---|
| JPEG, baseline, DCT-scaled | scaled pixels x (3 + sampled components) + 24 MB, i.e. 30-40 MB for any size | 50 MP: 26 MB; 100 MP: 21 MB; 200 MP: 29 MB |
| JPEG, progressive, DCT-scaled | the above + 2.3 B per source pixel per sample (4:2:0: 3.5 B/px) for the coefficients | 50 MP: 160 MB; 100 MP: 295 MB |
| JPEG, too small to scale (long edge < 2880 px), WebP, interlaced PNG, PNG < ~4000 px | 5 B/px RGB8, 12 B/px RGBA8 | RGB8 3.5-4 B/px, RGBA8 11 B/px |
| PNG, streamed | output buffer (about 1.5-2.9k px on the long edge, 12 B/px) + 24 MB | 50 MP: 30 MB |
| HEIC grid, tile by tile | output buffer + 24 B per tile pixel + 40 MB, i.e. 65-85 MB for any size | 48 MP: 35 MB; 100 MP: 36 MB; 200 MP: 48 MB |
| HEIC / AVIF single image (no grid), 8-bit | 6 B/px + output buffer | 50 MP: 238 MB (4.2 B/px + buffer) |
| HEIC / AVIF single image, 10-bit+ | 9 B/px + output buffer | 20 MP: 151 MB |
| HEIC / AVIF with alpha | as single image (alpha is decoded whole) | |
| RAW embedded preview | 6 B/px of the *preview* | 6 B/px (12 MP preview: 73 MB) |
| RAW develop (rawler) | 48 B/px of the sensor | 36-43 B/px (4 MP: 179 MB) |

Before and after on the same synthetic files (release build, this host; time is the whole thumbnail + preview +
ThumbHash render, one thread):

| File | Before: peak / time | After: peak / time |
|---|---|---|
| JPEG 50 MP baseline (8000x6250) | 178 MB / 0.67 s | 26 MB / 0.37 s |
| JPEG 50 MP progressive | 296 MB / 0.90 s | 160 MB / 0.64 s |
| JPEG 100 MP baseline (12500x8000) | 336 MB / 1.13 s | 21 MB / 0.51 s |
| JPEG 100 MP progressive | 590 MB / 1.78 s | 294 MB / 0.99 s |
| JPEG 200 MP baseline (16000x12500) | refused by the decoder (allocation limit) | 29 MB / 0.91 s |
| HEIC 48 MP grid (8064x6048, 512 px tiles) | 285 MB / 4.98 s | 35 MB / 3.39 s |
| HEIC 48 MP grid, rotated | 285 MB / 5.06 s | 34 MB / 3.11 s |
| HEIC 100 MP grid | 589 MB / 9.1 s | 36 MB / 6.8 s |
| HEIC 200 MP grid | 1168 MB / 18.1 s | 48 MB / 13.1 s |
| HEIC 50 MP single image | 334 MB / 3.7 s | 238 MB / 3.6 s |
| HEIC 20 MP single image, 10-bit | 150 MB / 2.8 s | 151 MB / 3.0 s |
| PNG 50 MP RGB8 (8000x6250, 95 MB file) | 174 MB / 1.06 s | 30 MB / 2.6 s (test-profile build, not release) |

The synthetic JPEGs are smoother than camera files (about 1 bit per pixel against 2-3), so Huffman decoding time on
real photos is somewhat higher; the memory numbers do not depend on content. `memcal::peak_memory_*` in the worker
tests repeat the JPEG and HEIC grid measurements in a child process and assert both a ceiling and that the estimate
covers the measurement (the large cases run with `--ignored`).

Consequences with the default 640 MB budget and a typical 100 MB original (leaving about 490 MB for the decode):

- **JPEG**: baseline JPEGs have no practical ceiling any more: the worker's pixel guard (400 MP) and the original size
  limit are what stop a file, not memory. Progressive JPEGs hold every coefficient until the last scan, so they top out
  around 150 MP (4:2:0) or 75 MP (4:4:4). Before, all JPEGs stopped at about 95 MP.
- **HEIC grids** (every iPhone photo, 12 or 48 MP): up to libheif's own 200 MP limit, whatever the memory budget.
  Single-image HEIC and AVIF (some Android cameras, Canon HIF, most AVIF) and any HEIC with alpha: about 90 MP 8-bit and
  60 MP 10-bit, up from about 45 and 36 MP.
- **PNG**: non-interlaced PNGs up to the 400 MP guard; interlaced PNGs and WebP about 100 MP (RGB8) and 40 MP (RGBA).
- A 100 MP RAW is fine **if it has an embedded preview** (the worker picks the cheapest preview that is at
  least 1440 px, usually 1.6 MP; a preview that would need more than the budget is never decoded), but with
  no preview it is refused (the develop path needs about 4.8 GB, and is also capped at 150 MP regardless of
  budget, and disabled with `WORKER_RAW_DEVELOP=0`). In practice the develop fallback only runs for sensors
  up to roughly 10 MP.

Why these reductions are safe: the DCT-scaled decode and the box-averaged streams only change *how the 1440 px
preview is reached* (IDCT-domain reduction or an integer box average, then the same Lanczos resize as before); the
preview, thumbnail and ThumbHash come from the same pipeline as before and are compared against the full-decode output in
the tests (PSNR above 30 dB on the synthetic fixtures; the differences are filter differences). The reduced decoders
are only used when their estimate is lower than the plain decode's, and if one fails on a file it cannot read the job
falls back to a full decode when that fits the budget (`jobs.decode_fallbacks`).

One gap is accounted for rather than eliminated: between the first reservation (the original) and the
second (the decode), the job releases and re-reserves instead of topping up in place (topping up could
deadlock two jobs each waiting for the other). While it waits, its downloaded original is briefly
unaccounted. That is bounded by `WORKER_HEAVY_CONCURRENCY` originals, and the 384 MB between the budget
and the container limit absorbs it.

Photos refused before this change ("decoding a ... image needs about N MB, over the worker memory budget") are requeued
by migration `0010_requeue_memory_refusals.sql` the first time it runs, so a deploy reprocesses them. Anything the
migration does not match (originals larger than the budget itself, RAW without a preview) stays failed and is retried by
"Retry failed" in the app or `POST /assets/processing/retry`, which requeues every failed job of the caller.

Remaining follow-ups (`docs/PLAN.md`): streamed WebP and interlaced PNG, decoding HEIC alpha grids by tile, a
reduced-size decode for single-image HEIC (libheif has none), and colour-profile conversion.

## Semantic search model

Search embeds every photo and every query with **OpenAI CLIP ViT-B/32** and compares them by cosine similarity
(`docs/library.md`, "Search"). The model runs in-process, in Rust, with **ONNX Runtime** through the `ort` crate and the
`tokenizers` crate, in `crates/cyanotype-clip`: no Python, no sidecar, no network at runtime.

### Which model, and why

| Candidate | Licence | Verdict |
|---|---|---|
| **OpenAI CLIP ViT-B/32** (ONNX export by Xenova / transformers.js, int8) | code and weights MIT (github.com/openai/CLIP) | **chosen** |
| OpenCLIP ViT-B/32 LAION-2B | MIT | about 3 points better zero-shot (66.6 vs 63.3 % ImageNet) but nobody hosts a quantised ONNX export with a stable URL; quantising it ourselves would mean hosting an artifact we cannot pin by an upstream checksum. The swap path is open: change `MODEL_ID`/`MODEL_VERSION` and `models/clip.lock` |
| SigLIP base/16 | Apache 2.0 | better, but ViT-B/16 at 224 is about 4x the compute of B/32 (196 tokens against 49): the backfill would take hours more on this CPU |
| MobileCLIP (Apple) | Apple's own model licence, research use | rejected: not a licence a self-hosted product can pass on |

B/32 is the quality-per-MB point for a 2-core shared host: 4.4 GFLOPs an image, 512-d vectors, and a 64 MB text tower.
Attribution: Radford et al., "Learning Transferable Visual Models From Natural Language Supervision" (2021); weights from
`openai/clip-vit-base-patch32`; ONNX exports from `Xenova/clip-vit-base-patch32` on Hugging Face, **pinned to commit
`d15189d7028b43f1d3e65039190477f6af591c2a`**. The export repository does not state a licence of its own; the weights it
converts are MIT, and we take the export as a conversion of them (not verified with the repository's author).

| File (`models/clip.lock`) | Size | sha256 | Goes into |
|---|---|---|---|
| `vision_model_int8.onnx` | 88.6 MB | `0ab0c1b3...6cb9bd93` | worker image |
| `text_model_int8.onnx` | 64.1 MB | `18845f2c...5efd3a21` | assets image |
| `tokenizer.json` | 2.2 MB | `f7f3b7af...697a90cd` | assets image |

(Full checksums in `models/clip.lock`, verified by `scripts/fetch-models.sh` for the images and for a checkout alike; a
mismatch deletes the download and fails the build.)

### 8-bit against full precision

The 8-bit exports are 4x smaller and a little faster (118 against 144 ms an image on one thread), but dynamic
quantisation moves the vectors: the cosine between an int8 and the fp32 vector of the same photo is **0.957** on average
(0.90 at worst) for images and **0.919** (0.61 at worst) for text. Whether that matters for *search* was measured on 209
CC0 photos (Wikimedia Commons, 21 categories, 42 queries, labels from the category the photo was found under, so noisy but
equally noisy for every row), ranking all photos for each query:

| Image tower / text tower | P@1 | P@5 | mAP |
|---|---|---|---|
| fp32 / fp32 (reference) | 0.952 | 0.910 | 0.849 |
| fp16 / fp16 (176 MB + 127 MB, no loss) | 0.952 | 0.910 | 0.849 |
| **int8 / int8 (shipped)** | 0.952 | 0.910 | 0.854 |
| int8 / fp32 | 0.976 | 0.948 | 0.866 |

Retrieval quality is unchanged within the noise of that set (the vectors move, the rankings do not), so int8 it is: 153 MB
instead of 482 MB of model, and a text tower that loads in a moment. fp16 would be lossless, but ONNX Runtime has no fast
fp16 path on CPU (measured: 145 ms against 10 ms a query for the text tower). The 20-photo smoke test in
`crates/cyanotype-clip/tests/model.rs` runs on every CI build: **top-3 recall 40/40 queries, top-1 39/40**.

### Memory, speed and sizes (measured on this host, 4 cores shared with other work)

Model files: OpenAI CLIP ViT-B/32 (MIT), sizes in the table above (vision int8 88.6 MB, text int8 64.1 MB, tokenizer 2.2 MB).

**Resident memory (1 thread)**

| What | Measured |
|---|---|
| Image tower after load | 146 MB |
| Image tower, batch of 4 | 159 MB |
| Text tower | about 70 MB |
| Worker container RSS in Docker | not yet measured in production; see the Datadog `worker.rss_bytes` metric |

**Model-only speed**

| What | Measured |
|---|---|
| Image, 1 thread | 126 to 158 ms |
| Image, 2 threads | 75 ms |
| Text query | p50 10 ms, p95 14 ms |

**Whole embed job path (debug build)**

| Threads | Throughput | 50k-photo backfill |
|---|---|---|
| 1 | 270 photos/min | about 2 to 3 h |
| 2 | 370 photos/min | about 1.5 to 2 h |

Release-build throughput: not yet measured in production; see the Datadog `embed.duration` metric.

**Search latency, 50k synthetic library (warm cache)**

| Filter | p50 | p95 |
|---|---|---|
| none | 42 ms | 64 ms |
| favorites | 44 ms | 53 ms |
| one year | 60 ms | 72 ms |
| a city | 100 ms | 130 ms |
| album of 40 photos | 8 ms | 9 ms |
| one camera | 87 ms | 98 ms |
| has location | 81 ms | 96 ms |
| JPEG only | 66 ms | 84 ms |
| HEIC only | 85 ms | 105 ms |

The first queries on a cold cache take up to 4.5 s.

**HNSW recall against exact search:** 0.997 / 0.998 / 0.977 at top 10 / 60 / 300 unfiltered; 1.0 with filters that keep 33 to 50% of the library.

**Index sizes at 50k photos:** vectors 56 MB, primary key 4 MB, HNSW 65 MB.

### ONNX Runtime, linking and the base image

`ort` 2.0.0-rc.13 (`ort-sys` downloads the prebuilt ONNX Runtime 1.28.0 for the target at build time, verifies its pinned
sha256 and links it **statically**; feature `api-24`, `tls-rustls`, nothing else) and `tokenizers` 0.23.2 (feature `onig`
only). Both are pinned with `=` and by `Cargo.lock`. The prebuilt ONNX Runtime is compiled with a newer GCC than Debian
bookworm has, and the link failed on libstdc++ symbols (`std::basic_string<wchar_t>::_M_replace_cold`), so **the builder and
every runtime image moved from Debian bookworm to trixie** (`lukemathwalker/cargo-chef:...-rust-1.99.0-trixie`,
`debian:trixie-slim`; libstdc++6 is part of trixie-slim). The worker's runtime packages are the same libraries with their
trixie names (`libdav1d7`); the Dockerfile's `ldd` check still fails the build if anything is unresolved.

## Caching

- Dependencies: the cargo-chef `cook` layer is reused until `Cargo.toml`/`Cargo.lock` change. Editing
  source only rebuilds the workspace crates.
- Cargo registry and git checkouts are BuildKit cache mounts (`sharing=locked`).
- CI: `CACHE=gha` makes bake use `type=gha` cache (one scope, `mode=max`, so the builder and cook layers
  are stored). Locally, `CACHE=local` writes to `.buildcache/`; with neither set the normal BuildKit
  cache is used.
- `.dockerignore` keeps `target/`, `.git`, `.env*`, docs and other non-build files out of the context,
  so unrelated edits do not invalidate layers.

## Web image

The UI has its own `apps/web/Dockerfile` (context `apps/web`, no Rust): `node:24-alpine` runs
`pnpm install --frozen-lockfile` and `pnpm build`, and the result is copied into `caddy:2-alpine`
(`apps/web/Caddyfile`, :8080, unprivileged). `docker-bake.hcl` has a `web` target in the default group, so
`just images` and the publish workflow build and push `cyanotype-web` with the other images (its own gha
cache scope). Caching headers: `/static/*` (Vite's hashed output; `build.assetsDir` is `static` because
`/assets/*` is the API) is `immutable` for a year, everything else, `index.html` included, is `no-cache`, and
unknown paths fall back to `index.html` for client-side routes. `/static/*` misses stay real 404s.

## Web app

`apps/web`: React 19, TypeScript, Vite, React Router, plain CSS (no UI kit), `thumbhash` for placeholders.
Versions are pinned by `pnpm-lock.yaml` and `packageManager` in `package.json`.

- `just web-dev`: Vite on :5173 proxying `/auth`, `/assets`, `/healthz` to the gateway (`GATEWAY_URL`, default
  `http://localhost:3000`, so run `just up` or the services first).
- `just web-check`: `tsc --noEmit`, eslint and vitest (API client error handling, lightbox and grid keyboard
  behaviour with Testing Library). `just web-build`: production build. CI runs both when `apps/web/**` changes.
- `pnpm test:mobile` (in `apps/web`): Playwright renders every page and overlay with a mocked API at 320 to 414 px
  (and 1280 px) and asserts there is no horizontal scroll. Not in CI: it needs a Chromium
  (`pnpm exec playwright install chromium`, or `CHROMIUM_PATH=/path/to/chrome`). `SHOT_DIR=dir` saves screenshots.
- Session: the HttpOnly cookie from `/auth/login`, sent with `credentials: 'include'`; `GET /auth/me` decides
  signed in or out. The API has no "is registration open" endpoint, so the register link is always offered and a
  403 shows a "registration is closed" message (and hides the link for the rest of the tab session).
- Library: `GET /assets` pages with `before`/`next_before`. Each list item carries `thumbhash` (base64) and a
  presigned `thumb_url` (both null until the worker has produced the derived images). The lightbox fetches
  `GET /assets/{id}` for the `preview_url`, and ←/→/Esc work. An expired thumbnail URL is refreshed once from
  `GET /assets/{id}`.
- `src/api.ts` is a hand-written typed client matching the Rust types. Generating it from a utoipa OpenAPI
  document is a follow-up.
- Not built yet: browser upload (blocked by CORS on Linode Object Storage; the empty state points to the CLI),
  a service worker (the manifest and icons are in place for PWA install later).

## Publishing

`.github/workflows/publish.yml` runs on push to main and `v*` tags (never on pull requests). One
`docker buildx bake --push` (gha cache) pushes `ghcr.io/<owner>/cyanotype-{auth,assets,worker,cli,backup,web}`
with tags `sha-<short>`, `latest` (main) and `X.Y.Z`/`X.Y`/`X` (tags), authenticated with `GITHUB_TOKEN`
(`packages: write`). `REGISTRY`, `TAGS` and `REVISION` are bake variables. `.github/workflows/deploy.yml`
then deploys a version to the VPS; see `docs/deploy.md`.

## Tasks

`just` is the entry point (see the `justfile`); CI calls the same recipes (`just check`, `just migrate`,
`just images`). CI path filters skip Rust jobs for docs-only changes and run the image job only when the
Dockerfile, bake file, `.dockerignore` or Rust sources change.

## End-to-end script

`scripts/e2e.sh` registers a throwaway user, uploads a generated PNG through the gateway (check, PUT to
S3 via the presigned URL, complete), waits for the worker to produce a thumbnail, then fetches the original
and thumbnail. It needs `curl`, `python3` and `sha256sum`, and the presigned URLs (`S3_PUBLIC_ENDPOINT`)
must be reachable from where it runs.

```sh
just up
just e2e                                   # BASE_URL defaults to http://localhost:3000
BASE_URL=https://staging.example scripts/e2e.sh
```

## Why Cargo and not Bazel/Buck2

The slowness was a build-graph problem (four images each compiling everything), not a limit of Cargo,
and it is fixed by compiling once and caching dependencies. Bazel or Buck2 would add a large maintenance
cost (BUILD files for every crate, rules for C dependencies such as libwebp) for little gain in a
single-language repo. Revisit if the repo grows several languages and contributors and CI is still slow
after this; [moonrepo](https://moonrepo.dev) would be the lighter next step (task graph and affected-only
runs on top of Cargo/npm without replacing them).
