//! The text half of semantic search: turning a query into a vector. The image half runs in the worker (`embed` jobs);
//! both use the same model from `cyanotype-clip`, so the two vectors live in one space and cosine distance compares them.
//!
//! The tower is loaded on the first query (a deployment that never searches never pays for it) and then stays
//! resident: ~70 MB of RSS (`docs/build.md`), and a query is ~10 ms of CPU with it loaded against a second or more to
//! load. Recent queries are cached inside the encoder (`cyanotype_clip::TextEncoder`).
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use cyanotype_clip::{TextEncoder, model_dir};

/// Query text to a unit vector. Blocking (model inference): call from `spawn_blocking`.
pub trait TextEmbedder: Send + Sync + 'static {
    fn embed(&self, query: &str) -> Result<Arc<Vec<f32>>>;
}

/// The search model is not installed on this deployment (see `scripts/fetch-models.sh`).
#[derive(Debug, thiserror::Error)]
#[error("search model files not found in {0}")]
pub struct ModelMissing(pub PathBuf);

pub struct ClipText {
    threads: usize,
    encoder: Mutex<Option<Arc<TextEncoder>>>,
}

impl ClipText {
    /// `threads`: intra-op threads of the text session (`SEARCH_THREADS`, default 2: a query is latency, not throughput).
    pub fn new(threads: usize) -> Self {
        Self {
            threads,
            encoder: Mutex::new(None),
        }
    }

    fn encoder(&self) -> Result<Arc<TextEncoder>> {
        // Held across the load so concurrent first queries load once.
        let mut guard = self.encoder.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(e) = guard.as_ref() {
            return Ok(e.clone());
        }
        let dir = model_dir();
        if !dir.join(cyanotype_clip::TEXT_FILE).exists() || !dir.join(cyanotype_clip::TOKENIZER_FILE).exists() {
            return Err(ModelMissing(dir).into());
        }
        let started = std::time::Instant::now();
        let enc = Arc::new(TextEncoder::load(&dir, self.threads)?);
        tracing::info!(
            model = cyanotype_clip::MODEL_ID,
            threads = self.threads,
            load_ms = started.elapsed().as_millis() as u64,
            "text model loaded"
        );
        *guard = Some(enc.clone());
        Ok(enc)
    }
}

impl TextEmbedder for ClipText {
    fn embed(&self, query: &str) -> Result<Arc<Vec<f32>>> {
        self.encoder()?.embed(query)
    }
}
