//! HEIC / HEIF / AVIF via libheif (vendored and statically linked; see docs/build.md).
//!
//! * Only the primary image is decoded. Thumbnails, depth maps and HDR gain maps are ignored.
//! * libheif applies the `irot`/`imir` transforms while decoding, so the pixels (and
//!   [`HeifHeader::width`]/`height`) are already upright. The EXIF orientation tag in a HEIF file is
//!   ignored on purpose: the HEIF spec says the transform properties are authoritative, and
//!   applying both would rotate twice.
//! * 10/12-bit images are converted to 8-bit by libheif; alpha is kept (and un-premultiplied).
//! * Decoding is full-size (libheif cannot decode a HEIC at reduced size), so the caller must
//!   budget memory from [`HeifHeader`] before calling [`decode`].
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, RgbImage, RgbaImage};
use libheif_rs::{ColorSpace, DecodingOptions, HeifContext, ImageHandle, LibHeif, RgbChroma, SecurityLimits};

use crate::exif::{self, ExifInfo};

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
    Ok(HeifHeader {
        width,
        height,
        has_alpha: handle.has_alpha_channel(),
        bit_depth: handle.luma_bits_per_pixel(),
        exif,
    })
}

/// Decode the primary image to upright 8-bit RGB(A).
pub fn decode(bytes: &[u8]) -> Result<DynamicImage> {
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

    let (w, h, mut buf) = {
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
        let mut buf = Vec::with_capacity(row * h);
        for y in 0..h {
            buf.extend_from_slice(&plane.data[y * plane.stride..y * plane.stride + row]);
        }
        (w, h, buf)
    };
    let premultiplied = img.is_premultiplied_alpha();
    drop(img);

    if has_alpha {
        if premultiplied {
            unpremultiply(&mut buf);
        }
        Ok(DynamicImage::ImageRgba8(
            RgbaImage::from_raw(w as u32, h as u32, buf).context("rgba buffer")?,
        ))
    } else {
        Ok(DynamicImage::ImageRgb8(
            RgbImage::from_raw(w as u32, h as u32, buf).context("rgb buffer")?,
        ))
    }
}

fn unpremultiply(rgba: &mut [u8]) {
    for p in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpremultiply_restores_color() {
        let mut px = [64u8, 32, 0, 128, 10, 20, 30, 255, 5, 5, 5, 0];
        unpremultiply(&mut px);
        assert_eq!(&px[..4], &[128, 64, 0, 128]);
        assert_eq!(&px[4..8], &[10, 20, 30, 255]); // opaque untouched
        assert_eq!(&px[8..], &[5, 5, 5, 0]); // fully transparent untouched
    }

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
