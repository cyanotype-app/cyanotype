//! Decode, auto-orient, resize, WebP-encode and ThumbHash. Pure CPU work; call from
//! `spawn_blocking`.
use std::io::Cursor;

use anyhow::{Context, Result, bail};
use cyanotype_core::recipe::Recipe;
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image as FirImage, ImageRef as FirImageRef},
};
use image::{DynamicImage, ImageReader, RgbImage};

pub const THUMB_EDGE: u32 = 320;
pub const PREVIEW_EDGE: u32 = 1440;
const THUMB_QUALITY: f32 = 75.0;
const PREVIEW_QUALITY: f32 = 80.0;
/// Refuse to decode anything with more pixels than this (decompression-bomb guard).
pub const MAX_PIXELS: u64 = 400_000_000;

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
#[cfg(test)]
pub fn raw_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()
        .context("read image dimensions")
}

/// Header size and sample layout without a full decode, for memory estimates.
pub fn raw_header(bytes: &[u8]) -> Result<(u32, u32, image::ColorType)> {
    use image::ImageDecoder;
    let dec = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_decoder()
        .context("read image header")?;
    let (w, h) = dec.dimensions();
    Ok((w, h, dec.color_type()))
}

/// Estimated peak bytes per pixel to decode and resize an image of this layout. Measured on 20-50 MP
/// images (docs/build.md): RGB8 about 3.5, RGBA8 about 11 (premultiplied copy), 16-bit more.
pub fn raster_bytes_per_pixel(c: image::ColorType) -> u64 {
    use image::ColorType::*;
    match c {
        L8 | Rgb8 => 5,
        La8 | Rgba8 => 12,
        L16 | Rgb16 => 10,
        La16 | Rgba16 => 16,
        _ => 16,
    }
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

/// Resize with Lanczos3; alpha-aware (premultiplied) when the image has alpha. 8-bit RGB(A) input is
/// borrowed rather than copied, which matters at 50+ MP.
pub fn resize(img: &DynamicImage, w: u32, h: u32) -> Result<DynamicImage> {
    if (img.width(), img.height()) == (w, h) {
        return Ok(img.clone());
    }
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    let mut resizer = Resizer::new();
    if img.color().has_alpha() {
        let owned;
        let rgba = match img.as_rgba8() {
            Some(b) => b,
            None => {
                owned = img.to_rgba8();
                &owned
            }
        };
        let src = FirImageRef::new(rgba.width(), rgba.height(), rgba.as_raw(), PixelType::U8x4)?;
        let mut premul = FirImage::new(src.width(), src.height(), PixelType::U8x4);
        let muldiv = fast_image_resize::MulDiv::default();
        muldiv.multiply_alpha(&src, &mut premul)?;
        let mut dst = FirImage::new(w, h, PixelType::U8x4);
        resizer.resize(&premul, &mut dst, &opts)?;
        muldiv.divide_alpha_inplace(&mut dst)?;
        let buf = image::RgbaImage::from_raw(w, h, dst.into_vec()).context("rgba buffer")?;
        Ok(DynamicImage::ImageRgba8(buf))
    } else {
        let owned;
        let rgb = match img.as_rgb8() {
            Some(b) => b,
            None => {
                owned = img.to_rgb8();
                &owned
            }
        };
        let src = FirImageRef::new(rgb.width(), rgb.height(), rgb.as_raw(), PixelType::U8x3)?;
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

// ---- JPEG: DCT-scaled decoding -------------------------------------------------------------------------------------

/// What the scaled JPEG path needs to know about a file, read from its frame header (SOFn) without decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpegInfo {
    pub width: u32,
    pub height: u32,
    pub progressive: bool,
    /// The SOFn marker byte: 0xC0 baseline, 0xC1 extended sequential, 0xC2 progressive (Huffman), others are
    /// lossless or arithmetic coded.
    pub sof: u8,
    /// Sample precision in bits (8 for nearly every file).
    pub precision: u8,
    /// Per component: horizontal and vertical sampling factors.
    pub components: Vec<(u8, u8)>,
}

/// Read the frame header of a JPEG. `None` when there is none before the first scan, or the file is not a JPEG.
pub fn jpeg_info(bytes: &[u8]) -> Option<JpegInfo> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let m = bytes[i + 1];
        if m == 0xFF {
            i += 1; // fill byte
            continue;
        }
        if m == 0x01 || (0xD0..=0xD8).contains(&m) {
            i += 2; // markers without a length
            continue;
        }
        if m == 0xD9 || m == 0xDA {
            return None; // end of image / start of scan before any frame header
        }
        let len = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
        // SOF0..SOF15 except DHT (C4), JPG (C8) and DAC (CC).
        if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            let seg = bytes.get(i + 4..i + 2 + len)?;
            if seg.len() < 6 {
                return None;
            }
            let (h, w) = (
                u32::from(u16::from_be_bytes([seg[1], seg[2]])),
                u32::from(u16::from_be_bytes([seg[3], seg[4]])),
            );
            let n = usize::from(seg[5]);
            if seg.len() < 6 + 3 * n || n == 0 {
                return None;
            }
            let components = (0..n).map(|c| (seg[7 + 3 * c] >> 4, seg[7 + 3 * c] & 15)).collect();
            return Some(JpegInfo {
                width: w,
                height: h,
                progressive: m == 0xC2,
                sof: m,
                precision: seg[0],
                components,
            });
        }
        i += 2 + len;
    }
    None
}

impl JpegInfo {
    /// The scaled decoder handles baseline and progressive Huffman JPEGs of 8-bit gray, YCbCr/RGB and CMYK/YCCK.
    /// Anything else (12-bit, lossless, arithmetic coding, a zero height from a DNL marker) takes the full decode.
    pub fn scalable(&self) -> bool {
        matches!(self.sof, 0xC0..=0xC2)
            && self.precision == 8
            && self.height > 0
            && self.width > 0
            && matches!(self.components.len(), 1 | 3 | 4)
            && self
                .components
                .iter()
                .all(|&(h, v)| (1..=4).contains(&h) && (1..=4).contains(&v))
    }

    /// Reduction chosen for this image: `n` in eighths, i.e. the decoder produces `n/8` of the size. 8 means no
    /// reduction. It is the smallest `n` whose long edge is still at least [`PREVIEW_EDGE`].
    #[cfg(test)]
    pub fn scale_eighths(&self) -> u32 {
        self.scale_eighths_for(PREVIEW_EDGE)
    }

    /// [`Self::scale_eighths`] for an output whose long edge must be at least `edge` (a crop needs more of the original).
    pub fn scale_eighths_for(&self, edge: u32) -> u32 {
        let long = self.width.max(self.height);
        [1u32, 2, 4]
            .into_iter()
            .find(|&n| scaled_len(long, n) >= edge)
            .unwrap_or(8)
    }

    /// Size of the decoded image at `n` eighths.
    pub fn scaled_dims(&self, n: u32) -> (u32, u32) {
        (scaled_len(self.width, n), scaled_len(self.height, n))
    }

    /// Estimated peak bytes of [`decode_jpeg_scaled_to`] at the reduction [`Self::scale_eighths_for`] picks for `edge`.
    ///
    /// Baseline: the decoder works one MCU row at a time and keeps only its scaled component planes plus the packed
    /// output. Progressive: every coefficient of the full-size image (2 bytes each) is kept until the last scan.
    pub fn scaled_cost_bytes_for(&self, edge: u32) -> u64 {
        let n = self.scale_eighths_for(edge);
        let (sw, sh) = self.scaled_dims(n);
        let spx = u64::from(sw) * u64::from(sh);
        let fpx = u64::from(self.width) * u64::from(self.height);
        let (hmax, vmax) = self.components.iter().fold((1u32, 1u32), |(a, b), &(h, v)| {
            (a.max(u32::from(h)), b.max(u32::from(v)))
        });
        // Samples per image pixel over all components: 1.5 for 4:2:0 YCbCr, 3 for 4:4:4, 1 for gray.
        let per_px: f64 = self
            .components
            .iter()
            .map(|&(h, v)| f64::from(u32::from(h) * u32::from(v)) / f64::from(hmax * vmax))
            .sum();
        let out_ch = 3.0; // packed RGB (gray and CMYK are converted to RGB too)
        let planes = spx as f64 * (per_px + out_ch);
        // x1.15 on the coefficients: block padding to whole MCUs and allocator slack (measured +295..310 MB at 100 MP
        // 4:2:0 against 300 MB computed).
        let coeffs = if self.progressive {
            fpx as f64 * per_px * 2.0 * 1.15
        } else {
            0.0
        };
        // Resize and encode buffers on top of the decoded image: bounded by the preview, a few MB.
        (planes + coeffs) as u64 + (24 << 20)
    }
}

fn scaled_len(len: u32, eighths: u32) -> u32 {
    (len * eighths).div_ceil(8).max(1)
}

/// Decode a JPEG at a reduced size using the IDCT's scaling (1/2, 1/4 or 1/8), so the full-size image never exists.
/// The result is at least [`PREVIEW_EDGE`] on the long edge whenever the source is. Colour handling matches the full
/// decode: ICC profiles are ignored in both, Adobe CMYK/YCCK is converted to RGB. EXIF orientation is not applied.
#[cfg(test)]
pub fn decode_jpeg_scaled(bytes: &[u8]) -> Result<DynamicImage> {
    decode_jpeg_scaled_to(bytes, PREVIEW_EDGE)
}

/// [`decode_jpeg_scaled`] for an output at least `edge` on the long edge (a render of a crop).
pub fn decode_jpeg_scaled_to(bytes: &[u8], edge: u32) -> Result<DynamicImage> {
    let mut dec = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    dec.read_info().context("read JPEG header")?;
    let edge = u16::try_from(edge).unwrap_or(u16::MAX);
    let (w, h) = dec.scale(edge, edge).context("choose JPEG scale")?;
    let px = dec.decode().context("decode JPEG")?;
    let info = dec.info().context("JPEG info")?;
    let (w, h) = (u32::from(w), u32::from(h));
    match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => image::GrayImage::from_raw(w, h, px).map(DynamicImage::ImageLuma8),
        jpeg_decoder::PixelFormat::RGB24 => RgbImage::from_raw(w, h, px).map(DynamicImage::ImageRgb8),
        jpeg_decoder::PixelFormat::CMYK32 => {
            // jpeg-decoder undoes Adobe's inversion for both CMYK and YCCK, so these are ink amounts (255 = full
            // ink). The usual naive conversion, the same as for the full-size decode: R = (1 - C)(1 - K).
            let mut rgb = Vec::with_capacity(px.len() / 4 * 3);
            for p in px.as_chunks::<4>().0 {
                let paper = 255 - u32::from(p[3]);
                rgb.extend(
                    p[..3]
                        .iter()
                        .map(|&c| (((255 - u32::from(c)) * paper + 127) / 255) as u8),
                );
            }
            RgbImage::from_raw(w, h, rgb).map(DynamicImage::ImageRgb8)
        }
        jpeg_decoder::PixelFormat::L16 => anyhow::bail!("16-bit JPEG"),
    }
    .context("JPEG pixel buffer")
}

// ---- PNG: row streaming --------------------------------------------------------------------------------------------

fn png_decoder(bytes: &[u8]) -> png::Decoder<Cursor<&[u8]>> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    // Palette and low bit depths expanded, 16-bit cut to 8: rows come out as 8-bit gray/gray+alpha/RGB/RGBA.
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    dec
}

/// `Some(has_alpha)` when the PNG can be decoded row by row: not interlaced (Adam7 needs the whole image to put the
/// passes together). Other PNGs take the normal decode.
pub fn png_streamable(bytes: &[u8]) -> Option<bool> {
    let reader = png_decoder(bytes).read_info().ok()?;
    if reader.info().interlaced {
        return None;
    }
    let (color, _) = reader.output_color_type();
    Some(matches!(color, png::ColorType::GrayscaleAlpha | png::ColorType::Rgba))
}

/// Decode a PNG one row at a time, averaging rows down into a buffer of roughly [`PREVIEW_EDGE`] on the long edge as
/// they arrive. Peak memory is that buffer plus one row and the inflate window, whatever the PNG's size.
#[cfg(test)]
pub fn decode_png_streamed(bytes: &[u8]) -> Result<DynamicImage> {
    decode_png_streamed_to(bytes, PREVIEW_EDGE)
}

/// [`decode_png_streamed`] averaged down to roughly `edge` instead.
pub fn decode_png_streamed_to(bytes: &[u8], edge: u32) -> Result<DynamicImage> {
    let mut reader = png_decoder(bytes).read_info().context("read PNG header")?;
    let (w, h) = (reader.info().width, reader.info().height);
    let (color, _) = reader.output_color_type();
    let alpha = matches!(color, png::ColorType::GrayscaleAlpha | png::ColorType::Rgba);
    let mut acc = crate::stream::Accum::new(w, h, edge, alpha)?;
    let ch = if alpha { 4 } else { 3 };
    let mut row_buf = vec![0u8; w as usize * ch];
    for y in 0..h {
        let row = reader
            .next_row()
            .context("decode PNG row")?
            .context("PNG ended early")?;
        let data = row.data();
        match color {
            png::ColorType::Rgb | png::ColorType::Rgba => acc.add_row(y, data, false),
            png::ColorType::Grayscale => {
                for (o, &g) in row_buf.as_chunks_mut::<3>().0.iter_mut().zip(data) {
                    *o = [g, g, g];
                }
                acc.add_row(y, &row_buf, false);
            }
            png::ColorType::GrayscaleAlpha => {
                for (o, p) in row_buf.as_chunks_mut::<4>().0.iter_mut().zip(data.as_chunks::<2>().0) {
                    *o = [p[0], p[0], p[0], p[1]];
                }
                acc.add_row(y, &row_buf, false);
            }
            png::ColorType::Indexed => anyhow::bail!("palette PNG was not expanded"),
        }
    }
    acc.finish()
}

/// Decode `bytes`, orient by EXIF `orientation`, and produce all derived artifacts.
pub fn derive(bytes: &[u8], orientation: u8) -> Result<Derived> {
    derive_image(decode(bytes)?, orientation)
}

/// Resize first, then orient the (small) results: rotating a full-size image would need a second
/// full-size buffer. `orientation` is applied exactly once, here.
pub fn derive_image(img: DynamicImage, orientation: u8) -> Result<Derived> {
    let (w, h) = (img.width(), img.height());
    derive_scaled(img, orientation, w, h)
}

/// Like [`derive_image`] for an `img` that is a reduced-size decode (DCT-scaled JPEG, streamed HEIF/PNG) of a source
/// that is `full_w` x `full_h` as stored. Output sizes, the reported dimensions and the orientation all follow the
/// source; `img` must be at least as large as the preview in the long edge, so nothing is upscaled.
pub fn derive_scaled(img: DynamicImage, orientation: u8, full_w: u32, full_h: u32) -> Result<Derived> {
    let (sw, sh) = (full_w, full_h);
    let (width, height) = oriented_dims(sw, sh, orientation);

    let (pw, ph) = fit_within(sw, sh, PREVIEW_EDGE);
    let preview = resize(&img, pw, ph)?;
    drop(img);
    // Thumb comes from the preview: far fewer pixels to filter than the original.
    let (tw, th) = fit_within(sw, sh, THUMB_EDGE);
    let thumb = apply_orientation(resize(&preview, tw, th)?, orientation);
    let preview = apply_orientation(preview, orientation);

    Ok(Derived {
        thumb_webp: encode_webp(&thumb, THUMB_QUALITY)?,
        preview_webp: encode_webp(&preview, PREVIEW_QUALITY)?,
        thumbhash: thumbhash(&thumb)?,
        width,
        height,
    })
}

// ---- Edit recipes: rotate, straighten, crop --------------------------------------------------------------------------

/// The long edge of the full-size JPEG of an edited photo ("Download edited", share links).
pub const EXPORT_EDGE: u32 = 4096;
const EXPORT_QUALITY: u8 = 90;

/// Turn the picture by `deg` degrees (positive = clockwise) about its centre, keeping the frame, magnified by `zoom` so
/// that no empty corner shows (`Recipe::straighten_zoom`). Bilinear.
pub fn straighten(img: &DynamicImage, deg: f32, zoom: f64) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if img.color().has_alpha() {
        let src = img.to_rgba8();
        let out = straighten_raw(src.as_raw(), w, h, 4, deg, zoom);
        DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, out).expect("same size"))
    } else {
        let src = img.to_rgb8();
        let out = straighten_raw(src.as_raw(), w, h, 3, deg, zoom);
        DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, out).expect("same size"))
    }
}

fn straighten_raw(src: &[u8], w: u32, h: u32, ch: usize, deg: f32, zoom: f64) -> Vec<u8> {
    let (sin, cos) = f64::from(deg).to_radians().sin_cos();
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    let (wi, hi) = (w as usize, h as usize);
    let mut out = vec![0u8; src.len()];
    for y in 0..hi {
        let v = (y as f64 + 0.5 - cy) / zoom;
        for x in 0..wi {
            let u = (x as f64 + 0.5 - cx) / zoom;
            // Where this output pixel comes from: the frame point turned back by the angle.
            let sx = u * cos + v * sin + cx - 0.5;
            let sy = -u * sin + v * cos + cy - 0.5;
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let at = |px: f64, py: f64| {
                let px = (px.max(0.0) as usize).min(wi - 1);
                let py = (py.max(0.0) as usize).min(hi - 1);
                (py * wi + px) * ch
            };
            let (i00, i10, i01, i11) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
            let o = (y * wi + x) * ch;
            for c in 0..ch {
                let top = f64::from(src[i00 + c]) * (1.0 - fx) + f64::from(src[i10 + c]) * fx;
                let bottom = f64::from(src[i01 + c]) * (1.0 - fx) + f64::from(src[i11 + c]) * fx;
                out[o + c] = (top * (1.0 - fy) + bottom * fy + 0.5) as u8;
            }
        }
    }
    out
}

/// Apply a recipe to a picture that is already upright (EXIF orientation applied): quarter turns, straighten, crop.
pub fn apply_recipe(img: DynamicImage, r: &Recipe) -> DynamicImage {
    let mut img = match r.rotate % 360 {
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        _ => img,
    };
    if r.straighten != 0.0 {
        let zoom = r.straighten_zoom(img.width(), img.height());
        img = straighten(&img, r.straighten, zoom);
    }
    let (x, y, w, h) = r.crop_pixels(img.width(), img.height());
    if (x, y, w, h) != (0, 0, img.width(), img.height()) {
        img = img.crop_imm(x, y, w, h);
    }
    img
}

/// Encode as a JPEG without any metadata. Transparency is flattened onto white.
pub fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let rgb = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        let mut out = RgbImage::new(rgba.width(), rgba.height());
        for (o, p) in out.pixels_mut().zip(rgba.pixels()) {
            let a = u32::from(p[3]);
            let mix = |c: u8| ((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
            *o = image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])]);
        }
        out
    } else {
        img.to_rgb8()
    };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&rgb)
        .context("encode JPEG")?;
    Ok(out)
}

/// The full-size JPEG of an upright `img` with `r` applied, at most [`EXPORT_EDGE`] on the long edge.
pub fn export_jpeg(img: DynamicImage, r: &Recipe) -> Result<Vec<u8>> {
    let out = apply_recipe(img, r);
    let (w, h) = fit_within(out.width(), out.height(), EXPORT_EDGE);
    encode_jpeg(&resize(&out, w, h)?, EXPORT_QUALITY)
}

/// Like [`derive_image`] for an upright `img` with a recipe: thumbnail, preview and ThumbHash of the edited picture, and the
/// whole upright picture at preview size (WebP) for the editor to work on.
pub fn derive_edited(img: DynamicImage, r: &Recipe) -> Result<(Derived, Vec<u8>)> {
    let (bw, bh) = fit_within(img.width(), img.height(), PREVIEW_EDGE);
    let base = encode_webp(&resize(&img, bw, bh)?, PREVIEW_QUALITY)?;
    let out = apply_recipe(img, r);
    let (width, height) = (out.width(), out.height());
    let (pw, ph) = fit_within(width, height, PREVIEW_EDGE);
    let preview = resize(&out, pw, ph)?;
    let (tw, th) = fit_within(width, height, THUMB_EDGE);
    let thumb = resize(&preview, tw, th)?;
    drop(out);
    let derived = Derived {
        thumb_webp: encode_webp(&thumb, THUMB_QUALITY)?,
        preview_webp: encode_webp(&preview, PREVIEW_QUALITY)?,
        thumbhash: thumbhash(&thumb)?,
        width,
        height,
    };
    Ok((derived, base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif::testutil::{ascii, exif_app1, rationals, with_app1};
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

    /// The RIFF chunk types of a WebP file.
    fn webp_chunks(b: &[u8]) -> Vec<String> {
        assert!(b.starts_with(b"RIFF") && &b[8..12] == b"WEBP");
        let mut chunks = Vec::new();
        let mut pos = 12;
        while pos + 8 <= b.len() {
            let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
            chunks.push(String::from_utf8_lossy(&b[pos..pos + 4]).into_owned());
            pos += 8 + len + (len & 1);
        }
        chunks
    }

    /// Privacy: share links hand out the thumbnail and preview, so neither may carry the original's metadata.
    /// A JPEG with a GPS position, camera make and XMP goes in; the WebP files that come out have no EXIF, XMP or
    /// ICC chunk (only image data), and no EXIF reader finds a position in them.
    #[test]
    fn derived_images_carry_no_exif_or_gps() {
        let gps = [
            (0x0001, 2, ascii("N")),
            (0x0002, 5, rationals(&[(48, 1), (51, 1), (30, 1)])),
            (0x0003, 2, ascii("E")),
            (0x0004, 5, rationals(&[(2, 1), (17, 1), (40, 1)])),
        ];
        let app1 = exif_app1(
            &[
                (0x010F, 2, ascii("SECRETMAKE")),
                (0x0112, 3, 6u16.to_le_bytes().to_vec()),
            ],
            &[],
            &gps,
        );
        let src = with_app1(&jpeg(1600, 900), &app1);
        let info = crate::exif::parse(&src);
        assert!(
            info.latitude.is_some() && info.longitude.is_some(),
            "fixture must have a position"
        );

        let d = derive(&src, info.orientation).unwrap();
        for (name, webp) in [("thumb", &d.thumb_webp), ("preview", &d.preview_webp)] {
            let chunks = webp_chunks(webp);
            assert!(
                chunks
                    .iter()
                    .all(|c| matches!(c.as_str(), "VP8 " | "VP8L" | "VP8X" | "ALPH")),
                "{name} has metadata chunks: {chunks:?}"
            );
            for needle in [&b"EXIF"[..], b"XMP ", b"ICCP", b"SECRETMAKE", b"GPS"] {
                assert!(
                    !webp.windows(needle.len()).any(|w| w == needle),
                    "{name} contains {:?}",
                    String::from_utf8_lossy(needle)
                );
            }
            let again = crate::exif::parse(webp);
            assert_eq!((again.latitude, again.longitude, again.camera_make), (None, None, None));
        }
    }

    // ---- DCT-scaled JPEG ----

    use crate::testgen::{self, JpegSpec, psnr};

    fn info_of(w: u32, h: u32) -> JpegInfo {
        JpegInfo {
            width: w,
            height: h,
            progressive: false,
            sof: 0xC0,
            precision: 8,
            components: vec![(2, 2), (1, 1), (1, 1)],
        }
    }

    #[test]
    fn scale_picks_the_smallest_size_that_still_covers_the_preview() {
        // (width, height) -> eighths. The long edge after scaling must stay >= 1440 and the next step down must not.
        for (w, h, want) in [
            (12000, 9000, 1), // 1/8 -> 1500
            (8000, 6000, 2),  // 1/8 -> 1000 (too small), 1/4 -> 2000
            (11520, 1000, 1), // panorama: only the long edge counts
            (4000, 3000, 4),  // 1/4 -> 1000, 1/2 -> 2000
            (2880, 2000, 4),  // 1/2 -> 1440, exactly enough
            (2879, 2000, 8),  // 1/2 -> 1440 (ceil), still enough?
            (2000, 1500, 8),  // 1/2 -> 1000: no reduction possible
            (1440, 1080, 8),
            (800, 600, 8),
        ] {
            let i = info_of(w, h);
            let n = i.scale_eighths();
            let long = |n| scaled_len(w.max(h), n);
            assert!(n == 8 || long(n) >= PREVIEW_EDGE, "{w}x{h}: {n}/8 gives {}", long(n));
            if n > 1 && n < 8 {
                assert!(long(n / 2) < PREVIEW_EDGE, "{w}x{h}: {n}/8 is not the smallest");
            }
            if (w, h) != (2879, 2000) {
                assert_eq!(n, want, "{w}x{h}");
            }
        }
    }

    #[test]
    fn jpeg_info_reads_the_frame_header() {
        let base = testgen::jpeg(JpegSpec::new(321, 123));
        let i = jpeg_info(&base).unwrap();
        assert_eq!((i.width, i.height, i.progressive, i.precision), (321, 123, false, 8));
        assert_eq!(i.components, vec![(2, 2), (1, 1), (1, 1)]);
        assert!(i.scalable());

        let mut s = JpegSpec::new(100, 50);
        s.progressive = true;
        s.subsample = false;
        let i = jpeg_info(&testgen::jpeg(s)).unwrap();
        assert!(i.progressive);
        assert_eq!(i.components, vec![(1, 1), (1, 1), (1, 1)]);

        assert!(jpeg_info(b"not a jpeg").is_none());
        assert!(jpeg_info(&base[..30]).is_none());
        // 12-bit samples are left to the full decoder.
        let mut twelve = base.clone();
        let sof = twelve.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        twelve[sof + 4] = 12;
        assert!(!jpeg_info(&twelve).unwrap().scalable());
        // Arithmetic-coded and lossless frames likewise.
        let mut arith = base.clone();
        arith[sof + 1] = 0xC9;
        assert_eq!(jpeg_info(&arith).unwrap().sof, 0xC9);
        assert!(!jpeg_info(&arith).unwrap().scalable());
    }

    #[test]
    fn scaled_decode_returns_the_chosen_size() {
        let bytes = testgen::jpeg(JpegSpec::new(4001, 2999));
        let i = jpeg_info(&bytes).unwrap();
        assert_eq!(i.scale_eighths(), 4);
        let img = decode_jpeg_scaled(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), i.scaled_dims(4));
        assert_eq!((img.width(), img.height()), (2001, 1500));
    }

    /// The scaled path must give the same pictures as decoding everything and resizing, within the difference
    /// between a Lanczos filter and the IDCT's own reduction. Smooth synthetic content, so the bar is high.
    fn assert_scaled_matches_full(spec: JpegSpec, orientation: u8) {
        let bytes = testgen::jpeg(spec);
        let info = jpeg_info(&bytes).unwrap();
        assert!(
            info.scalable() && info.scale_eighths() < 8,
            "fixture must take the scaled path"
        );
        let (w, h) = (u32::from(spec.w), u32::from(spec.h));
        let full = derive(&bytes, orientation).unwrap();
        let scaled = derive_scaled(decode_jpeg_scaled(&bytes).unwrap(), orientation, w, h).unwrap();
        assert_eq!((scaled.width, scaled.height), (full.width, full.height));
        for (name, a, b) in [
            ("preview", &full.preview_webp, &scaled.preview_webp),
            ("thumb", &full.thumb_webp, &scaled.thumb_webp),
        ] {
            let (da, db) = (
                webp::Decoder::new(a).decode().unwrap(),
                webp::Decoder::new(b).decode().unwrap(),
            );
            assert_eq!((da.width(), da.height()), (db.width(), db.height()), "{name} size");
            let (ia, ib) = (da.to_image().to_rgb8(), db.to_image().to_rgb8());
            let p = psnr(&ia, &ib);
            assert!(p > 30.0, "{name} differs from the full decode: {p:.1} dB");
        }
        assert_eq!(full.thumbhash.len(), scaled.thumbhash.len());
    }

    #[test]
    fn scaled_baseline_matches_full_decode() {
        assert_scaled_matches_full(JpegSpec::new(3000, 2000), 1);
    }

    #[test]
    fn scaled_progressive_matches_full_decode() {
        let mut s = JpegSpec::new(3000, 1900);
        s.progressive = true;
        assert_scaled_matches_full(s, 1);
    }

    #[test]
    fn scaled_444_with_restart_markers_matches_full_decode() {
        let mut s = JpegSpec::new(3000, 1800);
        s.subsample = false;
        s.restart_interval = 7;
        assert_scaled_matches_full(s, 1);
    }

    #[test]
    fn scaled_progressive_with_restart_markers_matches_full_decode() {
        let mut s = JpegSpec::new(2950, 1700);
        s.progressive = true;
        s.restart_interval = 11;
        assert_scaled_matches_full(s, 1);
    }

    #[test]
    fn scaled_decode_keeps_orientation() {
        for o in [1, 3, 5, 6, 8] {
            assert_scaled_matches_full(JpegSpec::new(2900, 1000), o);
        }
        // And end to end through the EXIF tag in the file: a stored 3200x1600 with orientation 6 is shown 1600x3200.
        let app1 = exif_app1(&[(0x0112, 3, 6u16.to_le_bytes().to_vec())], &[], &[]);
        let bytes = with_app1(&testgen::jpeg(JpegSpec::new(3200, 1600)), &app1);
        let o = crate::exif::parse(&bytes).orientation;
        assert_eq!(o, 6);
        let d = derive_scaled(decode_jpeg_scaled(&bytes).unwrap(), o, 3200, 1600).unwrap();
        assert_eq!((d.width, d.height), (1600, 3200));
        assert_eq!(webp_dims(&d.preview_webp), (720, 1440));
        assert_eq!(webp_dims(&d.thumb_webp), (160, 320));
    }

    #[test]
    fn scaled_decode_pixels_are_where_they_should_be() {
        // Left half red, right half blue, rotated 90 CW by the EXIF tag: red ends up on top.
        let mut img = RgbImage::new(3200, 1600);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 1600 {
                Rgb([230, 20, 20])
            } else {
                Rgb([20, 20, 230])
            };
        }
        let mut raw = Vec::new();
        jpeg_encoder::Encoder::new(&mut raw, 90)
            .encode(img.as_raw(), 3200, 1600, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        let app1 = exif_app1(&[(0x0112, 3, 6u16.to_le_bytes().to_vec())], &[], &[]);
        let bytes = with_app1(&raw, &app1);
        let d = derive_scaled(decode_jpeg_scaled(&bytes).unwrap(), 6, 3200, 1600).unwrap();
        let prev = webp::Decoder::new(&d.preview_webp)
            .decode()
            .unwrap()
            .to_image()
            .to_rgb8();
        let (top, bottom) = (prev.get_pixel(360, 200).0, prev.get_pixel(360, 1240).0);
        assert!(top[0] > 190 && top[2] < 70, "top should be red, got {top:?}");
        assert!(
            bottom[2] > 190 && bottom[0] < 70,
            "bottom should be blue, got {bottom:?}"
        );
    }

    #[test]
    fn scaled_cmyk_converts_like_the_full_decoder() {
        for ycck in [false, true] {
            scaled_cmyk(ycck);
        }
    }

    fn scaled_cmyk(ycck: bool) {
        let mut s = JpegSpec::new(3000, 2000);
        s.cmyk = true;
        s.ycck = ycck;
        s.subsample = false;
        let bytes = testgen::jpeg(s);
        let info = jpeg_info(&bytes).unwrap();
        assert_eq!(info.components.len(), 4);
        assert!(info.scalable());
        let scaled = decode_jpeg_scaled(&bytes).unwrap().to_rgb8();
        assert_eq!(
            (scaled.width(), scaled.height()),
            info.scaled_dims(info.scale_eighths())
        );
        // The picture is what went in: the fixture stores the RGB test pattern as Adobe-style inverted CMYK with no black ink.
        let (sw, sh) = (scaled.width(), scaled.height());
        for (x, y) in [(sw / 4, sh / 4), (sw / 2, sh / 2), (3 * sw / 4, sh / 3)] {
            let want = testgen::pixel(x * 8 / info.scale_eighths(), y * 8 / info.scale_eighths(), 3000, 2000);
            let got = scaled.get_pixel(x, y).0;
            for c in 0..3 {
                assert!(
                    (i32::from(got[c]) - i32::from(want[c])).abs() < 24,
                    "({x},{y}) channel {c}: got {got:?}, want {want:?}"
                );
            }
        }
        // The full decoder, when it takes the file at all, agrees about the colours.
        if let Ok(full) = decode(&bytes) {
            let (fw, fh) = (full.width(), full.height());
            let want = resize(&full, scaled.width(), scaled.height()).unwrap().to_rgb8();
            assert_eq!((fw, fh), (3000, 2000));
            let p = psnr(&want, &scaled);
            assert!(p > 28.0, "CMYK differs from the full decoder: {p:.1} dB");
        }
    }

    #[test]
    fn scaled_grayscale_jpeg() {
        let (w, h) = (3000u32, 2000u32);
        let luma: Vec<u8> = (0..w * h).map(|i| ((i % w) * 255 / w) as u8).collect();
        let mut raw = Vec::new();
        jpeg_encoder::Encoder::new(&mut raw, 90)
            .encode(&luma, w as u16, h as u16, jpeg_encoder::ColorType::Luma)
            .unwrap();
        let info = jpeg_info(&raw).unwrap();
        assert_eq!(info.components.len(), 1);
        let d = derive_scaled(decode_jpeg_scaled(&raw).unwrap(), 1, w, h).unwrap();
        assert_eq!(webp_dims(&d.preview_webp), (1440, 960));
    }

    #[test]
    fn truncated_jpeg_fails_cleanly_in_the_scaled_path() {
        let bytes = testgen::jpeg(JpegSpec::new(3000, 2000));
        // Either an error or a partial picture is acceptable; a panic is not.
        let _ = decode_jpeg_scaled(&bytes[..bytes.len() / 2]);
        assert!(decode_jpeg_scaled(&bytes[..40]).is_err());
    }

    // ---- PNG streaming ----

    fn png_of(img: DynamicImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn streamed_png_matches_full_decode() {
        let img = RgbImage::from_fn(2950, 2000, |x, y| Rgb(testgen::pixel(x, y, 2950, 2000)));
        let bytes = png_of(DynamicImage::ImageRgb8(img));
        assert_eq!(png_streamable(&bytes), Some(false));
        let full = derive(&bytes, 1).unwrap();
        let streamed = derive_scaled(decode_png_streamed(&bytes).unwrap(), 1, 2950, 2000).unwrap();
        assert_eq!((streamed.width, streamed.height), (2950, 2000));
        let (a, b) = (
            webp::Decoder::new(&full.preview_webp)
                .decode()
                .unwrap()
                .to_image()
                .to_rgb8(),
            webp::Decoder::new(&streamed.preview_webp)
                .decode()
                .unwrap()
                .to_image()
                .to_rgb8(),
        );
        assert!(psnr(&a, &b) > 35.0);
    }

    #[test]
    fn streamed_png_handles_gray_alpha_palette_and_16_bit() {
        use image::{GrayAlphaImage, GrayImage, ImageBuffer, LumaA, Rgba};
        let (w, h) = (2900u32, 1450u32);
        let gray = DynamicImage::ImageLuma8(GrayImage::from_fn(w, h, |x, y| image::Luma([((x + y) % 251) as u8])));
        let ga = DynamicImage::ImageLumaA8(GrayAlphaImage::from_fn(w, h, |x, _| LumaA([200, (x * 255 / w) as u8])));
        let rgba = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(w, h, |x, y| {
            let [r, g, b] = testgen::pixel(x, y, w, h);
            Rgba([r, g, b, 40 + (y * 200 / h) as u8])
        }));
        let rgb16 = DynamicImage::ImageRgb16(ImageBuffer::from_fn(w, h, |x, y| {
            let [r, g, b] = testgen::pixel(x, y, w, h);
            image::Rgb([u16::from(r) * 257, u16::from(g) * 257, u16::from(b) * 257])
        }));
        for (name, img) in [("gray", gray), ("gray+alpha", ga), ("rgba", rgba), ("rgb16", rgb16)] {
            let bytes = png_of(img);
            let alpha = png_streamable(&bytes).unwrap_or_else(|| panic!("{name} not streamable"));
            let streamed = decode_png_streamed(&bytes).unwrap();
            assert_eq!(streamed.color().has_alpha(), alpha, "{name}");
            assert!(
                streamed.width() >= PREVIEW_EDGE && streamed.width() < 2 * PREVIEW_EDGE,
                "{name}"
            );
            let full = decode(&bytes).unwrap();
            let target = resize(&full, streamed.width(), streamed.height()).unwrap();
            if alpha {
                // Compare colour where it is visible (the colour of a nearly transparent pixel means nothing) and
                // alpha everywhere. Box averaging and Lanczos differ on the fixture's pixel-level texture, so allow
                // a small mean difference and a larger worst case.
                let (mut sum, mut n, mut worst_alpha) = (0u64, 0u64, 0i32);
                let (a, b) = (target.to_rgba8(), streamed.to_rgba8());
                for (p, q) in a.pixels().zip(b.pixels()) {
                    worst_alpha = worst_alpha.max((i32::from(p[3]) - i32::from(q[3])).abs());
                    if p[3] > 48 && q[3] > 48 {
                        for c in 0..3 {
                            sum += u64::from(p[c].abs_diff(q[c]));
                            n += 1;
                        }
                    }
                }
                let mean = sum as f64 / n.max(1) as f64;
                assert!(
                    mean < 4.0 && worst_alpha < 12,
                    "{name}: mean colour diff {mean:.2}, worst alpha diff {worst_alpha}"
                );
            } else {
                let p = psnr(&target.to_rgb8(), &streamed.to_rgb8());
                assert!(p > 30.0, "{name}: {p:.1} dB");
            }
        }
    }

    #[test]
    fn interlaced_png_is_not_streamed() {
        // A 1x1 interlaced PNG written by hand through the png crate.
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 4, 4);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[7u8; 48]).unwrap();
        }
        assert_eq!(png_streamable(&out), Some(false));
        // Flip the IHDR interlace byte (offset 28) and fix nothing else: still parses as interlaced for the header.
        let mut adam = out.clone();
        adam[28] = 1;
        assert_eq!(png_streamable(&adam), None);
    }

    #[test]
    fn garbage_fails() {
        assert!(derive(b"definitely not an image", 1).is_err());
    }

    // ---- recipes

    fn recipe(rotate: u16, straighten: f32, crop: Option<(f64, f64, f64, f64)>) -> Recipe {
        Recipe {
            rotate,
            straighten,
            crop: crop.map(|(x, y, w, h)| cyanotype_core::recipe::Crop { x, y, w, h }),
        }
    }

    fn gradient(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| Rgb([(x * 10) as u8, (y * 10) as u8, 7])))
    }

    #[test]
    fn quarter_turns_are_clockwise() {
        let src = gradient(4, 2);
        let r90 = apply_recipe(src.clone(), &recipe(90, 0.0, None)).to_rgb8();
        assert_eq!((r90.width(), r90.height()), (2, 4));
        // The top left of the source ends up top right; its bottom left ends up top left.
        assert_eq!(r90.get_pixel(1, 0).0, [0, 0, 7]);
        assert_eq!(r90.get_pixel(0, 0).0, [0, 10, 7]);
        let r180 = apply_recipe(src.clone(), &recipe(180, 0.0, None)).to_rgb8();
        assert_eq!(r180.get_pixel(0, 0).0, [30, 10, 7]);
        let r270 = apply_recipe(src, &recipe(270, 0.0, None)).to_rgb8();
        assert_eq!((r270.width(), r270.height()), (2, 4));
        assert_eq!(
            r270.get_pixel(0, 3).0,
            [0, 0, 7],
            "the top left goes to the bottom left"
        );
    }

    #[test]
    fn crop_is_cut_from_the_turned_frame() {
        let src = gradient(10, 10);
        let out = apply_recipe(src.clone(), &recipe(0, 0.0, Some((0.5, 0.0, 0.5, 1.0)))).to_rgb8();
        assert_eq!((out.width(), out.height()), (5, 10));
        assert_eq!(out.get_pixel(0, 0).0, [50, 0, 7]);
        // The crop follows the turn: in a 90 degree frame the right half of it is the source's upper half.
        let out = apply_recipe(src, &recipe(90, 0.0, Some((0.5, 0.0, 0.5, 1.0)))).to_rgb8();
        assert_eq!((out.width(), out.height()), (5, 10));
        // Turned clockwise, frame (5, 0) is the source's (0, 4).
        assert_eq!(out.get_pixel(0, 0).0, [0, 40, 7]);
    }

    #[test]
    fn straightening_fills_the_frame_and_keeps_its_size() {
        // White with a black 1 px frame: if the zoom were too small the frame would show in the corners.
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(200, 100, |x, y| {
            if x == 0 || y == 0 || x == 199 || y == 99 {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            }
        }));
        for deg in [-12.5f32, -3.0, 0.1, 5.0, 30.0, 45.0] {
            let out = apply_recipe(img.clone(), &recipe(0, deg, None)).to_rgb8();
            assert_eq!((out.width(), out.height()), (200, 100));
            let dark = out
                .enumerate_pixels()
                .filter(|(x, y, p)| *x >= 3 && *y >= 3 && *x < 197 && *y < 97 && p.0[0] < 200)
                .count();
            assert_eq!(dark, 0, "{deg} degrees left the frame showing");
        }
        // And it really turns: a vertical white bar on black leans.
        let bar = DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |x, _| {
            if (45..55).contains(&x) {
                Rgb([255, 255, 255])
            } else {
                Rgb([0, 0, 0])
            }
        }));
        let out = apply_recipe(bar, &recipe(0, 10.0, None)).to_rgb8();
        let centre_of = |y: u32| {
            let xs: Vec<u32> = (0..100).filter(|&x| out.get_pixel(x, y).0[0] > 128).collect();
            xs.iter().sum::<u32>() as f64 / xs.len() as f64
        };
        // Clockwise: the top of a vertical line moves right of its bottom.
        assert!(
            centre_of(10) > centre_of(90) + 5.0,
            "{} {}",
            centre_of(10),
            centre_of(90)
        );
    }

    #[test]
    fn edited_renders_have_the_edited_size() {
        let src = decode(&jpeg(2000, 1000)).unwrap();
        let r = recipe(90, 0.0, Some((0.0, 0.0, 1.0, 0.5)));
        let (d, base) = derive_edited(src, &r).unwrap();
        assert_eq!((d.width, d.height), (1000, 1000));
        assert_eq!(
            webp_dims(&base),
            (1440, 720),
            "the editor gets the whole picture, upright and uncropped"
        );
        assert_eq!(webp_dims(&d.preview_webp), (1000, 1000));
        assert_eq!(webp_dims(&d.thumb_webp), (320, 320));
        assert!(!d.thumbhash.is_empty());
        // The size the database stores (cyanotype_core) is the size the worker renders.
        assert_eq!(r.output_dims(2000, 1000), (1000, 1000));
    }

    #[test]
    fn export_is_a_bare_jpeg_at_most_4096_wide() {
        let src = DynamicImage::ImageRgb8(RgbImage::from_pixel(6000, 3000, Rgb([10, 120, 200])));
        let bytes = export_jpeg(src, &recipe(0, 0.0, None)).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (4096, 2048));
        assert!(!bytes.windows(4).any(|w| w == b"Exif"), "no metadata");
        // A transparent corner becomes white, not black.
        let mut rgba = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 0]));
        rgba.put_pixel(7, 7, image::Rgba([255, 0, 0, 255]));
        let flat = decode(&encode_jpeg(&DynamicImage::ImageRgba8(rgba), 95).unwrap())
            .unwrap()
            .to_rgb8();
        assert!(flat.get_pixel(0, 0).0.iter().all(|&c| c > 240));
    }

    #[test]
    fn a_crop_is_rendered_from_more_of_the_original() {
        let info = jpeg_info(&crate::testgen::jpeg(crate::testgen::JpegSpec::new(6000, 4000))).unwrap();
        assert_eq!(info.scale_eighths_for(PREVIEW_EDGE), 2, "6000 px at a quarter is 1500");
        assert_eq!(
            info.scale_eighths_for(4096),
            8,
            "a 4096 px render needs more than half of 6000"
        );
        assert!(info.scaled_cost_bytes_for(4096) > info.scaled_cost_bytes_for(PREVIEW_EDGE));
    }
}
