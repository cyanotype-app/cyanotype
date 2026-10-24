//! Face detection with YuNet (OpenCV zoo, `face_detection_yunet_2026may.onnx`, MIT): a 75k-parameter anchor-free detector
//! with five landmarks per face. The graph takes any size that is a multiple of 32 and returns, for each of the strides
//! 8, 16 and 32, one row per grid cell: class and objectness scores, a box and five landmarks, all relative to the cell.
//! Everything around the network is here and unit-tested without it: the input tensor, the decoding of the twelve outputs
//! and the non-maximum suppression.
use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result, bail};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use ort::value::Tensor;

/// The strides of the three output levels, finest first.
pub const STRIDES: [usize; 3] = [8, 16, 32];

/// One detected face, in pixels of the image that was searched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    /// sqrt(class score * objectness), 0..1.
    pub score: f32,
    /// Left, top, width, height.
    pub bbox: [f32; 4],
    /// Right eye, left eye, nose tip, right mouth corner, left mouth corner, as YuNet orders them (the "right" ones
    /// are on the left of the picture: the subject's right). This is also the order of the ArcFace template.
    pub landmarks: [[f32; 2]; 5],
}

impl Detection {
    /// The shorter side of the box.
    pub fn min_side(&self) -> f32 {
        self.bbox[2].min(self.bbox[3])
    }

    pub fn area(&self) -> f32 {
        self.bbox[2].max(0.0) * self.bbox[3].max(0.0)
    }
}

/// How an image is searched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectorConfig {
    /// Detections below this score are dropped.
    pub score_threshold: f32,
    /// Two boxes overlapping more than this (intersection over union) are one face: the weaker is dropped.
    pub nms_iou: f32,
    /// The image is scaled so its long edge is this many pixels (smaller images are enlarged by at most `max_scale`).
    /// YuNet finds faces of about 10 to 300 px, so a bigger input finds smaller faces, at a cost that grows with the area.
    pub long_edge: u32,
    pub max_scale: f32,
    /// The most faces kept per image (the strongest).
    pub max_faces: usize,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            score_threshold: 0.7,
            nms_iou: 0.3,
            long_edge: 1280,
            max_scale: 1.0,
            max_faces: 64,
        }
    }
}

/// The three output tensors of one level, as flat slices.
pub struct Level<'a> {
    pub stride: usize,
    /// `cells` values.
    pub cls: &'a [f32],
    /// `cells` values.
    pub obj: &'a [f32],
    /// `cells * 4` values: centre offset x, y (in cells), log width, log height (in strides).
    pub bbox: &'a [f32],
    /// `cells * 10` values: five landmarks as offsets (in strides) from the cell's corner.
    pub kps: &'a [f32],
}

/// Turn the network's outputs for an `input_w` x `input_h` tensor into detections in that tensor's pixels (not yet
/// suppressed). The formulas are those of OpenCV's `FaceDetectorYN` and libfacedetection.
pub fn decode(levels: &[Level], input_w: usize, input_h: usize, score_threshold: f32) -> Result<Vec<Detection>> {
    let mut out = Vec::new();
    for l in levels {
        if l.stride == 0 || !input_w.is_multiple_of(l.stride) || !input_h.is_multiple_of(l.stride) {
            bail!("a {input_w}x{input_h} input is not a multiple of stride {}", l.stride);
        }
        let (cols, rows) = (input_w / l.stride, input_h / l.stride);
        let cells = cols * rows;
        if l.cls.len() != cells || l.obj.len() != cells || l.bbox.len() != cells * 4 || l.kps.len() != cells * 10 {
            bail!(
                "stride {}: expected {cells} cells, got {} class scores, {} objectness, {} box and {} landmark values",
                l.stride,
                l.cls.len(),
                l.obj.len(),
                l.bbox.len(),
                l.kps.len()
            );
        }
        let s = l.stride as f32;
        for r in 0..rows {
            for c in 0..cols {
                let i = r * cols + c;
                let score = (l.cls[i].clamp(0.0, 1.0) * l.obj[i].clamp(0.0, 1.0)).sqrt();
                if score < score_threshold {
                    continue;
                }
                let b = &l.bbox[i * 4..i * 4 + 4];
                let cx = (c as f32 + b[0]) * s;
                let cy = (r as f32 + b[1]) * s;
                let w = b[2].exp() * s;
                let h = b[3].exp() * s;
                let k = &l.kps[i * 10..i * 10 + 10];
                let mut landmarks = [[0.0f32; 2]; 5];
                for (n, p) in landmarks.iter_mut().enumerate() {
                    *p = [(k[n * 2] + c as f32) * s, (k[n * 2 + 1] + r as f32) * s];
                }
                out.push(Detection {
                    score,
                    bbox: [cx - w / 2.0, cy - h / 2.0, w, h],
                    landmarks,
                });
            }
        }
    }
    Ok(out)
}

/// Intersection over union of two `[x, y, w, h]` boxes.
pub fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let (ax2, ay2, bx2, by2) = (a[0] + a[2], a[1] + a[3], b[0] + b[2], b[1] + b[3]);
    let iw = (ax2.min(bx2) - a[0].max(b[0])).max(0.0);
    let ih = (ay2.min(by2) - a[1].max(b[1])).max(0.0);
    let inter = iw * ih;
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// Greedy non-maximum suppression: strongest first, each kept box drops the weaker ones that overlap it by more than
/// `max_iou`. At most `max_keep` are kept. The result is ordered by score, highest first.
pub fn nms(mut dets: Vec<Detection>, max_iou: f32, max_keep: usize) -> Vec<Detection> {
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for d in dets {
        if kept.len() >= max_keep {
            break;
        }
        if kept.iter().all(|k| iou(&k.bbox, &d.bbox) <= max_iou) {
            kept.push(d);
        }
    }
    kept
}

/// The geometry of a search: how an image of `w` x `h` pixels becomes the network's input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// Size of the scaled image.
    pub scaled_w: u32,
    pub scaled_h: u32,
    /// Size of the input tensor: the scaled image, padded at the right and bottom to a multiple of 32.
    pub input_w: u32,
    pub input_h: u32,
    /// Scaled pixels per original pixel.
    pub scale: f32,
}

pub fn plan(w: u32, h: u32, cfg: &DetectorConfig) -> Plan {
    let long = w.max(h).max(1) as f32;
    let scale = (cfg.long_edge as f32 / long).min(cfg.max_scale.max(f32::MIN_POSITIVE));
    let scaled_w = ((w as f32 * scale).round() as u32).max(1);
    let scaled_h = ((h as f32 * scale).round() as u32).max(1);
    let pad = |v: u32| v.div_ceil(32).max(1) * 32;
    Plan {
        scaled_w,
        scaled_h,
        input_w: pad(scaled_w),
        input_h: pad(scaled_h),
        scale: scaled_w as f32 / w.max(1) as f32,
    }
}

/// Move detections from the input tensor's pixels back to the original image's, clipping the box to the image.
pub fn to_image(dets: Vec<Detection>, plan: &Plan, w: u32, h: u32) -> Vec<Detection> {
    let inv_x = w as f32 / plan.scaled_w as f32;
    let inv_y = h as f32 / plan.scaled_h as f32;
    dets.into_iter()
        .filter_map(|mut d| {
            let x1 = (d.bbox[0] * inv_x).clamp(0.0, w as f32);
            let y1 = (d.bbox[1] * inv_y).clamp(0.0, h as f32);
            let x2 = ((d.bbox[0] + d.bbox[2]) * inv_x).clamp(0.0, w as f32);
            let y2 = ((d.bbox[1] + d.bbox[3]) * inv_y).clamp(0.0, h as f32);
            if x2 - x1 < 1.0 || y2 - y1 < 1.0 {
                return None;
            }
            d.bbox = [x1, y1, x2 - x1, y2 - y1];
            for p in &mut d.landmarks {
                *p = [p[0] * inv_x, p[1] * inv_y];
            }
            Some(d)
        })
        .collect()
}

/// The detector. One session, one image at a time.
pub struct Detector {
    session: Mutex<ort::session::Session>,
    cfg: DetectorConfig,
}

impl Detector {
    pub fn load(file: &Path, threads: usize, cfg: DetectorConfig) -> Result<Self> {
        Ok(Self {
            session: Mutex::new(crate::session(file, threads)?),
            cfg,
        })
    }

    pub fn config(&self) -> &DetectorConfig {
        &self.cfg
    }

    /// Faces in an RGB8 image, strongest first, in the image's pixels.
    pub fn detect(&self, rgb: &[u8], w: u32, h: u32) -> Result<Vec<Detection>> {
        if w == 0 || h == 0 || rgb.len() != w as usize * h as usize * 3 {
            bail!("not a {w}x{h} RGB8 buffer ({} bytes)", rgb.len());
        }
        let plan = plan(w, h, &self.cfg);
        let data = input_tensor(rgb, w, h, &plan)?;
        let (iw, ih) = (plan.input_w as usize, plan.input_h as usize);
        let input =
            Tensor::from_array(([1usize, 3, ih, iw], data.into_boxed_slice())).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
        let outputs = session
            .run(ort::inputs!["input" => input])
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let get = |name: String| -> Result<Vec<f32>> {
            let (_, v) = outputs[name.as_str()]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("{e}"))
                .with_context(|| format!("output {name}"))?;
            Ok(v.to_vec())
        };
        let mut owned = Vec::new();
        for s in STRIDES {
            owned.push((
                s,
                get(format!("cls_{s}"))?,
                get(format!("obj_{s}"))?,
                get(format!("bbox_{s}"))?,
                get(format!("kps_{s}"))?,
            ));
        }
        let levels: Vec<Level> = owned
            .iter()
            .map(|(stride, cls, obj, bbox, kps)| Level {
                stride: *stride,
                cls,
                obj,
                bbox,
                kps,
            })
            .collect();
        let dets = decode(&levels, iw, ih, self.cfg.score_threshold)?;
        let dets = nms(dets, self.cfg.nms_iou, self.cfg.max_faces);
        Ok(to_image(dets, &plan, w, h))
    }
}

/// The input tensor: the image scaled by `plan`, as BGR floats 0..255, channel-first, padded with zeros.
pub fn input_tensor(rgb: &[u8], w: u32, h: u32, plan: &Plan) -> Result<Vec<f32>> {
    let scaled: std::borrow::Cow<[u8]> = if (plan.scaled_w, plan.scaled_h) == (w, h) {
        std::borrow::Cow::Borrowed(rgb)
    } else {
        let src = ImageRef::new(w, h, rgb, PixelType::U8x3).context("source image")?;
        let mut dst = Image::new(plan.scaled_w, plan.scaled_h, PixelType::U8x3);
        let alg = if plan.scale < 1.0 {
            ResizeAlg::Convolution(FilterType::Bilinear)
        } else {
            ResizeAlg::Convolution(FilterType::CatmullRom)
        };
        Resizer::new()
            .resize(&src, &mut dst, &ResizeOptions::new().resize_alg(alg))
            .context("resize")?;
        std::borrow::Cow::Owned(dst.into_vec())
    };
    let (iw, ih) = (plan.input_w as usize, plan.input_h as usize);
    let (sw, sh) = (plan.scaled_w as usize, plan.scaled_h as usize);
    let mut out = vec![0f32; 3 * iw * ih];
    let plane = iw * ih;
    for y in 0..sh {
        for x in 0..sw {
            let p = (y * sw + x) * 3;
            let o = y * iw + x;
            out[o] = f32::from(scaled[p + 2]);
            out[plane + o] = f32::from(scaled[p + 1]);
            out[2 * plane + o] = f32::from(scaled[p]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(score: f32, x: f32, y: f32, w: f32, h: f32) -> Detection {
        Detection {
            score,
            bbox: [x, y, w, h],
            landmarks: [[x, y]; 5],
        }
    }

    #[test]
    fn iou_of_identical_disjoint_and_half_overlapping_boxes() {
        let a = [0.0, 0.0, 10.0, 10.0];
        assert!((iou(&a, &a) - 1.0).abs() < 1e-6);
        assert_eq!(iou(&a, &[20.0, 20.0, 5.0, 5.0]), 0.0);
        // 10x10 and 10x10 shifted by 5: intersection 50, union 150.
        assert!((iou(&a, &[5.0, 0.0, 10.0, 10.0]) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(iou(&[0.0, 0.0, 0.0, 0.0], &[0.0, 0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn nms_keeps_the_strongest_of_overlapping_boxes_and_all_the_separate_ones() {
        let dets = vec![
            det(0.8, 100.0, 100.0, 50.0, 50.0),
            det(0.95, 104.0, 102.0, 50.0, 50.0),
            det(0.7, 300.0, 100.0, 40.0, 40.0),
            det(0.9, 101.0, 99.0, 52.0, 48.0),
        ];
        let kept = nms(dets, 0.3, 10);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.95);
        assert_eq!(kept[1].score, 0.7);
    }

    #[test]
    fn nms_keeps_at_most_max_keep_strongest() {
        let dets: Vec<_> = (0..10)
            .map(|i| det(0.5 + i as f32 * 0.01, i as f32 * 100.0, 0.0, 20.0, 20.0))
            .collect();
        let kept = nms(dets, 0.3, 3);
        assert_eq!(kept.len(), 3);
        assert!(kept[0].score > kept[1].score && kept[1].score > kept[2].score);
    }

    #[test]
    fn decode_reads_the_cell_the_box_and_the_landmarks() {
        // A 64x32 input: stride 32 has 2x1 cells. Cell (col 1, row 0) is hot.
        let (cls, obj) = (vec![0.0, 0.81], vec![0.0, 1.0]);
        let mut bbox = vec![0.0; 8];
        bbox[4..8].copy_from_slice(&[0.5, 0.5, 0.0, 0.0]); // centre at the middle of the cell; width = height = exp(0) * 32
        let mut kps = vec![0.0; 20];
        kps[10..20].copy_from_slice(&[0.25, 0.25, 0.75, 0.25, 0.5, 0.5, 0.25, 0.75, 0.75, 0.75]);
        let level = Level {
            stride: 32,
            cls: &cls,
            obj: &obj,
            bbox: &bbox,
            kps: &kps,
        };
        let d = decode(&[level], 64, 32, 0.5).unwrap();
        assert_eq!(d.len(), 1);
        let d = d[0];
        assert!((d.score - 0.9).abs() < 1e-6);
        // Centre x = (1 + 0.5) * 32 = 48, y = (0 + 0.5) * 32 = 16; 32 x 32.
        assert_eq!(d.bbox, [32.0, 0.0, 32.0, 32.0]);
        assert_eq!(d.landmarks[0], [40.0, 8.0]);
        assert_eq!(d.landmarks[2], [48.0, 16.0]);
        assert_eq!(d.landmarks[4], [56.0, 24.0]);
    }

    #[test]
    fn decode_clamps_scores_and_applies_the_threshold() {
        let (cls, obj) = (vec![3.0, 0.3], vec![2.0, 0.3]);
        let (bbox, kps) = (vec![0.0; 8], vec![0.0; 20]);
        let level = Level {
            stride: 32,
            cls: &cls,
            obj: &obj,
            bbox: &bbox,
            kps: &kps,
        };
        let d = decode(&[level], 64, 32, 0.5).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].score, 1.0);
    }

    #[test]
    fn decode_rejects_outputs_that_do_not_fit_the_input() {
        let v = vec![0.0; 3];
        let level = Level {
            stride: 32,
            cls: &v,
            obj: &v,
            bbox: &v,
            kps: &v,
        };
        assert!(decode(&[level], 64, 32, 0.5).is_err());
        let level = Level {
            stride: 32,
            cls: &v,
            obj: &v,
            bbox: &v,
            kps: &v,
        };
        assert!(decode(&[level], 60, 32, 0.5).is_err(), "not a multiple of the stride");
    }

    #[test]
    fn the_plan_scales_the_long_edge_and_pads_to_32() {
        let cfg = DetectorConfig {
            long_edge: 960,
            max_scale: 1.0,
            ..DetectorConfig::default()
        };
        let p = plan(1440, 1080, &cfg);
        assert_eq!((p.scaled_w, p.scaled_h), (960, 720));
        assert_eq!((p.input_w, p.input_h), (960, 736));
        assert!((p.scale - 960.0 / 1440.0).abs() < 1e-6);
        // A small image is not enlarged beyond max_scale.
        let p = plan(300, 200, &cfg);
        assert_eq!((p.scaled_w, p.scaled_h, p.input_w, p.input_h), (300, 200, 320, 224));
        let up = DetectorConfig { max_scale: 2.0, ..cfg };
        let p = plan(300, 200, &up);
        assert_eq!((p.scaled_w, p.scaled_h), (600, 400));
    }

    #[test]
    fn detections_go_back_to_the_original_pixels_and_are_clipped() {
        let cfg = DetectorConfig {
            long_edge: 960,
            ..DetectorConfig::default()
        };
        let p = plan(1920, 1080, &cfg);
        assert_eq!((p.scaled_w, p.scaled_h), (960, 540));
        let inside = Detection {
            score: 0.9,
            bbox: [100.0, 50.0, 40.0, 40.0],
            landmarks: [[110.0, 60.0]; 5],
        };
        let hanging = Detection {
            score: 0.9,
            bbox: [940.0, 500.0, 60.0, 60.0],
            landmarks: [[950.0, 510.0]; 5],
        };
        let outside = Detection {
            score: 0.9,
            bbox: [970.0, 0.0, 20.0, 20.0],
            landmarks: [[0.0, 0.0]; 5],
        };
        let out = to_image(vec![inside, hanging, outside], &p, 1920, 1080);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].bbox, [200.0, 100.0, 80.0, 80.0]);
        assert_eq!(out[0].landmarks[0], [220.0, 120.0]);
        assert_eq!(out[1].bbox[0] + out[1].bbox[2], 1920.0);
        assert_eq!(out[1].bbox[1] + out[1].bbox[3], 1080.0);
    }

    #[test]
    fn the_input_tensor_is_bgr_channel_first_with_zero_padding() {
        // 2x1 image, pixels (R,G,B) = (10,20,30) and (40,50,60), no scaling; padded to 32x32.
        let rgb = [10u8, 20, 30, 40, 50, 60];
        let p = Plan {
            scaled_w: 2,
            scaled_h: 1,
            input_w: 32,
            input_h: 32,
            scale: 1.0,
        };
        let t = input_tensor(&rgb, 2, 1, &p).unwrap();
        let plane = 32 * 32;
        assert_eq!(t.len(), 3 * plane);
        assert_eq!([t[0], t[1], t[2]], [30.0, 60.0, 0.0]); // blue plane
        assert_eq!([t[plane], t[plane + 1]], [20.0, 50.0]); // green
        assert_eq!([t[2 * plane], t[2 * plane + 1]], [10.0, 40.0]); // red
        assert_eq!(t[32], 0.0); // second row is padding
    }
}
