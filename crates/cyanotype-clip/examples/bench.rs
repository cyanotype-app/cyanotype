//! Memory and speed of the two towers on this machine:
//!
//! ```text
//! SEARCH_MODEL_DIR=... cargo run --release -p cyanotype-clip --example bench -- <model> <threads> <image|text>
//! ```
//!
//! `<model>` is `clip-b32` or `siglip2-b16`. One tower per process, so the resident memory printed is that tower's.
//! `BENCH_FILE=/path/to/other.onnx` benchmarks another quantisation of the same export.
use std::{path::PathBuf, time::Instant};

use cyanotype_clip::{ImageEncoder, Model, TextEncoder, preprocess_rgb8};

fn rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let model = Model::from_name(&args.next().unwrap_or_default()).expect("model: clip-b32 or siglip2-b16");
    let threads: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(1);
    let tower = args.next().unwrap_or_else(|| "image".into());
    let dir = model.dir();
    let file = std::env::var_os("BENCH_FILE").map(PathBuf::from);
    println!(
        "{} {tower} tower, {threads} thread(s); baseline rss {:.0} MB",
        model.name(),
        rss_mb()
    );
    if tower == "image" {
        let images: Vec<_> = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/images"))?
            .filter_map(|e| {
                let p = e.ok()?.path();
                (p.extension()? == "jpg").then(|| std::fs::read(p).ok()).flatten()
            })
            .collect();
        let t = Instant::now();
        let pre: Vec<Vec<f32>> = images
            .iter()
            .map(|b| {
                let im = image::load_from_memory(b).unwrap().to_rgb8();
                preprocess_rgb8(model, im.width(), im.height(), im.as_raw()).unwrap()
            })
            .collect();
        println!(
            "preprocess {:.1} ms/image (decode included)",
            t.elapsed().as_secs_f64() * 1000.0 / pre.len() as f64
        );
        let enc = match &file {
            Some(f) => ImageEncoder::load_file(model, f, threads)?,
            None => ImageEncoder::load(model, &dir, threads)?,
        };
        println!("image tower loaded: rss {:.0} MB", rss_mb());
        enc.embed(&pre[..1])?;
        println!("after the first image: rss {:.0} MB", rss_mb());
        for batch in [1usize, 4] {
            let t = Instant::now();
            let mut n = 0;
            for _ in 0..2 {
                for c in pre.chunks(batch) {
                    enc.embed(c)?;
                    n += c.len();
                }
            }
            let per = t.elapsed().as_secs_f64() / n as f64;
            println!(
                "image batch {batch}: {:.0} ms/image = {:.0} images/min, rss {:.0} MB",
                per * 1000.0,
                60.0 / per,
                rss_mb()
            );
        }
    } else {
        let text = match &file {
            Some(f) => TextEncoder::load_file(model, &dir, f, threads)?,
            None => TextEncoder::load(model, &dir, threads)?,
        };
        println!("text tower loaded: rss {:.0} MB", rss_mb());
        let queries = [
            "dog on the beach",
            "red car at night",
            "birthday cake",
            "a photo of two people hiking in the mountains",
            "x",
        ];
        let mut lat = Vec::new();
        for i in 0..60 {
            let q = format!("{} {}", queries[i % queries.len()], i);
            let t = Instant::now();
            text.embed(&q)?;
            lat.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "text: p50 {:.1} ms p95 {:.1} ms, rss {:.0} MB",
            lat[30],
            lat[56],
            rss_mb()
        );
    }
    Ok(())
}
