//! Time and memory of the pet detector on real photos: `PETS_MODEL_DIR=models/pets cargo run --release -p cyanotype-pets --example bench -- a.jpg b.jpg ...`
//! (the photos are scaled to at most 1440 px on the long edge first, like the worker's previews). With `siglip` as the first argument the
//! search model's image tower is loaded and run on each photo too, as the worker has it in memory next to the detector (`SEARCH_MODEL_DIR`).
//! Prints the resident memory (`/proc/self/status`) at each step and the time per photo. A tool for measuring, not part of the product.
use std::time::Instant;

use anyhow::{Context, Result};
use cyanotype_pets::{Detector, DetectorConfig};

fn status(field: &str) -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(field))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse::<f64>().ok()))
        })
        .unwrap_or(0.0)
        / 1024.0
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let with_siglip = args.first().is_some_and(|a| a == "siglip");
    if with_siglip {
        args.remove(0);
    }
    let threads: usize = std::env::var("PETS_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    println!("resident at start: {:.0} MB", status("VmRSS:"));
    let enc = if with_siglip {
        let m = cyanotype_clip::Model::Siglip2B16;
        let e = cyanotype_clip::ImageEncoder::load(m, &m.dir(), threads)?;
        println!(
            "resident with the search image tower loaded: {:.0} MB",
            status("VmRSS:")
        );
        Some(e)
    } else {
        None
    };
    let started = Instant::now();
    let detector = Detector::load(&cyanotype_pets::model_dir(), threads, DetectorConfig::default())?;
    println!(
        "detector loaded in {:.0} ms, resident {:.0} MB",
        started.elapsed().as_secs_f64() * 1000.0,
        status("VmRSS:")
    );
    let mut ms = Vec::new();
    // The most the detection itself added to the process's resident memory over the photos (its input, activations, boxes).
    let mut extra = 0f64;
    for path in &args {
        let img = image::open(path).with_context(|| path.clone())?;
        let img = if img.width().max(img.height()) > 1440 {
            img.resize(1440, 1440, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let rgb = img.to_rgb8();
        let (w, h) = rgb.dimensions();
        if let Some(e) = &enc {
            let pre = cyanotype_clip::preprocess_rgb8(cyanotype_clip::Model::Siglip2B16, w, h, rgb.as_raw())?;
            e.embed(&[pre])?;
        }
        // The peak of the detection alone: reset the high-water mark (Linux: writing 5 to clear_refs), then read it back.
        let _ = std::fs::write("/proc/self/clear_refs", "5");
        let before = status("VmRSS:");
        let t = Instant::now();
        let found = detector.detect(rgb.as_raw(), w, h)?;
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
        extra = extra.max(status("VmHWM:") - before);
        println!("{path}: {w}x{h}, {} found, {:.0} ms", found.len(), ms.last().unwrap());
        if std::env::var_os("PETS_VERBOSE").is_some() {
            for d in &found {
                let b = d.bbox;
                println!(
                    "    {} {:.2} at {:.2},{:.2} size {:.2} x {:.2} (fractions)",
                    d.kind.name(),
                    d.score,
                    b[0] / w as f32,
                    b[1] / h as f32,
                    b[2] / w as f32,
                    b[3] / h as f32
                );
            }
        }
    }
    ms.sort_by(f64::total_cmp);
    if !ms.is_empty() {
        println!(
            "per photo: median {:.0} ms, p95 {:.0} ms, max {:.0} ms",
            ms[ms.len() / 2],
            ms[((ms.len() as f64 * 0.95) as usize).saturating_sub(1)],
            ms[ms.len() - 1]
        );
    }
    println!(
        "resident after the photos: {:.0} MB; a detection added at most {extra:.0} MB while it ran",
        status("VmRSS:")
    );
    Ok(())
}
