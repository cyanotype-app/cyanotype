//! Text in photos: PP-OCR text detection (DB) and recognition (CTC) on ONNX Runtime, and QR codes.
//!
//! One [`Engine`] holds the two sessions (loaded lazily by the worker and dropped when idle, like the image tower of the
//! search model). [`Engine::read`] takes an RGB8 image and returns the lines of text it found, in reading order, each with
//! its text, a confidence, a quadrilateral and its words with theirs, all in *normalised* coordinates (0..1 of the image
//! width and height) so they are valid for the preview, the original and any size the viewer draws them at.
//!
//! Pipeline (the PaddleOCR reference, without OpenCV):
//! 1. detection: the image is scaled so its long side is at most `det_side` pixels and both sides are multiples of 32,
//!    normalised, and the network returns a probability map; pixels over `thresh` are grouped into components, each is
//!    fitted with its smallest rotated rectangle, kept when its mean probability is at least `box_thresh`, and grown
//!    by `unclip` (the network draws text tighter than it is);
//! 2. recognition: every rectangle is cut out (rotated upright, 48 px tall) and read in batches; CTC decoding gives the text,
//!    and the time steps of the characters place its words along the line;
//! 3. lines under `min_score` are dropped.
//!
//! There is no angle classifier: text upside down is not read.
pub mod dict;
pub mod geom;
pub mod qr;

mod det;
mod rec;

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, bail};

pub use det::DetParams;

/// The files of one OCR model: a detection graph, a recognition graph and the recognition dictionary.
#[derive(Debug, Clone)]
pub struct Files {
    pub det: PathBuf,
    pub rec: PathBuf,
    /// `inference.yml` of the recognition model (the dictionary is inside), or a plain `.txt` dictionary.
    pub dict: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub det: DetParams,
    /// Height recognition crops are scaled to (48 for PP-OCRv5 and v6).
    pub rec_height: u32,
    /// The widest crop (a longer line is squeezed to fit).
    pub rec_max_width: u32,
    pub rec_batch: usize,
    /// A batch holds crops whose widths add up to at most this (a batch of long lines is small, of short ones larger): the
    /// memory of a recognition run grows with it.
    pub rec_batch_px: usize,
    /// Lines whose confidence is lower are dropped.
    pub min_score: f32,
    pub threads: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            det: DetParams::default(),
            rec_height: 48,
            rec_max_width: 3200,
            rec_batch: 8,
            rec_batch_px: 1600,
            min_score: 0.5,
            threads: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    /// Top-left, top-right, bottom-right, bottom-left, normalised.
    pub quad: [[f32; 2]; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    /// Mean probability of the characters read, 0..1.
    pub score: f32,
    pub quad: [[f32; 2]; 4],
    pub words: Vec<Word>,
}

pub struct Engine {
    cfg: Config,
    det: Mutex<ort::session::Session>,
    rec: Mutex<ort::session::Session>,
    dict: Vec<String>,
    /// Output classes of the recognition graph: the blank, the dictionary and maybe a trailing space.
    classes: usize,
}

pub(crate) fn session(path: &Path, threads: usize) -> Result<ort::session::Session> {
    use ort::{ep::CPU, session::Session, session::builder::GraphOptimizationLevel};
    Session::builder()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_intra_threads(threads.max(1))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_inter_threads(1)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        // Shapes change from image to image: the arena would keep the largest one's buffers for the process's life.
        .with_execution_providers([CPU::default().with_arena_allocator(false).build()])
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("load {}", path.display()))
}

impl Engine {
    pub fn load(files: &Files, cfg: Config) -> Result<Self> {
        let dict = dict::load(&files.dict)?;
        let det = session(&files.det, cfg.threads)?;
        let rec = session(&files.rec, cfg.threads)?;
        let mut engine = Self {
            cfg,
            det: Mutex::new(det),
            rec: Mutex::new(rec),
            dict,
            classes: 0,
        };
        // A blank image tells the recognition graph's class count, so a dictionary that does not fit it fails now, not on a
        // customer's photo.
        engine.classes = engine.probe_classes()?;
        let n = engine.dict.len();
        if engine.classes != n + 1 && engine.classes != n + 2 {
            bail!(
                "the recognition model has {} classes but the dictionary has {n} characters",
                engine.classes
            );
        }
        Ok(engine)
    }

    fn probe_classes(&self) -> Result<usize> {
        let h = self.cfg.rec_height as usize;
        Ok(rec::run_batch(&self.rec, &[vec![0f32; 3 * h * 64]], h, 64)?.c)
    }

    /// Read the text of an RGB8 image.
    pub fn read(&self, rgb: &[u8], width: u32, height: u32) -> Result<Vec<Line>> {
        if width == 0 || height == 0 || rgb.len() != width as usize * height as usize * 3 {
            bail!("not a {width}x{height} RGB8 buffer ({} bytes)", rgb.len());
        }
        let rects = det::detect(&self.det, &self.cfg.det, rgb, width, height)?;
        let mut lines = rec::read_all(
            &self.rec,
            &self.cfg,
            &self.dict,
            self.classes,
            rgb,
            (width, height),
            &rects,
        )?;
        lines.retain(|l| l.score >= self.cfg.min_score && !l.text.trim().is_empty());
        Ok(order(lines))
    }
}

/// Reading order: top to bottom, and left to right among lines that share a row.
fn order(mut lines: Vec<Line>) -> Vec<Line> {
    let cy = |l: &Line| (l.quad[0][1] + l.quad[1][1] + l.quad[2][1] + l.quad[3][1]) / 4.0;
    let cx = |l: &Line| (l.quad[0][0] + l.quad[1][0] + l.quad[2][0] + l.quad[3][0]) / 4.0;
    let hgt = |l: &Line| ((l.quad[3][1] - l.quad[0][1]).abs() + (l.quad[2][1] - l.quad[1][1]).abs()) / 2.0;
    lines.sort_by(|a, b| cy(a).partial_cmp(&cy(b)).unwrap_or(std::cmp::Ordering::Equal));
    let mut rows: Vec<Vec<Line>> = Vec::new();
    for l in lines {
        match rows.last_mut() {
            Some(row) if (cy(&l) - cy(&row[0])).abs() < 0.5 * hgt(&l).min(hgt(&row[0])) => row.push(l),
            _ => rows.push(vec![l]),
        }
    }
    rows.into_iter()
        .flat_map(|mut r| {
            r.sort_by(|a, b| cx(a).partial_cmp(&cx(b)).unwrap_or(std::cmp::Ordering::Equal));
            r
        })
        .collect()
}

/// The text of `lines`, one line per row, for the viewer and for search.
pub fn plain_text(lines: &[Line]) -> String {
    lines.iter().map(|l| l.text.trim()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f32, y: f32) -> Line {
        let (w, h) = (0.2, 0.04);
        Line {
            text: text.into(),
            score: 0.9,
            quad: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            words: vec![],
        }
    }

    #[test]
    fn lines_come_out_top_to_bottom_and_left_to_right_within_a_row() {
        let out = order(vec![
            line("third", 0.1, 0.5),
            line("right", 0.6, 0.101),
            line("left", 0.1, 0.1),
            line("second", 0.1, 0.3),
        ]);
        let t: Vec<&str> = out.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(t, vec!["left", "right", "second", "third"]);
        assert_eq!(plain_text(&out), "left\nright\nsecond\nthird");
    }
}
