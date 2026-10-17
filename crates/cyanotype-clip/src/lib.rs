//! CLIP semantic search: an image tower (the worker embeds each photo's preview) and a text tower (the assets service
//! embeds each query), both ONNX models run in-process with ONNX Runtime. No Python, no network at runtime.
//!
//! The two towers live in one crate so they cannot drift apart: the same model id and version tag every stored
//! vector, the same normalisation produces both sides of a cosine comparison, and the parity tests exercise both.
//!
//! Model: OpenAI CLIP ViT-B/32 (MIT licence, code and weights), as the 8-bit dynamically quantised ONNX export
//! published by Xenova/transformers.js. See `docs/build.md` for the licence, sizes and measured quality loss.
mod image_tower;
mod text_tower;

use std::path::PathBuf;

pub use image_tower::{IMAGE_SIZE, ImageEncoder, preprocess_rgb8};
pub use text_tower::{CONTEXT_LENGTH, TextEncoder};

/// Stored with every vector. Changing the model (or anything that changes the vectors, such as the preprocessing)
/// means a new id or a higher [`MODEL_VERSION`]: the worker then re-embeds every photo and search only reads
/// vectors that match both.
pub const MODEL_ID: &str = "clip-vit-b32-openai-int8";
pub const MODEL_VERSION: i32 = 1;
/// Embedding dimensions (fixed by the `halfvec(512)` column and its index).
pub const DIM: usize = 512;

pub const VISION_FILE: &str = "vision_model_int8.onnx";
pub const TEXT_FILE: &str = "text_model_int8.onnx";
pub const TOKENIZER_FILE: &str = "tokenizer.json";

/// Where the model files are: `CLIP_MODEL_DIR`, else the directory the Docker images install them in.
pub fn model_dir() -> PathBuf {
    std::env::var_os("CLIP_MODEL_DIR").map_or_else(|| PathBuf::from("/opt/cyanotype/models/clip"), PathBuf::from)
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
