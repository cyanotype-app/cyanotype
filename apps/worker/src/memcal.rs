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

/// Writes a synthetic JPEG or HEIC grid for the calibration runs above:
///
///   CYANOTYPE_MEMGEN=/tmp/x.jpg:jpeg:W:H:{base|prog} cargo test -p cyanotype-worker memgen
///   CYANOTYPE_MEMGEN=/tmp/x.png:png:W:H cargo test -p cyanotype-worker memgen   (8-bit RGB PNG)
///   CYANOTYPE_MEMGEN=/tmp/x.png:png16:W:H cargo test -p cyanotype-worker memgen   (16-bit source for `heif-enc -b 10`)
///   CYANOTYPE_MEMGEN=/tmp/x.heic:grid:W:H[:irot] cargo test -p cyanotype-worker memgen   (512x512 tiles, 1 tile repeated)
#[test]
fn memgen() {
    let Ok(spec) = std::env::var("CYANOTYPE_MEMGEN") else {
        return;
    };
    let p: Vec<&str> = spec.split(':').collect();
    let (w, h): (u32, u32) = (p[2].parse().unwrap(), p[3].parse().unwrap());
    let bytes = match p[1] {
        "jpeg" => {
            let mut s = crate::testgen::JpegSpec::new(w as u16, h as u16);
            s.progressive = p.get(4) == Some(&"prog");
            crate::testgen::jpeg(s)
        }
        "png" => {
            let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb(crate::testgen::pixel(x, y, w, h)));
            let mut out = Vec::new();
            image::DynamicImage::ImageRgb8(img)
                .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        }
        "png16" => {
            let img = image::ImageBuffer::from_fn(w, h, |x, y| {
                let [r, g, b] = crate::testgen::pixel(x, y, w, h);
                image::Rgb([u16::from(r) * 257, u16::from(g) * 257, u16::from(b) * 257])
            });
            let mut out = Vec::new();
            image::DynamicImage::ImageRgb16(img)
                .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        }
        "grid" => crate::testgen::grid_heic_of_size(w, h, p.get(4).is_some()),
        other => panic!("unknown kind {other}"),
    };
    std::fs::write(p[0], bytes).unwrap();
}

// ---- peak memory regression tests ----------------------------------------------------------------------------------
//
// Peak resident memory is per process, so each case runs in a child process (this test binary, running `peak_child`),
// which builds a large synthetic input, resets its high-water mark, runs the same `prepare` + `render` the worker
// runs, and prints the growth of VmHWM. The parent asserts the growth is small, and that the memory estimate the budget
// reserves really covers it.

use std::process::Command;

struct Peak {
    /// Growth of the child's peak resident set while decoding, in MiB.
    mb: u64,
    /// What the worker would have reserved for the decode, in MiB.
    estimate_mb: u64,
    secs: f64,
    path: String,
}

fn peak_of(case: &str) -> Peak {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["memcal::peak_child", "--exact", "--nocapture", "--test-threads=1"])
        .env("CYANOTYPE_PEAK_CASE", case)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "child failed for {case}:\n{text}");
    let line = text
        .lines()
        .find(|l| l.starts_with("PEAK "))
        .unwrap_or_else(|| panic!("no PEAK line for {case}:\n{text}"));
    let field = |k: &str| {
        line.split_whitespace()
            .find_map(|f| f.strip_prefix(k))
            .unwrap_or_else(|| panic!("no {k} in {line}"))
            .to_owned()
    };
    let p = Peak {
        mb: field("mb=").parse().unwrap(),
        estimate_mb: field("estimate_mb=").parse().unwrap(),
        secs: field("secs=").parse().unwrap(),
        path: field("path="),
    };
    eprintln!(
        "PEAKMEM {case}: +{} MB (estimate {} MB) via {} in {:.2}s",
        p.mb, p.estimate_mb, p.path, p.secs
    );
    p
}

#[test]
fn peak_child() {
    let Ok(case) = std::env::var("CYANOTYPE_PEAK_CASE") else {
        return;
    };
    let (name, bytes): (&str, Vec<u8>) = match case.as_str() {
        "jpeg50" => ("a.jpg", crate::testgen::jpeg(crate::testgen::JpegSpec::new(8000, 6250))),
        "jpeg100" => (
            "a.jpg",
            crate::testgen::jpeg(crate::testgen::JpegSpec::new(12500, 8000)),
        ),
        "jpeg200" => (
            "a.jpg",
            crate::testgen::jpeg(crate::testgen::JpegSpec::new(16000, 12500)),
        ),
        "jpeg100_progressive" => {
            let mut s = crate::testgen::JpegSpec::new(12500, 8000);
            s.progressive = true;
            ("a.jpg", crate::testgen::jpeg(s))
        }
        "heic_grid48" => ("a.heic", crate::testgen::grid_heic_of_size(8064, 6048, false)),
        "heic_grid48_rotated" => ("a.heic", crate::testgen::grid_heic_of_size(8064, 6048, true)),
        "heic_grid200" => ("a.heic", crate::testgen::grid_heic_of_size(16000, 12500, false)),
        other => panic!("unknown case {other}"),
    };
    let bytes = Arc::new(bytes);
    let kind = formats::sniff(&bytes, name).expect("supported");
    let limits = media::Limits {
        max_decode_bytes: u64::MAX,
        allow_raw_develop: false,
    };
    reset_peak();
    let base = kb("VmRSS:");
    let t = std::time::Instant::now();
    let p = media::prepare(kind, &bytes, limits).unwrap();
    let d = media::render(&p, &bytes).unwrap();
    let peak = kb("VmHWM:");
    assert!(!d.thumb_webp.is_empty() && !d.preview_webp.is_empty());
    eprintln!(
        "PEAK mb={} estimate_mb={} secs={:.2} path={}",
        peak.saturating_sub(base) / 1024,
        p.cost_bytes >> 20,
        t.elapsed().as_secs_f64(),
        p.path()
    );
}

/// Before DCT-scaling a 50 MP baseline JPEG peaked at about 178 MB; now it is the scaled planes and little else.
#[test]
fn peak_memory_50mp_jpeg() {
    let p = peak_of("jpeg50");
    assert_eq!(p.path, "jpeg_scaled");
    assert!(p.mb < 64, "50 MP JPEG used +{} MB", p.mb);
    assert!(
        p.mb <= p.estimate_mb,
        "estimate {} MB is below the measured +{} MB",
        p.estimate_mb,
        p.mb
    );
}

/// An iPhone-sized grid: about 285 MB when decoded whole, now one output buffer and a tile.
#[test]
fn peak_memory_48mp_grid_heic() {
    {
        let case = "heic_grid48";
        let p = peak_of(case);
        assert_eq!(p.path, "heif_tiled");
        assert!(p.mb < 100, "{case} used +{} MB", p.mb);
        assert!(
            p.mb <= p.estimate_mb,
            "{case}: estimate {} MB is below the measured +{} MB",
            p.estimate_mb,
            p.mb
        );
    }
}

/// The big cases take a while to build and decode; run with `--ignored`.
#[test]
#[ignore = "slow: builds 100-200 MP inputs"]
fn peak_memory_large_inputs() {
    for (case, limit_mb) in [
        ("jpeg100", 64),
        ("jpeg200", 64),
        ("jpeg100_progressive", 400), // every coefficient of the full image is held
        ("heic_grid200", 100),
        ("heic_grid48_rotated", 100),
    ] {
        let p = peak_of(case);
        assert!(p.mb < limit_mb, "{case} used +{} MB (limit {limit_mb})", p.mb);
        assert!(
            p.mb <= p.estimate_mb,
            "{case}: estimate {} MB is below the measured +{} MB",
            p.estimate_mb,
            p.mb
        );
    }
}
