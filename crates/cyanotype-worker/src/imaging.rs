//! Decode, auto-orient, resize, WebP-encode and ThumbHash. Pure CPU work; call from
//! `spawn_blocking`.
use std::io::Cursor;

use anyhow::{Context, Result, bail};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image as FirImage};
use image::{DynamicImage, ImageReader};

pub const THUMB_EDGE: u32 = 320;
pub const PREVIEW_EDGE: u32 = 1440;
const THUMB_QUALITY: f32 = 75.0;
const PREVIEW_QUALITY: f32 = 80.0;
/// Refuse to decode anything with more pixels than this (decompression-bomb guard).
const MAX_PIXELS: u64 = 400_000_000;

pub struct Derived {
    pub thumb_webp: Vec<u8>,
    pub preview_webp: Vec<u8>,
    pub thumbhash: Vec<u8>,
    /// Oriented size of the source image.
    pub width: u32,
    pub height: u32,
}

/// Dimensions after applying an EXIF orientation (5..=8 swap axes).
pub fn oriented_dims(w: u32, h: u32, orientation: u8) -> (u32, u32) {
    if (5..=8).contains(&orientation) { (h, w) } else { (w, h) }
}

/// Fit `(w, h)` inside a square of `max_edge` on the long side, preserving aspect ratio.
/// Never upscales and never returns a zero side.
pub fn fit_within(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max_edge {
        return (w, h);
    }
    let scale = f64::from(max_edge) / f64::from(long);
    let s = |v: u32| ((f64::from(v) * scale).round() as u32).max(1);
    (s(w), s(h))
}

/// Image header dimensions without a full decode (not orientation-adjusted).
pub fn raw_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()
        .context("read image dimensions")
}

pub fn apply_orientation(img: DynamicImage, orientation: u8) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

pub fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let (w, h) = reader.into_dimensions().context("read image dimensions")?;
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        bail!("image is {w}x{h}, over the pixel limit");
    }
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .decode()
        .context("decode image")
}

/// Resize with Lanczos3; alpha-aware (premultiplied) when the image has alpha.
fn resize(img: &DynamicImage, w: u32, h: u32) -> Result<DynamicImage> {
    if (img.width(), img.height()) == (w, h) {
        return Ok(img.clone());
    }
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    let mut resizer = Resizer::new();
    if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        let src = FirImage::from_vec_u8(rgba.width(), rgba.height(), rgba.into_raw(), PixelType::U8x4)?;
        let mut premul = FirImage::new(src.width(), src.height(), PixelType::U8x4);
        let muldiv = fast_image_resize::MulDiv::default();
        muldiv.multiply_alpha(&src, &mut premul)?;
        let mut dst = FirImage::new(w, h, PixelType::U8x4);
        resizer.resize(&premul, &mut dst, &opts)?;
        muldiv.divide_alpha_inplace(&mut dst)?;
        let buf = image::RgbaImage::from_raw(w, h, dst.into_vec()).context("rgba buffer")?;
        Ok(DynamicImage::ImageRgba8(buf))
    } else {
        let rgb = img.to_rgb8();
        let src = FirImage::from_vec_u8(rgb.width(), rgb.height(), rgb.into_raw(), PixelType::U8x3)?;
        let mut dst = FirImage::new(w, h, PixelType::U8x3);
        resizer.resize(&src, &mut dst, &opts)?;
        let buf = image::RgbImage::from_raw(w, h, dst.into_vec()).context("rgb buffer")?;
        Ok(DynamicImage::ImageRgb8(buf))
    }
}

pub fn encode_webp(img: &DynamicImage, quality: f32) -> Result<Vec<u8>> {
    let mem = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
            .encode(quality)
            .to_vec()
    } else {
        let rgb = img.to_rgb8();
        webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height())
            .encode(quality)
            .to_vec()
    };
    Ok(mem)
}

pub fn thumbhash(img: &DynamicImage) -> Result<Vec<u8>> {
    // ThumbHash input must be at most 100x100.
    let (w, h) = fit_within(img.width(), img.height(), 100);
    let small = resize(img, w, h)?.to_rgba8();
    Ok(thumbhash::rgba_to_thumb_hash(w as usize, h as usize, small.as_raw()))
}

/// Decode `bytes`, orient by EXIF `orientation`, and produce all derived artifacts.
pub fn derive(bytes: &[u8], orientation: u8) -> Result<Derived> {
    let img = apply_orientation(decode(bytes)?, orientation);
    let (width, height) = (img.width(), img.height());

    let (pw, ph) = fit_within(width, height, PREVIEW_EDGE);
    let preview = resize(&img, pw, ph)?;
    drop(img);
    // Thumb comes from the preview: far fewer pixels to filter than the original.
    let (tw, th) = fit_within(width, height, THUMB_EDGE);
    let thumb = resize(&preview, tw, th)?;

    Ok(Derived {
        thumb_webp: encode_webp(&thumb, THUMB_QUALITY)?,
        preview_webp: encode_webp(&preview, PREVIEW_QUALITY)?,
        thumbhash: thumbhash(&thumb)?,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif::testutil::{exif_app1, with_app1};
    use image::{Rgb, RgbImage};

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = RgbImage::from_fn(w, h, |x, y| Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 90]));
        let mut out = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    }

    fn webp_dims(b: &[u8]) -> (u32, u32) {
        let d = webp::Decoder::new(b).decode().expect("valid webp");
        (d.width(), d.height())
    }

    #[test]
    fn fit_within_cases() {
        assert_eq!(fit_within(4000, 3000, 1440), (1440, 1080));
        assert_eq!(fit_within(3000, 4000, 320), (240, 320));
        assert_eq!(fit_within(100, 50, 320), (100, 50)); // no upscaling
        assert_eq!(fit_within(10_000, 1, 320), (320, 1)); // never zero
    }

    #[test]
    fn oriented_dims_swap() {
        assert_eq!(oriented_dims(4, 2, 1), (4, 2));
        assert_eq!(oriented_dims(4, 2, 6), (2, 4));
        assert_eq!(oriented_dims(4, 2, 8), (2, 4));
    }

    #[test]
    fn derive_sizes_landscape() {
        let d = derive(&jpeg(2000, 1000), 1).unwrap();
        assert_eq!((d.width, d.height), (2000, 1000));
        assert_eq!(webp_dims(&d.preview_webp), (1440, 720));
        assert_eq!(webp_dims(&d.thumb_webp), (320, 160));
        assert!(!d.thumbhash.is_empty());
    }

    #[test]
    fn derive_orients_portrait_from_exif() {
        // Stored 800x400, orientation 6 (rotate 90 CW) => displayed 400x800.
        let d = derive(&jpeg(800, 400), 6).unwrap();
        assert_eq!((d.width, d.height), (400, 800));
        assert_eq!(webp_dims(&d.thumb_webp), (160, 320));
        assert_eq!(webp_dims(&d.preview_webp), (400, 800));
    }

    #[test]
    fn small_images_are_not_upscaled() {
        let d = derive(&jpeg(64, 48), 1).unwrap();
        assert_eq!(webp_dims(&d.thumb_webp), (64, 48));
        assert_eq!(webp_dims(&d.preview_webp), (64, 48));
    }

    #[test]
    fn rotation_moves_pixels() {
        // Left half red, right half blue. Rotating 90 CW puts red on top.
        let img = RgbImage::from_fn(20, 10, |x, _| if x < 10 { Rgb([255, 0, 0]) } else { Rgb([0, 0, 255]) });
        let r = apply_orientation(DynamicImage::ImageRgb8(img), 6).to_rgb8();
        assert_eq!((r.width(), r.height()), (10, 20));
        assert_eq!(r.get_pixel(5, 2).0, [255, 0, 0]);
        assert_eq!(r.get_pixel(5, 17).0, [0, 0, 255]);
    }

    #[test]
    fn png_with_alpha_roundtrips() {
        let img = image::RgbaImage::from_pixel(600, 300, image::Rgba([10, 200, 10, 128]));
        let mut png = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let d = derive(&png, 1).unwrap();
        assert_eq!(webp_dims(&d.thumb_webp), (320, 160));
    }

    #[test]
    fn raw_dims_and_exif_end_to_end() {
        let app1 = exif_app1(&[(0x0112, 3, 8u16.to_le_bytes().to_vec())], &[], &[]);
        let bytes = with_app1(&jpeg(300, 100), &app1);
        assert_eq!(raw_dimensions(&bytes).unwrap(), (300, 100));
        let info = crate::exif::parse(&bytes);
        assert_eq!(info.orientation, 8);
        let d = derive(&bytes, info.orientation).unwrap();
        assert_eq!((d.width, d.height), (100, 300));
    }

    #[test]
    fn garbage_fails() {
        assert!(derive(b"definitely not an image", 1).is_err());
    }
}
