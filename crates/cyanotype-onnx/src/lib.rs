//! The ONNX Runtime environment and session settings shared by the model crates (search, faces, text, pets).
//!
//! `ort` allows one environment per process, and only once: after the first is built, building another fails even if the
//! first was dropped. The worker links every model crate, so the environment lives here, is built on first use and is kept
//! for the life of the process (it is small; the models' memory belongs to their sessions, which are dropped on unload).
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use ort::{environment::Environment, ep::CPU, session::Session, session::builder::GraphOptimizationLevel};

static ENV: OnceLock<std::result::Result<Environment, String>> = OnceLock::new();

/// The process's ONNX Runtime environment, built the first time a model loads.
pub fn environment() -> Result<&'static Environment> {
    ENV.get_or_init(|| ort::init().with_name("cyanotype").build().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| anyhow::anyhow!("ONNX Runtime environment: {e}"))
}

/// A CPU session for the model at `path`, with `threads` threads for one run.
pub fn session(path: &Path, threads: usize) -> Result<Session> {
    Session::builder(environment()?)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_intra_threads(threads.max(1))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        // One run at a time per session.
        .with_inter_threads(1)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        // Shapes change from run to run (batch sizes, image sizes): the arena would keep the largest run's buffers for the
        // life of the session.
        .with_execution_providers([CPU::default().with_arena_allocator(false).build()])
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("load {}", path.display()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_environment_is_built_once_and_shared() {
        let a = super::environment().unwrap() as *const _;
        let b = super::environment().unwrap() as *const _;
        assert_eq!(a, b);
    }
}
