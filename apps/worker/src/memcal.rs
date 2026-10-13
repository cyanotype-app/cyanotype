//! Memory calibration for the per-pixel cost constants in `media`/`raw`. Run one file per process
//! (peak RSS is per process):
//!
//!   CYANOTYPE_MEMCAL_FILE=/path/to/file cargo test -p cyanotype-worker memcal -- --nocapture
//!
//! Prints the estimate the worker would reserve next to the measured peak resident set growth while
//! decoding. Skipped unless the variable is set. See docs/build.md.
use std::sync::Arc;

use crate::{formats, media};

fn reset_peak() {
    // "5" resets VmHWM (peak RSS) for this process.
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

fn kb(field: &str) -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap();
    s.lines()
        .find(|l| l.starts_with(field))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[test]
fn memcal() {
    let Ok(path) = std::env::var("CYANOTYPE_MEMCAL_FILE") else {
        return;
    };
    let name = std::path::Path::new(&path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let bytes = Arc::new(std::fs::read(&path).unwrap());
    let kind = formats::sniff(&bytes, &name).expect("supported file");
    let limits = media::Limits {
        max_decode_bytes: u64::MAX,
        allow_raw_develop: std::env::var("CYANOTYPE_MEMCAL_NO_DEVELOP").is_err(),
    };
    reset_peak();
    let base = kb("VmRSS:");
    let t = std::time::Instant::now();
    let p = media::prepare(kind, &bytes, limits).unwrap();
    let after_prepare = kb("VmHWM:");
    let d = media::render(&p, &bytes).unwrap();
    let peak = kb("VmHWM:");
    let px = u64::from(p.width) * u64::from(p.height);
    eprintln!(
        "MEMCAL {name}: kind={kind:?} file={}MB display={}x{} ({}MP) estimate={}MB prepare_peak=+{}MB total_peak=+{}MB (decode only +{}MB) {:.1}B/px thumb={}B ({:?})",
        bytes.len() >> 20,
        p.width,
        p.height,
        px / 1_000_000,
        p.cost_bytes >> 20,
        (after_prepare - base) / 1024,
        (peak - base) / 1024,
        (peak.saturating_sub(base) / 1024).saturating_sub(bytes.len() as u64 >> 20),
        ((peak - base) * 1024) as f64 / px as f64,
        d.thumb_webp.len(),
        t.elapsed()
    );
}
