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

**Photos with a rotate/crop recipe** (`docs/library.md`, "Rotate and crop") are decoded for the recipe, not for the plain preview: the
render asks for the original at the size the crop needs (the preview edge divided by the crop's share of the long edge, at most 8,192 px;
4,096 px divided by that share for the edited JPEG), plus 8 bytes per output pixel for the copies the turn, the straighten and the
crop make. The job tries that edge, then half of it, and so on down to the plain preview edge, and takes the first whose estimate fits
the budget, so a tight budget makes a crop of a very large file *softer*, never a failed job (only an original that does not fit even the
plain preview is refused, as before). The JPEG path uses the DCT-scaled decoder at the larger factor (a 6,000 px JPEG at an edge of 4,096 px
decodes whole, about 3 bytes a pixel plus the copies), HEIF and streamed PNG their reduced decoders with the larger target, and a
RAW is rendered from the same embedded preview as before.

Photos refused before this change ("decoding a ... image needs about N MB, over the worker memory budget") are requeued
by migration `0010_requeue_memory_refusals.sql` the first time it runs, so a deploy reprocesses them. Anything the
migration does not match (originals larger than the budget itself, RAW without a preview) stays failed and is retried by
"Retry failed" in the app or `POST /assets/processing/retry`, which requeues every failed job of the caller.

Remaining follow-ups (`docs/PLAN.md`): streamed WebP and interlaced PNG, decoding HEIC alpha grids by tile, a
reduced-size decode for single-image HEIC (libheif has none), and colour-profile conversion.

## Semantic search model

Search embeds every photo and every query with a text-image model and compares them by cosine similarity
(`docs/library.md`, "Search"). Two models are supported and `SEARCH_MODEL` picks one: **SigLIP 2 base (patch 16, 224 px)**,
the default, and **OpenAI CLIP ViT-B/32**, the previous default. They run in-process, in Rust, with **ONNX Runtime** through
the `ort` crate and the `tokenizers` crate, in `crates/cyanotype-clip`: no Python, no sidecar, no network at runtime.

### Which model, and why (the bake-off)

Measured with `crates/cyanotype-clip/tests/bakeoff.rs` (`just bakeoff`) on **209 CC0 / public-domain photos from Wikimedia
Commons in 21 categories** (`scripts/fetch-eval-set.sh`, the list with sources in `crates/cyanotype-clip/tests/eval/photos.tsv`),
**42 queries** (each category by name and as "a photo of a ..."), labels from the category a photo was found under (noisy:
a sunset over a mountain is a "sunset", so a "mountain" query is penalised for it, equally for every model), every photo ranked
for every query:

| Image tower / text tower | File sizes (image + text) | P@1 | P@5 | mAP | R@10 |
|---|---|---|---|---|---|
| **CLIP B/32 int8 / int8** (before) | 89 + 64 MB | 0.952 | 0.924 | 0.859 | 0.820 |
| **SigLIP 2 B/16 4-bit / int8 (shipped)** | 63 + 283 MB | 0.952 | **0.981** | 0.899 | 0.847 |
| SigLIP 2 B/16 fp32 / fp32 | 372 + 1,129 MB | 1.000 | 0.962 | 0.904 | 0.866 |
| SigLIP 2 B/16 fp32 / int8 | 372 + 283 MB | 1.000 | 0.962 | 0.909 | 0.870 |
| SigLIP 2 B/16 fp16 / fp16 | 186 + 565 MB | 1.000 | 0.962 | 0.904 | 0.866 |
| SigLIP 2 B/16 int8 / int8 | 95 + 283 MB | 0.952 | 0.905 | 0.809 | 0.774 |
| SigLIP 2 B/16 int8 / fp32 | 95 + 1,129 MB | 0.952 | 0.886 | 0.787 | 0.736 |

(21 to 42 queries over 209 photos: differences of 0.01 to 0.02 are noise, one query moves P@1 by 0.024. The fp16 graphs give
the fp32 numbers exactly and run no faster on this CPU, so they are not an option; `SigLIP 2 224` is the one tested, the
256 px variant costs a third more compute for a gain that was not measured.) Findings:

- **SigLIP 2 beats CLIP**: P@5 0.981 against 0.924, mAP 0.899 against 0.859, recall@10 0.847 against 0.820 on the same photos,
  and it is **much** better outside English: category names in Spanish, German and French (P@5 / mAP) 0.905 / 0.845, 0.886 /
  0.823, 0.943 / 0.870 against CLIP's 0.476 / 0.432, 0.457 / 0.409, 0.457 / 0.453 ("perro en la playa" finds the dog on the
  beach; CLIP's English-only tokenizer garbles it). The queries each model misses (rank 1 wrong or P@5 under 0.6): CLIP
  "bridge" and "a photo of a bridge" (top 5: two bridges and a beach, a mountain, a forest), "mountain" (a sunset first);
  SigLIP 2: only "mountain" and "a photo of a mountain" (a sunset first, P@5 0.8). Its remaining misses are the label noise.
- **The export's 8-bit image tower is the one thing not to ship.** SigLIP 2's 8-bit ONNX image tower (dynamic quantisation,
  the published `vision_model_int8.onnx`) moves the vectors far (cosine to fp32 **0.76** on average, 0.52 at worst, against
  0.957 for CLIP's 8-bit tower) and costs 0.09 mAP, which would make SigLIP 2 *worse* than CLIP. The same export's
  **4-bit weight-only quantisation** (`vision_model_q4.onnx`, ONNX Runtime's `MatMulNBits`, activations stay fp32) keeps cosine
  0.957 (0.91 at worst) to fp32, loses nothing measurable (mAP 0.899 against 0.904) and is the smallest file, 63 MB. The
  text tower is fine in 8 bits (cosine 0.985 to fp32), and its 4-bit and fp16 forms are bigger, not smaller (the 256,000-token
  embedding table stays fp32 or fp16): so image q4 and text int8. (The export's `bnb4`, `q4f16` and `uint8` variants were
  measured too: bnb4 and q4f16 rank as well as q4 (mAP 0.89), bnb4 is 16 % slower; uint8 is as bad as int8, mAP 0.805.)
- **Per-query comparisons use the 20-photo smoke test too** (`crates/cyanotype-clip/tests/model.rs`, runs on every CI build):
  top-1 39/40 and top-3 40/40 queries for both models.
- **Cost: about 4x the compute per photo.** ViT-B/16 sees 196 patches against B/32's 49. Measured numbers are in "Memory, speed
  and sizes" below: the one-time backfill takes about four times as long as CLIP's did, and this is the price of the move.
  Resident memory of the image tower is the same (about 150 MB), and the text tower is bigger (335 MB, below).

Decision: **SigLIP 2 B/16 (image 4-bit, text 8-bit) is the default.** Better ranking in English, usable in other languages,
and the only one of the two with a calibrated score, which the result cut-off needs (below). CLIP stays selectable
(`SEARCH_MODEL=clip-b32`) and is the model a library is searched with while it is re-indexed (`docs/library.md`, "Changing the
search model").

### Provenance and licences

| Model | Weights | Export (the files in `models/*.lock`) |
|---|---|---|
| **SigLIP 2 base, patch 16, 224 px** | Google, **Apache 2.0** (`google/siglip2-base-patch16-224`, model card `license: apache-2.0`; Tschannen et al., "SigLIP 2", 2025) | `onnx-community/siglip2-base-patch16-224-ONNX` on Hugging Face (the transformers.js team's conversion), **pinned to commit `ba1f3b0843f24bc5417d38e19c37b287d719b2f4`** (2025-02-21, eight months before this change). The export repository's card states no licence of its own; the weights it converts are Apache 2.0, and we take the files as a conversion of them (not confirmed with the repository's authors). Every file is pinned by sha256 in `models/siglip2.lock`; `scripts/fetch-models.sh` checks them. |
| **OpenAI CLIP ViT-B/32** | OpenAI, **MIT** (github.com/openai/CLIP; Radford et al., 2021) | `Xenova/clip-vit-base-patch32`, pinned to commit `d15189d7028b43f1d3e65039190477f6af591c2a`; same note about the export's missing licence statement. `models/clip.lock`. |

The calibration constants of SigLIP 2 (`logit_scale` 4.7245, so a scale of 112.669, and `logit_bias` -16.7717) are read from
the checkpoint (`model.safetensors` of `google/siglip2-base-patch16-224`, revision `75de2d55`) and written into
`Model::calibration`. Changing a lock file is a model change: bump `Model::version()` in the same commit so photos are
re-embedded. A published export is used rather than our own because we cannot host files, and exporting at image build
time would need PyTorch in the build.

Other candidates, not shipped: OpenCLIP ViT-B/32 LAION-2B (about 3 points better zero-shot than OpenAI's, no stable quantised
export to pin), MobileCLIP (Apple's model licence is research-only), SigLIP 1 base/16 (superseded by SigLIP 2 at the same cost).

### What differs between the models in code

| | CLIP B/32 | SigLIP 2 B/16 |
|---|---|---|
| Image input | shorter side to 224 (bicubic), centre crop, CLIP mean/std | **whole image squashed to 224 x 224** (bilinear, anti-aliased, no crop, aspect ratio not kept), mean 0.5 / std 0.5 (SiglipImageProcessor) |
| Text input | BPE (OpenAI), up to 77 tokens, unpadded | Gemma tokenizer (256,000 tokens), **lower-cased by us** (the tokenizer file does not, though the model was trained on lower-case text), truncated to 63 + `<eos>`, **padded with `<pad>` to exactly 64** (the graph pools the last position) |
| Output | `image_embeds` / `text_embeds` (512) | `pooler_output` (768), unit-length |
| Calibration | none | `sigmoid(112.669 x cos - 16.7717)` |
| Query preparation | none | short queries (up to 4 words) become "a photo of a ..." |
| Vectors | `halfvec(512)`, HNSW `asset_embeddings_clip_b32_hnsw` | `halfvec(768)`, `asset_embeddings_siglip2_b16_hnsw` |

Parity tests (`crates/cyanotype-clip/tests/model.rs`) check all of this against a Python reference (Pillow, `tokenizers`,
onnxruntime, with small committed fixtures: `tests/fixtures/reference.json`, `reference_siglip2.json`).

### The query template

SigLIP 2 was trained on captions. A bare keyword ("airplane") scores far lower against its own photo than "a photo of a
airplane", and since the cut-off reads those absolute scores the template is what makes them usable: on the evaluation set the
best photo of an answerable keyword query had a probability of at least 0.9 % with the bare query and a query nothing answers
(a giraffe) up to 0.8 %, against at least 6.7 % and at most 1.7 % with the template: the two kinds of query are four times apart
instead of barely apart. Ranking is unchanged within noise (keyword queries, P@5 / mAP: bare 0.962 / 0.889, "a photo of a ..."
0.981 / 0.899). **Averaging several templates** ("a photo of {q}", "a picture of {q}", ...) was measured on both models and
**not kept**: SigLIP 2 0.962 / 0.889 bare, 0.962 / 0.889 averaged over two, 0.962 / 0.890 over three; CLIP 0.914 / 0.860, 0.914 /
0.860, 0.914 / 0.857: nothing beyond noise, for a text tower run two or three times per query. (The single "a photo of a" template
on CLIP gave 0.933 / 0.859: P@5 up by one query's worth, mAP flat, and no absolute scale to calibrate, so CLIP is left alone.)

### The cut-off ("only results that match")

Which results are *strong* and which *weaker* is cut from the ranked cosines by `crates/cyanotype-clip/src/cutoff.rs`
(rules in `docs/library.md`). Tuned on the same evaluation set, 42 answerable queries and **12 that nothing in the set answers**
("a giraffe", "a submarine", "an astronaut", "a bowl of soup", "a tennis match", "a library bookshelf", "a dinosaur skeleton",
"a medieval castle", "an elephant", "a snowman", "a wedding dress", "a chess board"), precision and recall of what is returned
against a plain top 10:

| Model, list | Precision | Recall | Returned per answerable query | Returned per unanswerable query | Unanswerable queries that return nothing |
|---|---|---|---|---|---|
| CLIP, top 10 | 0.817 | 0.821 | 10.0 | 10.0 | 0 of 12 |
| CLIP, strong tier | 0.870 | 0.718 | 8.2 | 18.6 | 0 of 12 |
| CLIP, strong + weaker | 0.692 | 0.801 | 11.5 | 40.4 | 0 of 12 |
| **SigLIP 2, top 10** | 0.843 | 0.847 | 10.0 | 10.0 | 0 of 12 |
| **SigLIP 2, strong tier** (what a search shows) | **0.870** | **0.804** | 9.2 | **0.4** | **9 of 12** |
| **SigLIP 2, strong + weaker** | 0.786 | 0.895 | 11.3 | 0.8 | 6 of 12 |

(Recall is of all photos of the category, about 10 a query, so a list of 10 cannot do better than about 0.9.) The strong tier has
the precision of CLIP's with a recall 8 points higher, and, unlike any CLIP list, returns almost nothing for a subject the library does
not have; against a plain top 10 it trades 4 points of recall for 3 points of precision and the empty answer, and "Show weaker
matches" brings the recall to 0.895.

What the numbers say, and the choices they led to:

- **The model's probability is real but small for short queries.** The best photo of an answerable keyword query has a median
  probability of 17 % with the template (7 % bare), the worst 6.7 % (0.9 % bare), because the sigmoid was trained on full
  captions. A threshold at 50 % would return nothing, so the strong tier uses p >= 0.3 %: absolute-only thresholds give F1 over
  the 21 keyword queries of 0.84 / 0.83 / 0.83 at 0.2 % / 0.3 % / 0.5 % (flat; 1 % gives 0.79), and 0.3 % is the lowest in that
  range that still sends most unanswerable queries home empty. With the full rules, 9 of the 12 unanswerable queries return
  nothing strong (the other three one or two photos each), and 6 of 12 return nothing even with the weaker tier. Plain top 10
  returns ten photos for "a giraffe". (Python harness numbers; the table above is the Rust harness.)
- **Relative and gap rules do the rest**, and matter more in a real library than in this 209-photo set (10 relevant photos per
  query, so a "within 0.06" rule is rarely what stops the list here). Relative only, SigLIP 2, on the 42 queries: within 0.02 of the
  best: precision 0.94, recall 0.60; within 0.03: 0.90 / 0.73; within 0.04: 0.83 / 0.84; within 0.05: 0.72 / 0.89 (CLIP: 0.92 /
  0.55, 0.85 / 0.72, 0.72 / 0.82, 0.51 / 0.88; Python harness, bare queries). The largest-gap rule on top changed precision and
  recall by less than 0.01 here, and is kept as a guard for long, smooth tails. For SigLIP 2 the relative rule is generous
  (0.06) because the absolute floor does the work against junk; for CLIP, which has only the relative rule, it is tight (0.03).
- **CLIP gets no absolute floor** and cannot return nothing: the cosine of the best photo is 0.26 to 0.31 for the answerable
  queries and 0.22 to 0.27 for the unanswerable ones, ranges that overlap, so any single number cuts some real query or lets
  some junk through. Only the relative rule (within 0.03) and the gap rule apply, and the table's CLIP rows show it: 0.87
  precision at 0.72 recall for the strong tier, but photos returned for every unanswerable query.
- Whatever the cut-off decides, **"Show weaker matches"** is there: strong + weaker returns more than the top 10 at lower
  precision, so a query is never a dead end.

The same set also calibrates the photo-to-photo cut-off of "More like this" (`similar_bakeoff`, in the same `just bakeoff` run; numbers
and rules in `docs/library.md`, "More like this"): every photo is a source, a result is relevant when it is in the source's category.
The strong tier has a precision of 0.92 for both models (a plain top 10: 0.52 to 0.54) at a recall of 0.20 to 0.22.

### 8-bit against full precision (CLIP)

CLIP's 8-bit exports are 4x smaller and a little faster (118 against 144 ms an image on one thread), and dynamic quantisation
moves the vectors: the cosine between an int8 and the fp32 vector of the same photo is 0.957 on average (0.90 at worst) for
images and 0.919 (0.61 at worst) for text. On the evaluation set the rankings did not move (fp32 / fp32 mAP 0.849, int8 / int8
0.854), so int8 it stays. fp16 is lossless but ONNX Runtime has no fast fp16 path on CPU (145 ms against 10 ms a query for the
text tower).

### Memory, speed and sizes (measured on this host, 4 cores shared with other work)

The host was busy with other builds throughout (load average 10 to 17 on 4 cores), so absolute times are pessimistic and noisy;
the ratios between rows are the reading. All numbers are release builds (`cargo run --release -p cyanotype-clip --example
bench -- <model> <threads> <image|text>`), ONNX Runtime linked as in the services, one tower per process.

| Model files (`models/*.lock`) | Size | Goes into |
|---|---|---|
| SigLIP 2 `vision_model_q4.onnx` | 63.3 MB | worker image |
| SigLIP 2 `text_model_int8.onnx` + `tokenizer.json` | 283.4 + 34.4 MB (after `onnx-split`: 86.8 MB + 196.6 MB side file) | assets image |
| CLIP `text_model_int8.onnx` + `tokenizer.json` | 64.1 + 2.2 MB | assets image, for the transition only (below) |
| CLIP `vision_model_int8.onnx` | 88.6 MB | nowhere (only for `SEARCH_MODEL=clip-b32`: bake it into the worker) |

**Image tower, one image at a time (what the worker does)**

| | CLIP B/32 int8 | SigLIP 2 B/16 q4 | SigLIP 2 B/16 fp32 | SigLIP 2 B/16 int8 |
|---|---|---|---|---|
| Resident after load / after use | 148 / 152 MB | **137 / 154 MB** | 622 / 636 MB | 172 / 186 MB |
| 1 thread | 139 ms | 562 ms | 564 ms | 516 ms |
| 2 threads | 116 ms | 433 ms | | |
| Preprocessing (decode of a 330 px JPEG included) | 2.2 ms | 4.3 ms | | |

(A batch of 4 was no faster per image than 1 for either model, 135 against 139 ms and 627 against 562 ms, so the worker runs
photos one at a time; see "The parity test that flaked" for why that also matters for determinism.)

**Text tower** (2 threads unless noted)

| | CLIP int8 | SigLIP 2 int8, as downloaded | SigLIP 2 int8, embedding table split off |
|---|---|---|---|
| Resident | 116 MB | 523 MB | **335 MB** |
| Query p50 / p95 | 13.7 / 19.7 ms | 292 / 363 ms | 150 to 300 / 170 to 420 ms (1 to 2 threads) |

The query time of SigLIP 2's text tower is compute, not memory: 64 padded positions through 12 layers of width 768 (CLIP: about
10 tokens through width 512). On a quiet core it measured 154 / 173 ms on one thread and 127 / 151 ms on two; a query is cached
after its first use (paging, a changed filter, a retype cost nothing).

**Why the text tower does not fit the old assets limit, and what was done.** 256,000 tokens x 768 = 197 MB of the 283 MB file is
one embedding table, and a query reads at most 64 of its rows. ONNX Runtime 1.28 (the version `ort` 2.0.0-rc.13 links) keeps 523 MB
resident for this 283 MB file (the file's bytes and the weights built from them); the assets container had 384 MB. Options considered: raise
the limit to 640 MB (total over budget), quantise the table ourselves (a model file we cannot pin by an upstream checksum),
move text encoding elsewhere (a sidecar, against the design), or **move the table into an external data file**, which ONNX
Runtime reads without the second copy: **335 MB resident (all of it heap: the table is read, not mapped), same vectors**. `onnx-split`
(`crates/cyanotype-clip/src/split.rs`, compiled with plain `rustc` from `tools/onnx_split.rs` in its own Docker stage) does
that to the downloaded file at image build time, so no unpinned file is hosted: the lock pins the input, the transformation is
a deterministic function of it, and a test (`a_split_text_model_gives_the_same_vectors`) checks that the split model gives the
same vectors. (ONNX Runtime 1.31 in Python maps external data instead of copying it: the same file grew the process by 10 MB there. The
split file is ready for that; nothing else changes when `ort` moves to a newer runtime.) With 335 MB for the tower plus the service itself the
**assets limit goes from 384 MB to 512 MB**.

**Memory table (compose limits)**

| Service | Before | Now |
|---|---|---|
| postgres | 768m | 768m |
| worker | 1280m | 1280m (the image tower's resident memory is unchanged, about 150 MB, outside the 640 MB decode budget) |
| assets | 384m | **512m** (the SigLIP 2 text tower is 335 MB resident; the whole service measured 35 MB idle and 370 MB after its first searches, debug build) |
| auth | 256m | 256m |
| gateway | 128m | 128m |
| backup | 128m | 128m |
| web | 64m | 64m |
| **Services** | 3,008m | **3,136m** |
| Datadog Agent (profile) | 384m | 384m |
| **With the Agent** | 3,392m = 3.31 GiB | **3,520m = 3.44 GiB** (limit ~3.6 GB) |

**Whole embed job path (S3 download of the preview, WebP decode, preprocessing, inference, store), release build**

`cargo test --release -p cyanotype-worker embed_throughput -- --ignored --nocapture` (Postgres and S3 in containers, 120 previews of
the fixture photos, the model loaded before timing; host load average 7 to 10 on 4 cores while it ran):

| Model | Threads | Per photo | Throughput | 10,000 photos | 50,000-photo backfill |
|---|---|---|---|---|---|
| CLIP B/32 int8 | 1 | 143 ms | 420 photos/min | 24 min | 2.0 h |
| CLIP B/32 int8 | 2 | 121 ms | 496 photos/min | 20 min | 1.7 h |
| **SigLIP 2 B/16 q4** | 1 (the default, `WORKER_EMBED_THREADS`) | 552 ms | **109 photos/min** | 1.5 h | **7.6 h** |
| **SigLIP 2 B/16 q4** | 2 | 444 ms | 135 photos/min | 1.2 h | 6.2 h |

Resident size of the process at the end: 159 MB (SigLIP 2), 186 to 189 MB (CLIP, the debug-built test harness included).
SigLIP 2 is 3.9x slower per photo on one thread, and a second thread buys only 23 % (the early layers are memory bound on this
host), so the backfill after switching a 50,000-photo library takes most of a working day on one thread, **while searches keep
working with CLIP** (`docs/library.md`, "Changing the search model"); a small library (a few thousand photos) is done in an hour.
A busy production host (a shared 2 vCPU Linode) will be slower than these numbers. The earlier figure for CLIP (270 photos/min)
was a debug build.

**Search latency, 50k synthetic library (warm cache)**: unchanged by the model, the index is the same shape (768-d vectors are
1.5 KB a row against 1 KB, so the index is about half as large again; the measurement of `docs/library.md` was made with the
512-d CLIP index, not repeated for 768).

### ONNX Runtime and the 8-bit kernels: the parity test that flaked

`image_embeddings_match_the_reference` failed once in CI with cosine 0.9828 against a 0.985 threshold. Findings:

- **Not threads.** Sessions are one thread and, as `embedding_does_not_depend_on_the_thread_count` now shows, 1, 2 and 4 threads
  give the same vector (cosine > 0.9999) and the same session twice gives bit-identical output.
- **Not the test's own batching**: it embeds one image per call. But there *was* a determinism bug the test exposed on the
  way: the 8-bit graphs quantise their activations with one scale per *batch* (`DynamicQuantizeLinear` is per tensor), so a
  photo's stored vector depended on which photos the worker happened to embed in the same call (cosine 0.99 between a photo
  alone and in a batch of two). `ImageEncoder::embed` now runs every image on its own, which costs nothing (a batch of 4 was no
  faster per image).
- **Most likely the CPU.** The reference vectors were made on one machine (AVX2, no VNNI: an AMD EPYC here), the quantised
  int8 matrix multiplication of ONNX Runtime (`MatMulInteger`, u8 x s8) uses a saturating 16-bit multiply-add on AVX2 and an
  exact one on AVX-512 VNNI, and GitHub's runners are a mix of both. The same graph on two CPU types gives vectors that agree to about
  0.98, about what the failure showed. This could not be reproduced here (one CPU type); CI now prints the runner's CPU model,
  and the failure message of the test includes it, so a recurrence is read against it.
- **The fix**: the test no longer pins an int8 result to another machine's. It compares (1) the *preprocessing* as a tensor
  against Pillow's (plain arithmetic, the same on every CPU, tight tolerance: that is what catches a preprocessing mistake),
  (2) the quantised embedding against the reference quantised embedding with a tolerance that covers the CPU spread (0.95), and
  (3) against the **fp32 graph of the same export** (0.88, CPU independent: measured cosines are 0.90 to 0.97), which also
  fails loudly for a swapped channel order or a wrong mean (cosine below 0.8). The same structure applies to SigLIP 2's 4-bit
  tower and to both text towers.

### ONNX Runtime, linking and the base image

`ort` 2.0.0-rc.13 (`ort-sys` downloads the prebuilt ONNX Runtime 1.28.0 for the target at build time, verifies its pinned
sha256 and links it **statically**; feature `api-24`, `tls-rustls`, nothing else) and `tokenizers` 0.23.2 (feature `onig`
only). Both are pinned with `=` and by `Cargo.lock`. The prebuilt ONNX Runtime is compiled with a newer GCC than Debian
bookworm has, and the link failed on libstdc++ symbols (`std::basic_string<wchar_t>::_M_replace_cold`), so **the builder and
every runtime image moved from Debian bookworm to trixie** (`lukemathwalker/cargo-chef:...-rust-1.99.0-trixie`,
`debian:trixie-slim`; libstdc++6 is part of trixie-slim). The worker's runtime packages are the same libraries with their
trixie names (`libdav1d7`); the Dockerfile's `ldd` check still fails the build if anything is unresolved.

### Near-duplicate threshold

Near-duplicate review (`docs/library.md`, "Near-duplicates") judges a pair of photos by the cosine of their image embeddings plus a fact that has
to agree (`cyanotype_core::duplicates::is_duplicate`); the thresholds per model are `Model::duplicate_rules()` in `cyanotype-clip`
(`duplicates.rs`), calibrated like the burst threshold, with the same 209 CC0 photos:

- **Copies** of every photo (1,254 pairs): resized to a half and a quarter (Lanczos), recompressed at JPEG quality 70 and 40, half size at
  quality 60, and a 3 % border cropped off. `duplicate_bakeoff` in `tests/bakeoff.rs` (`DUP_MODEL=CLIP` or `SigLIP` picks one model; it needs
  the evaluation set and the models like the other bake-offs: `SEARCH_MODEL_DIR=... SEARCH_EVAL_DIR=... cargo test --release -p cyanotype-clip
  --test bakeoff duplicate_bakeoff -- --ignored --nocapture`, about 5 minutes).
- **Different photos**: all 21,736 pairs, of which 936 are in the same category (the hardest: other dogs, other beaches). The set holds two
  pairs that are the same picture twice under different file names (cosine 1.0000, `bridge`/`train`, `sailboat`/`sunset`), which count as
  "different" here, so every precision below is a floor.

The cosine of a copy is lower than "the same picture" suggests, because the shipped image towers are quantised (SigLIP 2 4-bit, CLIP 8-bit):

| | SigLIP 2 | CLIP B/32 |
|---|---|---|
| copies, median / 5th percentile | 0.962 / 0.852 | 0.952 / 0.850 |
| same size, JPEG q70 / q40, median | 0.986 / 0.963 | 0.971 / 0.957 |
| half size / quarter size, median | 0.965 / 0.889 | 0.962 / 0.888 |
| different photos, median / 99th / 99.9th percentile | 0.519 / 0.773 / 0.931 | 0.480 / 0.772 / 0.915 |
| same-category pairs, median / 99th | 0.676 / 0.949 | 0.655 / 0.937 |

Cosine alone, over the 1,254 copies and the 21,736 different pairs (recall = copies found; precision = copies found among everything found; the
pool of negatives is 17 times the positives, so it is a pessimistic precision for a real library):

| min cosine | SigLIP 2 recall | SigLIP 2 precision | different pairs merged | CLIP recall | CLIP precision |
|---|---|---|---|---|---|
| 0.80 | 98.7 % | 88.3 % | 164 (0.76 %) | 98.3 % | 88.5 % |
| **0.85** | **95.5 %** | **91.7 %** | 109 (0.50 %) | 94.9 % | 92.5 % |
| 0.88 | 91.9 % | 93.0 % | 87 (0.40 %) | 89.1 % | 95.3 % |
| 0.90 | 87.2 % | 94.6 % | 62 (0.29 %) | 81.8 % | 97.0 % |
| **0.92** | **80.6 %** | **96.8 %** | 33 (0.15 %) | 73.0 % | 97.9 % |
| **0.94** | **71.5 %** | **98.1 %** | 17 (0.08 %) | 61.4 % | 98.7 % |
| 0.96 | 52.4 % | 99.0 % | 7 (0.03 %) | 41.2 % | 99.2 % |
| 0.98 | 20.9 % | 98.5 % | 4 (0.02 %) | 9.2 % | 98.3 % |

So a single threshold would have to trade a lot of recall for precision, and the rules put the evidence on the pair instead:

| the pair shares | threshold (SigLIP 2 / CLIP) | what the table says (SigLIP 2) |
|---|---|---|
| the same camera, capture times within 60 s | 0.85 / 0.84 | 95.5 % of the copies; a different pair at 0.85 or more is only merged if it also shares a camera and a minute (not measurable here: the set has no EXIF) |
| the same pixel dimensions | 0.92 / 0.91 | 80.6 % of the copies (a re-encode at the same size is the easy case: q70 median 0.986), 96.8 % precision before the dimensions check |
| neither has a capture time (a copy that lost its EXIF) | 0.94 / 0.93 | 71.5 % of the copies at 98.1 % precision with nothing else to go on |

By kind of copy at the metadata rule (SigLIP 2, 0.85): same size q70 and q40, a cropped border and a half size are all found (5th percentile
0.93 to 0.97 against 0.85); the quarter size (5th percentile 0.78) and a half size recompressed at q60 (0.885) are the ones lost. A copy
that has lost its metadata and is resized can only be found by the strictest rule (a quarter size is not); that is by design: "a smaller
copy of something with no date" and "another photo of the same kind of thing" cannot be told apart by this model.

**Not measured**: real libraries (the set has no EXIF, so the metadata gates were reasoned about, not scored, and the time/camera facts
are exact on the evidence of a copy that kept its EXIF); photos that differ only in a small moving subject (those are bursts or "similar
shots", which the same thresholds would pair when they share a camera and a minute: a burst itself is excluded).

## Text in photos (OCR)

The worker reads the text off each photo's preview (`docs/library.md`, "Text in photos") with **PP-OCRv6 small**, PaddlePaddle's own ONNX exports of its
detection and recognition models, run with the same ONNX Runtime as the search models. The code is `crates/cyanotype-ocr` (the PaddleOCR pipeline
without OpenCV: a DB probability map, connected components, the smallest rotated rectangle around each, CTC decoding) plus QR reading with `rxing`.

### Which model, and why (the bake-off)

38 images from Wikimedia Commons, all CC0 or public domain (`crates/cyanotype-ocr/tests/bakeoff/images.tsv`, fetched by `scripts/fetch-ocr-set.sh`): 7 receipts
and forms (Latin, Vietnamese, Russian), 15 street signs (Latin, Cyrillic, Hebrew, Korean, Lao), 9 screenshots and 7 cards, letters and scans.
The last column of the list is every word a person can read in the picture, accents removed and people's names left out; the score is the share of those
words found (a token, a part of a word of five letters or more, or one edit away). The images are 1024 to 1280 px (the real preview is up to 1440). One
thread, release build, this host (2 vCPU shared with other work, so times are indicative). `cargo run --release -p cyanotype-ocr --example bakeoff`
with `OCR_VARIANT` = `v6`, `v3latin`, `v5latin` or `v3en`.

| Model | Words found | Receipts | Signs | Screens | Documents | Mean / median / slowest | Resident after load | Highest process peak |
|---|---|---|---|---|---|---|---|---|
| **PP-OCRv6 small det + rec** (31 MB, multilingual dictionary) | **87.4 %** (327/374) | 84.8 | 78.7 | 97.4 | 76.5 | **1.9 s / 0.8 s / 10.2 s** | 78 MB | **257 MB** |
| same, 1280 px detection | 87.2 % | 85.9 | 78.7 | 96.1 | 76.5 | 2.6 s / 1.4 s / 12.9 s | 78 MB | 324 MB |
| same, 2 threads | 87.4 % | 84.8 | 78.7 | 97.4 | 76.5 | 1.8 s / 0.8 s / 12.6 s | 78 MB | 257 MB |
| PP-OCRv3 mobile det (2.4 MB) + PP-OCRv5 Latin rec (7.9 MB) | 80.7 % | 76.1 | 78.7 | 94.8 | 60.5 | 2.2 s / 1.2 s / 12.0 s | 52 MB | 223 MB |
| PP-OCRv5 det (88 MB) + Latin rec * | 87.4 % | 81.5 | 78.7 | 98.1 | 79.0 | 7.4 s / 5.9 s / 20.4 s | 230 MB | 725 MB |
| PP-OCRv3 det + PP-OCRv5 English rec * | 83.4 % | 77.2 | 80.9 | 97.4 | 65.4 | 3.2 s / 1.1 s / 21.9 s | 52 MB | 264 MB |

\* measured before the two speed and memory changes below (150 boxes at most, batches by width), so only its accuracy and its order of magnitude are comparable;
the same change took PP-OCRv6 small from 2.9 s mean, 21 s slowest and a 492 MB peak to the numbers in the first row, with the same 87.4 %.

PP-OCRv6 small wins: the same accuracy as the 88 MB server detector at a quarter of the time and a third of the memory, and 7 points above the small Latin-only
models, mostly on documents (long lines of small type). It is the official PaddlePaddle export (`PaddlePaddle/PP-OCRv6_small_det_onnx` and `..._rec_onnx`,
Apache-2.0, published June 2026), pinned by commit and sha256 in `models/ocr.lock`, fetched by `scripts/fetch-models.sh` and baked into the worker image
(31 MB). Words it misses: scripts its dictionary lacks (the Russian receipt and the Cyrillic sign score 0 of 13 words; Vietnamese diacritics are
dropped but the words mostly still match), an old handwritten hand, very small type on a crumpled receipt, text on a stone plaque.

**Memory.** 78 MB resident with both networks loaded; the highest process peak while reading any of the 38 images was 257 MB, so a job needs about 180 MB above
the resident models; the worker holds 224 MB of its byte budget per job (`apps/worker/src/ocr.rs`). Two things keep the peak down: boxes are read biggest first and at
most 150 (a screenshot with 360 text boxes took 21 s and 262 MB), and recognition batches close when their padded widths pass 1,600 px (a batch of long lines is
small), which took the 58-line letter from 482 MB to 257 MB. The worker's container limit (1280m) already covers the decode budget (640 MB), the image tower (about
150 MB), this model (about 80 MB resident, outside the budget) and the runtime. The models are dropped after 5 idle minutes, and only one of the two ever works at a time.

**Speed.** About 0.8 s for a typical photo, 1 to 3 s for a receipt, up to 10 s for a dense page, on one thread of a shared host: a 50,000-photo library is read
in the background in roughly 12 to 16 hours of otherwise idle worker time (most photos have no text and cost about 0.4 s, the detector alone).

### QR codes

`rqrr` (pure Rust, small) found none of the 8 codes in the 6 real photos of `tests/bakeoff/qr.tsv`, not even a clean stylised one; `rxing` (a port of ZXing, Apache-2.0,
published September 2026, with only its QR reader compiled in) found **5 of 8**: a stylised dot code, a sticker, and all three codes on a fire-extinguisher box
(each decoded to the right address); not read: a billboard at night behind trees, a deliberately damaged code and the code on a crumpled Austrian till receipt
(3 px per module). Each photo is scanned at its own size and again at twice it (the second pass found the second and third code on the box). 829 ms for the six.
The payload is data: shown as text, a link only for `http(s)`, never opened by the server or the page.

### Provenance and licences

| What | Licence | Source |
|---|---|---|
| PP-OCRv6 small detection and recognition (ONNX) | Apache-2.0 | `huggingface.co/PaddlePaddle/PP-OCRv6_small_det_onnx` @ 28fe589, `..._rec_onnx` @ b8f84f0 |
| `rxing` 0.9.3 | Apache-2.0 | crates.io, 2026-09-16 (>= 14 days), checksum in `Cargo.lock` |
| the bake-off images | CC0 / public domain | listed with their Commons titles in `tests/bakeoff/*.tsv` |

Candidates compared but not shipped: PP-OCRv3 and v5 exports by `monkt/paddleocr-onnx` on Hugging Face (Apache-2.0, third-party conversion).

## Caching

- Dependencies: the cargo-chef `cook` layer is reused until `Cargo.toml`/`Cargo.lock` change. Editing
  source only rebuilds the workspace crates.
- Cargo registry and git checkouts are BuildKit cache mounts (`sharing=locked`).
- CI: `CACHE=gha` makes bake use `type=gha` cache (`mode=max`, so the builder and cook layers are stored). Every image has
  its own scope (`cyanotype-<target>`): an export replaces its scope's manifest, so images sharing one scope undid each
  other's layers, and layers are stored once by digest, so the builder layers they share are not duplicated. `CACHE_WRITE=false` (pull requests)
  reads only: main's publish run is what fills the cache, and PRs do not spend the 10 GB repository budget on copies.
  Locally, `CACHE=local` writes to `.buildcache*/`; with neither set the normal BuildKit cache is used.
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

The image also carries the downloadable CLI under `/srv/cli` (`/cli/*`): bake builds the Dockerfile's `cli-dist` stage
(cross-compiled with zig + cargo-zigbuild, five targets, signed when a BuildKit secret is given) and hands it to the web
Dockerfile as the named context `cli-dist-ctx`. `docker buildx bake cli-dist` writes it to `./cli-dist-out`. See
[`releases.md`](releases.md) for targets, signing, tags and the cache scope (`cyanotype-cli-dist`).

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
(`packages: write`). A first job (`cli-dist`, environment `release`) builds and signs the CLI downloads with the release
key and the second job passes them to the web image as a build context (`docs/releases.md`). `REGISTRY`, `TAGS` and
`REVISION` are bake variables. `.github/workflows/deploy.yml`
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
