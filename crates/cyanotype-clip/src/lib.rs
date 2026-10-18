//! Semantic search models: an image tower (the worker embeds each photo's preview) and a text tower (the assets service
//! embeds each query), both ONNX models run in-process with ONNX Runtime. No Python, no network at runtime.
//!
//! The two towers live in one crate so they cannot drift apart: the same model id and version tag every stored
//! vector, the same normalisation produces both sides of a cosine comparison, and the parity tests exercise both.
//!
//! Two models are known ([`Model`]); `SEARCH_MODEL` picks the one the worker embeds with and the assets service prefers
//! for queries. Both are published ONNX exports pinned by sha256 in `models/*.lock`; see `docs/build.md`
//! ("Semantic search model") for the licences, the bake-off between them and the measured sizes.
mod burst;
mod cutoff;
mod image_tower;
pub mod split;
mod text_tower;

use std::path::PathBuf;

pub use burst::BurstRules;
pub use cutoff::{ImageRules, MIN_BEFORE_GAP, Rules, Tiers, image_tiers, image_tiers_with, tiers};
pub use image_tower::{ImageEncoder, preprocess_rgb8};
pub use text_tower::TextEncoder;

/// The models search can run. Everything that differs between them (files, input size, preprocessing, tokenisation,
/// embedding size, calibration) hangs off this type, so a third model is one more variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Model {
    /// OpenAI CLIP ViT-B/32, 8-bit ONNX export (MIT). 512 dimensions.
    ClipB32,
    /// Google SigLIP 2 base, patch 16 at 224 px, 8-bit ONNX export (Apache 2.0). 768 dimensions.
    Siglip2B16,
}

/// What the model's own sigmoid head says about a cosine similarity: `sigmoid(scale * cos + bias)` is the probability
/// that image and text match (SigLIP is trained with that loss; CLIP's softmax logits have no such reading).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Calibration {
    pub scale: f32,
    pub bias: f32,
}

impl Calibration {
    /// Match probability for a cosine similarity.
    pub fn probability(self, cosine: f32) -> f32 {
        1.0 / (1.0 + (-(self.scale * cosine + self.bias)).exp())
    }

    /// The cosine similarity at which the probability is `p`.
    pub fn cosine_at(self, p: f32) -> f32 {
        ((p / (1.0 - p)).ln() - self.bias) / self.scale
    }
}

/// The default when `SEARCH_MODEL` is not set.
pub const DEFAULT_MODEL: Model = Model::Siglip2B16;

impl Model {
    pub const ALL: [Model; 2] = [Model::ClipB32, Model::Siglip2B16];

    /// The `SEARCH_MODEL` value.
    pub fn name(self) -> &'static str {
        match self {
            Model::ClipB32 => "clip-b32",
            Model::Siglip2B16 => "siglip2-b16",
        }
    }

    pub fn from_name(s: &str) -> Option<Model> {
        Self::ALL
            .into_iter()
            .find(|m| m.name() == s.trim().to_ascii_lowercase())
    }

    /// `SEARCH_MODEL`, else [`DEFAULT_MODEL`]. An unknown name is an error, not a silent fallback: a typo would
    /// otherwise start a re-index of the whole library with a different model.
    pub fn from_env() -> anyhow::Result<Model> {
        match std::env::var("SEARCH_MODEL") {
            Ok(v) if !v.trim().is_empty() => Model::from_name(&v).ok_or_else(|| {
                anyhow::anyhow!(
                    "SEARCH_MODEL={v:?} is not a known model (one of: {})",
                    Model::ALL.map(Model::name).join(", ")
                )
            }),
            _ => Ok(DEFAULT_MODEL),
        }
    }

    /// Stored with every vector (`asset_embeddings.model`). Changing the model (or anything that changes the vectors,
    /// such as the preprocessing) means a new id or a higher [`Model::version`]: the worker then re-embeds every photo
    /// and search only reads vectors that match both.
    pub fn id(self) -> &'static str {
        match self {
            Model::ClipB32 => "clip-vit-b32-openai-int8",
            Model::Siglip2B16 => "siglip2-base-patch16-224-q4",
        }
    }

    /// The pipeline version of the vectors (preprocessing, quantisation) under [`Model::id`]. It is also the `embed`
    /// job's version, and the jobs table holds one job per (kind, photo, version), so versions are unique across
    /// models: a model's next pipeline change takes the next number after the highest of all.
    pub fn version(self) -> i32 {
        match self {
            Model::ClipB32 => 1,
            Model::Siglip2B16 => 2,
        }
    }

    /// Name of this model's HNSW index in the database (migration 0013): `asset_embeddings_<key>_hnsw`.
    pub fn index_name(self) -> &'static str {
        match self {
            Model::ClipB32 => "asset_embeddings_clip_b32_hnsw",
            Model::Siglip2B16 => "asset_embeddings_siglip2_b16_hnsw",
        }
    }

    /// Embedding dimensions (fixed by the model, and by its `halfvec(N)` index in the database, migration 0013).
    pub fn dim(self) -> usize {
        match self {
            Model::ClipB32 => 512,
            Model::Siglip2B16 => 768,
        }
    }

    /// Input side of the image tower in pixels.
    pub fn image_size(self) -> usize {
        224
    }

    /// Tokens the text tower takes: CLIP up to 77 (unpadded, it pools at the end-of-text token), SigLIP 2 exactly 64
    /// (padded, it pools the last position).
    pub fn context_length(self) -> usize {
        match self {
            Model::ClipB32 => 77,
            Model::Siglip2B16 => 64,
        }
    }

    /// Present for models trained with a sigmoid loss.
    pub fn calibration(self) -> Option<Calibration> {
        match self {
            Model::ClipB32 => None,
            // `logit_scale` (4.7245, exp = 112.669) and `logit_bias` (-16.7717) of google/siglip2-base-patch16-224
            // (model.safetensors, revision 75de2d55).
            Model::Siglip2B16 => Some(Calibration {
                scale: 112.668_9,
                bias: -16.771_723,
            }),
        }
    }

    /// What is actually embedded for a user's `query`.
    ///
    /// SigLIP 2 was trained on captions, and a bare keyword ("airplane") scores far lower against its own photo than
    /// "a photo of a airplane" does, which matters because the absolute cut-off (`cutoff.rs`) reads those scores: on the 209
    /// photo evaluation set the best photo of an answerable query had a probability of at least 0.9 % with the bare query
    /// and a negative query (a giraffe, nothing in the set) up to 0.8 %, against at least 6.7 % and at most 1.7 % with
    /// the template: the two kinds of query are four times apart instead of barely apart. Ranking is the same within
    /// noise (mAP 0.887 against 0.893). The template is added to short queries only (up to 4 words) that do not already read
    /// like a caption ("a photo of ...", "a picture of ..."), and CLIP is left alone (averaging templates measured no better).
    pub fn prepare_query(self, query: &str) -> String {
        let q = query.split_whitespace().collect::<Vec<_>>().join(" ");
        if self != Model::Siglip2B16 {
            return q;
        }
        let lower = q.to_lowercase();
        let words = lower.split(' ').count();
        let captioned = [
            "a photo",
            "a picture",
            "an image",
            "photo of",
            "picture of",
            "image of",
            "this is",
        ]
        .iter()
        .any(|p| lower.starts_with(p));
        if q.is_empty() || words > 4 || captioned {
            return q;
        }
        let article = if ["a ", "an ", "the "].iter().any(|a| lower.starts_with(a)) {
            ""
        } else {
            "a "
        };
        format!("a photo of {article}{q}")
    }

    pub fn vision_file(self) -> &'static str {
        match self {
            Model::ClipB32 => "vision_model_int8.onnx",
            // 4-bit weights (MatMulNBits), fp32 activations: the 8-bit export loses too much (cosine 0.76 to fp32).
            Model::Siglip2B16 => "vision_model_q4.onnx",
        }
    }

    pub fn text_file(self) -> &'static str {
        "text_model_int8.onnx"
    }

    pub fn tokenizer_file(self) -> &'static str {
        "tokenizer.json"
    }

    /// Sub-directory of the models directory that holds this model's files (and the name of `models/<dir>.lock`).
    pub fn dir_name(self) -> &'static str {
        match self {
            Model::ClipB32 => "clip",
            Model::Siglip2B16 => "siglip2",
        }
    }

    /// Where this model's files are: `<SEARCH_MODEL_DIR>/<dir_name>`, with `SEARCH_MODEL_DIR` defaulting to the
    /// directory the Docker images install them in.
    pub fn dir(self) -> PathBuf {
        models_root().join(self.dir_name())
    }
}

/// The directory holding one sub-directory per model.
pub fn models_root() -> PathBuf {
    std::env::var_os("SEARCH_MODEL_DIR").map_or_else(|| PathBuf::from("/opt/cyanotype/models"), PathBuf::from)
}

/// Scale to unit length, so cosine similarity is a dot product and the cosine index sees unit vectors.
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

pub(crate) fn session(path: &std::path::Path, threads: usize) -> anyhow::Result<ort::session::Session> {
    use anyhow::Context;
    use ort::{ep::CPU, session::Session, session::builder::GraphOptimizationLevel};
    Session::builder()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_intra_threads(threads.max(1))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        // One run at a time per session; the arena would otherwise keep the largest batch's buffers forever.
        .with_inter_threads(1)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_execution_providers([CPU::default().with_arena_allocator(false).build()])
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("load {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_siglip_queries_get_the_caption_template_and_others_do_not() {
        let m = Model::Siglip2B16;
        assert_eq!(m.prepare_query("airplane"), "a photo of a airplane");
        assert_eq!(m.prepare_query("  city   at night "), "a photo of a city at night");
        assert_eq!(m.prepare_query("the beach"), "a photo of the beach");
        assert_eq!(m.prepare_query("perro en la playa"), "a photo of a perro en la playa");
        // Long queries and ones that already read like a caption are left alone.
        assert_eq!(
            m.prepare_query("a small white fluffy dog sitting on a couch"),
            "a small white fluffy dog sitting on a couch"
        );
        assert_eq!(m.prepare_query("a photo of my dog"), "a photo of my dog");
        assert_eq!(m.prepare_query("This is a dog"), "This is a dog");
        assert_eq!(m.prepare_query("one two three four five"), "one two three four five");
        assert_eq!(m.prepare_query(""), "");
        // CLIP: only whitespace is normalised.
        assert_eq!(Model::ClipB32.prepare_query("  dog "), "dog");
    }

    #[test]
    fn model_names_round_trip_and_versions_are_unique() {
        for m in Model::ALL {
            assert_eq!(Model::from_name(m.name()), Some(m));
        }
        assert_eq!(Model::from_name(" SigLIP2-B16 "), Some(Model::Siglip2B16));
        assert_eq!(Model::from_name("clip"), None);
        let mut versions: Vec<i32> = Model::ALL.iter().map(|m| m.version()).collect();
        versions.sort_unstable();
        versions.dedup();
        assert_eq!(
            versions.len(),
            Model::ALL.len(),
            "job versions must not collide across models"
        );
        let ids: std::collections::HashSet<_> = Model::ALL.iter().map(|m| m.id()).collect();
        assert_eq!(ids.len(), Model::ALL.len());
    }

    #[test]
    fn calibration_inverts() {
        let c = Model::Siglip2B16.calibration().unwrap();
        for p in [0.001f32, 0.005, 0.5, 0.9] {
            assert!((c.probability(c.cosine_at(p)) - p).abs() < p * 1e-3 + 1e-6);
        }
        assert!(Model::ClipB32.calibration().is_none());
    }
}
