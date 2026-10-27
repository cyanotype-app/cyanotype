//! The comparison behind `docs/pets.md`, "Telling individuals apart": which embedding of an animal's crop finds *the same animal* in other
//! photos, in the setting the app uses (the owner tags `k` photos of a pet, the app proposes more).
//!
//! The set is `tests/eval/individuals.tsv`: Public domain and CC0 photos from Wikimedia Commons of individual dogs and cats, each labelled
//! with which animal it is (anonymous ids) and a look-alike group (animals of one breed or colour, who are each other's hard impostors),
//! checked by eye. The shipped detector (D-FINE-N) boxes the animal, the crop is embedded by each candidate, and then, for every animal,
//! many random draws of `k` seed photos are made and every other photo of the same kind in the set is scored against them:
//!
//!  * precision and recall of what would be proposed at a threshold, per scoring rule (the centre of the seeds, the best seed, the mean of
//!    the three best seeds),
//!  * how often a look-alike (another animal of the same group) would be proposed, and how often any stranger would be,
//!  * the threshold-free average precision of the ranking, to compare the candidates.
//!
//! ```text
//! scripts/fetch-pets-id-eval-set.sh                    # the photos -> models/pets-id-eval
//! scripts/fetch-models.sh                              # SigLIP 2 -> models/siglip2, D-FINE-N -> models/pets
//! # DINOv2-small: models/pets-embed/dinov2-small-model.onnx (+ _int8, _q4), see models/pets-embed.lock
//! PETS_ID_DIR=models/pets-id-eval PETS_EMBED_DIR=models/pets-embed SEARCH_MODEL_DIR=$PWD/models \
//!   cargo test --release -p cyanotype-pets --test individuals -- --ignored --nocapture
//! ```
//!
//! Embeddings are cached in `PETS_ID_CACHE` (default `<PETS_ID_DIR>/embeddings.json`) so the scoring can be explored without running the
//! models again; delete it to start over. `PETS_THREADS` (default 2) sets the intra-op threads. Asserts nothing about the numbers.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    time::Instant,
};

use cyanotype_clip::{ImageEncoder, Model, preprocess_rgb8};
use cyanotype_pets::{
    Detector, DetectorConfig,
    crop::{self, Rgb},
    dino::{self, DinoEncoder, Pooling},
};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Photo {
    file: String,
    identity: String,
    kind: String,
    group: String,
    size: (u32, u32),
}

fn photos() -> Vec<Photo> {
    let list = std::fs::read_to_string(repo().join("crates/cyanotype-pets/tests/eval/individuals.tsv")).unwrap();
    list.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Photo {
                file: f[0].to_string(),
                identity: f[1].to_string(),
                kind: f[2].to_string(),
                group: f[3].to_string(),
                size: (f[4].parse().unwrap(), f[5].parse().unwrap()),
            }
        })
        .collect()
}

/// The photo as the worker's preview would be: upright and at most 1440 px on the long edge.
fn load(dir: &Path, p: &Photo) -> (Vec<u8>, u32, u32) {
    let open = || -> image::ImageResult<image::DynamicImage> {
        let mut decoder = image::ImageReader::open(dir.join(&p.file))?
            .with_guessed_format()?
            .into_decoder()?;
        let orientation = image::ImageDecoder::orientation(&mut decoder)?;
        let mut img = image::DynamicImage::from_decoder(decoder)?;
        img.apply_orientation(orientation);
        Ok(img)
    };
    let img = open().unwrap_or_else(|e| panic!("{}: {e}", p.file)).to_rgb8();
    let (w, h) = img.dimensions();
    assert_eq!((w, h), p.size, "{} is not the pinned photo", p.file);
    let long = w.max(h);
    if long <= 1440 {
        return (img.into_raw(), w, h);
    }
    let s = 1440.0 / long as f32;
    let (nw, nh) = (
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
    );
    use fast_image_resize::{
        FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
        images::{Image, ImageRef},
    };
    let src = ImageRef::new(w, h, img.as_raw(), PixelType::U8x3).unwrap();
    let mut dst = Image::new(nw, nh, PixelType::U8x3);
    Resizer::new()
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
        )
        .unwrap();
    (dst.into_vec(), nw, nh)
}

fn threads() -> usize {
    std::env::var("PETS_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2)
}

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse::<f64>().ok()))
        })
        .unwrap_or(0.0)
        / 1024.0
}

/// The picture turned a quarter turn clockwise (what Rotate does to a photo).
fn rotate_cw(rgb: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; rgb.len()];
    for y in 0..h {
        for x in 0..w {
            // (x, y) goes to (h - 1 - y, x) in a picture h wide and w tall.
            let to = (x * h + (h - 1 - y)) * 3;
            let from = (y * w + x) * 3;
            out[to..to + 3].copy_from_slice(&rgb[from..from + 3]);
        }
    }
    (out, h as u32, w as u32)
}

/// A crop scaled down so that its shorter side is `side` pixels (as it would be in a preview where the animal is that small).
fn shrink(c: &Rgb, side: u32) -> Rgb {
    use fast_image_resize::{
        FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
        images::{Image, ImageRef},
    };
    let short = c.width.min(c.height);
    if short <= side {
        return c.clone();
    }
    let f = side as f32 / short as f32;
    let (nw, nh) = (
        ((c.width as f32 * f).round() as u32).max(1),
        ((c.height as f32 * f).round() as u32).max(1),
    );
    let src = ImageRef::new(c.width, c.height, &c.data, PixelType::U8x3).unwrap();
    let mut dst = Image::new(nw, nh, PixelType::U8x3);
    Resizer::new()
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
        )
        .unwrap();
    Rgb {
        width: nw,
        height: nh,
        data: dst.into_vec(),
    }
}

/// The sizes the crop is made small for ("how small an animal is worth a fingerprint").
const SMALL: [u32; 5] = [32, 48, 64, 96, 128];

/// The crops compared: padding and squareness.
const CROPS: [(&str, f32, bool); 4] = [
    ("box", 0.0, false),
    ("pad10", 0.10, false),
    ("sq10", 0.10, true),
    ("sq25", 0.25, true),
];

type Cache = BTreeMap<String, Vec<f32>>;

fn cache_path(dir: &Path) -> PathBuf {
    std::env::var_os("PETS_ID_CACHE").map_or_else(|| dir.join("embeddings.json"), PathBuf::from)
}

fn read_cache(path: &Path) -> Cache {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Embed every photo's animal with every candidate (what is not cached yet). Keys are `variant|file`, and `box|file` holds the
/// detector's box (four numbers) and `time|file` the milliseconds each model took.
fn embed_all(dir: &Path, list: &[Photo], cache: &mut Cache) {
    let t = threads();
    let embed_dir = std::env::var_os("PETS_EMBED_DIR").map_or_else(|| repo().join("models/pets-embed"), PathBuf::from);
    let missing = |cache: &Cache, p: &Photo| !cache.contains_key(&format!("siglip/sq10|{}", p.file));
    if !list.iter().any(|p| missing(cache, p)) {
        return;
    }
    let detector = Detector::load(&cyanotype_pets::model_dir(), t, DetectorConfig::default()).expect("detector");
    let siglip = ImageEncoder::load(Model::Siglip2B16, &Model::Siglip2B16.dir(), t).expect("siglip");
    let dinos: Vec<(&str, DinoEncoder)> = [
        ("fp32", "dinov2-small-model.onnx"),
        ("int8", "dinov2-small-model_int8.onnx"),
        ("q4", "dinov2-small-model_q4.onnx"),
    ]
    .into_iter()
    .filter(|(_, f)| embed_dir.join(f).exists())
    .map(|(n, f)| (n, DinoEncoder::load(&embed_dir.join(f), t).expect("dino")))
    .collect();
    let started = Instant::now();
    for (i, p) in list.iter().enumerate() {
        if !missing(cache, p) {
            continue;
        }
        let (rgb, w, h) = load(dir, p);
        let det_started = Instant::now();
        let dets = detector.detect(&rgb, w, h).unwrap();
        let det_ms = det_started.elapsed().as_secs_f64() * 1000.0;
        // The labelled animal: the biggest, surest box of its kind.
        let best = dets
            .iter()
            .filter(|d| d.kind.name() == p.kind)
            .max_by(|a, b| (a.score * a.bbox[2] * a.bbox[3]).total_cmp(&(b.score * b.bbox[2] * b.bbox[3])));
        let Some(best) = best else {
            println!("no {} found in {}", p.kind, p.file);
            cache.insert(format!("miss|{}", p.file), vec![1.0]);
            continue;
        };
        let bbox = [
            best.bbox[0] / w as f32,
            best.bbox[1] / h as f32,
            best.bbox[2] / w as f32,
            best.bbox[3] / h as f32,
        ];
        cache.insert(format!("box|{}", p.file), bbox.to_vec());
        cache.insert(format!("time|detector|{}", p.file), vec![det_ms as f32]);
        // The same animal after a Rotate: the photo turned, found again, its crop embedded.
        {
            let (r, rw, rh) = rotate_cw(&rgb, w, h);
            let rdets = detector.detect(&r, rw, rh).unwrap();
            let rbest = rdets
                .iter()
                .filter(|d| d.kind.name() == p.kind)
                .max_by(|a, b| (a.score * a.bbox[2] * a.bbox[3]).total_cmp(&(b.score * b.bbox[2] * b.bbox[3])));
            if let Some(rb) = rbest {
                let rbox = [
                    rb.bbox[0] / rw as f32,
                    rb.bbox[1] / rh as f32,
                    rb.bbox[2] / rw as f32,
                    rb.bbox[3] / rh as f32,
                ];
                if let Some(c) = crop::crop(&r, rw, rh, rbox, 0.10, true) {
                    let input = preprocess_rgb8(Model::Siglip2B16, c.width, c.height, &c.data).unwrap();
                    cache.insert(
                        format!("rot/siglip/sq10|{}", p.file),
                        siglip.embed(&[input]).unwrap().pop().unwrap(),
                    );
                    let input = dino::preprocess(&c).unwrap();
                    if let Some((_, enc)) = dinos.first() {
                        let out = enc.embed(&input, &[Pooling::Cls]).unwrap();
                        cache.insert(format!("rot/dino-cls/sq10|{}", p.file), out.into_iter().next().unwrap());
                    }
                }
            }
        }
        for (name, pad, square) in CROPS {
            let Some(c) = crop::crop(&rgb, w, h, bbox, pad, square) else {
                continue;
            };
            let s = Instant::now();
            let input = preprocess_rgb8(Model::Siglip2B16, c.width, c.height, &c.data).unwrap();
            let v = siglip.embed(&[input]).unwrap().pop().unwrap();
            cache.insert(
                format!("time|siglip|{}|{}", name, p.file),
                vec![s.elapsed().as_secs_f64() as f32 * 1000.0],
            );
            cache.insert(format!("siglip/{name}|{}", p.file), v);
            if name == "sq10" {
                for side in SMALL {
                    let small = shrink(&c, side);
                    let input = preprocess_rgb8(Model::Siglip2B16, small.width, small.height, &small.data).unwrap();
                    let v = siglip.embed(&[input]).unwrap().pop().unwrap();
                    cache.insert(format!("siglip/small{side}|{}", p.file), v);
                }
            }
            for (prec, enc) in &dinos {
                let s = Instant::now();
                let input = dino::preprocess(&c).unwrap();
                let out = enc
                    .embed(&input, &[Pooling::Cls, Pooling::PatchMean, Pooling::Both])
                    .unwrap();
                cache.insert(
                    format!("time|dino-{prec}|{}|{}", name, p.file),
                    vec![s.elapsed().as_secs_f64() as f32 * 1000.0],
                );
                for (pool, v) in ["cls", "mean", "both"].into_iter().zip(out) {
                    cache.insert(format!("dino-{prec}-{pool}/{name}|{}", p.file), v);
                }
            }
        }
        if (i + 1) % 20 == 0 {
            println!(
                "embedded {} of {} ({:.0}s)",
                i + 1,
                list.len(),
                started.elapsed().as_secs_f64()
            );
            std::fs::write(cache_path(dir), serde_json::to_string(&cache).unwrap()).unwrap();
        }
    }
    std::fs::write(cache_path(dir), serde_json::to_string(&cache).unwrap()).unwrap();
    println!("peak memory of this process {:.0} MB (all models loaded)", rss_mb());
}

/// How small a crop may be: the shipped candidate's crops shrunk to a few sizes (what the picture would be if the animal were that small
/// in the preview), added to the cache after the main run (`dino-int8-cls/small<side>`).
fn embed_small(dir: &Path, list: &[Photo], cache: &mut Cache) {
    let embed_dir = std::env::var_os("PETS_EMBED_DIR").map_or_else(|| repo().join("models/pets-embed"), PathBuf::from);
    let file = embed_dir.join("dinov2-small-model_int8.onnx");
    let todo: Vec<&Photo> = list
        .iter()
        .filter(|p| {
            cache.contains_key(&format!("box|{}", p.file))
                && !cache.contains_key(&format!("dino-int8-cls/small32|{}", p.file))
        })
        .collect();
    if todo.is_empty() || !file.exists() {
        return;
    }
    let enc = DinoEncoder::load(&file, threads()).expect("dino");
    for p in todo {
        let (rgb, w, h) = load(dir, p);
        let b = cache[&format!("box|{}", p.file)].clone();
        let Some(c) = crop::crop(&rgb, w, h, [b[0], b[1], b[2], b[3]], 0.10, true) else {
            continue;
        };
        for side in SMALL {
            let small = shrink(&c, side);
            let out = enc.embed(&dino::preprocess(&small).unwrap(), &[Pooling::Cls]).unwrap();
            cache.insert(
                format!("dino-int8-cls/small{side}|{}", p.file),
                out.into_iter().next().unwrap(),
            );
        }
    }
    std::fs::write(cache_path(dir), serde_json::to_string(&cache).unwrap()).unwrap();
}

// ---- scoring ----

/// A tiny deterministic generator (xorshift), so that draws are the same on every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn centre(vs: &[&Vec<f32>]) -> Vec<f32> {
    let mut c = vec![0f32; vs[0].len()];
    for v in vs {
        for (a, b) in c.iter_mut().zip(v.iter()) {
            *a += b;
        }
    }
    let n = c.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    c.iter_mut().for_each(|x| *x /= n);
    c
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    Centre,
    Best,
    Top3,
}

impl Rule {
    const ALL: [Rule; 3] = [Rule::Centre, Rule::Best, Rule::Top3];
    fn name(self) -> &'static str {
        match self {
            Rule::Centre => "centre",
            Rule::Best => "best",
            Rule::Top3 => "top3",
        }
    }
    fn score(self, seeds: &[&Vec<f32>], c: &[f32], x: &[f32]) -> f32 {
        match self {
            Rule::Centre => dot(c, x),
            Rule::Best => seeds.iter().map(|s| dot(s, x)).fold(f32::MIN, f32::max),
            Rule::Top3 => {
                let mut v: Vec<f32> = seeds.iter().map(|s| dot(s, x)).collect();
                v.sort_by(|a, b| b.total_cmp(a));
                let n = v.len().min(3);
                v[..n].iter().sum::<f32>() / n as f32
            }
        }
    }
}

struct Candidate {
    score: f32,
    positive: bool,
    lookalike: bool,
}

/// One draw: the animal `id`'s photos split into `k` seeds and the rest, against every other photo of the kind.
struct Run {
    candidates: Vec<Candidate>,
}

fn average_precision(c: &[Candidate]) -> f64 {
    let positives = c.iter().filter(|x| x.positive).count();
    if positives == 0 {
        return 0.0;
    }
    let mut order: Vec<&Candidate> = c.iter().collect();
    order.sort_by(|a, b| b.score.total_cmp(&a.score));
    let (mut hit, mut sum) = (0usize, 0.0);
    for (i, x) in order.iter().enumerate() {
        if x.positive {
            hit += 1;
            sum += hit as f64 / (i + 1) as f64;
        }
    }
    sum / positives as f64
}

const DRAWS: usize = 40;

fn runs(items: &[(&Photo, &Vec<f32>)], k: usize, rule: Rule, seed: u64) -> Vec<Run> {
    let mut rng = Rng(seed | 1);
    let ids: BTreeSet<&str> = items.iter().map(|(p, _)| p.identity.as_str()).collect();
    let mut out = Vec::new();
    for id in ids {
        let own: Vec<usize> = (0..items.len()).filter(|&i| items[i].0.identity == id).collect();
        let kind = &items[own[0]].0.kind;
        let group = &items[own[0]].0.group;
        // At least three photos left to find.
        if own.len() < k + 3 {
            continue;
        }
        for _ in 0..DRAWS {
            let mut pool = own.clone();
            let mut seeds = Vec::new();
            for _ in 0..k {
                seeds.push(pool.swap_remove(rng.below(pool.len())));
            }
            let sv: Vec<&Vec<f32>> = seeds.iter().map(|&i| items[i].1).collect();
            let c = centre(&sv);
            let candidates = (0..items.len())
                .filter(|i| !seeds.contains(i) && items[*i].0.kind == *kind)
                .map(|i| {
                    let (p, v) = items[i];
                    Candidate {
                        score: rule.score(&sv, &c, v),
                        positive: p.identity == id,
                        lookalike: p.identity != id && p.group == *group && group != "-",
                    }
                })
                .collect();
            out.push(Run { candidates });
        }
    }
    out
}

struct At {
    precision: f64,
    recall: f64,
    /// Share of look-alike photos proposed.
    lookalike_rate: f64,
    /// Share of draws in which at least one look-alike is proposed, and at least one stranger of any kind.
    any_lookalike: f64,
    any_stranger: f64,
    /// Proposals per draw.
    proposed: f64,
}

fn at(runs: &[Run], t: f32) -> At {
    let (mut prec, mut precn, mut rec, mut la, mut lan) = (0.0, 0usize, 0.0, 0.0, 0usize);
    let (mut any_la, mut any_st, mut proposed) = (0usize, 0usize, 0usize);
    for r in runs {
        let hits: Vec<&Candidate> = r.candidates.iter().filter(|c| c.score >= t).collect();
        let tp = hits.iter().filter(|c| c.positive).count();
        let positives = r.candidates.iter().filter(|c| c.positive).count();
        proposed += hits.len();
        if !hits.is_empty() {
            prec += tp as f64 / hits.len() as f64;
            precn += 1;
        }
        rec += tp as f64 / positives.max(1) as f64;
        let looks = r.candidates.iter().filter(|c| c.lookalike).count();
        if looks > 0 {
            la += hits.iter().filter(|c| c.lookalike).count() as f64 / looks as f64;
            lan += 1;
            any_la += usize::from(hits.iter().any(|c| c.lookalike));
        }
        any_st += usize::from(hits.iter().any(|c| !c.positive));
    }
    let n = runs.len().max(1) as f64;
    At {
        precision: if precn == 0 { f64::NAN } else { prec / precn as f64 },
        recall: rec / n,
        lookalike_rate: if lan == 0 { f64::NAN } else { la / lan as f64 },
        any_lookalike: if lan == 0 { f64::NAN } else { any_la as f64 / lan as f64 },
        any_stranger: any_st as f64 / n,
        proposed: proposed as f64 / n,
    }
}

fn variants(cache: &Cache) -> Vec<String> {
    let mut v: BTreeSet<String> = BTreeSet::new();
    for k in cache.keys() {
        if let Some((variant, _)) = k.split_once('|')
            && variant.contains('/')
            && !variant.starts_with("rot/")
        {
            v.insert(variant.to_string());
        }
    }
    v.into_iter().collect()
}

#[test]
#[ignore = "needs the evaluation set and the models; prints a table (see the module comment)"]
fn individuals() {
    let dir = std::env::var_os("PETS_ID_DIR").map_or_else(|| repo().join("models/pets-id-eval"), PathBuf::from);
    let list = photos();
    let mut cache = read_cache(&cache_path(&dir));
    embed_all(&dir, &list, &mut cache);
    embed_small(&dir, &list, &mut cache);
    let missed: Vec<&str> = list
        .iter()
        .filter(|p| cache.contains_key(&format!("miss|{}", p.file)))
        .map(|p| p.file.as_str())
        .collect();
    println!(
        "{} photos, {} animals; the detector found no animal of the kind in {} ({:?})",
        list.len(),
        list.iter().map(|p| &p.identity).collect::<BTreeSet<_>>().len(),
        missed.len(),
        missed
    );
    let only = std::env::var("PETS_VARIANTS").ok();
    let mut summary: Vec<(String, f64)> = Vec::new();
    for variant in variants(&cache) {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|v| variant.contains(v)))
        {
            continue;
        }
        let items: Vec<(&Photo, &Vec<f32>)> = list
            .iter()
            .filter_map(|p| cache.get(&format!("{variant}|{}", p.file)).map(|v| (p, v)))
            .collect();
        for rule in Rule::ALL {
            let mut line = format!("{variant:<22} {:<7}", rule.name());
            let mut total = 0.0;
            for k in [3usize, 5, 10] {
                let rs = runs(&items, k, rule, 7);
                let map = rs.iter().map(|r| average_precision(&r.candidates)).sum::<f64>() / rs.len().max(1) as f64;
                total += map;
                line += &format!("  k={k}: mAP {map:.3} ({} runs)", rs.len());
            }
            println!("{line}");
            summary.push((format!("{variant} {}", rule.name()), total / 3.0));
        }
    }
    summary.sort_by(|a, b| b.1.total_cmp(&a.1));
    println!("\nbest by mean average precision over k = 3, 5, 10:");
    for (name, m) in summary.iter().take(12) {
        println!("  {m:.3}  {name}");
    }
    // The detail for chosen variants: PETS_DETAIL="siglip/sq10:centre,dino-fp32-cls/sq10:best".
    if let Ok(detail) = std::env::var("PETS_DETAIL") {
        for spec in detail.split(',') {
            let (variant, rule) = spec.split_once(':').unwrap_or((spec, "centre"));
            let rule = Rule::ALL.into_iter().find(|r| r.name() == rule).expect("rule");
            let items: Vec<(&Photo, &Vec<f32>)> = list
                .iter()
                .filter_map(|p| cache.get(&format!("{variant}|{}", p.file)).map(|v| (p, v)))
                .collect();
            let grid: Vec<f32> = (0..=40).map(|i| 0.30 + i as f32 * 0.0175).collect();
            for k in [3usize, 5, 10] {
                let rs = runs(&items, k, rule, 7);
                println!(
                    "\n{variant} {} k={k}: threshold, precision, recall, look-alikes proposed (share of them), draws with a look-alike, draws with any stranger, proposals per draw",
                    rule.name()
                );
                for t in &grid {
                    let a = at(&rs, *t);
                    println!(
                        "  {t:.3}  P {:.3}  R {:.3}  look-alike {:.3}  draws-with-look-alike {:.3}  draws-with-stranger {:.3}  proposed {:.1}",
                        a.precision, a.recall, a.lookalike_rate, a.any_lookalike, a.any_stranger, a.proposed
                    );
                }
            }
        }
    }
    // After a Rotate: how alike is the same animal's crop in the turned photo to the crop it had (the number a new scan's carry-over compares).
    for (turned, plain, name) in [
        ("rot/siglip/sq10", "siglip/sq10", "SigLIP 2"),
        ("rot/dino-cls/sq10", "dino-fp32-cls/sq10", "DINOv2-small (CLS)"),
    ] {
        let mut same: Vec<f32> = Vec::new();
        let mut others: Vec<f32> = Vec::new();
        for p in &list {
            let (Some(a), Some(b)) = (
                cache.get(&format!("{plain}|{}", p.file)),
                cache.get(&format!("{turned}|{}", p.file)),
            ) else {
                continue;
            };
            same.push(dot(a, b));
            // And what the turned crop is like to the crops of the same photo's neighbours in the set: other animals of the group.
            for q in &list {
                if q.file != p.file
                    && q.group == p.group
                    && q.identity != p.identity
                    && p.group != "-"
                    && let Some(c) = cache.get(&format!("{plain}|{}", q.file))
                {
                    others.push(dot(b, c));
                }
            }
        }
        if same.is_empty() {
            continue;
        }
        same.sort_by(|a, b| a.total_cmp(b));
        others.sort_by(|a, b| a.total_cmp(b));
        let pct = |v: &[f32], q: f64| v[((v.len() as f64 * q) as usize).min(v.len() - 1)];
        println!(
            "rotate, {name}: crop of the turned photo against the crop it had: n {} min {:.3} p5 {:.3} p25 {:.3} median {:.3}; against look-alikes' crops: median {:.3} p95 {:.3} p99 {:.3} max {:.3}",
            same.len(),
            same[0],
            pct(&same, 0.05),
            pct(&same, 0.25),
            pct(&same, 0.5),
            if others.is_empty() { f32::NAN } else { pct(&others, 0.5) },
            if others.is_empty() {
                f32::NAN
            } else {
                pct(&others, 0.95)
            },
            if others.is_empty() {
                f32::NAN
            } else {
                pct(&others, 0.99)
            },
            others.last().copied().unwrap_or(f32::NAN),
        );
    }
    // The cost of each model on this machine.
    let mut times: HashMap<String, Vec<f32>> = HashMap::new();
    for (k, v) in &cache {
        if let Some(rest) = k.strip_prefix("time|") {
            let parts: Vec<&str> = rest.split('|').collect();
            let key = if parts[0] == "detector" {
                "detector".to_string()
            } else {
                format!("{} {}", parts[0], parts[1])
            };
            times.entry(key).or_default().push(v[0]);
        }
    }
    let mut t: Vec<_> = times.into_iter().collect();
    t.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, v) in t {
        let mut s = v.clone();
        s.sort_by(|a, b| a.total_cmp(b));
        println!("time {name:<24} median {:.0} ms over {} crops", s[s.len() / 2], s.len());
    }
}

/// What shipping the fingerprint costs next to the detector: only D-FINE-N and DINOv2-small (8-bit) loaded, twenty photos' animals
/// found and fingerprinted. Run it alone (`-- --ignored fingerprint_cost`): the peak memory is the process's.
#[test]
#[ignore = "a measurement; needs the evaluation set and the models"]
fn fingerprint_cost() {
    let dir = std::env::var_os("PETS_ID_DIR").map_or_else(|| repo().join("models/pets-id-eval"), PathBuf::from);
    let list: Vec<Photo> = photos().into_iter().step_by(13).take(20).collect();
    let t = threads();
    let before = rss_mb();
    let detector = Detector::load(&cyanotype_pets::model_dir(), t, DetectorConfig::default()).expect("detector");
    let enc = DinoEncoder::load(&cyanotype_pets::model_dir().join(cyanotype_pets::DINO_FILE), t).expect("dino");
    let loaded = rss_mb();
    let (mut det_ms, mut fp_ms, mut n) = (0.0, 0.0, 0);
    for p in &list {
        let (rgb, w, h) = load(&dir, p);
        let s = Instant::now();
        let dets = detector.detect(&rgb, w, h).unwrap();
        det_ms += s.elapsed().as_secs_f64() * 1000.0;
        for d in dets.iter().filter(|d| d.kind.name() == p.kind).take(1) {
            let b = [
                d.bbox[0] / w as f32,
                d.bbox[1] / h as f32,
                d.bbox[2] / w as f32,
                d.bbox[3] / h as f32,
            ];
            let s = Instant::now();
            if let Some(c) = crop::crop(&rgb, w, h, b, crop::PAD, true) {
                enc.embed(&dino::preprocess(&c).unwrap(), &[Pooling::Cls]).unwrap();
                fp_ms += s.elapsed().as_secs_f64() * 1000.0;
                n += 1;
            }
        }
    }
    println!(
        "memory before {before:.0} MB, both models loaded {loaded:.0} MB, peak after {} photos {:.0} MB; detector {:.0} ms a photo, fingerprint {:.0} ms an animal ({n} animals, {t} threads)",
        list.len(),
        rss_mb(),
        det_ms / list.len() as f64,
        fp_ms / n.max(1) as f64
    );
}
