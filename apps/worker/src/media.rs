//! Per-format dispatch: inspect a file (size, EXIF, how much memory rendering will need) and render
//! thumbnail/preview/ThumbHash. Pure CPU work; call from `spawn_blocking`.
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::{
    exif::{self, ExifInfo},
    formats::Kind,
    heif, imaging, raw,
};

/// HEIC/AVIF: libheif's YCbCr planes and the RGB conversion coexist, then we copy into a packed
/// buffer. 10-bit sources keep 16-bit planes.
pub const HEIF_BYTES_PER_PIXEL_8BIT: u64 = 12;
pub const HEIF_BYTES_PER_PIXEL_HDR: u64 = 16;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Largest acceptable estimated peak for decoding one image, in bytes.
    pub max_decode_bytes: u64,
    pub allow_raw_develop: bool,
}

enum Work {
    Raster { orientation: u8 },
    Heif,
    Raw(Box<raw::RawHeader>),
}

pub struct Prepared {
    /// Display size (EXIF orientation / HEIF transforms applied; RAW: the real sensor size).
    pub width: u32,
    pub height: u32,
    pub exif: ExifInfo,
    /// Estimated peak memory of [`render`], in bytes.
    pub cost_bytes: u64,
    /// Set when rendering must be refused (too big for the memory budget, no preview, ...).
    pub refusal: Option<String>,
    work: Work,
}

/// Read headers and metadata without decoding pixels.
pub fn prepare(kind: Kind, bytes: &Arc<Vec<u8>>, limits: Limits) -> Result<Prepared> {
    match kind {
        Kind::Jpeg | Kind::Png | Kind::Webp => {
            let exif = exif::parse(bytes);
            let (w, h, color) = imaging::raw_header(bytes)?;
            let bpp = imaging::raster_bytes_per_pixel(color);
            let (width, height) = imaging::oriented_dims(w, h, exif.orientation);
            let pixels = u64::from(w) * u64::from(h);
            let refusal = (pixels > imaging::MAX_PIXELS)
                .then(|| format!("image is {w}x{h}, over the pixel limit"))
                .or_else(|| over_budget(pixels * bpp, limits, w, h));
            Ok(Prepared {
                width,
                height,
                cost_bytes: pixels * bpp,
                refusal,
                work: Work::Raster {
                    orientation: exif.orientation,
                },
                exif,
            })
        }
        Kind::Heif => {
            let h = heif::inspect(bytes)?;
            let pixels = u64::from(h.width) * u64::from(h.height);
            let bpp = if h.bit_depth > 8 {
                HEIF_BYTES_PER_PIXEL_HDR
            } else {
                HEIF_BYTES_PER_PIXEL_8BIT
            };
            Ok(Prepared {
                width: h.width,
                height: h.height,
                cost_bytes: pixels * bpp,
                refusal: over_budget(pixels * bpp, limits, h.width, h.height),
                exif: h.exif,
                work: Work::Heif,
            })
        }
        Kind::Raw => {
            let hdr = raw::inspect(
                bytes,
                raw::Limits {
                    max_decode_bytes: limits.max_decode_bytes,
                    allow_develop: limits.allow_raw_develop,
                },
            )?;
            Ok(Prepared {
                width: hdr.width,
                height: hdr.height,
                cost_bytes: hdr.cost_bytes(),
                refusal: hdr.plan.as_ref().err().cloned(),
                exif: hdr.exif.clone(),
                work: Work::Raw(Box::new(hdr)),
            })
        }
    }
}

fn over_budget(cost: u64, limits: Limits, w: u32, h: u32) -> Option<String> {
    (cost > limits.max_decode_bytes).then(|| {
        format!(
            "decoding a {w}x{h} image needs about {} MB, over the worker memory budget ({} MB available to this job)",
            cost >> 20,
            limits.max_decode_bytes >> 20
        )
    })
}

/// Decode and produce all derived artifacts. The caller must have reserved `cost_bytes` and checked
/// `refusal`.
pub fn render(p: &Prepared, bytes: &Arc<Vec<u8>>) -> Result<imaging::Derived> {
    if let Some(r) = &p.refusal {
        bail!("{r}");
    }
    match &p.work {
        Work::Raster { orientation } => imaging::derive(bytes, *orientation),
        Work::Heif => {
            // libheif already applied irot/imir; EXIF orientation is not applied on top.
            imaging::derive_image(heif::decode(bytes).context("decode HEIF")?, 1)
        }
        Work::Raw(h) => raw::render(bytes, h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Arc<Vec<u8>> {
        let img = image::RgbImage::new(w, h);
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        Arc::new(b)
    }

    #[test]
    fn oversized_raster_is_refused_not_decoded() {
        let b = png(2000, 1000); // 2 MP => 10 MB estimated
        let tight = Limits {
            max_decode_bytes: 6 << 20,
            allow_raw_develop: false,
        };
        let p = prepare(Kind::Png, &b, tight).unwrap();
        assert_eq!(
            p.cost_bytes,
            2_000_000 * imaging::raster_bytes_per_pixel(image::ColorType::Rgb8)
        );
        let msg = p.refusal.clone().expect("refused");
        assert!(msg.contains("memory budget"), "{msg}");
        assert!(render(&p, &b).is_err());

        let roomy = Limits {
            max_decode_bytes: 64 << 20,
            allow_raw_develop: false,
        };
        let p = prepare(Kind::Png, &b, roomy).unwrap();
        assert!(p.refusal.is_none());
        assert_eq!(render(&p, &b).unwrap().width, 2000);
    }
}
