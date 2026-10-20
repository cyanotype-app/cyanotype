//! Camera RAW (CR2/CR3, NEF/NRW, ARW, RAF, ORF, RW2, PEF, DNG, SRW, ...).
//!
//! Thumbnails and previews come from the **embedded JPEG preview** that nearly every RAW carries:
//! it is fast and cheap, and its pixel size is known from the JPEG header *before* anything is
//! decoded, so the caller can budget memory. Previews are found by validating JPEG streams inside the
//! file (SOI + a well-formed marker chain up to a baseline/progressive SOF), which works across all
//! containers without a per-camera table. Only when no usable preview exists does the optional,
//! pixel-capped `rawler` develop pipeline run.
//!
//! Metadata: EXIF from the TIFF container (kamadak-exif) when the RAW is TIFF-based, otherwise from
//! the embedded JPEG (RAF, RW2), topped up from rawler's metadata (CR3 and friends). The real sensor
//! dimensions come from rawler, never from the preview.
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageReader};
use rawler::{decoders::RawDecodeParams, imgop::develop::RawDevelop, rawsource::RawSource};

use crate::exif::{self, ExifInfo};
use crate::imaging::{self, PREVIEW_EDGE};

/// Never develop a RAW with more sensor pixels than this, whatever the memory budget says.
pub const DEVELOP_MAX_PIXELS: u64 = 150_000_000;
/// Peak bytes per sensor pixel of rawler's develop pipeline: u16 samples, an f32 copy, an f32 RGB
/// demosaic output and the 16-bit RGB result coexist (measured; see docs/build.md).
pub const DEVELOP_BYTES_PER_PIXEL: u64 = 48;
/// Peak bytes per preview pixel when decoding an embedded JPEG (RGB8 decode plus decoder scratch).
pub const PREVIEW_BYTES_PER_PIXEL: u64 = 6;
/// Previews smaller than this (long edge) are only used when nothing better can be produced.
const MIN_GOOD_PREVIEW_EDGE: u32 = 640;
/// Absolute floor: a smaller embedded image than this is not worth showing.
const MIN_PREVIEW_EDGE: u32 = 96;
const MAX_CANDIDATES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegCand {
    pub offset: usize,
    pub width: u32,
    pub height: u32,
}

impl JpegCand {
    pub fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
    fn long_edge(&self) -> u32 {
        self.width.max(self.height)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Decode this embedded JPEG.
    Preview(JpegCand),
    /// No usable preview: full develop of `pixels` sensor pixels.
    Develop { pixels: u64 },
}

#[derive(Debug)]
pub struct RawHeader {
    /// Real (cropped) sensor size, not oriented. `None` when rawler does not know the camera.
    pub sensor: Option<(u32, u32)>,
    /// Size shown in the library: sensor size with the EXIF orientation applied (or, if the sensor
    /// size is unknown, the preview's).
    pub width: u32,
    pub height: u32,
    pub exif: ExifInfo,
    /// How to produce the derived images, or why we refuse to.
    pub plan: Result<Plan, String>,
}

impl RawHeader {
    /// Estimated peak decode memory for [`render`], in bytes.
    pub fn cost_bytes(&self) -> u64 {
        match &self.plan {
            Ok(Plan::Preview(c)) => c.pixels() * PREVIEW_BYTES_PER_PIXEL,
            Ok(Plan::Develop { pixels }) => pixels * DEVELOP_BYTES_PER_PIXEL,
            Err(_) => 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Refuse any single decode estimated above this many bytes.
    pub max_decode_bytes: u64,
    /// Allow the full-develop fallback.
    pub allow_develop: bool,
}

// ---- embedded JPEG discovery ------------------------------------------------------------------

fn be16(b: &[u8], p: usize) -> Option<usize> {
    Some(usize::from(*b.get(p)?) << 8 | usize::from(*b.get(p + 1)?))
}

/// Dimensions of the JPEG starting at `off` if it is a well-formed baseline/extended/progressive
/// 8-bit stream (SOF0/1/2). Lossless (SOF3, i.e. raw sensor data), arithmetic and hierarchical
/// variants return `None`.
fn jpeg_dims_at(b: &[u8], off: usize) -> Option<(u32, u32)> {
    let mut p = off + 2;
    for _ in 0..256 {
        if *b.get(p)? != 0xFF {
            return None;
        }
        while *b.get(p + 1)? == 0xFF {
            p += 1;
        }
        let m = *b.get(p + 1)?;
        p += 2;
        match m {
            0xC0..=0xC2 => {
                let len = be16(b, p)?;
                if len < 8 || *b.get(p + 2)? != 8 {
                    return None;
                }
                let h = be16(b, p + 3)? as u32;
                let w = be16(b, p + 5)? as u32;
                return (w > 0 && h > 0).then_some((w, h));
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xD9 | 0xDA | 0x00 | 0xD8 => return None,
            0x01 | 0xD0..=0xD7 => {}
            _ => {
                let len = be16(b, p)?;
                if len < 2 {
                    return None;
                }
                p += len;
            }
        }
    }
    None
}

/// Every plausible JPEG stream in the file, in file order.
pub fn scan_jpegs(bytes: &[u8]) -> Vec<JpegCand> {
    let mut out = Vec::new();
    for off in memchr::memmem::find_iter(bytes, &[0xFF, 0xD8, 0xFF]) {
        if let Some((width, height)) = jpeg_dims_at(bytes, off) {
            out.push(JpegCand {
                offset: off,
                width,
                height,
            });
            if out.len() >= MAX_CANDIDATES {
                break;
            }
        }
    }
    out
}

/// Pick the preview to decode: the cheapest one that is big enough for our largest derived image,
/// else the biggest available. Candidates over `max_decode_bytes` are never chosen.
pub fn choose_preview(cands: &[JpegCand], max_decode_bytes: u64) -> Option<JpegCand> {
    let fits =
        |c: &&JpegCand| c.pixels() * PREVIEW_BYTES_PER_PIXEL <= max_decode_bytes && c.long_edge() >= MIN_PREVIEW_EDGE;
    let ok: Vec<&JpegCand> = cands.iter().filter(fits).collect();
    ok.iter()
        .filter(|c| c.long_edge() >= PREVIEW_EDGE)
        .min_by_key(|c| c.pixels())
        .or_else(|| ok.iter().max_by_key(|c| c.pixels()))
        .map(|c| **c)
}

/// Panasonic RW2/RWL: a TIFF-like file ("IIU\0") whose IFD0 carries vendor sensor-size tags. EXIF
/// readers reject its magic, and rawler does not know every body, so read the few tags we need:
/// the sensor borders (4..=7, else SensorWidth/Height 2/3) and Orientation. Returns
/// `((width, height), orientation)`.
fn rw2_header(b: &[u8]) -> Option<((u32, u32), u8)> {
    if !b.starts_with(b"IIU\0") {
        return None;
    }
    let u16_at = |p: usize| Some(u16::from_le_bytes([*b.get(p)?, *b.get(p + 1)?]));
    let u32_at = |p: usize| {
        Some(u32::from_le_bytes([
            *b.get(p)?,
            *b.get(p + 1)?,
            *b.get(p + 2)?,
            *b.get(p + 3)?,
        ]))
    };
    let ifd = u32_at(4)? as usize;
    let n = u16_at(ifd)? as usize;
    let mut tags = std::collections::HashMap::new();
    for i in 0..n.min(512) {
        let e = ifd + 2 + i * 12;
        // Only inline SHORT (type 3) / LONG (type 4) values with count 1 are needed.
        let (tag, ty, count) = (u16_at(e)?, u16_at(e + 2)?, u32_at(e + 4)?);
        if count == 1 {
            match ty {
                3 => tags.insert(tag, u32::from(u16_at(e + 8)?)),
                4 => tags.insert(tag, u32_at(e + 8)?),
                _ => None,
            };
        }
    }
    let get = |t: u16| tags.get(&t).copied();
    let from_borders = || {
        let (top, left, bottom, right) = (get(4)?, get(5)?, get(6)?, get(7)?);
        Some((right.checked_sub(left)?, bottom.checked_sub(top)?))
    };
    let (w, h) = from_borders()
        .filter(|&(w, h)| w > 0 && h > 0)
        .or_else(|| Some((get(2)?, get(3)?)))
        .filter(|&(w, h)| w > 0 && h > 0 && w < 100_000 && h < 100_000)?;
    let orientation = get(0x0112).filter(|o| (1..=8).contains(o)).unwrap_or(1) as u8;
    Some(((w, h), orientation))
}

// ---- rawler glue ------------------------------------------------------------------------------

fn guard<T>(what: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => bail!("rawler panicked while {what}"),
    }
}

/// Sensor size (cropped to the default crop / active area) from rawler's header-level decode.
/// `dummy` mode skips pixel decompression for most decoders.
pub(crate) fn sensor_dims(src: &RawSource) -> Result<(u32, u32)> {
    guard("reading the sensor size", || {
        let dec = rawler::get_decoder(src).map_err(|e| anyhow::anyhow!("{e}"))?;
        let img = dec
            .raw_image(src, &RawDecodeParams::default(), true)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let (w, h) = match img.crop_area.or(img.active_area) {
            Some(r) => (r.d.w, r.d.h),
            None => (img.width, img.height),
        };
        if w == 0 || h == 0 || w > u32::MAX as usize || h > u32::MAX as usize {
            bail!("implausible sensor size {w}x{h}");
        }
        Ok((w as u32, h as u32))
    })
}

fn rawler_exif(src: &RawSource) -> Option<ExifInfo> {
    guard("reading metadata", || {
        let dec = rawler::get_decoder(src).map_err(|e| anyhow::anyhow!("{e}"))?;
        let md = dec
            .raw_metadata(src, &RawDecodeParams::default())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let e = &md.exif;
        let taken_at = e
            .date_time_original
            .as_deref()
            .or(e.create_date.as_deref())
            .and_then(|s| exif::parse_datetime(s, e.offset_time_original.as_deref().or(e.offset_time.as_deref())));
        let (mut lat, mut lon) = (None, None);
        if let Some(g) = &e.gps {
            let conv = |v: &Option<[rawler::formats::tiff::Rational; 3]>| {
                let v = v.as_ref()?;
                exif::dms_to_degrees(&[(v[0].n, v[0].d), (v[1].n, v[1].d), (v[2].n, v[2].d)])
            };
            let sign = |r: &Option<String>, neg: char| {
                if r.as_deref()
                    .is_some_and(|s| s.trim_start().to_ascii_uppercase().starts_with(neg))
                {
                    -1.0
                } else {
                    1.0
                }
            };
            lat = conv(&g.gps_latitude)
                .map(|v| v * sign(&g.gps_latitude_ref, 'S'))
                .filter(|v| v.abs() <= 90.0);
            lon = conv(&g.gps_longitude)
                .map(|v| v * sign(&g.gps_longitude_ref, 'W'))
                .filter(|v| v.abs() <= 180.0);
            if lat.is_none() || lon.is_none() || (lat == Some(0.0) && lon == Some(0.0)) {
                (lat, lon) = (None, None);
            }
        }
        let non_empty = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        let json = serde_json::to_value(&md.exif).ok().and_then(|v| match v {
            serde_json::Value::Object(m) => {
                let m: serde_json::Map<_, _> = m.into_iter().filter(|(_, v)| !v.is_null()).collect();
                (!m.is_empty()).then_some(serde_json::Value::Object(m))
            }
            _ => None,
        });
        Ok(ExifInfo {
            taken_at,
            latitude: lat,
            longitude: lon,
            camera_make: non_empty(&md.make),
            camera_model: non_empty(&md.model),
            lens_model: e
                .lens_model
                .as_deref()
                .and_then(non_empty)
                .or_else(|| md.lens.as_ref().and_then(|l| non_empty(&l.lens_name))),
            orientation: e.orientation.filter(|o| (1..=8).contains(o)).unwrap_or(1) as u8,
            pixel_dims: None,
            rating: None,
            json,
        })
    })
    .ok()
}

/// Gather metadata and decide how to render. Cheap: header parsing plus a scan for JPEGs; no pixel
/// decoding for the usual TIFF/ISO-BMFF RAWs.
pub fn inspect(bytes: &Arc<Vec<u8>>, limits: Limits) -> Result<RawHeader> {
    let src = RawSource::new_from_shared_vec(bytes.clone());
    let cands = scan_jpegs(bytes);
    let chosen = choose_preview(&cands, limits.max_decode_bytes);

    let mut exif = exif::parse(bytes);
    let from_rawler = rawler_exif(&src);
    if let Some(c) = &chosen {
        // RAF / RW2 are not TIFF containers, but their embedded JPEG carries EXIF.
        exif.fill_from(exif::parse(&bytes[c.offset..]));
    }
    if let Some(r) = from_rawler {
        // Orientation: the RAW's own tag wins; rawler is the fallback.
        if exif.orientation == 1 && r.orientation != 1 {
            exif.orientation = r.orientation;
        }
        exif.fill_from(r);
    }

    // rawler knows the exact crop, but not new cameras (and a few decoders panic in header-only mode);
    // then fall back to the size the camera recorded in EXIF, if it is at least as big as every preview.
    let precise = sensor_dims(&src).ok();
    let rw2 = rw2_header(bytes);
    if let Some((_, o)) = rw2
        && exif.orientation == 1
    {
        exif.orientation = o;
    }
    let sensor = precise.or(rw2.map(|(d, _)| d)).or(exif
        .pixel_dims
        .filter(|&(w, h)| cands.iter().all(|c| u64::from(w) * u64::from(h) >= c.pixels())));
    let plan = plan(&cands, chosen, precise, limits);

    // Library dimensions: the real sensor size. If rawler does not know this camera, fall back to
    // the biggest embedded JPEG (better than nothing, and the only size available).
    let fallback = cands.iter().max_by_key(|c| c.pixels()).map(|c| (c.width, c.height));
    let (width, height) = match sensor.or(fallback) {
        Some((w, h)) => imaging::oriented_dims(w, h, exif.orientation),
        None => (0, 0),
    };
    if width == 0 || height == 0 {
        bail!("unsupported RAW: unknown camera and no readable embedded preview");
    }
    Ok(RawHeader {
        sensor,
        width,
        height,
        exif,
        plan,
    })
}

fn plan(
    cands: &[JpegCand],
    chosen: Option<JpegCand>,
    sensor: Option<(u32, u32)>,
    limits: Limits,
) -> Result<Plan, String> {
    let develop_px = sensor.map(|(w, h)| u64::from(w) * u64::from(h));
    let develop_ok = |px: u64| {
        limits.allow_develop && px <= DEVELOP_MAX_PIXELS && px * DEVELOP_BYTES_PER_PIXEL <= limits.max_decode_bytes
    };
    match (chosen, develop_px) {
        (Some(c), px) => {
            // A tiny preview (e.g. a 160 px EXIF thumbnail) is only used if we cannot develop.
            let good = c.long_edge() >= MIN_GOOD_PREVIEW_EDGE;
            match px {
                Some(px) if !good && develop_ok(px) => Ok(Plan::Develop { pixels: px }),
                _ => Ok(Plan::Preview(c)),
            }
        }
        (None, Some(px)) => {
            if !cands.is_empty() {
                return Err(format!(
                    "every embedded preview needs more than the {} MB decode budget, and no develop fallback applies",
                    limits.max_decode_bytes >> 20
                ));
            }
            if !limits.allow_develop {
                return Err("no embedded preview, and RAW develop is disabled (WORKER_RAW_DEVELOP=0)".into());
            }
            if px > DEVELOP_MAX_PIXELS {
                return Err(format!(
                    "no embedded preview, and developing a {} MP RAW is over the {} MP limit",
                    px / 1_000_000,
                    DEVELOP_MAX_PIXELS / 1_000_000
                ));
            }
            if !develop_ok(px) {
                return Err(format!(
                    "no embedded preview, and developing a {} MP RAW needs about {} MB, over the worker memory budget ({} MB available to this job)",
                    px / 1_000_000,
                    (px * DEVELOP_BYTES_PER_PIXEL) >> 20,
                    limits.max_decode_bytes >> 20
                ));
            }
            Ok(Plan::Develop { pixels: px })
        }
        (None, None) => Err(if cands.is_empty() {
            "unsupported RAW: unknown camera and no embedded JPEG preview".into()
        } else {
            format!(
                "embedded previews are over the {} MB decode budget",
                limits.max_decode_bytes >> 20
            )
        }),
    }
}

/// Embedded previews are often stored unrotated even when the RAW says "rotate 90", but some
/// cameras write an already-upright preview. Compare aspect: the oriented sensor tells which one
/// we have, so the rotation is applied exactly once.
pub fn preview_orientation(orientation: u8, sensor: Option<(u32, u32)>, preview: (u32, u32)) -> u8 {
    let swaps = (5..=8).contains(&orientation);
    let Some((sw, sh)) = sensor else { return orientation };
    if !swaps {
        return orientation;
    }
    let near_square = |w: u32, h: u32| {
        let (a, b) = (f64::from(w.max(h)), f64::from(w.min(h)));
        a / b < 1.05
    };
    if near_square(sw, sh) || near_square(preview.0, preview.1) {
        return orientation;
    }
    let sensor_landscape = sw >= sh;
    let preview_landscape = preview.0 >= preview.1;
    // Same aspect as the *unrotated* sensor: it still needs the rotation. Opposite: already upright.
    if sensor_landscape == preview_landscape {
        orientation
    } else {
        1
    }
}

fn decode_jpeg(bytes: &[u8], max_alloc: u64) -> Result<DynamicImage> {
    let mut reader = ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(max_alloc);
    reader.limits(limits);
    reader.decode().context("decode embedded JPEG preview")
}

/// Produce the derived images according to `header.plan`.
pub fn render(bytes: &Arc<Vec<u8>>, header: &RawHeader) -> Result<imaging::Derived> {
    let (img, o) = render_image(bytes, header)?;
    let mut d = imaging::derive_image(img, o)?;
    if matches!(header.plan, Ok(Plan::Preview(_))) {
        // Report the real sensor size, not the preview's.
        d.width = header.width;
        d.height = header.height;
    }
    Ok(d)
}

/// The picture `header.plan` yields (the embedded preview or the developed sensor data) and the EXIF orientation to
/// apply to it.
pub fn render_image(bytes: &Arc<Vec<u8>>, header: &RawHeader) -> Result<(DynamicImage, u8)> {
    let plan = header.plan.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
    match plan {
        Plan::Preview(c) => {
            let img = decode_jpeg(&bytes[c.offset..], header.cost_bytes().max(64 << 20) * 2)?;
            let o = preview_orientation(header.exif.orientation, header.sensor, (img.width(), img.height()));
            Ok((img, o))
        }
        Plan::Develop { .. } => {
            let src = RawSource::new_from_shared_vec(bytes.clone());
            let img = guard("developing", || {
                let dec = rawler::get_decoder(&src).map_err(|e| anyhow::anyhow!("{e}"))?;
                let raw = dec
                    .raw_image(&src, &RawDecodeParams::default(), false)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let inter = RawDevelop::default()
                    .develop_intermediate(&raw)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                drop(raw);
                inter.to_dynamic_image().context("developed image buffer")
            })?;
            Ok((img, header.exif.orientation))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    /// Minimal JPEG header: SOI, an APP0, SOF0 with the given size, then SOS.
    fn fake_jpeg(w: u16, h: u16, sof: u8) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46];
        b.extend([0xFF, sof, 0x00, 0x0B, 0x08]);
        b.extend(h.to_be_bytes());
        b.extend(w.to_be_bytes());
        b.extend([0x01, 0x01, 0x11, 0x00]);
        b.extend([0xFF, 0xDA, 0x00, 0x02]);
        b
    }

    #[test]
    fn jpeg_dims_parsing() {
        assert_eq!(jpeg_dims_at(&fake_jpeg(6000, 4000, 0xC0), 0), Some((6000, 4000)));
        assert_eq!(jpeg_dims_at(&fake_jpeg(300, 200, 0xC2), 0), Some((300, 200)));
        // Lossless JPEG (SOF3) is raw sensor data, not a preview.
        assert_eq!(jpeg_dims_at(&fake_jpeg(6000, 4000, 0xC3), 0), None);
        // Truncated / zero-size.
        assert_eq!(jpeg_dims_at(&fake_jpeg(0, 10, 0xC0), 0), None);
        assert_eq!(jpeg_dims_at(&fake_jpeg(10, 10, 0xC0)[..12], 0), None);
    }

    #[test]
    fn scan_finds_embedded_streams_and_rejects_noise() {
        let mut file = b"II*\0\x08\0\0\0 random header ".to_vec();
        file.extend([0xFF, 0xD8, 0xFF, 0x12, 0x34, 0x56]); // noise that merely looks like a SOI
        file.extend(fake_jpeg(1620, 1080, 0xC0));
        file.extend([0u8; 17]);
        file.extend(fake_jpeg(8192, 5464, 0xC0));
        file.extend(fake_jpeg(4000, 3000, 0xC3));
        let c = scan_jpegs(&file);
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].width, c[0].height), (1620, 1080));
        assert_eq!((c[1].width, c[1].height), (8192, 5464));
    }

    #[test]
    fn choose_prefers_cheapest_big_enough() {
        let cs = [
            JpegCand {
                offset: 0,
                width: 160,
                height: 120,
            },
            JpegCand {
                offset: 1,
                width: 1620,
                height: 1080,
            },
            JpegCand {
                offset: 2,
                width: 6000,
                height: 4000,
            },
        ];
        assert_eq!(choose_preview(&cs, u64::MAX).unwrap().offset, 1);
        // Nothing reaches 1440 px: take the largest.
        assert_eq!(choose_preview(&cs[..1], u64::MAX).unwrap().offset, 0);
    }

    #[test]
    fn choose_falls_back_to_largest_and_respects_budget() {
        let small = JpegCand {
            offset: 0,
            width: 1000,
            height: 700,
        };
        let mid = JpegCand {
            offset: 1,
            width: 1200,
            height: 800,
        };
        assert_eq!(choose_preview(&[small, mid], u64::MAX).unwrap().offset, 1);
        let huge = JpegCand {
            offset: 2,
            width: 12000,
            height: 8000,
        }; // 96 MP
        // Over budget: never chosen, even though it is the only one big enough.
        assert_eq!(choose_preview(&[small, huge], 100 << 20).unwrap().offset, 0);
        assert_eq!(choose_preview(&[huge], 100 << 20), None);
    }

    const BIG: Limits = Limits {
        max_decode_bytes: 492 << 20, // what a 640 MB budget leaves after a 100 MB original
        allow_develop: true,
    };

    #[test]
    fn hundred_megapixel_raw_without_preview_is_refused() {
        // 11648x8736 = 101.8 MP sensor (GFX100-class), nothing embedded.
        let err = plan(&[], None, Some((11648, 8736)), BIG).unwrap_err();
        assert!(
            err.contains("MP RAW is over the 150 MP limit") || err.contains("over the worker memory budget"),
            "{err}"
        );
        // Even a 60 MP one: 48 B/px would need ~2.9 GB.
        let err = plan(&[], None, Some((9504, 6336)), BIG).unwrap_err();
        assert!(err.contains("needs about") && err.contains("memory budget"), "{err}");
        // And the cap holds regardless of budget.
        let huge_budget = Limits {
            max_decode_bytes: u64::MAX,
            ..BIG
        };
        let err = plan(&[], None, Some((16000, 12000)), huge_budget).unwrap_err();
        assert!(err.contains("150 MP"), "{err}");
        // Develop disabled: refuse even a small one.
        let off = Limits {
            allow_develop: false,
            ..BIG
        };
        assert!(
            plan(&[], None, Some((2000, 1500)), off)
                .unwrap_err()
                .contains("disabled")
        );
    }

    #[test]
    fn oversized_embedded_preview_is_never_decoded() {
        let giant = JpegCand {
            offset: 0,
            width: 11648,
            height: 8736,
        }; // 101.8 MP * 6 B = 583 MB
        assert_eq!(choose_preview(&[giant], BIG.max_decode_bytes), None);
        let err = plan(&[giant], None, None, BIG).unwrap_err();
        assert!(err.contains("decode budget"), "{err}");
        // A modest preview alongside it is used instead.
        let modest = JpegCand {
            offset: 9,
            width: 1620,
            height: 1080,
        };
        assert_eq!(choose_preview(&[giant, modest], BIG.max_decode_bytes), Some(modest));
    }

    #[test]
    fn small_preview_triggers_develop_only_when_allowed() {
        let thumb = JpegCand {
            offset: 0,
            width: 160,
            height: 120,
        };
        let p = plan(&[thumb], Some(thumb), Some((2048, 1536)), BIG).unwrap();
        assert_eq!(p, Plan::Develop { pixels: 2048 * 1536 });
        let off = Limits {
            allow_develop: false,
            ..BIG
        };
        assert_eq!(
            plan(&[thumb], Some(thumb), Some((2048, 1536)), off).unwrap(),
            Plan::Preview(thumb)
        );
        // Too big to develop in budget: fall back to the thumbnail rather than fail.
        assert_eq!(
            plan(&[thumb], Some(thumb), Some((9504, 6336)), BIG).unwrap(),
            Plan::Preview(thumb)
        );
    }

    // ---- a synthetic RAW: TIFF IFD0 + an embedded JPEG preview, no real sensor data -------------

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        // Left half red, right half blue: tells us which way the preview was turned.
        let img = image::RgbImage::from_fn(w, h, |x, _| {
            if x < w / 2 {
                image::Rgb([230, 20, 20])
            } else {
                image::Rgb([20, 20, 230])
            }
        });
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    }

    fn synthetic_raw(orientation: u16, preview: &[u8], sensor: (u16, u16)) -> Arc<Vec<u8>> {
        use crate::exif::testutil::{ascii, exif_app1};
        let ifd0 = vec![
            (0x010F, 2, ascii("ACME")),
            (0x0110, 2, ascii("Synth 1")),
            (0x0112, 3, orientation.to_le_bytes().to_vec()),
        ];
        let sub = vec![
            (0x9003, 2, ascii("2022:05:06 07:08:09")),
            (0xA002, 3, sensor.0.to_le_bytes().to_vec()),
            (0xA003, 3, sensor.1.to_le_bytes().to_vec()),
        ];
        let app1 = exif_app1(&ifd0, &sub, &[]);
        let mut f = app1[10..].to_vec(); // the bare TIFF stream: skip FFE1, length and "Exif\0\0"
        f.extend(vec![0u8; 64]);
        f.extend(preview);
        Arc::new(f)
    }

    fn px(img: &[u8], x: u32, y: u32) -> [u8; 3] {
        let d = image::load_from_memory(img).unwrap().to_rgb8();
        d.get_pixel(x, y).0
    }

    #[test]
    fn synthetic_raw_end_to_end_orients_an_unrotated_preview() {
        // Portrait shot (orientation 6) whose stored preview is still landscape 300x200; sensor 600x400.
        let raw = synthetic_raw(6, &jpeg(300, 200), (600, 400));
        assert_eq!(crate::formats::sniff(&raw, "x.nef"), Some(crate::formats::Kind::Raw));
        let h = inspect(&raw, BIG).unwrap();
        assert_eq!(h.exif.camera_make.as_deref(), Some("ACME"));
        assert_eq!(h.exif.camera_model.as_deref(), Some("Synth 1"));
        assert_eq!(h.exif.orientation, 6);
        assert_eq!(
            h.sensor,
            Some((600, 400)),
            "sensor size comes from the camera's EXIF, not the preview"
        );
        assert_eq!(
            (h.width, h.height),
            (400, 600),
            "reported size is the oriented sensor size"
        );
        assert!(matches!(h.plan, Ok(Plan::Preview(_))));
        let d = render(&raw, &h).unwrap();
        assert_eq!((d.width, d.height), (400, 600));
        // Rotated 90 degrees clockwise: the red left half ends up on top.
        let top = px(&d.preview_webp, 60, 20);
        let bottom = px(&d.preview_webp, 60, 180);
        assert!(top[0] > 150 && top[2] < 100, "top should be red, got {top:?}");
        assert!(
            bottom[2] > 150 && bottom[0] < 100,
            "bottom should be blue, got {bottom:?}"
        );
        assert_eq!(
            image::load_from_memory(&d.preview_webp).unwrap().dimensions(),
            (200, 300)
        );
    }

    #[test]
    fn synthetic_raw_with_already_upright_preview_is_not_rotated_again() {
        // Same camera, but this one stored the preview already turned upright (200x300).
        let raw = synthetic_raw(6, &jpeg(200, 300), (600, 400));
        let h = inspect(&raw, BIG).unwrap();
        assert_eq!((h.width, h.height), (400, 600));
        let d = render(&raw, &h).unwrap();
        assert_eq!(
            image::load_from_memory(&d.preview_webp).unwrap().dimensions(),
            (200, 300)
        );
        // Not rotated: still red on the left, blue on the right.
        let left = px(&d.preview_webp, 20, 150);
        let right = px(&d.preview_webp, 180, 150);
        assert!(left[0] > 150 && right[2] > 150, "left {left:?} right {right:?}");
    }

    #[test]
    fn corrupt_or_unknown_raw_fails_cleanly() {
        // TIFF magic, no preview, unknown camera: a clear error, no panic.
        let junk = Arc::new([b"II*\0\x08\0\0\0".as_slice(), &[0u8; 200]].concat());
        assert!(inspect(&junk, BIG).is_err());
        // A preview cut off mid-stream: must not panic.
        let mut j = jpeg(300, 200);
        j.truncate(j.len() / 3);
        let raw = synthetic_raw(1, &j, (600, 400));
        if let Ok(h) = inspect(&raw, BIG) {
            // The JPEG decoder tolerates truncation (grey fill) or errors; either is fine, a panic is not.
            let _ = render(&raw, &h);
        }
    }

    #[test]
    fn rw2_header_reads_sensor_borders_and_orientation() {
        // IFD0 at offset 8 with SensorTopBorder(4)=12, Left(5)=20, Bottom(6)=3000+12, Right(7)=4000+20, Orientation=6.
        let entries: [(u16, u16); 5] = [(4, 12), (5, 20), (6, 3012), (7, 4020), (0x0112, 6)];
        let mut b = b"IIU\0".to_vec();
        b.extend(8u32.to_le_bytes());
        b.extend((entries.len() as u16).to_le_bytes());
        for (tag, v) in entries {
            b.extend(tag.to_le_bytes());
            b.extend(3u16.to_le_bytes());
            b.extend(1u32.to_le_bytes());
            b.extend(u32::from(v).to_le_bytes());
        }
        b.extend(0u32.to_le_bytes());
        assert_eq!(rw2_header(&b), Some(((4000, 3000), 6)));
        assert_eq!(rw2_header(b"II*\0\x08\0\0\0"), None);
        assert_eq!(rw2_header(&b[..14]), None);
    }

    #[test]
    fn orientation_not_applied_twice() {
        // 6000x4000 sensor shot in portrait (orientation 6 = rotate 90 CW).
        let sensor = Some((6000, 4000));
        // Unrotated landscape preview still needs the rotation.
        assert_eq!(preview_orientation(6, sensor, (1620, 1080)), 6);
        // Preview already stored portrait: apply nothing.
        assert_eq!(preview_orientation(6, sensor, (1080, 1620)), 1);
        // Landscape shot, orientation 1: untouched either way.
        assert_eq!(preview_orientation(1, sensor, (1620, 1080)), 1);
        // 180 and flips do not change the aspect, so they are always applied.
        assert_eq!(preview_orientation(3, sensor, (1620, 1080)), 3);
        // Unknown sensor: trust the tag.
        assert_eq!(preview_orientation(8, None, (1080, 1620)), 8);
        // Square-ish: ambiguous, trust the tag.
        assert_eq!(preview_orientation(6, Some((4000, 3900)), (1000, 990)), 6);
    }
}
