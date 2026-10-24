//! Time and memory of the face models (docs/faces.md): CPU time per photo and resident memory, alone and next to the SigLIP 2 image
//! tower the worker also keeps loaded.
//!
//!   FACES_MODEL_DIR=models/faces SEARCH_MODEL_DIR=models \
//!     cargo run --release -p cyanotype-faces --example bench -- [--threads N] [--long-edge PX] [--detect-edge PX] [--clip] [--rounds N] photo.jpg...
//!
//! Every photo is scaled to `--long-edge` pixels (default 1440, the size of a stored preview) and put through the whole job:
//! detection, alignment, fingerprints. `--clip` loads the search model's image tower first and runs a few embeddings, so the
//! resident figures are those of a worker with both loaded. Prints one line per round and the memory at each step.
use std::time::Instant;

use anyhow::{Context, Result};
use cyanotype_faces::{Engine, EngineConfig};

fn status(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .unwrap_or(0)
        / 1024
}

fn mem(label: &str) {
    println!(
        "{label:<44} resident {:>5} MB   peak {:>5} MB",
        status("VmRSS:"),
        status("VmHWM:")
    );
}

fn main() -> Result<()> {
    let mut threads = 1usize;
    let mut long_edge = 1440u32;
    let mut rounds = 3usize;
    let mut clip = false;
    let mut detect_edge = EngineConfig::default().detector.long_edge;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--threads" => threads = args.next().context("--threads N")?.parse()?,
            "--long-edge" => long_edge = args.next().context("--long-edge PX")?.parse()?,
            "--rounds" => rounds = args.next().context("--rounds N")?.parse()?,
            "--detect-edge" => detect_edge = args.next().context("--detect-edge PX")?.parse()?,
            "--clip" => clip = true,
            _ => {
                // A directory stands for the JPEGs in it.
                if std::path::Path::new(&a).is_dir() {
                    let mut found: Vec<String> = std::fs::read_dir(&a)?
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("jpg")))
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect();
                    found.sort();
                    files.extend(found);
                } else {
                    files.push(a);
                }
            }
        }
    }
    anyhow::ensure!(!files.is_empty(), "give some photos");
    mem("start");

    let clip_encoder = if clip {
        let model = cyanotype_clip::Model::from_env()?;
        let enc = cyanotype_clip::ImageEncoder::load(model, &model.dir(), threads)?;
        mem(&format!("{} image tower loaded", model.name()));
        let px = vec![128u8; 640 * 480 * 3];
        let t = cyanotype_clip::preprocess_rgb8(model, 640, 480, &px)?;
        for _ in 0..3 {
            enc.embed(std::slice::from_ref(&t))?;
        }
        mem("... after three embeddings");
        Some(enc)
    } else {
        None
    };

    // Photos are decoded one at a time, just before they are analysed, so the figures are not inflated by a pile of pixels.
    let load = |f: &String| -> Result<(Vec<u8>, u32, u32)> {
        let img = image::open(f).with_context(|| f.clone())?;
        let (w, h) = (img.width(), img.height());
        let s = long_edge as f32 / w.max(h) as f32;
        let img = img.resize_exact(
            ((w as f32 * s).round() as u32).max(1),
            ((h as f32 * s).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        );
        let rgb = img.to_rgb8();
        let (w, h) = rgb.dimensions();
        Ok((rgb.into_raw(), w, h))
    };
    println!("{} photos, each scaled to {long_edge} px on the long edge", files.len());
    mem("before the face models");

    let t = Instant::now();
    let engine = Engine::load(
        &cyanotype_faces::model_dir(),
        EngineConfig {
            threads,
            detector: cyanotype_faces::DetectorConfig {
                long_edge: detect_edge,
                ..Default::default()
            },
            ..EngineConfig::default()
        },
    )?;
    println!(
        "face models loaded in {} ms ({threads} thread(s), detector input {detect_edge} px)",
        t.elapsed().as_millis()
    );
    mem("face models loaded");

    let mut per_photo = Vec::new();
    let mut total_faces = 0usize;
    for round in 0..rounds {
        let t = Instant::now();
        let mut faces = 0usize;
        for f in &files {
            let (rgb, w, h) = load(f)?;
            let s = Instant::now();
            let found = engine.analyse(&rgb, w, h)?;
            faces += found.iter().filter(|f| f.embedding.is_some()).count();
            if round > 0 || rounds == 1 {
                per_photo.push((s.elapsed().as_secs_f64() * 1000.0, found.len()));
            }
        }
        total_faces = faces;
        println!(
            "round {round}: {} photos, {faces} fingerprinted faces, {:.0} ms per photo (including the decode)",
            files.len(),
            t.elapsed().as_secs_f64() * 1000.0 / files.len() as f64
        );
    }
    mem("after the rounds");
    per_photo.sort_by(|a, b| a.0.total_cmp(&b.0));
    if !per_photo.is_empty() {
        let q = |p: f64| per_photo[((per_photo.len() - 1) as f64 * p) as usize].0;
        println!(
            "per photo (excluding the first, cold round when there are several): median {:.0} ms, p95 {:.0} ms, slowest {:.0} ms; {total_faces} fingerprinted faces in the last round",
            q(0.5),
            q(0.95),
            per_photo.last().unwrap().0
        );
    }
    // The per-stage split, on the photo with the most faces.
    let mut busiest: Option<(usize, String)> = None;
    for f in &files {
        let (rgb, w, h) = load(f)?;
        let n = engine.detector().detect(&rgb, w, h)?.len();
        if busiest.as_ref().is_none_or(|b| n > b.0) {
            busiest = Some((n, f.clone()));
        }
    }
    if let Some((_, f)) = busiest {
        let (rgb, w, h) = load(&f)?;
        let t = Instant::now();
        let dets = engine.detector().detect(&rgb, w, h)?;
        let detect_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let mut n = 0;
        for d in &dets {
            if d.min_side() >= engine.config().min_face_px && engine.embed_face(&rgb, w, h, d)?.is_some() {
                n += 1;
            }
        }
        println!(
            "split on the busiest photo: detection {detect_ms:.0} ms ({} faces), alignment + fingerprints {:.0} ms for {n} ({:.1} ms each)",
            dets.len(),
            t.elapsed().as_secs_f64() * 1000.0,
            t.elapsed().as_secs_f64() * 1000.0 / n.max(1) as f64
        );
    }
    drop(clip_encoder);
    Ok(())
}
