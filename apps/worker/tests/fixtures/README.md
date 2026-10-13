# Worker test fixtures

Tiny HEIC/AVIF files used by the unit tests in `apps/worker/src/heif.rs`. Every file is under 2 KB.

They are synthetic: the pixels are generated programmatically (no photograph, nobody's copyright), so
they are released under CC0 1.0 like the rest of this directory's content. Provenance:

| File | What it is |
|---|---|
| `rot6.heic` | 96x64 stored, with an `irot` (90 deg clockwise, so 64x96 upright) **and** an EXIF block that still says orientation 6, plus Make/Model/lens/DateTimeOriginal/OffsetTime/GPS. Proves irot is applied once and the EXIF orientation is not applied on top. |
| `plain.heic` | The same 96x64 picture with no rotation. |
| `tenbit.heic` | 10-bit (from a 16-bit PNG), checks the down-conversion to 8-bit. |
| `alpha.heic` | RGBA, with a translucent quadrant. |
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

Real camera RAW files are not committed (4-22 MB each). `scripts/fetch-raw-samples.sh` downloads CC0
samples from <https://raw.pixls.us> with pinned checksums for the optional sample tests
(`CYANOTYPE_RAW_SAMPLES=<dir>`), and the RAW unit tests otherwise use synthetic files built in the test code.
