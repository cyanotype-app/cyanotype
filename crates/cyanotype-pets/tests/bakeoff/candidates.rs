//! The detectors of the bake-off, each decoded the way its publisher's reference code does, so a candidate is measured as shipped:
//! YOLOX-s and NanoDet-Plus (the OpenCV Zoo's `yolox.py` and `nanodet.py`), D-FINE-N and RT-DETR-R18 (Hugging Face
//! `transformers`' post-processing). All return COCO class ids (cat 15, dog 16) with a score and a box in the pixels of the image given.
use std::{path::Path, sync::Mutex};

use anyhow::Result;
use cyanotype_pets::{iou, session};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use ort::value::Tensor;

/// Resize an RGB8 image (`src_w` x `src_h`) to `dst_w` x `dst_h` with an antialiasing filter.
pub fn resize_rgb(rgb: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32, filter: FilterType) -> Result<Vec<u8>> {
    if (src_w, src_h) == (dst_w, dst_h) {
        return Ok(rgb.to_vec());
    }
    let src = ImageRef::new(src_w, src_h, rgb, PixelType::U8x3)?;
    let mut dst = Image::new(dst_w, dst_h, PixelType::U8x3);
    Resizer::new().resize(
        &src,
        &mut dst,
        &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)),
    )?;
    Ok(dst.into_vec())
}

/// COCO class ids (80-class numbering) of the animals.
pub const CAT: usize = 15;
pub const DOG: usize = 16;

#[derive(Clone, Copy, Debug)]
pub struct Raw {
    pub class: usize,
    pub score: f32,
    /// Left, top, width, height.
    pub bbox: [f32; 4],
}

pub trait Candidate {
    fn run(&self, rgb: &[u8], w: u32, h: u32) -> Result<Vec<Raw>>;
}

/// Class-wise greedy non-maximum suppression.
pub fn nms(mut dets: Vec<Raw>, max_iou: f32) -> Vec<Raw> {
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Raw> = Vec::new();
    for d in dets {
        if kept
            .iter()
            .all(|k| k.class != d.class || iou(&k.bbox, &d.bbox) <= max_iou)
        {
            kept.push(d);
        }
    }
    kept
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

pub struct Yolox {
    session: Mutex<ort::session::Session>,
    /// Score floor below which candidates are not even suppressed (keeps the work small).
    pub floor: f32,
}

impl Yolox {
    pub fn load(file: &Path, threads: usize) -> Result<Self> {
        Ok(Self {
            session: Mutex::new(session(file, threads)?),
            floor: 0.05,
        })
    }
}

impl Candidate for Yolox {
    fn run(&self, rgb: &[u8], w: u32, h: u32) -> Result<Vec<Raw>> {
        const S: usize = 640;
        // Letterbox: scale to fit, top-left, padded with 114; RGB floats 0..255 (the zoo's demo converts BGR to RGB).
        let ratio = (S as f32 / h as f32).min(S as f32 / w as f32);
        let (nw, nh) = (((w as f32 * ratio) as u32).max(1), ((h as f32 * ratio) as u32).max(1));
        let small = resize_rgb(rgb, w, h, nw, nh, FilterType::Bilinear)?;
        let mut data = vec![114f32; 3 * S * S];
        for y in 0..nh as usize {
            for x in 0..nw as usize {
                for c in 0..3 {
                    data[c * S * S + y * S + x] = f32::from(small[(y * nw as usize + x) * 3 + c]);
                }
            }
        }
        let input = Tensor::from_array(([1usize, 3, S, S], data.into_boxed_slice()))?;
        let mut s = self.session.lock().unwrap();
        let out = s.run(ort::inputs!["images" => input])?;
        let (shape, v) = out["output"].try_extract_tensor::<f32>()?;
        assert_eq!(shape[1], 8400);
        let n_cls = shape[2] as usize - 5;
        let mut dets = Vec::new();
        let mut row = 0usize;
        for stride in [8usize, 16, 32] {
            let g = S / stride;
            for gy in 0..g {
                for gx in 0..g {
                    let r = &v[row * (n_cls + 5)..(row + 1) * (n_cls + 5)];
                    row += 1;
                    let obj = r[4];
                    if obj < self.floor {
                        continue;
                    }
                    let (mut best, mut bi) = (0f32, 0usize);
                    for (i, p) in r[5..].iter().enumerate() {
                        if *p > best {
                            (best, bi) = (*p, i);
                        }
                    }
                    // Keep the animals' own scores whatever the best class is, so a cat taken for a dog is seen as both.
                    for class in [CAT, DOG, bi] {
                        let score = obj * r[5 + class];
                        if score < self.floor {
                            continue;
                        }
                        let cx = (r[0] + gx as f32) * stride as f32;
                        let cy = (r[1] + gy as f32) * stride as f32;
                        let bw = r[2].exp() * stride as f32;
                        let bh = r[3].exp() * stride as f32;
                        dets.push(Raw {
                            class,
                            score,
                            bbox: [(cx - bw / 2.0) / ratio, (cy - bh / 2.0) / ratio, bw / ratio, bh / ratio],
                        });
                    }
                }
            }
        }
        dets.dedup_by(|a, b| a.class == b.class && a.bbox == b.bbox);
        Ok(nms(dets, 0.5))
    }
}

pub struct NanoDet {
    session: Mutex<ort::session::Session>,
}

impl NanoDet {
    pub fn load(file: &Path, threads: usize) -> Result<Self> {
        Ok(Self {
            session: Mutex::new(session(file, threads)?),
        })
    }
}

impl Candidate for NanoDet {
    fn run(&self, rgb: &[u8], w: u32, h: u32) -> Result<Vec<Raw>> {
        const S: usize = 416;
        // Letterbox as the zoo's demo does: scale the long side to 416, centre, pad with zeros (before normalising).
        let (nw, nh, left, top) = if w == h {
            (S as u32, S as u32, 0, 0)
        } else if h > w {
            let nw = (S as f32 / (h as f32 / w as f32)) as u32;
            (nw, S as u32, ((S as u32 - nw) as f32 * 0.5) as u32, 0)
        } else {
            let nh = (S as f32 * (h as f32 / w as f32)) as u32;
            (S as u32, nh, 0, ((S as u32 - nh) as f32 * 0.5) as u32)
        };
        let small = resize_rgb(rgb, w, h, nw.max(1), nh.max(1), FilterType::Box)?;
        let mean = [103.53f32, 116.28, 123.675];
        let std = [57.375f32, 57.12, 58.395];
        let mut data = vec![0f32; 3 * S * S];
        for c in 0..3 {
            // The padding is black before normalising, like the demo (the mean and std are applied to the padded picture).
            let pad = (0.0 - mean[c]) / std[c];
            data[c * S * S..(c + 1) * S * S].fill(pad);
        }
        for y in 0..nh as usize {
            for x in 0..nw as usize {
                for c in 0..3 {
                    let v = f32::from(small[(y * nw as usize + x) * 3 + c]);
                    data[c * S * S + (y + top as usize) * S + x + left as usize] = (v - mean[c]) / std[c];
                }
            }
        }
        let input = Tensor::from_array(([1usize, 3, S, S], data.into_boxed_slice()))?;
        let mut s = self.session.lock().unwrap();
        let out = s.run(ort::inputs!["input.1" => input])?;
        let mut dets = Vec::new();
        for (stride, cls_name, reg_name) in [(8usize, "792", "795"), (16, "814", "817"), (32, "836", "839")] {
            let (_, cls) = out[cls_name].try_extract_tensor::<f32>()?;
            let (_, reg) = out[reg_name].try_extract_tensor::<f32>()?;
            let g = S / stride;
            for i in 0..g * g {
                let (gy, gx) = (i / g, i % g);
                let scores = &cls[i * 80..(i + 1) * 80];
                let wanted = [CAT, DOG];
                let top_other = scores.iter().copied().fold(0f32, f32::max);
                if wanted.iter().all(|&c| scores[c] < 0.05) && top_other < 0.05 {
                    continue;
                }
                // Distribution focal loss: softmax over 8 bins per side, expectation, times the stride.
                let mut d = [0f32; 4];
                for (side, dv) in d.iter_mut().enumerate() {
                    let bins = &reg[i * 32 + side * 8..i * 32 + side * 8 + 8];
                    let m = bins.iter().copied().fold(f32::MIN, f32::max);
                    let e: Vec<f32> = bins.iter().map(|b| (b - m).exp()).collect();
                    let sum: f32 = e.iter().sum();
                    *dv = e.iter().enumerate().map(|(k, x)| k as f32 * x / sum).sum::<f32>() * stride as f32;
                }
                let cx = (gx * stride) as f32 + 0.5 * (stride as f32 - 1.0);
                let cy = (gy * stride) as f32 + 0.5 * (stride as f32 - 1.0);
                let (x1, y1) = ((cx - d[0]).clamp(0.0, S as f32), (cy - d[1]).clamp(0.0, S as f32));
                let (x2, y2) = ((cx + d[2]).clamp(0.0, S as f32), (cy + d[3]).clamp(0.0, S as f32));
                // Back to the picture: remove the padding, undo the scale.
                let (sx, sy) = (w as f32 / nw as f32, h as f32 / nh as f32);
                let bbox = [
                    (x1 - left as f32) * sx,
                    (y1 - top as f32) * sy,
                    (x2 - x1) * sx,
                    (y2 - y1) * sy,
                ];
                for class in wanted {
                    if scores[class] >= 0.05 {
                        dets.push(Raw {
                            class,
                            score: scores[class],
                            bbox,
                        });
                    }
                }
            }
        }
        Ok(nms(dets, 0.6))
    }
}

/// D-FINE and RT-DETR (Hugging Face exports): the picture squashed to 640 x 640, values 0..1, no normalisation; 300 queries of
/// 80 logits (sigmoid) and a centre/size box as fractions of the picture.
pub struct Detr {
    session: Mutex<ort::session::Session>,
}

impl Detr {
    pub fn load(file: &Path, threads: usize) -> Result<Self> {
        Ok(Self {
            session: Mutex::new(session(file, threads)?),
        })
    }
}

impl Candidate for Detr {
    fn run(&self, rgb: &[u8], w: u32, h: u32) -> Result<Vec<Raw>> {
        const S: usize = 640;
        let small = resize_rgb(rgb, w, h, S as u32, S as u32, FilterType::Bilinear)?;
        let mut data = vec![0f32; 3 * S * S];
        for i in 0..S * S {
            for c in 0..3 {
                data[c * S * S + i] = f32::from(small[i * 3 + c]) / 255.0;
            }
        }
        let input = Tensor::from_array(([1usize, 3, S, S], data.into_boxed_slice()))?;
        let mut s = self.session.lock().unwrap();
        let out = s.run(ort::inputs!["pixel_values" => input])?;
        let (_, logits) = out["logits"].try_extract_tensor::<f32>()?;
        let (_, boxes) = out["pred_boxes"].try_extract_tensor::<f32>()?;
        let mut dets = Vec::new();
        for q in 0..300 {
            let b = &boxes[q * 4..q * 4 + 4];
            let bbox = [
                (b[0] - b[2] / 2.0) * w as f32,
                (b[1] - b[3] / 2.0) * h as f32,
                b[2] * w as f32,
                b[3] * h as f32,
            ];
            for class in [CAT, DOG] {
                let score = sigmoid(logits[q * 80 + class]);
                if score >= 0.05 {
                    dets.push(Raw { class, score, bbox });
                }
            }
        }
        Ok(nms(dets, 0.7))
    }
}
