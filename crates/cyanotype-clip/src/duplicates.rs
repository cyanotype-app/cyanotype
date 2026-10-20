//! When two photos are the same picture twice (`docs/library.md`, "Near-duplicates"): thresholds on the cosine between their image
//! embeddings, per model, for three kinds of evidence about the pair (`cyanotype_core::duplicates`).
use crate::Model;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DuplicateRules {
    /// The two photos were taken with the same camera within a minute of each other (so two independent facts agree).
    pub with_metadata: f32,
    /// The two photos have the same pixel dimensions (a re-encode at the same size).
    pub same_size: f32,
    /// Neither photo has a capture time (a copy that lost its EXIF): the cosine has to carry the decision alone.
    pub stripped: f32,
}

impl Model {
    /// Calibrated with `duplicate_bakeoff` in `tests/bakeoff.rs` on copies of the 209-photo evaluation set (resized to a half and a
    /// quarter, recompressed at JPEG quality 70 and 40, half size at 60, a 3 % border cropped: 1,254 pairs) against the 21,736 pairs of
    /// different photos (936 of them in the same category: other dogs, other beaches). Numbers in `docs/build.md`, "Near-duplicate
    /// threshold". For SigLIP 2 (the 4-bit image tower is noisier than a copy deserves): copies have a median cosine of 0.96 (a quarter
    /// size 0.89), different photos a median of 0.52 and a 99.9th percentile of 0.93. Cosine alone finds 95 % of the copies at 0.85
    /// (92 % precision against that pool), 81 % at 0.92 (97 %) and 71 % at 0.94 (98 %); each gate rides on a fact that also has to hold
    /// (same camera and minute, same size, no capture time on either), which removes most of the different pairs.
    pub fn duplicate_rules(self) -> DuplicateRules {
        match self {
            Model::ClipB32 => DuplicateRules {
                with_metadata: 0.84,
                same_size: 0.91,
                stripped: 0.93,
            },
            Model::Siglip2B16 => DuplicateRules {
                with_metadata: 0.85,
                same_size: 0.92,
                stripped: 0.94,
            },
        }
    }
}
