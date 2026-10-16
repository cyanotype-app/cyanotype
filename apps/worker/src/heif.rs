//! HEIC / HEIF / AVIF via libheif (vendored and statically linked; see docs/build.md).
//!
//! * Only the primary image is decoded. Thumbnails, depth maps and HDR gain maps are ignored.
//! * libheif applies the `irot`/`imir` transforms while decoding, so the pixels (and
//!   [`HeifHeader::width`]/`height`) are already upright. The EXIF orientation tag in a HEIF file is
//!   ignored on purpose: the HEIF spec says the transform properties are authoritative, and
//!   applying both would rotate twice.
//! * 10/12-bit images are converted to 8-bit by libheif; alpha is kept (and un-premultiplied).
//! * libheif cannot decode a HEIC at reduced size, so memory is saved on the output side:
//!   - Grid images (what iPhones write: a grid of independently coded tiles, typically 512x512) are decoded one tile at
//!     a time through libheif's tile API (`heif_tiles.rs`), and every tile is averaged down into a buffer about the
//!     size of the preview as soon as it is decoded. Peak memory is that buffer plus one tile, whatever the size.
//!   - Every other image is decoded whole by libheif, but its pixels go straight from libheif's plane into the same
//!     downscaling buffer instead of being copied into a packed full-size buffer first.
//!
//!   The caller budgets from [`HeifHeader::cost_bytes`] before calling [`decode_reduced`].
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use libheif_rs::{ColorSpace, DecodingOptions, HeifContext, ImageHandle, LibHeif, RgbChroma, SecurityLimits};

use crate::{
    exif::{self, ExifInfo},
    heif_tiles::{TileReader, Tiling},
    stream::{Accum, Block},
};

/// libheif's own refusal threshold (it checks before allocating). The worker memory budget is
/// the tighter bound in practice.
const MAX_PIXELS: u64 = 200_000_000;
/// libheif's cap on its total allocations while decoding one image.
const MAX_TOTAL_MEMORY: u64 = 1 << 30;

fn lib() -> &'static LibHeif {
    static LIB: OnceLock<LibHeif> = OnceLock::new();
    LIB.get_or_init(LibHeif::new)
}

/// The libheif version this binary was built with, e.g. `[1, 23, 1]`.
#[allow(dead_code)]
pub fn libheif_version() -> [u8; 3] {
    lib().version()
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeifHeader {
    /// Display size of the primary image: irot/imir already applied.
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
    /// Luma bit depth (8, 10, 12).
    pub bit_depth: u8,
    pub exif: ExifInfo,
    /// Set for grid images without alpha that are worth decoding tile by tile (see [`inspect`]).
    pub tiling: Option<Tiling>,
}

/// Largest tile (pixels) decoded on its own; bigger "tiles" are the whole image in all but name.
const MAX_TILE_PIXELS: u64 = 4096 * 4096;
/// Most tiles in a grid we will walk (libheif's own default limit is 4096 x 4096 / 64).
const MAX_TILES: u64 = 65_536;

/// Peak bytes per pixel of one tile: libheif's YCbCr planes (up to 16-bit 4:4:4), the interleaved RGB it converts them
/// to, and the HEVC decoder's reference pictures.
const TILE_BYTES_PER_PIXEL: u64 = 24;
/// Fixed cost of a tiled decode on top of the buffers: decoder state and thread-local tables.
const TILED_OVERHEAD: u64 = 40 << 20;
/// Whole-image decode, 8-bit (measured 4.2 B/px on 50 MP): libheif's YCbCr planes and the RGB image coexist while
/// converting, plus the HEVC decoder's picture buffers. The pixels are read from libheif's plane in place (no packed copy).
pub const FULL_BYTES_PER_PIXEL_8BIT: u64 = 6;
/// Whole-image decode of 10/12-bit (measured 6.2 B/px on 20 MP): 16-bit planes.
pub const FULL_BYTES_PER_PIXEL_HDR: u64 = 9;

impl HeifHeader {
    /// Estimated peak bytes of [`decode_reduced`] for this image, targeting a [`crate::imaging::PREVIEW_EDGE`] output.
    pub fn cost_bytes(&self) -> u64 {
        let out = Accum::bytes_for(self.width, self.height, crate::imaging::PREVIEW_EDGE, self.has_alpha);
        match &self.tiling {
            Some(t) => out + u64::from(t.tile_width) * u64::from(t.tile_height) * TILE_BYTES_PER_PIXEL + TILED_OVERHEAD,
            None => {
                let bpp = if self.bit_depth > 8 {
                    FULL_BYTES_PER_PIXEL_HDR
                } else {
                    FULL_BYTES_PER_PIXEL_8BIT
                };
                u64::from(self.width) * u64::from(self.height) * bpp + out
            }
        }
    }
}

fn open(bytes: &[u8]) -> Result<HeifContext<'_>> {
    let mut ctx = HeifContext::read_from_bytes(bytes).context("not a readable HEIF/AVIF file")?;
    let mut limits: SecurityLimits = ctx.security_limits();
    limits.set_max_image_size_pixels(MAX_PIXELS);
    limits.set_max_total_memory(MAX_TOTAL_MEMORY);
    ctx.set_security_limits(&limits)
        .context("set libheif security limits")?;
    // Decoding runs inside our own blocking pool; keep libheif single-threaded so memory and CPU
    // stay bounded by WORKER_HEAVY_CONCURRENCY.
    ctx.set_max_decoding_threads(1);
    Ok(ctx)
}

/// EXIF from the primary image's "Exif" item. The item payload starts with a 4-byte big-endian
/// offset to the TIFF header (skipping the optional "Exif\0\0" prefix).
fn exif_of(handle: &ImageHandle) -> ExifInfo {
    let mut ids = [0u32; 8];
    let n = handle.metadata_block_ids(&mut ids, b"Exif").min(ids.len());
    for &id in &ids[..n] {
        let Ok(data) = handle.metadata(id) else { continue };
        if data.len() < 8 {
            continue;
        }
        let off = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let Some(tiff) = data.get(4usize.saturating_add(off)..) else {
            continue;
        };
        if tiff.starts_with(b"II*\0") || tiff.starts_with(b"MM\0*") {
            return exif::parse_tiff(tiff);
        }
    }
    ExifInfo {
        orientation: 1,
        ..Default::default()
    }
}

/// Read the header only: size, depth, alpha and EXIF. No pixel decoding.
pub fn inspect(bytes: &[u8]) -> Result<HeifHeader> {
    let ctx = open(bytes)?;
    let handle = ctx.primary_image_handle().context("no primary image")?;
    let (width, height) = (handle.width(), handle.height());
    if width == 0 || height == 0 {
        bail!("empty HEIF image");
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        bail!("image is {width}x{height}, over the pixel limit");
    }
    let mut exif = exif_of(&handle);
    // The transforms are already part of `width`/`height` and of the decoded pixels.
    exif.orientation = 1;
    let has_alpha = handle.has_alpha_channel();
    // Alpha lives in a second image (itself a grid) that the tile API would have to be walked in step with; an
    // image with alpha is decoded whole, as before. Any trouble reading the tile layout means the same.
    // Only plain grids: if the display size is not the grid's own size (turned or not), a crop (`clap`) or another
    // property is in play and libheif's tile offsets stop meaning what the tiling code assumes.
    let (iw, ih) = (handle.ispe_width().max(0) as u32, handle.ispe_height().max(0) as u32);
    let plain = (iw, ih) == (width, height) || (ih, iw) == (width, height);
    let tiling = if has_alpha || !plain {
        None
    } else {
        tiling_of(bytes, width, height)
    };
    Ok(HeifHeader {
        width,
        height,
        has_alpha,
        bit_depth: handle.luma_bits_per_pixel(),
        exif,
        tiling,
    })
}

/// The tile layout, when the image is a grid of several reasonably sized tiles that agrees with the header.
fn tiling_of(bytes: &[u8], width: u32, height: u32) -> Option<Tiling> {
    let reader = TileReader::open(bytes, MAX_PIXELS, MAX_TOTAL_MEMORY).ok()?;
    let t = reader.tiling().ok()?;
    let sane = t.tile_count() > 1
        && t.tile_count() <= MAX_TILES
        && t.tile_width > 0
        && t.tile_height > 0
        && u64::from(t.tile_width) * u64::from(t.tile_height) <= MAX_TILE_PIXELS
        && (t.width, t.height) == (width, height)
        // The tiles must cover the image, or something about the layout is not what we think it is.
        && u64::from(t.columns) * u64::from(t.tile_width) >= u64::from(width) + u64::from(t.left_shift)
        && u64::from(t.rows) * u64::from(t.tile_height) >= u64::from(height) + u64::from(t.top_shift);
    sane.then_some(t)
}

/// Decode the primary image to upright 8-bit RGB(A), reduced by the integer factor that leaves its long edge at least
/// `target` pixels (see `stream::Accum`). Tiled where [`inspect`] found a grid, whole otherwise.
pub fn decode_reduced(bytes: &[u8], header: &HeifHeader, target: u32) -> Result<DynamicImage> {
    match &header.tiling {
        Some(t) => decode_tiled(bytes, header, t, target),
        None => decode_whole(bytes, target),
    }
}

fn decode_tiled(bytes: &[u8], header: &HeifHeader, t: &Tiling, target: u32) -> Result<DynamicImage> {
    let reader = TileReader::open(bytes, MAX_PIXELS, MAX_TOTAL_MEMORY)?;
    let mut acc = Accum::new(header.width, header.height, target, false)?;
    for ty in 0..t.rows {
        for tx in 0..t.columns {
            let tile = reader
                .decode_tile(tx, ty, false)
                .with_context(|| format!("tile {tx},{ty}"))?;
            let px = tile.pixels()?;
            // The grid may start left of / above the image (rotated or cropped grids) and overhang its far edges;
            // `add_block` clips both.
            let x0 = i64::from(tx) * i64::from(t.tile_width) - i64::from(t.left_shift);
            let y0 = i64::from(ty) * i64::from(t.tile_height) - i64::from(t.top_shift);
            acc.add_block(&Block {
                x0,
                y0,
                w: px.width,
                h: px.height,
                data: px.data,
                stride: px.stride,
                premultiplied: px.premultiplied,
            });
        }
    }
    acc.finish()
}

fn decode_whole(bytes: &[u8], target: u32) -> Result<DynamicImage> {
    let ctx = open(bytes)?;
    let handle = ctx.primary_image_handle().context("no primary image")?;
    let has_alpha = handle.has_alpha_channel();
    let chroma = if has_alpha { RgbChroma::Rgba } else { RgbChroma::Rgb };

    let mut opts = DecodingOptions::new().context("libheif decoding options")?;
    opts.set_ignore_transformations(false); // irot/imir applied by libheif
    opts.set_convert_hdr_to_8bit(true);
    let img = lib()
        .decode(&handle, ColorSpace::Rgb(chroma), Some(opts))
        .context("libheif decode")?;
    drop(handle);

    let planes = img.planes();
    let plane = planes.interleaved.context("decoder returned no interleaved plane")?;
    let (w, h) = (plane.width as usize, plane.height as usize);
    let channels = if has_alpha { 4 } else { 3 };
    if plane.bits_per_pixel != 8 {
        bail!(
            "unexpected pixel format ({} bits per pixel stored)",
            plane.storage_bits_per_pixel
        );
    }
    let row = w * channels;
    if plane.stride < row || plane.data.len() < plane.stride * (h.saturating_sub(1)) + row {
        bail!("decoder returned a truncated pixel plane");
    }
    let mut acc = Accum::new(w as u32, h as u32, target, has_alpha)?;
    acc.add_block(&Block {
        x0: 0,
        y0: 0,
        w: w as u32,
        h: h as u32,
        data: plane.data,
        stride: plane.stride,
        premultiplied: img.is_premultiplied_alpha(),
    });
    acc.finish()
}

/// The whole image at full size, for comparisons in tests.
#[cfg(test)]
pub fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    let (w, h) = {
        let hdr = inspect(bytes)?;
        (hdr.width, hdr.height)
    };
    decode_whole(bytes, w.max(h).max(1)) // target >= long edge: factor 1, no reduction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_libheif_version() {
        // libheif-sys 5.3.1+1.23.1 vendors libheif 1.23.1; bumping the crate must be a deliberate change.
        assert_eq!(libheif_version(), [1, 23, 1]);
    }

    const ROT6: &[u8] = include_bytes!("../tests/fixtures/rot6.heic");
    const PLAIN: &[u8] = include_bytes!("../tests/fixtures/plain.heic");
    const TENBIT: &[u8] = include_bytes!("../tests/fixtures/tenbit.heic");
    const ALPHA: &[u8] = include_bytes!("../tests/fixtures/alpha.heic");
    const AVIF: &[u8] = include_bytes!("../tests/fixtures/rot6.avif");

    fn near(p: image::Rgb<u8>, want: [u8; 3]) -> bool {
        p.0.iter()
            .zip(want)
            .all(|(a, b)| (i32::from(*a) - i32::from(b)).abs() < 40)
    }

    /// The stored image is 96x64 with a white marker top-left, red left half, blue right half, and an
    /// irot of 270 ccw (= rotate 90 CW). Upright it is 64x96: red on top, blue below, marker top-right.
    #[test]
    fn irot_is_applied_once_by_libheif() {
        let h = inspect(ROT6).unwrap();
        assert_eq!((h.width, h.height), (64, 96), "header reports the display size");
        assert_eq!(h.exif.orientation, 1, "EXIF orientation is ignored for HEIF");
        let img = decode(ROT6).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (64, 96));
        assert!(near(*img.get_pixel(58, 5), [255, 255, 255]), "marker top-right");
        assert!(near(*img.get_pixel(10, 30), [220, 30, 30]), "red on top");
        assert!(near(*img.get_pixel(10, 80), [30, 30, 220]), "blue below");
    }

    /// The fixture carries both an irot and an EXIF orientation of 6 (heif-enc 1.17 copies the
    /// source's EXIF verbatim). Applying both would rotate it 180 degrees; `inspect` must not.
    #[test]
    fn exif_orientation_is_present_but_ignored() {
        let ctx = open(ROT6).unwrap();
        let handle = ctx.primary_image_handle().unwrap();
        assert_eq!(exif_of(&handle).orientation, 6, "fixture has EXIF orientation 6");
        assert_eq!(inspect(ROT6).unwrap().exif.orientation, 1);
    }

    #[test]
    fn exif_is_extracted_from_the_heif_item() {
        let h = inspect(ROT6).unwrap();
        assert_eq!(h.exif.camera_make.as_deref(), Some("ACME"));
        assert_eq!(h.exif.camera_model.as_deref(), Some("Shooter 9"));
        assert_eq!(h.exif.lens_model.as_deref(), Some("ACME 24mm f/2"));
        assert_eq!(
            h.exif.taken_at,
            crate::exif::parse_datetime("2021:12:31 23:59:58", Some("-05:00"))
        );
        assert!((h.exif.latitude.unwrap() + 33.85).abs() < 1e-6);
        assert!((h.exif.longitude.unwrap() + 151.21).abs() < 1e-6);
    }

    #[test]
    fn untransformed_image_keeps_its_orientation() {
        let h = inspect(PLAIN).unwrap();
        assert_eq!((h.width, h.height), (96, 64));
        let img = decode(PLAIN).unwrap().to_rgb8();
        assert!(near(*img.get_pixel(3, 3), [255, 255, 255]), "marker top-left");
        assert!(near(*img.get_pixel(20, 40), [220, 30, 30]));
        assert!(near(*img.get_pixel(80, 40), [30, 30, 220]));
    }

    #[test]
    fn ten_bit_is_converted_to_eight_bit() {
        let h = inspect(TENBIT).unwrap();
        assert_eq!(h.bit_depth, 10);
        let img = decode(TENBIT).unwrap();
        let rgb = img.as_rgb8().expect("8-bit RGB output");
        assert!(near(*rgb.get_pixel(3, 3), [255, 255, 255]));
        assert!(near(*rgb.get_pixel(20, 40), [219, 27, 27]));
        assert!(near(*rgb.get_pixel(80, 40), [27, 27, 219]));
    }

    #[test]
    fn alpha_is_preserved() {
        let h = inspect(ALPHA).unwrap();
        assert!(h.has_alpha);
        let img = decode(ALPHA).unwrap();
        let rgba = img.as_rgba8().expect("RGBA output");
        assert!(rgba.get_pixel(10, 10).0[3] > 240);
        assert!(rgba.get_pixel(80, 10).0[3] > 240);
        let a = rgba.get_pixel(80, 50).0[3];
        assert!((40..100).contains(&a), "translucent corner, got {a}");
    }

    #[test]
    fn avif_decodes_through_the_same_path() {
        // heif-enc 1.17 writes no irot for AVIF, so this one is stored upright.
        let h = inspect(AVIF).unwrap();
        assert_eq!((h.width, h.height), (96, 64));
        let img = decode(AVIF).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (96, 64));
        assert!(near(*img.get_pixel(3, 3), [255, 255, 255]));
        assert!(near(*img.get_pixel(20, 40), [220, 30, 30]));
        assert!(near(*img.get_pixel(80, 40), [30, 30, 220]));
    }

    // ---- grid images, decoded tile by tile ----

    use crate::testgen::{self, GridSpec, GridTransform, heic_grid, heic_tile_parts};

    const GRID_PLAIN: &[u8] = include_bytes!("../tests/fixtures/grid_plain.heic");
    const GRID_IROT: &[u8] = include_bytes!("../tests/fixtures/grid_irot.heic");
    const GRID_IMIR_IROT: &[u8] = include_bytes!("../tests/fixtures/grid_imir_irot.heic");
    const GRID_IROT180: &[u8] = include_bytes!("../tests/fixtures/grid_irot180.heic");
    const TILE512: &[u8] = include_bytes!("../tests/fixtures/tile512.heic");

    /// Mean and worst absolute channel difference between two same-sized images.
    fn diff(a: &DynamicImage, b: &DynamicImage) -> (f64, u8) {
        let (a, b) = (a.to_rgb8(), b.to_rgb8());
        assert_eq!((a.width(), a.height()), (b.width(), b.height()));
        let (mut sum, mut worst) = (0u64, 0u8);
        for (x, y) in a.as_raw().iter().zip(b.as_raw()) {
            sum += u64::from(x.abs_diff(*y));
            worst = worst.max(x.abs_diff(*y));
        }
        (sum as f64 / a.as_raw().len() as f64, worst)
    }

    #[test]
    fn grid_layout_is_detected() {
        let h = inspect(GRID_PLAIN).unwrap();
        let t = h.tiling.expect("grid image has a tiling");
        assert_eq!((t.columns, t.rows, t.tile_width, t.tile_height), (3, 2, 64, 64));
        assert_eq!(
            (h.width, h.height),
            (180, 120),
            "the grid is cropped to its output size"
        );
        assert_eq!((t.left_shift, t.top_shift), (0, 0));
        // Single-image HEICs, alpha, 10-bit and AVIF are not grids.
        for f in [PLAIN, ROT6, TENBIT, ALPHA, AVIF] {
            assert!(inspect(f).unwrap().tiling.is_none());
        }
    }

    /// Tile-by-tile decoding must give the picture libheif's own whole-image decode gives.
    #[test]
    fn tiled_decode_matches_the_whole_decode() {
        for (name, file) in [
            ("plain", GRID_PLAIN),
            ("irot", GRID_IROT),
            ("imir+irot", GRID_IMIR_IROT),
            ("irot180", GRID_IROT180),
        ] {
            let h = inspect(file).unwrap();
            assert!(h.tiling.is_some(), "{name}");
            let whole = decode(file).unwrap();
            assert_eq!(
                (whole.width(), whole.height()),
                (h.width, h.height),
                "{name}: display size"
            );
            // No reduction (target above the long edge): every pixel.
            let tiled = decode_reduced(file, &h, 4000).unwrap();
            assert_eq!((tiled.width(), tiled.height()), (h.width, h.height), "{name}");
            let (mean, worst) = diff(&whole, &tiled);
            assert!(
                mean < 1.0 && worst < 24,
                "{name}: tiled vs whole mean {mean:.2} worst {worst}"
            );

            // Reduced: against the whole decode averaged down by the same factor.
            let target = 30; // factor 4 on the 180 px edge
            let reduced = decode_reduced(file, &h, target).unwrap();
            let mut acc = Accum::new(whole.width(), whole.height(), target, false).unwrap();
            let rgb = whole.to_rgb8();
            acc.add_block(&Block {
                x0: 0,
                y0: 0,
                w: rgb.width(),
                h: rgb.height(),
                data: rgb.as_raw(),
                stride: rgb.width() as usize * 3,
                premultiplied: false,
            });
            let want = acc.finish().unwrap();
            assert_eq!(
                (reduced.width(), reduced.height()),
                (want.width(), want.height()),
                "{name}"
            );
            let (mean, worst) = diff(&want, &reduced);
            assert!(mean < 1.0 && worst < 24, "{name}: reduced mean {mean:.2} worst {worst}");
        }
    }

    /// The source is 180x120 with a white 12x12 marker top-left, red left half and blue right half. `irot` 1 turns it 90
    /// degrees counter-clockwise (120x180: red at the bottom, marker bottom-left); `imir` axis 1 flips left-right
    /// first.
    #[test]
    fn tiled_decode_applies_rotation_and_mirroring_once() {
        let probe = |file: &[u8]| {
            let h = inspect(file).unwrap();
            let img = decode_reduced(file, &h, 4000).unwrap().to_rgb8();
            (h, img)
        };
        let (h, img) = probe(GRID_PLAIN);
        assert_eq!((img.width(), img.height()), (180, 120));
        assert!(near(*img.get_pixel(5, 5), [255, 255, 255]), "marker top-left");
        assert!(near(*img.get_pixel(40, 80), [220, 30, 30]) && near(*img.get_pixel(140, 80), [30, 30, 220]));
        assert!(h.tiling.unwrap().left_shift == 0);

        let (h, img) = probe(GRID_IROT);
        assert_eq!((h.width, h.height), (120, 180));
        assert!(
            near(*img.get_pixel(5, 174), [255, 255, 255]),
            "marker bottom-left after a quarter turn ccw"
        );
        assert!(near(*img.get_pixel(60, 40), [30, 30, 220]), "blue on top");
        assert!(near(*img.get_pixel(60, 140), [220, 30, 30]), "red below");

        // Half a turn moves the overhang to the left/top, so the first tile column and row start outside the image.
        let (h, img) = probe(GRID_IROT180);
        let t = h.tiling.unwrap();
        assert_eq!((t.left_shift, t.top_shift), (12, 8));
        assert!(
            near(*img.get_pixel(174, 114), [255, 255, 255]),
            "marker bottom-right after a half turn"
        );
        assert!(near(*img.get_pixel(40, 40), [30, 30, 220]) && near(*img.get_pixel(140, 40), [220, 30, 30]));

        let (h, img) = probe(GRID_IMIR_IROT);
        let whole = decode(GRID_IMIR_IROT).unwrap().to_rgb8();
        assert_eq!((h.width, h.height), (whole.width(), whole.height()));
        // Wherever libheif puts the marker for the whole decode, the tiled decode puts it too.
        for (x, y) in [(5, 5), (h.width - 6, 5), (5, h.height - 6), (h.width - 6, h.height - 6)] {
            assert_eq!(
                near(*whole.get_pixel(x, y), [255, 255, 255]),
                near(*img.get_pixel(x, y), [255, 255, 255]),
                "marker at ({x},{y})"
            );
        }
    }

    #[test]
    fn every_tile_is_decoded_and_the_picture_is_whole() {
        // 4x3 tiles of one 512x512 picture in 2048x1536: a tile missing or misplaced would show as a hole or seam.
        let tile = heic_tile_parts(TILE512);
        let file = heic_grid(&[tile], GridSpec::new(4, 3, 512, 2048, 1536), GridTransform::default());
        let h = inspect(&file).unwrap();
        assert_eq!(h.tiling.map(|t| t.tile_count()), Some(12));
        let img = decode_reduced(&file, &h, 1024).unwrap();
        assert_eq!((img.width(), img.height()), (2048 / 2, 1536 / 2));
        let whole = decode(&file).unwrap();
        let mut acc = Accum::new(2048, 1536, 1024, false).unwrap();
        let rgb = whole.to_rgb8();
        acc.add_block(&Block {
            x0: 0,
            y0: 0,
            w: 2048,
            h: 1536,
            data: rgb.as_raw(),
            stride: 2048 * 3,
            premultiplied: false,
        });
        let (mean, worst) = diff(&acc.finish().unwrap(), &img);
        assert!(mean < 1.0 && worst < 24, "mean {mean:.2} worst {worst}");
    }

    #[test]
    fn grid_cost_does_not_grow_with_the_image() {
        let tile = heic_tile_parts(TILE512);
        let small = inspect(&heic_grid(
            std::slice::from_ref(&tile),
            GridSpec::new(4, 3, 512, 2048, 1536),
            GridTransform::default(),
        ))
        .unwrap();
        // 48 MP, like an iPhone: 8064 x 6048 in 16 x 12 tiles.
        let big = inspect(&heic_grid(
            &[tile],
            GridSpec::new(16, 12, 512, 8064, 6048),
            GridTransform::default(),
        ))
        .unwrap();
        assert!(big.tiling.is_some());
        let (cs, cb) = (small.cost_bytes(), big.cost_bytes());
        assert!(cb < 96 << 20, "a 48 MP grid is budgeted at {} MB", cb >> 20);
        assert!(
            cb < cs + (16 << 20),
            "cost should follow the output, not the source ({} vs {} MB)",
            cb >> 20,
            cs >> 20
        );
        // The same size as a single image would be budgeted at full size.
        let whole = HeifHeader {
            tiling: None,
            ..big.clone()
        };
        assert!(whole.cost_bytes() > 4 * cb);
    }

    #[test]
    fn broken_grids_fail_cleanly() {
        let tile = heic_tile_parts(TILE512);
        let file = heic_grid(&[tile], GridSpec::new(4, 3, 512, 2048, 1536), GridTransform::default());
        for cut in [file.len() - 3000, file.len() / 2, 200] {
            // No panic and no abort; an error or a partial result is fine.
            if let Ok(h) = inspect(&file[..cut]) {
                let _ = decode_reduced(&file[..cut], &h, 500);
            }
        }
        // A tile stream that is not HEVC.
        let junk = (heic_tile_parts(TILE512).0, vec![0xAB; 500]);
        let file = heic_grid(&[junk], GridSpec::new(2, 2, 512, 1024, 1024), GridTransform::default());
        if let Ok(h) = inspect(&file) {
            assert!(decode_reduced(&file, &h, 500).is_err());
        }
    }

    /// Regenerates the grid fixtures from `heif-enc` (libheif-examples + libheif-plugin-x265):
    ///   cargo test -p cyanotype-worker regenerate_grid_fixtures -- --ignored
    #[test]
    #[ignore = "writes tests/fixtures; needs heif-enc"]
    fn regenerate_grid_fixtures() {
        use image::{Rgb, RgbImage};
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let tmp = std::env::temp_dir().join(format!("cyanotype-grid-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let enc = |name: &str, img: &RgbImage, q: &str| -> (Vec<u8>, Vec<u8>) {
            let (src, out) = (tmp.join(format!("{name}.png")), tmp.join(format!("{name}.heic")));
            img.save(&src).unwrap();
            let st = std::process::Command::new("heif-enc")
                .args(["-q", q, "-o"])
                .arg(&out)
                .arg(&src)
                .status()
                .expect("heif-enc on PATH");
            assert!(st.success());
            heic_tile_parts(&std::fs::read(out).unwrap())
        };
        // The 180x120 marker picture (see the README), cut into 64x64 tiles; the grid overhangs it by 12 px / 8 px.
        let src = |x: u32, y: u32| {
            if x < 12 && y < 12 {
                Rgb([255, 255, 255])
            } else if x < 90 {
                Rgb([220, 30, 30])
            } else {
                Rgb([30, 30, 220])
            }
        };
        let mut tiles = Vec::new();
        for ty in 0..2 {
            for tx in 0..3 {
                let img = RgbImage::from_fn(64, 64, |x, y| src(tx * 64 + x, ty * 64 + y));
                tiles.push(enc(&format!("t{tx}{ty}"), &img, "90"));
            }
        }
        for (name, tf) in [
            ("grid_plain", GridTransform::default()),
            ("grid_irot", GridTransform { irot: 1, imir: None }),
            ("grid_imir_irot", GridTransform { irot: 3, imir: Some(1) }),
            ("grid_irot180", GridTransform { irot: 2, imir: None }),
        ] {
            let f = heic_grid(&tiles, GridSpec::new(3, 2, 64, 180, 120), tf);
            std::fs::write(dir.join(format!("{name}.heic")), f).unwrap();
        }
        // One 512x512 tile of photo-like content, repeated to build iPhone-sized grids at test time.
        let big = RgbImage::from_fn(512, 512, |x, y| Rgb(testgen::pixel(x + 700, y + 300, 2048, 2048)));
        let tile = enc("tile512", &big, "42");
        let single = heic_grid(&[tile], GridSpec::new(1, 1, 512, 512, 512), GridTransform::default());
        // Keep it as a plain single-image HEIC file: lift the pieces back out of a one-tile "grid" is pointless, so
        // write the heif-enc file itself.
        let _ = single;
        std::fs::copy(tmp.join("tile512.heic"), dir.join("tile512.heic")).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn truncated_file_fails_cleanly() {
        for cut in [20, 100, ROT6.len() / 2, ROT6.len() - 8] {
            // Either error is fine; what matters is no panic/abort.
            let _ = inspect(&ROT6[..cut]);
            let _ = decode(&ROT6[..cut]);
        }
    }

    #[test]
    fn garbage_is_an_error_not_a_crash() {
        assert!(inspect(b"definitely not a heif file at all").is_err());
        assert!(decode(&[0u8; 64]).is_err());
        // A valid ftyp with nothing after it.
        let mut b = Vec::new();
        b.extend(16u32.to_be_bytes());
        b.extend(b"ftypheic");
        b.extend([0u8; 4]);
        assert!(inspect(&b).is_err());
    }
}
