//! Text detection: the DB network's probability map turned into rotated rectangles.
use std::sync::Mutex;

use anyhow::{Result, bail};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use ort::value::Tensor;

use crate::geom::{self, P, Rect};

#[derive(Debug, Clone)]
pub struct DetParams {
    /// The image is scaled down so its long side is at most this (never up), then both sides are rounded to 32.
    pub side: u32,
    /// Probability above which a pixel is text.
    pub thresh: f32,
    /// Mean probability inside a box below which it is dropped.
    pub box_thresh: f32,
    /// How much the boxes are grown (the area times this over the perimeter, on every side).
    pub unclip: f32,
    /// Normalisation, in BGR channel order as the models were trained.
    pub mean: [f32; 3],
    pub std: [f32; 3],
    pub max_candidates: usize,
    /// At most this many boxes are read, the biggest first: a screenshot of a spreadsheet has hundreds of tiny ones, and the
    /// time (and the memory of the recognition batches) grows with their number.
    pub max_boxes: usize,
}

impl Default for DetParams {
    fn default() -> Self {
        Self {
            side: 960,
            thresh: 0.3,
            box_thresh: 0.6,
            unclip: 1.5,
            mean: [0.485, 0.456, 0.406],
            std: [0.229, 0.224, 0.225],
            max_candidates: 1000,
            max_boxes: 150,
        }
    }
}

/// The size the network sees: long side at most `side`, both rounded to a multiple of 32 (at least 32).
pub fn target_size(w: u32, h: u32, side: u32) -> (u32, u32) {
    let long = w.max(h);
    let scale = if long > side { side as f32 / long as f32 } else { 1.0 };
    let round = |v: u32| (((v as f32 * scale) / 32.0).round() as u32).max(1) * 32;
    (round(w), round(h))
}

pub(crate) fn detect(
    session: &Mutex<ort::session::Session>,
    p: &DetParams,
    rgb: &[u8],
    w: u32,
    h: u32,
) -> Result<Vec<Rect>> {
    let (nw, nh) = target_size(w, h, p.side);
    let src = ImageRef::new(w, h, rgb, PixelType::U8x3)?;
    let mut dst = Image::new(nw, nh, PixelType::U8x3);
    Resizer::new().resize(
        &src,
        &mut dst,
        &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear)),
    )?;
    let (nwu, nhu) = (nw as usize, nh as usize);
    let plane = nwu * nhu;
    let px = dst.buffer();
    let mut input = vec![0f32; 3 * plane];
    for i in 0..plane {
        for c in 0..3 {
            // BGR planes: channel 0 is the blue one.
            input[c * plane + i] = (f32::from(px[i * 3 + (2 - c)]) / 255.0 - p.mean[c]) / p.std[c];
        }
    }
    let tensor =
        Tensor::from_array(([1usize, 3, nhu, nwu], input.into_boxed_slice())).map_err(|e| anyhow::anyhow!("{e}"))?;
    let map: Vec<f32> = {
        let mut s = session.lock().unwrap_or_else(|e| e.into_inner());
        let name = s.inputs()[0].name().to_string();
        let outputs = s
            .run(ort::inputs![name.as_str() => tensor])
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if data.len() != plane {
            bail!("unexpected detection output shape {shape:?} for a {nw}x{nh} input");
        }
        data.to_vec()
    };
    let scale = ((w as f32 / nw as f32) + (h as f32 / nh as f32)) / 2.0;
    let mut rects = boxes(&map, nwu, nhu, p);
    for r in &mut rects {
        // The two axes are scaled by slightly different factors (rounding to 32); scale the centre exactly and the lengths by
        // their mean, which is within a few percent.
        *r = Rect {
            center: (r.center.0 * w as f32 / nw as f32, r.center.1 * h as f32 / nh as f32),
            ..r.scaled(scale)
        };
    }
    rects.sort_by(|a, b| b.area().partial_cmp(&a.area()).unwrap_or(std::cmp::Ordering::Equal));
    rects.truncate(p.max_boxes);
    Ok(rects)
}

/// Boxes (in the map's own pixels) from a probability map.
pub fn boxes(map: &[f32], w: usize, h: usize, p: &DetParams) -> Vec<Rect> {
    let bits: Vec<bool> = map.iter().map(|v| *v > p.thresh).collect();
    let mut out = Vec::new();
    for comp in geom::components(&bits, w, h, p.max_candidates) {
        if comp.len() < 6 {
            continue;
        }
        let pts: Vec<P> = comp.iter().map(|&(x, y)| (x as f32 + 0.5, y as f32 + 0.5)).collect();
        let Some(rect) = geom::min_area_rect(&geom::convex_hull(&pts)) else {
            continue;
        };
        if rect.len_v < 3.0 {
            continue;
        }
        if mean_inside(map, w, h, &rect) < p.box_thresh {
            continue;
        }
        let d = rect.area() * p.unclip / (2.0 * (rect.len_u + rect.len_v));
        let grown = rect.grown(d);
        if grown.len_v < 5.0 {
            continue;
        }
        out.push(grown);
    }
    out
}

/// Mean probability over the map pixels whose centres lie inside `r`.
fn mean_inside(map: &[f32], w: usize, h: usize, r: &Rect) -> f32 {
    let c = r.corners();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in c {
        x0 = x0.min(p.0);
        y0 = y0.min(p.1);
        x1 = x1.max(p.0);
        y1 = y1.max(p.1);
    }
    let (xa, xb) = ((x0.floor().max(0.0)) as usize, (x1.ceil().min(w as f32)) as usize);
    let (ya, yb) = ((y0.floor().max(0.0)) as usize, (y1.ceil().min(h as f32)) as usize);
    let (mut sum, mut n) = (0f32, 0u32);
    for y in ya..yb {
        for x in xa..xb {
            if r.contains((x as f32 + 0.5, y as f32 + 0.5)) {
                sum += map[y * w + x];
                n += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { sum / n as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_size_caps_the_long_side_and_rounds_to_32() {
        assert_eq!(target_size(1600, 1200, 960), (960, 736));
        assert_eq!(target_size(640, 480, 960), (640, 480), "never scaled up");
        assert_eq!(target_size(100, 30, 960), (96, 32));
        assert_eq!(target_size(10, 10, 960), (32, 32));
        assert_eq!(target_size(1200, 1600, 1280), (960, 1280));
    }

    fn painted(w: usize, h: usize, rects: &[(usize, usize, usize, usize, f32)]) -> Vec<f32> {
        let mut m = vec![0.01f32; w * h];
        for &(x0, y0, x1, y1, v) in rects {
            for y in y0..y1 {
                for x in x0..x1 {
                    m[y * w + x] = v;
                }
            }
        }
        m
    }

    #[test]
    fn two_text_lines_become_two_grown_boxes_and_noise_is_dropped() {
        let m = painted(
            200,
            100,
            &[
                (10, 10, 150, 22, 0.9),
                (10, 50, 120, 60, 0.8),
                (180, 90, 183, 92, 0.9),
                (160, 30, 180, 40, 0.4),
            ],
        );
        let mut b = boxes(&m, 200, 100, &DetParams::default());
        b.sort_by(|a, c| a.center.1.partial_cmp(&c.center.1).unwrap());
        assert_eq!(
            b.len(),
            2,
            "the speck is too small and the faint blob is under the box threshold: {b:?}"
        );
        // The pixel centres span 139 x 11; that grows by 139*11*1.5/(2*150) = 7.6 on every side.
        assert!(
            (b[0].len_u - 154.3).abs() < 0.5 && (b[0].len_v - 26.3).abs() < 0.5,
            "{:?}",
            b[0]
        );
        assert!((b[0].center.0 - 80.0).abs() < 0.5);
    }

    #[test]
    fn a_tilted_line_keeps_its_angle() {
        let (w, h) = (200usize, 120usize);
        let mut m = vec![0.0f32; w * h];
        let ang = 20f32.to_radians();
        for i in 0..120 {
            for j in 0..10 {
                let (x, y) = (
                    20.0 + i as f32 * ang.cos() - j as f32 * ang.sin(),
                    20.0 + i as f32 * ang.sin() + j as f32 * ang.cos(),
                );
                m[y as usize * w + x as usize] = 0.9;
            }
        }
        let b = boxes(&m, w, h, &DetParams::default());
        assert_eq!(b.len(), 1);
        assert!((b[0].u.1.atan2(b[0].u.0).to_degrees() - 20.0).abs() < 4.0, "{:?}", b[0]);
    }
}
