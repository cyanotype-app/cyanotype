//! Dogs and cats in photos: where they are, from an object detector (D-FINE-N, COCO's `dog` and `cat` classes) run in-process with ONNX
//! Runtime like the search and face models. No Python, no network at runtime. The worker's `pets` job runs it only on photos the search
//! model has already said hold a dog or a cat (`cyanotype_clip::zeroshot`), to put a box on each animal. The licence, the provenance and
//! the measurements that chose it are in `docs/pets.md`.
//!
//! The graph takes any `[1, 3, H, W]` float tensor of RGB values 0..1 (it is run at 640 x 640, the picture squashed to fit: the size it
//! was trained at) and returns 300 queries: 80 class logits and a centre/size box (fractions of the picture) each. Everything around the
//! network is here and unit-tested without it: the input tensor, the decoding of the outputs and the non-maximum suppression.
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, bail};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use ort::value::Tensor;

/// File name in the models directory (`models/pets.lock`).
pub const DETECTOR_FILE: &str = "dfine-n.onnx";

/// The side of the square the picture is squashed to.
pub const INPUT_SIDE: usize = 640;

/// Where the model file is: `PETS_MODEL_DIR`, else `<SEARCH_MODEL_DIR>/pets` (the Docker images install it there), else
/// `/opt/cyanotype/models/pets`.
pub fn model_dir() -> PathBuf {
    std::env::var_os("PETS_MODEL_DIR").map_or_else(
        || {
            std::env::var_os("SEARCH_MODEL_DIR")
                .map_or_else(|| PathBuf::from("/opt/cyanotype/models"), PathBuf::from)
                .join("pets")
        },
        PathBuf::from,
    )
}

/// Is the model file in `dir`?
pub fn models_installed(dir: &Path) -> bool {
    dir.join(DETECTOR_FILE).exists()
}

/// What is looked for. A kind is one line here (its COCO class) and one in `cyanotype_core::pets::KINDS`, never a new table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Dog,
    Cat,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Dog, Kind::Cat];

    /// The word stored with a detection (`pet_detections.kind`).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Dog => "dog",
            Kind::Cat => "cat",
        }
    }

    /// The model's class number (COCO, 80 classes counted from 0: person 0, bird 14, cat 15, dog 16, horse 17).
    pub fn class(self) -> usize {
        match self {
            Kind::Dog => 16,
            Kind::Cat => 15,
        }
    }
}

/// One animal found, in pixels of the image that was searched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    pub kind: Kind,
    /// The class probability, 0..1 (sigmoid of the logit: the model scores each class on its own).
    pub score: f32,
    /// Left, top, width, height.
    pub bbox: [f32; 4],
}

/// How an image is searched.
///
/// The model is a DETR: it answers with 300 guesses, and for one animal that is several boxes of different extent (the head, the body,
/// the whole), each with a modest probability (D-FINE-N's are small: a clear dog is 0.3 to 0.9, a small or half-hidden one 0.1 to 0.3).
/// What is kept per kind is the strongest box if it reaches `min_score`, and further boxes only if they are sure (`extra_score`) and not
/// the same animal again (`nms_iou`, `contained`). The measurements are in `docs/pets.md`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectorConfig {
    /// The strongest box of a kind is kept if it reaches this class probability, else the kind is not found. Low on purpose: the search
    /// model has already said an animal of the kind is in the photo (`docs/pets.md`, "The check"), so this only has to find where.
    pub min_score: f32,
    /// Another box of the same kind must reach this too (a second animal is a clear one).
    pub extra_score: f32,
    /// Two boxes of one kind overlapping more than this (intersection over union) are one animal: the weaker is dropped.
    pub nms_iou: f32,
    /// A box of which this much (of its own area) lies inside a stronger box of its kind is the same animal: the weaker is dropped.
    pub contained: f32,
    /// The most boxes kept per kind.
    pub max_per_kind: usize,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            min_score: 0.10,
            extra_score: 0.25,
            nms_iou: 0.5,
            contained: 0.85,
            max_per_kind: 6,
        }
    }
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

/// How much of the smaller of two `[x, y, w, h]` boxes lies inside the other.
pub fn overlap_of_smaller(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let iw = ((a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0])).max(0.0);
    let ih = ((a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1])).max(0.0);
    let smaller = (a[2] * a[3]).min(b[2] * b[3]);
    if smaller <= 0.0 { 0.0 } else { iw * ih / smaller }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Turn the network's outputs for one picture into detections in pixels of a `w` x `h` image: `logits` is `queries x classes`, `boxes`
/// `queries x 4` (centre x, centre y, width, height as fractions of the picture). Boxes are cut to the picture; tiny ones are dropped.
/// Per kind: below `min_score` dropped, then the strongest first; it is kept, and a weaker box is kept only if it reaches `extra_score`
/// and is not the same animal as one kept already (`nms_iou`, `contained`), at most `max_per_kind`. The result is ordered by score,
/// highest first.
pub fn decode(
    logits: &[f32],
    boxes: &[f32],
    queries: usize,
    classes: usize,
    w: u32,
    h: u32,
    cfg: &DetectorConfig,
) -> Result<Vec<Detection>> {
    if logits.len() != queries * classes || boxes.len() != queries * 4 {
        bail!(
            "expected {queries} queries of {classes} logits and 4 box values, got {} and {} values",
            logits.len(),
            boxes.len()
        );
    }
    if let Some(k) = Kind::ALL.iter().find(|k| k.class() >= classes) {
        bail!(
            "the model has {classes} classes, no class {} for {}",
            k.class(),
            k.name()
        );
    }
    let (fw, fh) = (w as f32, h as f32);
    let mut out: Vec<Detection> = Vec::new();
    for kind in Kind::ALL {
        let mut found: Vec<Detection> = (0..queries)
            .filter_map(|q| {
                let score = sigmoid(logits[q * classes + kind.class()]);
                if score < cfg.min_score {
                    return None;
                }
                let b = &boxes[q * 4..q * 4 + 4];
                let (x1, y1) = (
                    ((b[0] - b[2] / 2.0) * fw).clamp(0.0, fw),
                    ((b[1] - b[3] / 2.0) * fh).clamp(0.0, fh),
                );
                let (x2, y2) = (
                    ((b[0] + b[2] / 2.0) * fw).clamp(0.0, fw),
                    ((b[1] + b[3] / 2.0) * fh).clamp(0.0, fh),
                );
                (x2 - x1 >= 1.0 && y2 - y1 >= 1.0).then_some(Detection {
                    kind,
                    score,
                    bbox: [x1, y1, x2 - x1, y2 - y1],
                })
            })
            .collect();
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut kept: Vec<Detection> = Vec::new();
        for d in found {
            if kept.len() >= cfg.max_per_kind {
                break;
            }
            let new_animal = kept
                .iter()
                .all(|k| iou(&k.bbox, &d.bbox) <= cfg.nms_iou && overlap_of_smaller(&k.bbox, &d.bbox) <= cfg.contained);
            if new_animal && (kept.is_empty() || d.score >= cfg.extra_score) {
                kept.push(d);
            }
        }
        out.extend(kept);
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

/// The input tensor: the image squashed to [`INPUT_SIDE`] x [`INPUT_SIDE`], channel-first, values 0..1.
pub fn input_tensor(rgb: &[u8], w: u32, h: u32) -> Result<Vec<f32>> {
    let side = INPUT_SIDE as u32;
    let small: std::borrow::Cow<[u8]> = if (w, h) == (side, side) {
        std::borrow::Cow::Borrowed(rgb)
    } else {
        let src = ImageRef::new(w, h, rgb, PixelType::U8x3).context("source image")?;
        let mut dst = Image::new(side, side, PixelType::U8x3);
        // An antialiasing filter when it shrinks (a 1440 px preview, mostly), a smooth one when it grows.
        let alg = if w.max(h) > side {
            ResizeAlg::Convolution(FilterType::Bilinear)
        } else {
            ResizeAlg::Convolution(FilterType::CatmullRom)
        };
        Resizer::new()
            .resize(&src, &mut dst, &ResizeOptions::new().resize_alg(alg))
            .context("resize")?;
        std::borrow::Cow::Owned(dst.into_vec())
    };
    let plane = INPUT_SIDE * INPUT_SIDE;
    let mut out = vec![0f32; 3 * plane];
    for (i, px) in small.as_chunks::<3>().0.iter().enumerate() {
        for c in 0..3 {
            out[c * plane + i] = f32::from(px[c]) / 255.0;
        }
    }
    Ok(out)
}

pub fn session(path: &Path, threads: usize) -> Result<ort::session::Session> {
    use ort::{ep::CPU, session::Session, session::builder::GraphOptimizationLevel};
    Session::builder()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_intra_threads(threads.max(1))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        // One run at a time per session; the arena would otherwise keep the largest run's buffers forever.
        .with_inter_threads(1)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_execution_providers([CPU::default().with_arena_allocator(false).build()])
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("load {}", path.display()))
}

/// The detector. One session, one image at a time.
pub struct Detector {
    session: Mutex<ort::session::Session>,
    cfg: DetectorConfig,
}

impl Detector {
    /// Load the model from `dir` with `threads` intra-op threads.
    pub fn load(dir: &Path, threads: usize, cfg: DetectorConfig) -> Result<Self> {
        Ok(Self {
            session: Mutex::new(session(&dir.join(DETECTOR_FILE), threads)?),
            cfg,
        })
    }

    pub fn config(&self) -> &DetectorConfig {
        &self.cfg
    }

    /// The dogs and cats in an RGB8 image, strongest first, in the image's pixels.
    pub fn detect(&self, rgb: &[u8], w: u32, h: u32) -> Result<Vec<Detection>> {
        if w == 0 || h == 0 || rgb.len() != w as usize * h as usize * 3 {
            bail!("not a {w}x{h} RGB8 buffer ({} bytes)", rgb.len());
        }
        let data = input_tensor(rgb, w, h)?;
        let input = Tensor::from_array(([1usize, 3, INPUT_SIDE, INPUT_SIDE], data.into_boxed_slice()))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
        let outputs = session
            .run(ort::inputs!["pixel_values" => input])
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let (lshape, logits) = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("output logits")?;
        let (_, boxes) = outputs["pred_boxes"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("output pred_boxes")?;
        if lshape.len() != 3 {
            bail!("logits have shape {lshape:?}, expected [1, queries, classes]");
        }
        decode(logits, boxes, lshape[1] as usize, lshape[2] as usize, w, h, &self.cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `queries` queries of `classes` logits, all very unsure (-10), and boxes all zero.
    fn quiet(queries: usize, classes: usize) -> (Vec<f32>, Vec<f32>) {
        (vec![-10.0; queries * classes], vec![0.0; queries * 4])
    }

    fn set(l: &mut [f32], b: &mut [f32], classes: usize, q: usize, class: usize, logit: f32, bx: [f32; 4]) {
        l[q * classes + class] = logit;
        b[q * 4..q * 4 + 4].copy_from_slice(&bx);
    }

    #[test]
    fn the_kinds_are_the_coco_classes_of_a_dog_and_a_cat() {
        assert_eq!(
            (Kind::Dog.class(), Kind::Cat.class(), Kind::Dog.name(), Kind::Cat.name()),
            (16, 15, "dog", "cat")
        );
    }

    #[test]
    fn iou_of_identical_disjoint_and_half_overlapping_boxes() {
        let a = [0.0, 0.0, 10.0, 10.0];
        assert!((iou(&a, &a) - 1.0).abs() < 1e-6);
        assert_eq!(iou(&a, &[20.0, 20.0, 5.0, 5.0]), 0.0);
        // 10x10 and 10x10 shifted by 5: intersection 50, union 150.
        assert!((iou(&a, &[5.0, 0.0, 10.0, 10.0]) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(iou(&[0.0; 4], &[0.0; 4]), 0.0);
    }

    #[test]
    fn a_confident_dog_becomes_a_box_in_pixels() {
        let (mut l, mut b) = quiet(5, 80);
        // Query 2: a dog, centred at (0.5, 0.25), half the width and a quarter of the height of a 400 x 200 picture.
        set(&mut l, &mut b, 80, 2, 16, 2.0, [0.5, 0.25, 0.5, 0.25]);
        let out = decode(&l, &b, 5, 80, 400, 200, &DetectorConfig::default()).unwrap();
        assert_eq!(out.len(), 1);
        let d = out[0];
        assert_eq!(d.kind, Kind::Dog);
        assert!((d.score - sigmoid(2.0)).abs() < 1e-6);
        // Left = (0.5 - 0.25) * 400, top = (0.25 - 0.125) * 200, 200 x 50 pixels.
        for (got, want) in d.bbox.iter().zip([100.0, 25.0, 200.0, 50.0]) {
            assert!((got - want).abs() < 1e-3, "{:?}", d.bbox);
        }
    }

    #[test]
    fn other_classes_and_unsure_queries_are_not_animals() {
        let (mut l, mut b) = quiet(4, 80);
        set(&mut l, &mut b, 80, 0, 17, 5.0, [0.5, 0.5, 0.4, 0.4]); // a horse
        set(&mut l, &mut b, 80, 1, 0, 5.0, [0.5, 0.5, 0.4, 0.4]); // a person
        set(&mut l, &mut b, 80, 2, 15, -3.0, [0.5, 0.5, 0.4, 0.4]); // a cat at 5 %
        assert!(
            decode(&l, &b, 4, 80, 100, 100, &DetectorConfig::default())
                .unwrap()
                .is_empty()
        );
        // The floor is a setting: at 4 % that cat is found.
        let cfg = DetectorConfig {
            min_score: 0.04,
            ..DetectorConfig::default()
        };
        let out = decode(&l, &b, 4, 80, 100, 100, &cfg).unwrap();
        assert_eq!(out.iter().map(|d| d.kind).collect::<Vec<_>>(), vec![Kind::Cat]);
    }

    #[test]
    fn overlapping_boxes_of_one_kind_are_one_animal_and_a_dog_next_to_a_cat_are_two() {
        let (mut l, mut b) = quiet(4, 80);
        set(&mut l, &mut b, 80, 0, 16, 1.0, [0.3, 0.5, 0.3, 0.3]);
        set(&mut l, &mut b, 80, 1, 16, 3.0, [0.31, 0.5, 0.3, 0.3]); // the same dog, surer
        set(&mut l, &mut b, 80, 2, 15, 2.0, [0.3, 0.5, 0.3, 0.3]); // a cat in the same place (another kind)
        set(&mut l, &mut b, 80, 3, 16, 0.5, [0.8, 0.5, 0.2, 0.3]); // another dog elsewhere
        let out = decode(&l, &b, 4, 80, 1000, 1000, &DetectorConfig::default()).unwrap();
        let kinds: Vec<(Kind, usize)> = out
            .iter()
            .map(|d| (d.kind, (d.bbox[0] / 100.0).round() as usize))
            .collect();
        // (Column of each box's left edge, in hundreds of pixels.) Strongest first: the sure dog, the cat, the dog far to the right; the
        // weaker copy of the first dog is gone.
        assert_eq!(kinds, vec![(Kind::Dog, 2), (Kind::Cat, 2), (Kind::Dog, 7)]);
        assert!(out[0].score > out[1].score && out[1].score > out[2].score);
    }

    #[test]
    fn a_box_inside_a_stronger_one_is_the_same_animal_and_a_weak_second_box_is_not_a_second_animal() {
        let (mut l, mut b) = quiet(5, 80);
        set(&mut l, &mut b, 80, 0, 16, 0.0, [0.5, 0.5, 0.6, 0.6]); // the dog, 50 %
        set(&mut l, &mut b, 80, 1, 16, -1.0, [0.45, 0.4, 0.2, 0.2]); // its head (27 %), inside it, and not much alike by area
        set(&mut l, &mut b, 80, 2, 16, -1.2, [0.1, 0.1, 0.1, 0.1]); // somewhere else, at 23 %: too weak to be a second dog
        set(&mut l, &mut b, 80, 3, 16, -0.9, [0.1, 0.9, 0.1, 0.1]); // somewhere else, at 29 %: a second dog
        let out = decode(&l, &b, 5, 80, 1000, 1000, &DetectorConfig::default()).unwrap();
        let lefts: Vec<i32> = out.iter().map(|d| (d.bbox[0] / 10.0).round() as i32).collect();
        assert_eq!(lefts, vec![20, 5], "{out:?}");
        // The strongest box is kept even when it is weak, down to the floor.
        let (mut l, mut b) = quiet(2, 80);
        set(&mut l, &mut b, 80, 0, 16, -2.0, [0.5, 0.5, 0.4, 0.4]); // 12 %
        let out = decode(&l, &b, 2, 80, 100, 100, &DetectorConfig::default()).unwrap();
        assert_eq!(out.len(), 1);
        set(&mut l, &mut b, 80, 0, 16, -2.5, [0.5, 0.5, 0.4, 0.4]); // 7.6 %
        assert!(
            decode(&l, &b, 2, 80, 100, 100, &DetectorConfig::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_overlap_of_the_smaller_box_is_how_much_of_it_is_inside() {
        let big = [0.0, 0.0, 10.0, 10.0];
        assert!((overlap_of_smaller(&big, &[2.0, 2.0, 3.0, 3.0]) - 1.0).abs() < 1e-6);
        assert!(
            (overlap_of_smaller(&[2.0, 2.0, 3.0, 3.0], &big) - 1.0).abs() < 1e-6,
            "symmetric"
        );
        assert!((overlap_of_smaller(&big, &[8.0, 0.0, 4.0, 10.0]) - 0.5).abs() < 1e-6);
        assert_eq!(overlap_of_smaller(&big, &[20.0, 20.0, 1.0, 1.0]), 0.0);
        assert_eq!(overlap_of_smaller(&[0.0; 4], &big), 0.0);
    }

    #[test]
    fn boxes_are_cut_to_the_picture_and_slivers_are_dropped() {
        let (mut l, mut b) = quiet(3, 80);
        // Hangs over the left and top edges.
        set(&mut l, &mut b, 80, 0, 16, 2.0, [0.05, 0.05, 0.3, 0.3]);
        // Less than a pixel wide.
        set(&mut l, &mut b, 80, 1, 15, 2.0, [0.5, 0.5, 0.001, 0.4]);
        let out = decode(&l, &b, 3, 80, 200, 100, &DetectorConfig::default()).unwrap();
        assert_eq!(out.len(), 1);
        let [x, y, w, h] = out[0].bbox;
        assert_eq!((x, y), (0.0, 0.0));
        assert!((w - 40.0).abs() < 1e-3 && (h - 20.0).abs() < 1e-3, "{:?}", out[0].bbox);
    }

    #[test]
    fn at_most_so_many_boxes_per_kind_are_kept() {
        let queries = 20;
        let (mut l, mut b) = quiet(queries, 80);
        // Twenty well separated dogs, the later ones surer.
        for q in 0..queries {
            let x = 0.03 + 0.047 * q as f32;
            set(&mut l, &mut b, 80, q, 16, q as f32 * 0.1, [x, 0.5, 0.04, 0.2]);
        }
        let cfg = DetectorConfig {
            min_score: 0.001,
            extra_score: 0.001,
            max_per_kind: 3,
            ..DetectorConfig::default()
        };
        let out = decode(&l, &b, queries, 80, 1000, 1000, &cfg).unwrap();
        assert_eq!(out.len(), 3);
        assert!(out.windows(2).all(|w| w[0].score >= w[1].score));
        assert!((out[0].score - sigmoid(1.9)).abs() < 1e-6);
    }

    #[test]
    fn outputs_that_do_not_fit_are_refused() {
        let (l, b) = quiet(3, 80);
        assert!(decode(&l[1..], &b, 3, 80, 10, 10, &DetectorConfig::default()).is_err());
        assert!(decode(&l, &b[1..], 3, 80, 10, 10, &DetectorConfig::default()).is_err());
        // A model without a class 16 is not this model.
        let (l, b) = quiet(3, 10);
        assert!(decode(&l, &b, 3, 10, 10, 10, &DetectorConfig::default()).is_err());
    }

    #[test]
    fn the_input_is_the_picture_squashed_to_a_square_in_0_to_1() {
        // A 2 x 1 picture: left pixel red, right pixel blue.
        let rgb = [255u8, 0, 0, 0, 0, 255];
        let t = input_tensor(&rgb, 2, 1).unwrap();
        let plane = INPUT_SIDE * INPUT_SIDE;
        assert_eq!(t.len(), 3 * plane);
        // Row 0, first and last column: red on the left, blue on the right; channels are planes (R, G, B).
        let (first, last) = (0, INPUT_SIDE - 1);
        assert!(t[first] > 0.99 && t[plane + first] < 0.01 && t[2 * plane + first] < 0.01);
        assert!(t[last] < 0.01 && t[2 * plane + last] > 0.99);
        assert!(t.iter().all(|v| (0.0..=1.0).contains(v)));
        // A picture of the input's own size is not resampled.
        let full = vec![128u8; plane * 3];
        let t = input_tensor(&full, INPUT_SIDE as u32, INPUT_SIDE as u32).unwrap();
        assert!(t.iter().all(|v| (v - 128.0 / 255.0).abs() < 1e-6));
        assert!(input_tensor(&rgb[..5], 2, 1).is_err());
    }

    #[test]
    fn the_model_directory_follows_the_environment() {
        // Only the pure part: the default sits under the models root.
        let d = model_dir();
        assert!(d.ends_with("pets") || std::env::var_os("PETS_MODEL_DIR").is_some());
        assert!(!models_installed(Path::new("/nonexistent/dir")));
    }
}
