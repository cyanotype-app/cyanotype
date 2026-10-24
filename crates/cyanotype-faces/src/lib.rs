//! Faces in photos: where they are (YuNet), and a fingerprint of each (SFace), both ONNX models run in-process with ONNX
//! Runtime, like the search models in `cyanotype-clip`. No Python, no network at runtime.
//!
//! A photo goes through [`Engine::analyse`]: the detector finds faces and five landmarks each, every face worth a fingerprint is
//! aligned to the 112 x 112 ArcFace template ([`align`]) and the recognizer turns the crop into 128 numbers. Faces the
//! recognizer would only guess at (smaller than [`EngineConfig::min_face_px`], or turned too far) are reported without one.
//! The licences, the models' provenance and the measurements that chose them are in `docs/faces.md`.
pub mod align;
mod sface;
mod yunet;

use std::path::{Path, PathBuf};

use anyhow::Result;

pub use sface::{DIM, Recognizer, preprocess};
pub use yunet::{Detection, Detector, DetectorConfig, Level, Plan, decode, iou, nms, plan, to_image};

/// File names in the models directory (`models/faces.lock`).
pub const DETECTOR_FILE: &str = "yunet.onnx";
pub const RECOGNIZER_FILE: &str = "sface.onnx";

/// Where the model files are: `FACES_MODEL_DIR`, else `<SEARCH_MODEL_DIR>/faces` (the Docker images install them there), else
/// `/opt/cyanotype/models/faces`.
pub fn model_dir() -> PathBuf {
    std::env::var_os("FACES_MODEL_DIR").map_or_else(
        || {
            std::env::var_os("SEARCH_MODEL_DIR")
                .map_or_else(|| PathBuf::from("/opt/cyanotype/models"), PathBuf::from)
                .join("faces")
        },
        PathBuf::from,
    )
}

/// Are both model files in `dir`?
pub fn models_installed(dir: &Path) -> bool {
    dir.join(DETECTOR_FILE).exists() && dir.join(RECOGNIZER_FILE).exists()
}

/// Scale to unit length, so cosine similarity is a dot product.
pub fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

/// Cosine similarity of two vectors (not assumed normalised).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb).max(f32::MIN_POSITIVE)
}

pub(crate) fn session(path: &Path, threads: usize) -> Result<ort::session::Session> {
    use anyhow::Context;
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

/// What the engine does beyond the detector's own settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineConfig {
    pub detector: DetectorConfig,
    /// A face whose box is shorter than this (pixels of the searched image) is reported but not fingerprinted: at that size
    /// the crop is mostly interpolation and the fingerprint would join strangers.
    pub min_face_px: f32,
    /// Intra-op threads of both sessions.
    pub threads: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            detector: DetectorConfig::default(),
            min_face_px: 32.0,
            threads: 1,
        }
    }
}

/// One face found in a photo.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub detection: Detection,
    /// Unit-length, [`DIM`] values; `None` when the face was too small to fingerprint.
    pub embedding: Option<Vec<f32>>,
}

/// Detector and recognizer together.
pub struct Engine {
    detector: Detector,
    recognizer: Recognizer,
    cfg: EngineConfig,
}

impl Engine {
    /// Load both models from `dir`.
    pub fn load(dir: &Path, cfg: EngineConfig) -> Result<Self> {
        Ok(Self {
            detector: Detector::load(&dir.join(DETECTOR_FILE), cfg.threads, cfg.detector)?,
            recognizer: Recognizer::load(&dir.join(RECOGNIZER_FILE), cfg.threads)?,
            cfg,
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.cfg
    }

    pub fn detector(&self) -> &Detector {
        &self.detector
    }

    pub fn recognizer(&self) -> &Recognizer {
        &self.recognizer
    }

    /// The fingerprint of one detected face of `rgb` (`w` x `h`), or `None` when its landmarks cannot be aligned.
    pub fn embed_face(&self, rgb: &[u8], w: u32, h: u32, d: &Detection) -> Result<Option<Vec<f32>>> {
        match align::align(rgb, w, h, &d.landmarks) {
            Some(crop) => Ok(Some(self.recognizer.embed(&crop)?)),
            None => Ok(None),
        }
    }

    /// Find the faces of an RGB8 image and fingerprint the ones big enough, strongest first.
    pub fn analyse(&self, rgb: &[u8], w: u32, h: u32) -> Result<Vec<Face>> {
        self.detector
            .detect(rgb, w, h)?
            .into_iter()
            .map(|detection| {
                let embedding = if detection.min_side() >= self.cfg.min_face_px {
                    self.embed_face(rgb, w, h, &detection)?
                } else {
                    None
                };
                Ok(Face { detection, embedding })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_and_normalisation() {
        let mut v = vec![3.0, 4.0];
        l2_normalize(&mut v);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
        assert!((cosine(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
        assert!((cosine(&[2.0, 2.0], &[1.0, 1.0]) - 1.0).abs() < 1e-6);
        let mut z = vec![0.0; 3];
        l2_normalize(&mut z);
        assert_eq!(z, vec![0.0; 3]);
    }

    #[test]
    fn the_model_directory_follows_the_environment() {
        // Only the pure part: the default sits under the models root.
        let d = model_dir();
        assert!(d.ends_with("faces") || std::env::var_os("FACES_MODEL_DIR").is_some());
        assert!(!models_installed(Path::new("/nonexistent/dir")));
    }
}
