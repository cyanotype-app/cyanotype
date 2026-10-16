# Worker test fixtures

Small HEIC/AVIF files used by the unit tests in `apps/worker/src/heif.rs`. Every file is under 2 KB except `tile512.heic` (27 KB).

They are synthetic: the pixels are generated programmatically (no photograph, nobody's copyright), so
they are released under CC0 1.0 like the rest of this directory's content. Provenance:

| File | What it is |
|---|---|
| `rot6.heic` | 96x64 stored, with an `irot` (90 deg clockwise, so 64x96 upright) **and** an EXIF block that still says orientation 6, plus Make/Model/lens/DateTimeOriginal/OffsetTime/GPS. Proves irot is applied once and the EXIF orientation is not applied on top. |
| `plain.heic` | The same 96x64 picture with no rotation. |
| `tenbit.heic` | 10-bit (from a 16-bit PNG), checks the down-conversion to 8-bit. |
| `alpha.heic` | RGBA, with a translucent quadrant. |
| `grid_plain.heic` | A 3x2 grid of 64x64 HEVC tiles cropped to 180x120 (the grid overhangs by 12 and 8 px), the marker picture below at that size. Built the way iPhones store large photos: a `grid` item whose `dimg` references hidden `hvc1` tile items. |
| `grid_irot.heic` | The same grid with an `irot` of 90 deg counter-clockwise (120x180 displayed): the overhang moves to the top, so the first tile row starts outside the image. |
| `grid_irot180.heic` | The same grid turned 180 deg: the overhang moves to the left and top. |
| `grid_imir_irot.heic` | The same grid with an `imir` (left-right) and an `irot` of 270 deg. |
| `tile512.heic` | One 512x512 photo-like HEVC image. The tests lift its stream out and repeat it to build iPhone-sized grids (48 MP and 200 MP) in memory; nothing large is committed. |
| `rot6.avif` | AVIF through the same code path (heif-enc 1.17 writes no `irot` for AVIF, so this one is stored upright). |

The source picture is 96x64: a white 12x12 square at the top-left, the left half red (220,30,30) and the
right half blue (30,30,220). Tests look at those pixels to tell which way an image was turned.

Regenerate with `heif-enc` (Debian/Ubuntu: `libheif-examples` plus `libheif-plugin-x265`; libheif 1.17.6
was used) from JPEG/PNG sources produced by a small `image`-crate program:

```sh
heif-enc -q 80 src_o6.jpg -o rot6.heic        # src_o6.jpg: 96x64 JPEG with EXIF orientation 6 + the EXIF above
heif-enc -q 80 src_o1.jpg -o plain.heic       # same, orientation 1
heif-enc -q 80 -b 10 src16.png -o tenbit.heic
heif-enc -q 80 src_alpha.png -o alpha.heic
heif-enc -A -q 60 src_o6.jpg -o rot6.avif
```

`heif-enc` 1.17 only writes single-image HEICs and the statically linked libheif has no encoder, so the grid files are
assembled by `testgen::heic_grid` from tiles encoded with `heif-enc`; regenerate them with
`cargo test -p cyanotype-worker regenerate_grid_fixtures -- --ignored` (needs `heif-enc` on `PATH`).

Real camera RAW files are not committed (4-22 MB each). `scripts/fetch-raw-samples.sh` downloads CC0
samples from <https://raw.pixls.us> with pinned checksums for the optional sample tests
(`CYANOTYPE_RAW_SAMPLES=<dir>`), and the RAW unit tests otherwise use synthetic files built in the test code.
