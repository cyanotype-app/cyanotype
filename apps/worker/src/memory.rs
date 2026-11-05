//! Giving freed memory back to the operating system after a model is unloaded.
//!
//! Dropping an ONNX Runtime session frees its weights and activation buffers, but glibc's allocator keeps much of what was freed: the
//! threads of a session allocate from arenas, and a hole in the middle of a heap or an arena cannot be handed back. Without a trim the
//! worker stayed at the resident size of its busiest hour until a restart (about 160 MB after a pet fingerprint backfill, from 6 MB).
//! `malloc_trim(0)` walks every arena and returns the free pages to the kernel. The runtime image is Debian (glibc, the system allocator;
//! the worker sets no global allocator), so this is the right call; on other targets it is a no-op.

/// Resident set size of this process in bytes, `None` where `/proc/self/statm` is not there.
pub fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim() {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    // SAFETY: glibc's `malloc_trim` takes no pointers and is safe to call from any thread at any time.
    unsafe {
        malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim() {}

/// Return freed memory to the OS and log the resident size before and after. Call it after dropping a model, from every unload path.
pub fn release_freed(what: &str) {
    let before = rss_bytes();
    trim();
    let after = rss_bytes();
    if let (Some(b), Some(a)) = (before, after) {
        tracing::info!(
            model = what,
            rss_before_mb = b >> 20,
            rss_after_mb = a >> 20,
            "returned freed memory to the OS"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn mb() -> u64 {
        rss_bytes().unwrap_or(0) >> 20
    }

    /// What unloading the pet fingerprint model leaves behind: resident size before the model is loaded, after 50 embeddings, after
    /// dropping it, and after `release_freed`. Run alone, the numbers are the process's:
    /// `PETS_MODEL_DIR=<dir with dinov2-small-int8.onnx> cargo test -p cyanotype-worker memory::tests::unload_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement; needs the fingerprint model (scripts/fetch-models.sh)"]
    fn unload_cost() {
        let file = cyanotype_pets::model_dir().join(cyanotype_pets::DINO_FILE);
        let before_load = mb();
        let enc = cyanotype_pets::dino::DinoEncoder::load(&file, 1).expect("fingerprint model");
        let loaded = mb();
        let started = Instant::now();
        let n = 3 * cyanotype_pets::dino::SIDE * cyanotype_pets::dino::SIDE;
        for i in 0..50usize {
            let px: Vec<f32> = (0..n).map(|j| ((j ^ i) % 255) as f32 / 127.5 - 1.0).collect();
            enc.embed(&px, &[cyanotype_pets::dino::Pooling::Cls]).unwrap();
        }
        let worked = mb();
        let ms = started.elapsed().as_millis();
        drop(enc);
        let dropped = mb();
        release_freed("test");
        let trimmed = mb();
        eprintln!(
            "rss MB: before load {before_load}, loaded {loaded}, after 50 embeddings {worked} ({ms} ms), after drop {dropped}, after trim {trimmed}"
        );
    }
}
