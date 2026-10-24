//! Per-format dispatch: inspect a file (size, EXIF, how much memory rendering will need) and render
//! thumbnail/preview/ThumbHash. Pure CPU work; call from `spawn_blocking`.
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::{
    exif::{self, ExifInfo},
    formats::Kind,
    heif, imaging, raw,
};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Largest acceptable estimated peak for decoding one image, in bytes.
    pub max_decode_bytes: u64,
    pub allow_raw_develop: bool,
}

enum Work {
    /// Full-size decode with the `image` crate, then resize.
    Raster {
        orientation: u8,
    },
    /// JPEG decoded through the IDCT at a fraction of its size (`imaging::decode_jpeg_scaled`). `full` is the stored
    /// size, which the outputs and the reported dimensions follow.
    JpegScaled {
        orientation: u8,
        full: (u32, u32),
    },
    /// PNG decoded row by row and averaged down as it goes.
    PngStreamed {
        orientation: u8,
        full: (u32, u32),
    },
    Heif(Box<heif::HeifHeader>),
    Raw(Box<raw::RawHeader>),
}

pub struct Prepared {
    /// Display size (EXIF orientation / HEIF transforms applied; RAW: the real sensor size).
    pub width: u32,
    pub height: u32,
    pub exif: ExifInfo,
    /// Estimated peak memory of [`render`], in bytes.
    pub cost_bytes: u64,
    /// The long edge the reduced decoders aim for (at least): [`imaging::PREVIEW_EDGE`], more for the render of a crop.
    pub edge: u32,
    /// Set when rendering must be refused (too big for the memory budget, no preview, ...).
    pub refusal: Option<String>,
    work: Work,
}

/// Read headers and metadata without decoding pixels.
pub fn prepare(kind: Kind, bytes: &Arc<Vec<u8>>, limits: Limits) -> Result<Prepared> {
    prepare_for(kind, bytes, limits, imaging::PREVIEW_EDGE)
}

/// [`prepare`] for a render that needs the original at `edge` on the long edge (a crop). Formats that decode at a
/// fraction of their size are budgeted for that fraction; the rest decode whole anyway.
pub fn prepare_for(kind: Kind, bytes: &Arc<Vec<u8>>, limits: Limits, edge: u32) -> Result<Prepared> {
    match kind {
        Kind::Jpeg | Kind::Png | Kind::Webp => {
            let exif = exif::parse(bytes);
            let (w, h, color) = imaging::raw_header(bytes)?;
            let bpp = imaging::raster_bytes_per_pixel(color);
            let (width, height) = imaging::oriented_dims(w, h, exif.orientation);
            let pixels = u64::from(w) * u64::from(h);
            let orientation = exif.orientation;
            // Formats that can be decoded at a fraction of their size are budgeted for that, not for the full image.
            let full_cost = pixels * bpp;
            let (cost, work) = match reduced_decode(kind, bytes, orientation, (w, h), edge) {
                // Only when it is actually cheaper: just over the reduction threshold the fixed overhead can exceed
                // what a plain decode of a small image costs.
                Some((cost, work)) if cost < full_cost => (cost, work),
                _ => (full_cost, Work::Raster { orientation }),
            };
            let refusal = (pixels > imaging::MAX_PIXELS)
                .then(|| format!("image is {w}x{h}, over the pixel limit"))
                .or_else(|| over_budget(cost, limits, w, h));
            Ok(Prepared {
                width,
                height,
                cost_bytes: cost,
                edge,
                refusal,
                work,
                exif,
            })
        }
        Kind::Heif => {
            let h = heif::inspect(bytes)?;
            let cost = h.cost_bytes_for(edge);
            Ok(Prepared {
                width: h.width,
                height: h.height,
                cost_bytes: cost,
                edge,
                refusal: over_budget(cost, limits, h.width, h.height),
                exif: h.exif.clone(),
                work: Work::Heif(Box::new(h)),
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
                edge,
                refusal: hdr.plan.as_ref().err().cloned(),
                exif: hdr.exif.clone(),
                work: Work::Raw(Box::new(hdr)),
            })
        }
    }
}

/// The reduced-size decode for this file, if it has one that helps: its estimated peak and the work to do.
fn reduced_decode(kind: Kind, bytes: &[u8], orientation: u8, full: (u32, u32), edge: u32) -> Option<(u64, Work)> {
    match kind {
        Kind::Jpeg => {
            let info = imaging::jpeg_info(bytes)?;
            // Same size as the header the generic reader found; no reduction means the normal path is as good.
            if !info.scalable() || (info.width, info.height) != full || info.scale_eighths_for(edge) >= 8 {
                return None;
            }
            Some((info.scaled_cost_bytes_for(edge), Work::JpegScaled { orientation, full }))
        }
        Kind::Png => {
            let alpha = imaging::png_streamable(bytes)?;
            let cost = crate::stream::Accum::bytes_for(full.0, full.1, edge, alpha)
                + u64::from(full.0) * 8 // the source row and the converted row
                + PNG_STREAM_OVERHEAD;
            Some((cost, Work::PngStreamed { orientation, full }))
        }
        _ => None,
    }
}

/// Inflate window, chunk buffers and the encoders' working memory on top of the accumulation buffer.
const PNG_STREAM_OVERHEAD: u64 = 24 << 20;

fn over_budget(cost: u64, limits: Limits, w: u32, h: u32) -> Option<String> {
    (cost > limits.max_decode_bytes).then(|| {
        format!(
            "decoding a {w}x{h} image needs about {} MB, over the worker memory budget ({} MB available to this job)",
            cost >> 20,
            limits.max_decode_bytes >> 20
        )
    })
}

impl Prepared {
    /// Which decoder will run, for the `jobs.decode_path` metric.
    pub fn path(&self) -> &'static str {
        match &self.work {
            Work::Raster { .. } => "full",
            Work::JpegScaled { .. } => "jpeg_scaled",
            Work::PngStreamed { .. } => "png_streamed",
            Work::Heif(h) if h.tiling.is_some() => "heif_tiled",
            Work::Heif(_) => "heif",
            Work::Raw(_) => "raw",
        }
    }

    /// The plain full-size decode of a file that was prepared for a reduced one, for when the reduced decoder fails on
    /// something it does not handle. `None` when there was no reduced decode, or the full one does not fit `limits`.
    pub fn full_decode_fallback(&self, bytes: &Arc<Vec<u8>>, limits: Limits) -> Option<Prepared> {
        let orientation = match self.work {
            Work::JpegScaled { orientation, .. } | Work::PngStreamed { orientation, .. } => orientation,
            _ => return None,
        };
        let (w, h, color) = imaging::raw_header(bytes).ok()?;
        let cost = u64::from(w) * u64::from(h) * imaging::raster_bytes_per_pixel(color);
        (cost <= limits.max_decode_bytes).then(|| Prepared {
            width: self.width,
            height: self.height,
            exif: self.exif.clone(),
            cost_bytes: cost,
            edge: self.edge,
            refusal: None,
            work: Work::Raster { orientation },
        })
    }
}

/// Decode and produce all derived artifacts. The caller must have reserved `cost_bytes` and checked
/// `refusal`.
pub fn render(p: &Prepared, bytes: &Arc<Vec<u8>>) -> Result<imaging::Derived> {
    if let Some(r) = &p.refusal {
        bail!("{r}");
    }
    match &p.work {
        Work::Raster { orientation } => imaging::derive(bytes, *orientation),
        Work::JpegScaled { orientation, full } => imaging::derive_scaled(
            imaging::decode_jpeg_scaled_to(bytes, p.edge)?,
            *orientation,
            full.0,
            full.1,
        ),
        Work::PngStreamed { orientation, full } => imaging::derive_scaled(
            imaging::decode_png_streamed_to(bytes, p.edge)?,
            *orientation,
            full.0,
            full.1,
        ),
        Work::Heif(h) => {
            // libheif already applied irot/imir; EXIF orientation is not applied on top.
            let img = heif::decode_reduced(bytes, h, p.edge).context("decode HEIF")?;
            imaging::derive_scaled(img, 1, h.width, h.height)
        }
        Work::Raw(h) => raw::render(bytes, h),
    }
}

/// Decode for an edit recipe: the picture as it is displayed (EXIF orientation applied), reduced so that its long edge is
/// about `p.edge`. The caller has reserved `p.cost_bytes` and room for the copies the recipe makes.
pub fn decode_upright(p: &Prepared, bytes: &Arc<Vec<u8>>) -> Result<image::DynamicImage> {
    if let Some(r) = &p.refusal {
        bail!("{r}");
    }
    let (img, orientation) = match &p.work {
        Work::Raster { orientation } => (imaging::decode(bytes)?, *orientation),
        Work::JpegScaled { orientation, .. } => (imaging::decode_jpeg_scaled_to(bytes, p.edge)?, *orientation),
        Work::PngStreamed { orientation, .. } => (imaging::decode_png_streamed_to(bytes, p.edge)?, *orientation),
        // libheif already applied irot/imir.
        Work::Heif(h) => (heif::decode_reduced(bytes, h, p.edge).context("decode HEIF")?, 1),
        Work::Raw(h) => raw::render_image(bytes, h)?,
    };
    // Whole-image decodes and the coarse steps of the reduced ones can be larger than asked for: bring them down before
    // the quarter turns and the straighten copy them.
    let (w, h) = imaging::fit_within(img.width(), img.height(), p.edge);
    let img = if (w, h) == (img.width(), img.height()) {
        img
    } else {
        imaging::resize(&img, w, h)?
    };
    Ok(imaging::apply_orientation(img, orientation))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32) -> Arc<Vec<u8>> {
        let img = image::RgbImage::new(w, h);
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Jpeg)
            .unwrap();
        Arc::new(b)
    }

    fn png(w: u32, h: u32) -> Arc<Vec<u8>> {
        let img = image::RgbImage::new(w, h);
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        Arc::new(b)
    }

    fn limits(mb: u64) -> Limits {
        Limits {
            max_decode_bytes: mb << 20,
            allow_raw_develop: false,
        }
    }

    #[test]
    fn oversized_raster_is_refused_not_decoded() {
        // A JPEG too small to be decoded at a reduced size takes the plain path: 5 bytes per pixel.
        let b = jpeg(2000, 1000); // 2 MP => 10 MB estimated
        let p = prepare(Kind::Jpeg, &b, limits(6)).unwrap();
        assert_eq!(p.path(), "full");
        assert_eq!(
            p.cost_bytes,
            2_000_000 * imaging::raster_bytes_per_pixel(image::ColorType::Rgb8)
        );
        let msg = p.refusal.clone().expect("refused");
        assert!(msg.contains("memory budget"), "{msg}");
        assert!(render(&p, &b).is_err());

        let p = prepare(Kind::Jpeg, &b, limits(64)).unwrap();
        assert!(p.refusal.is_none());
        assert_eq!(render(&p, &b).unwrap().width, 2000);
    }

    #[test]
    fn big_jpegs_are_budgeted_for_the_reduced_decode() {
        // 6000x4000 = 24 MP would need 120 MB at full size; at 1/4 it is 1500x1000.
        let b = Arc::new(crate::testgen::jpeg(crate::testgen::JpegSpec::new(6000, 4000)));
        let p = prepare(Kind::Jpeg, &b, limits(64)).unwrap();
        assert_eq!(p.path(), "jpeg_scaled");
        assert!(p.refusal.is_none(), "{:?}", p.refusal);
        assert!(p.cost_bytes < 64 << 20, "{} MB", p.cost_bytes >> 20);
        assert_eq!((p.width, p.height), (6000, 4000));
        let d = render(&p, &b).unwrap();
        assert_eq!((d.width, d.height), (6000, 4000));
    }

    #[test]
    fn png_is_streamed_and_budgeted_for_the_output() {
        let b = png(6000, 4000); // 24 MP: 120 MB at the full-size estimate
        let p = prepare(Kind::Png, &b, limits(64)).unwrap();
        assert_eq!(p.path(), "png_streamed");
        assert!(p.cost_bytes < 64 << 20);
        assert!(p.refusal.is_none());
        let d = render(&p, &b).unwrap();
        assert_eq!((d.width, d.height), (6000, 4000));
        // Not worth it for a modest PNG: the plain decode is cheaper than the reduction buffers plus overhead.
        let small = png(3000, 2000);
        assert_eq!(prepare(Kind::Png, &small, limits(64)).unwrap().path(), "full");
    }

    #[test]
    fn full_decode_fallback_only_when_it_fits() {
        let b = Arc::new(crate::testgen::jpeg(crate::testgen::JpegSpec::new(6000, 4000)));
        let p = prepare(Kind::Jpeg, &b, limits(64)).unwrap();
        assert!(
            p.full_decode_fallback(&b, limits(64)).is_none(),
            "120 MB does not fit 64 MB"
        );
        let fb = p.full_decode_fallback(&b, limits(256)).expect("fits 256 MB");
        assert_eq!(fb.path(), "full");
        assert_eq!(fb.cost_bytes, 24_000_000 * 5);
        assert_eq!(render(&fb, &b).unwrap().width, 6000);
        // No fallback for a file that was not decoded at a reduced size.
        let small = jpeg(800, 600);
        let p = prepare(Kind::Jpeg, &small, limits(64)).unwrap();
        assert!(p.full_decode_fallback(&small, limits(256)).is_none());
    }

    #[test]
    fn grid_heic_is_budgeted_per_tile_and_non_grid_heic_per_pixel() {
        let grid = Arc::new(crate::testgen::grid_heic_of_size(8064, 6048, false));
        let p = prepare(Kind::Heif, &grid, limits(96)).unwrap();
        assert_eq!(p.path(), "heif_tiled");
        assert!(p.refusal.is_none(), "{:?}", p.refusal);
        assert!(p.cost_bytes < 96 << 20);
        // A 200 MP grid still costs about the same.
        let huge = Arc::new(crate::testgen::grid_heic_of_size(16000, 12500, false));
        let q = prepare(Kind::Heif, &huge, limits(96)).unwrap();
        assert!(q.refusal.is_none() && q.cost_bytes < 96 << 20);
    }
}
