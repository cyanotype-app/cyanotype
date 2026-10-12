# Build and automation

## Layout

One `Dockerfile`, one shared compile:

```
chef      rust + cargo-chef + C/C++ toolchain (build-essential, cmake; libwebp and libheif are built from source
          and linked statically) + libde265-dev, libdav1d-dev (the HEVC and AV1 decoders libheif calls)
planner   cargo chef prepare   -> recipe.json (changes only when manifests / Cargo.lock change)
builder   cargo chef cook      -> dependencies (cached layer)
          cargo build --release --workspace --locked   -> all four binaries, compiled once
runtime   debian:bookworm-slim, ca-certificates, non-root user
auth | assets | worker | cli   runtime + a single binary each (the --target); worker also gets libde265-0, libdav1d6, libjpeg62-turbo, libopenjp2-7
```

`docker buildx bake` (group `default` in `docker-bake.hcl`) builds the four targets together; BuildKit
sees the same `builder` stage in all of them and runs it once. `docker compose build` uses the same
`target:` per service, so it shares the stage too. Previously each image compiled the whole workspace
in parallel, which made a cold build take about 25 minutes.

## Worker system dependencies (HEIC/AVIF and RAW)

HEIC/HEIF/AVIF decoding uses libheif through `libheif-rs` with its `embedded-libheif` feature: the
libheif sources vendored in the `libheif-sys` crate are compiled by cmake during `cargo build` and linked
**statically**. The version is therefore pinned by `Cargo.lock` (`libheif-rs =3.0.0` -> `libheif-sys
5.3.1+1.23.1` = **libheif 1.23.1**; a unit test asserts it). Why this and not Debian's libheif:

- bookworm ships libheif 1.15.1, older than the `libheif-rs` API we use (1.17+), and the system library
  would put the HEIC decoder behind a moving apt version and separate `libheif-plugin-*` packages.
- One version everywhere: developer machines, CI and the image build the same libheif.
- The runtime image needs **no libheif packages and no plugins** (plugin loading is compiled out). It
  only needs the codec libraries libheif links dynamically: `libde265-0` (HEVC, ~0.5 MB) and
  `libdav1d6` (AV1/AVIF, ~0.7 MB), plus `libjpeg62-turbo` and `libopenjp2-7` (decoders libheif picks up from the builder image). The worker binary grows by about 2 MB for libheif.

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
| JPEG, PNG, WebP | magic bytes | EXIF via kamadak-exif; size from the header | full decode, resize, orient by EXIF |
| HEIC / HEIF / AVIF | `ftyp` brands (`heic`, `mif1`, `avif`, ...) | EXIF from the HEIF Exif item; size is the display size | libheif full-size decode to 8-bit RGB(A), resize. irot/imir are applied by libheif; the EXIF orientation is ignored (never applied twice). 10/12-bit is down-converted. HDR gain maps are ignored (the SDR base image is used). Only the primary image is decoded (no burst or sequence frames). |
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

The container limit for the worker is `mem_limit: 1024m` (`docker-compose.prod.yml`). Two mechanisms keep
decoding inside it, and a job that cannot fit is **refused with a permanent failure** (`jobs.last_error`
says why), not attempted:

1. `WORKER_HEAVY_CONCURRENCY` (default 2) bounds how many jobs run.
2. `WORKER_MEMORY_BUDGET_MB` (default 640) is a byte budget shared by all jobs (`apps/worker/src/budget.rs`).
   A job first reserves `2 x original + 48 MB` while it downloads (the S3 body is buffered and then
   copied into one buffer), then keeps `original + 48 MB` (3x the original for a RAW, whose header read can
   transiently decompress sensor data), and the thumbnail job then reserves
   `+ estimated decode peak` once the header says how big the image is. Requests queue first come first
   served. A job whose own need exceeds the whole budget fails permanently.

The decode estimate is bytes per pixel, measured with `apps/worker/src/memcal.rs`
(`CYANOTYPE_MEMCAL_FILE=<file> cargo test -p cyanotype-worker memcal -- --nocapture`, one process per file):

| Path | Estimate | Measured (peak RSS growth) |
|---|---|---|
| JPEG / PNG, RGB8 | 5 B/px | 3.5-4 B/px (50 MP JPEG: 174 MB) |
| RGBA8 (alpha PNG/WebP) | 12 B/px | 11 B/px (20 MP: 223 MB) |
| HEIC / AVIF, 8-bit | 12 B/px | 8.3 B/px (24 MP: 200 MB) |
| HEIC / AVIF, 10-bit+ | 16 B/px | not measured separately |
| RAW embedded preview | 6 B/px of the *preview* | 6 B/px (12 MP preview: 73 MB) |
| RAW develop (rawler) | 48 B/px of the sensor | 36-43 B/px (4 MP: 179 MB) |

Consequences with the default 640 MB budget and a typical 100 MB original (leaving about 490 MB for the
decode): a 100 MP RAW is fine **if it has an embedded preview** (the worker picks the cheapest preview that is at
least 1440 px, usually 1.6 MP; a preview that would need more than the budget is never decoded), but with
no preview it is refused (the develop path needs about 4.8 GB, and is also capped at 150 MP regardless of
budget, and disabled with `WORKER_RAW_DEVELOP=0`). In practice the develop fallback only runs for sensors
up to roughly 10 MP. HEIC/AVIF tops out around 45 MP and JPEG/PNG around 100 MP.

One gap is accounted for rather than eliminated: between the first reservation (the original) and the
second (the decode), the job releases and re-reserves instead of topping up in place (topping up could
deadlock two jobs each waiting for the other). While it waits, its downloaded original is briefly
unaccounted. That is bounded by `WORKER_HEAVY_CONCURRENCY` originals, and the 384 MB between the budget
and the container limit absorbs it.

Follow-up (tracked in `docs/PLAN.md`): DCT-scaled JPEG decoding and tiled/streamed HEIC decoding. HEIC is
decoded at full size and then resized, and JPEG too, so those two are the heaviest paths per megapixel.

## Caching

- Dependencies: the cargo-chef `cook` layer is reused until `Cargo.toml`/`Cargo.lock` change. Editing
  source only rebuilds the workspace crates.
- Cargo registry and git checkouts are BuildKit cache mounts (`sharing=locked`).
- CI: `CACHE=gha` makes bake use `type=gha` cache (one scope, `mode=max`, so the builder and cook layers
  are stored). Locally, `CACHE=local` writes to `.buildcache/`; with neither set the normal BuildKit
  cache is used.
- `.dockerignore` keeps `target/`, `.git`, `.env*`, docs and other non-build files out of the context,
  so unrelated edits do not invalidate layers.

## Publishing

`.github/workflows/publish.yml` runs on push to main and `v*` tags (never on pull requests). One
`docker buildx bake --push` (gha cache) pushes `ghcr.io/<owner>/cyanotype-{auth,assets,worker,cli,backup}`
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
