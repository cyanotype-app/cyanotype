//! Memory and speed of the two towers on this machine: `CLIP_MODEL_DIR=... cargo run --release -p cyanotype-clip --example bench -- [threads]`.
use std::time::Instant;

use cyanotype_clip::{ImageEncoder, TextEncoder, model_dir, preprocess_rgb8};

fn rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with("VmRSS:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn main() -> anyhow::Result<()> {
    let threads: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1);
    let dir = model_dir();
    let images: Vec<_> = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/images"))?
        .filter_map(|e| std::fs::read(e.ok()?.path()).ok())
        .collect();
    println!("baseline rss {:.0} MB, {threads} thread(s)", rss_mb());
    let t = Instant::now();
    let pre: Vec<Vec<f32>> = images
        .iter()
        .map(|b| {
            let im = image::load_from_memory(b).unwrap().to_rgb8();
            preprocess_rgb8(im.width(), im.height(), im.as_raw()).unwrap()
        })
        .collect();
    println!(
        "preprocess {:.1} ms/image",
        t.elapsed().as_secs_f64() * 1000.0 / pre.len() as f64
    );
    let enc = ImageEncoder::load(&dir, threads)?;
    println!("image tower loaded: rss {:.0} MB", rss_mb());
    for batch in [1usize, 4] {
        let t = Instant::now();
        let mut n = 0;
        for _ in 0..3 {
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
    let text = TextEncoder::load(&dir, threads.max(2))?;
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
    Ok(())
}
