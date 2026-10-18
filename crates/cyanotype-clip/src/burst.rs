//! When two photos taken moments apart are the same shot (`docs/library.md`, "Bursts"): a threshold on the cosine between
//! their image embeddings, per model.
use crate::Model;

/// A burst is a chain of consecutive photos of one camera, at most [`MAX_GAP_SECS`] apart in time, each visually
/// near-identical to the one before it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BurstRules {
    /// Two consecutive photos are the same shot when their image embeddings have at least this cosine similarity.
    pub min_cosine: f32,
}

impl Model {
    /// Calibrated with `burst_bakeoff` in `tests/bakeoff.rs` (`just bakeoff`) on simulated bursts of the 209-photo
    /// evaluation set (a camera move of 2, 5 and 10 % of the frame and an exposure drift between consecutive frames)
    /// against all 21,736 pairs of different photos; numbers in `docs/library.md`, "Bursts". The cosine of two different
    /// photos has a median of 0.48 (CLIP) / 0.52 (SigLIP 2) and a 99.9th percentile of 0.92 / 0.93, consecutive burst frames
    /// a median of 0.95 to 0.98, a 1st percentile of 0.81 to 0.92 even for a 10 % move. The thresholds keep at least 99 % of
    /// the frames of a 5 % move and merge at most 0.5 % of arbitrary pairs of different photos: of pairs that already
    /// share a camera and are at most two seconds apart, so the real risk is far lower than that share.
    pub fn burst_rules(self) -> BurstRules {
        match self {
            Model::ClipB32 => BurstRules { min_cosine: 0.87 },
            Model::Siglip2B16 => BurstRules { min_cosine: 0.91 },
        }
    }
}
