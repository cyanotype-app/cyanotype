//! The model comparison behind `docs/build.md` ("Semantic search model"): 209 CC0 photos in 21 categories, 42 queries
//! (each category by name and as "a photo of a ..."), 12 queries nothing in the set answers, and the same category names in
//! Spanish, German and French. Prints the tables; asserts only that the shipped configuration keeps its edge, so a model or
//! quantisation change shows what it did.
//!
//! ```text
//! scripts/fetch-eval-set.sh                        # the 209 photos -> models/eval
//! scripts/fetch-models.sh                          # the shipped models -> models/clip, models/siglip2
//! SEARCH_MODEL_DIR=models SEARCH_EVAL_DIR=models/eval \
//!   cargo test --release -p cyanotype-clip --test bakeoff -- --ignored --nocapture
//! ```
//!
//! `BAKEOFF_SIGLIP_ONNX=<dir>` adds the other precisions of the SigLIP 2 export (a checkout of
//! onnx-community/siglip2-base-patch16-224-ONNX: `onnx/vision_model.onnx`, `vision_model_fp16.onnx`, `vision_model_int8.onnx`,
//! `text_model.onnx`, ...); without it only the shipped files are compared.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use cyanotype_clip::{
    ImageEncoder, ImageRules, Model, TextEncoder, cosine, image_tiers, image_tiers_with, preprocess_rgb8, tiers,
};
use serde_json::Value;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn eval_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("SEARCH_EVAL_DIR").map_or_else(|| repo().join("models/eval"), PathBuf::from);
    dir.join("airplane_0.jpg").exists().then_some(dir)
}

struct Photo {
    category: String,
    pixels: image::RgbImage,
}

/// The photos from `photos.tsv`, each checked against its recorded size and mean colour.
fn photos(dir: &Path) -> Vec<Photo> {
    let list = std::fs::read_to_string(repo().join("crates/cyanotype-clip/tests/eval/photos.tsv")).unwrap();
    let mut out = Vec::new();
    for line in list.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split('\t').collect();
        let img = image::open(dir.join(f[0]))
            .unwrap_or_else(|e| panic!("{}: {e}", f[0]))
            .to_rgb8();
        let (w, h): (u32, u32) = (f[2].parse().unwrap(), f[3].parse().unwrap());
        assert_eq!((img.width(), img.height()), (w, h), "{} is not the pinned photo", f[0]);
        let n = (w * h) as f64;
        for c in 0..3 {
            let mean = img.pixels().map(|p| f64::from(p[c])).sum::<f64>() / n;
            let want: f64 = f[4 + c].parse().unwrap();
            assert!(
                (mean - want).abs() < 2.0,
                "{} is not the pinned photo (mean colour)",
                f[0]
            );
        }
        out.push(Photo {
            category: f[1].to_string(),
            pixels: img,
        });
    }
    out
}

struct Variant {
    name: String,
    model: Model,
    vision: PathBuf,
    text: PathBuf,
    /// Directory with the tokenizer.
    dir: PathBuf,
}

fn variants() -> Vec<Variant> {
    let root = cyanotype_clip::models_root();
    let mut v = Vec::new();
    for m in Model::ALL {
        let dir = root.join(m.dir_name());
        if dir.join(m.vision_file()).exists() && dir.join(m.text_file()).exists() {
            let label = match m {
                Model::ClipB32 => "CLIP B/32 int8 / int8 (shipped before)",
                Model::Siglip2B16 => "SigLIP 2 B/16 4-bit / int8 (shipped now)",
            };
            v.push(Variant {
                name: label.into(),
                model: m,
                vision: dir.join(m.vision_file()),
                text: dir.join(m.text_file()),
                dir,
            });
        }
    }
    if let Some(onnx) = std::env::var_os("BAKEOFF_SIGLIP_ONNX").map(PathBuf::from) {
        let sig = root.join(Model::Siglip2B16.dir_name());
        for (name, vision, text) in [
            ("SigLIP 2 B/16 fp32 / fp32", "vision_model.onnx", "text_model.onnx"),
            ("SigLIP 2 B/16 fp32 / int8", "vision_model.onnx", "text_model_int8.onnx"),
            (
                "SigLIP 2 B/16 fp16 / fp16",
                "vision_model_fp16.onnx",
                "text_model_fp16.onnx",
            ),
            (
                "SigLIP 2 B/16 int8 / int8",
                "vision_model_int8.onnx",
                "text_model_int8.onnx",
            ),
            ("SigLIP 2 B/16 int8 / fp32", "vision_model_int8.onnx", "text_model.onnx"),
        ] {
            let (vp, tp) = (onnx.join("onnx").join(vision), onnx.join("onnx").join(text));
            if vp.exists() && tp.exists() {
                v.push(Variant {
                    name: name.into(),
                    model: Model::Siglip2B16,
                    vision: vp,
                    text: tp,
                    dir: sig.clone(),
                });
            }
        }
    }
    v
}

fn embed_photos(v: &Variant, photos: &[Photo]) -> Vec<Vec<f32>> {
    let enc = ImageEncoder::load_file(v.model, &v.vision, 4).unwrap();
    photos
        .iter()
        .map(|p| {
            let pre = preprocess_rgb8(v.model, p.pixels.width(), p.pixels.height(), p.pixels.as_raw()).unwrap();
            enc.embed(&[pre]).unwrap().remove(0)
        })
        .collect()
}

#[derive(Default, Clone, Copy)]
struct Scores {
    p1: f64,
    p5: f64,
    map: f64,
    r10: f64,
}

/// P@1, P@5, average precision and recall@10 (of min(relevant, 10)) of one ranked list.
fn rank_scores(relevant: &[bool]) -> (f64, f64, f64, f64) {
    let total = relevant.iter().filter(|r| **r).count();
    let (mut hits, mut ap) = (0usize, 0.0);
    for (i, r) in relevant.iter().enumerate() {
        if *r {
            hits += 1;
            ap += hits as f64 / (i + 1) as f64;
        }
    }
    let at = |k: usize| relevant.iter().take(k).filter(|r| **r).count();
    (
        f64::from(u8::from(relevant[0])),
        at(5) as f64 / 5.0,
        ap / total.max(1) as f64,
        at(10) as f64 / total.clamp(1, 10) as f64,
    )
}

fn ranked(images: &[Vec<f32>], q: &[f32]) -> Vec<(f32, usize)> {
    let mut s: Vec<(f32, usize)> = images.iter().enumerate().map(|(i, v)| (cosine(q, v), i)).collect();
    s.sort_by(|a, b| b.0.total_cmp(&a.0));
    s
}

/// A query's results best first as (cosine, relevant); the category is None for a query nothing answers.
type RankedList<'a> = (Option<&'a String>, Vec<(f32, bool)>);

fn evaluate(
    photos: &[Photo],
    images: &[Vec<f32>],
    queries: &[(String, String)],
    embed: &dyn Fn(&str) -> Vec<f32>,
) -> (Scores, Vec<String>) {
    let mut sum = Scores::default();
    let mut failures = Vec::new();
    for (category, q) in queries {
        let order = ranked(images, &embed(q));
        let rel: Vec<bool> = order.iter().map(|(_, i)| &photos[*i].category == category).collect();
        let (p1, p5, ap, r10) = rank_scores(&rel);
        sum.p1 += p1;
        sum.p5 += p5;
        sum.map += ap;
        sum.r10 += r10;
        if p1 == 0.0 || p5 < 0.6 {
            let top: Vec<&str> = order
                .iter()
                .take(5)
                .map(|(_, i)| photos[*i].category.as_str())
                .collect();
            failures.push(format!("{q:?} (wants {category}): top 5 = {top:?}, P@5 {p5:.1}"));
        }
    }
    let n = queries.len() as f64;
    (
        Scores {
            p1: sum.p1 / n,
            p5: sum.p5 / n,
            map: sum.map / n,
            r10: sum.r10 / n,
        },
        failures,
    )
}

#[test]
#[ignore = "the model comparison: needs the evaluation set and the models, prints tables"]
fn bakeoff() {
    let Some(dir) = eval_dir() else {
        panic!("evaluation set not found: scripts/fetch-eval-set.sh, then SEARCH_EVAL_DIR=models/eval");
    };
    let photos = photos(&dir);
    assert_eq!(photos.len(), 209);
    let q: Value =
        serde_json::from_slice(&std::fs::read(repo().join("crates/cyanotype-clip/tests/eval/queries.json")).unwrap())
            .unwrap();
    let positive: Vec<(String, String)> = q["positive"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["category"].as_str().unwrap().to_string(),
                x["query"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let negative: Vec<String> = q["negative"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    let multilingual: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_slice(
        &std::fs::read(repo().join("crates/cyanotype-clip/tests/eval/queries_multilingual.json")).unwrap(),
    )
    .unwrap();
    // The keyword form of each category (the first of its two queries).
    let keywords: Vec<(String, String)> = positive.iter().step_by(2).cloned().collect();
    let cats: std::collections::BTreeSet<&str> = photos.iter().map(|p| p.category.as_str()).collect();

    println!(
        "\n{} photos, {} categories, {} queries\n",
        photos.len(),
        cats.len(),
        positive.len()
    );
    println!("| image tower / text tower | P@1 | P@5 | mAP | R@10 |\n|---|---|---|---|---|");
    let mut results = Vec::new();
    for v in variants() {
        let images = embed_photos(&v, &photos);
        let text = TextEncoder::load_file(v.model, &v.dir, &v.text, 4).unwrap();
        // Plain queries as typed, with the model's own preparation (SigLIP 2's caption template for short ones).
        let embed_q = |s: &str| text.embed_query(s).unwrap().to_vec();
        let (s, failures) = evaluate(&photos, &images, &positive, &embed_q);
        println!(
            "| {} | {:.3} | {:.3} | {:.3} | {:.3} |",
            v.name, s.p1, s.p5, s.map, s.r10
        );
        results.push((v, images, s, failures));
    }

    for (v, _, _, failures) in &results {
        println!("\nqueries with a miss at rank 1 or P@5 below 0.6, {}:", v.name);
        for f in failures {
            println!("  {f}");
        }
        if failures.is_empty() {
            println!("  none");
        }
    }

    println!("\nPrompt templates on the keyword queries (21), P@5 / mAP:");
    println!(
        "| image / text | bare | a photo of {{q}} | a photo of a {{q}} | average of the two | average of three |\n|---|---|---|---|---|---|"
    );
    for (v, images, _, _) in &results {
        let text = TextEncoder::load_file(v.model, &v.dir, &v.text, 4).unwrap();
        let with = |templates: &'static [&'static str]| {
            let embed = |s: &str| {
                let mut sum = vec![0f32; v.model.dim()];
                for t in templates {
                    let e = text.embed(&t.replace("{}", s)).unwrap();
                    sum.iter_mut().zip(e.iter()).for_each(|(a, b)| *a += b);
                }
                cyanotype_clip::l2_normalize(&mut sum);
                sum
            };
            let (s, _) = evaluate(&photos, images, &keywords, &embed);
            format!("{:.3} / {:.3}", s.p5, s.map)
        };
        println!(
            "| {} | {} | {} | {} | {} | {} |",
            v.name,
            with(&["{}"]),
            with(&["a photo of {}"]),
            with(&["a photo of a {}"]),
            with(&["{}", "a photo of {}"]),
            with(&["{}", "a photo of {}", "a picture of {}"]),
        );
    }

    println!("\nSpanish / German / French category names (P@5 / mAP):");
    println!("| image / text | es | de | fr |\n|---|---|---|---|");
    for (v, images, _, _) in &results {
        let text = TextEncoder::load_file(v.model, &v.dir, &v.text, 4).unwrap();
        let embed_q = |s: &str| text.embed_query(s).unwrap().to_vec();
        let mut cells = Vec::new();
        for lang in ["es", "de", "fr"] {
            let qs: Vec<(String, String)> = multilingual[lang]
                .iter()
                .filter(|(c, _)| cats.contains(c.as_str()))
                .map(|(c, q)| (c.clone(), q.clone()))
                .collect();
            let (s, _) = evaluate(&photos, images, &qs, &embed_q);
            cells.push(format!("{:.3} / {:.3}", s.p5, s.map));
        }
        println!("| {} | {} |", v.name, cells.join(" | "));
    }

    // The cut-off: what the strong tier returns against plain top-k, on the 42 queries, and on the 12 nothing answers.
    println!("\nCut-off (strong tier, and strong + weak) against top-k, 42 answerable queries; 12 unanswerable ones:");
    println!(
        "| image / text, list | precision | recall | returned per query | unanswerable queries: returned | of 12 returning nothing |\n|---|---|---|---|---|---|"
    );
    for (v, images, _, _) in &results {
        let text = TextEncoder::load_file(v.model, &v.dir, &v.text, 4).unwrap();
        let lists: Vec<RankedList> = positive
            .iter()
            .map(|(c, q)| (Some(c), (c, q)))
            .map(|(c, (cat, q))| {
                let order = ranked(images, &text.embed_query(q).unwrap());
                (
                    c,
                    order.iter().map(|(s, i)| (*s, &photos[*i].category == cat)).collect(),
                )
            })
            .chain(negative.iter().map(|q| {
                let order = ranked(images, &text.embed_query(q).unwrap());
                (None, order.iter().map(|(s, _)| (*s, false)).collect())
            }))
            .collect();
        let row = |name: &str, take: &dyn Fn(&[f32]) -> usize| {
            let (mut tp, mut ret, mut rel, mut npos, mut neg_ret, mut neg_empty) = (0, 0, 0, 0, 0, 0);
            for (c, list) in &lists {
                let cos: Vec<f32> = list.iter().map(|x| x.0).collect();
                let k = take(&cos);
                if c.is_some() {
                    tp += list[..k].iter().filter(|x| x.1).count();
                    ret += k;
                    rel += list.iter().filter(|x| x.1).count();
                    npos += 1;
                } else {
                    neg_ret += k;
                    neg_empty += usize::from(k == 0);
                }
            }
            println!(
                "| {} | {:.3} | {:.3} | {:.1} | {:.1} | {}/12 |",
                name,
                tp as f64 / ret.max(1) as f64,
                tp as f64 / rel as f64,
                ret as f64 / npos as f64,
                neg_ret as f64 / 12.0,
                neg_empty
            );
        };
        row(&format!("{}: top 10", v.name), &|c| 10.min(c.len()));
        row(&format!("{}: strong", v.name), &|c| tiers(v.model, c).strong);
        row(&format!("{}: strong + weak", v.name), &|c| tiers(v.model, c).weak);
    }

    // What the shipped model must keep.
    let shipped = results
        .iter()
        .find(|(v, ..)| v.model == Model::Siglip2B16 && v.name.contains("shipped now"));
    let before = results.iter().find(|(v, ..)| v.model == Model::ClipB32);
    if let (Some(new), Some(old)) = (shipped, before) {
        assert!(
            new.2.map > old.2.map,
            "SigLIP 2 should beat CLIP on mAP: {} against {}",
            new.2.map,
            old.2.map
        );
        assert!(new.2.p5 >= old.2.p5 - 0.01);
    }
}

/// Every photo as the source of a "More like this": the other 208 best first as (cosine, same category).
fn image_lists(photos: &[Photo], images: &[Vec<f32>]) -> Vec<Vec<(f32, bool)>> {
    (0..photos.len())
        .map(|q| {
            let mut l: Vec<(f32, bool)> = (0..photos.len())
                .filter(|&i| i != q)
                .map(|i| (cosine(&images[q], &images[i]), photos[i].category == photos[q].category))
                .collect();
            l.sort_by(|a, b| b.0.total_cmp(&a.0));
            l
        })
        .collect()
}

/// Precision, recall (of the 9 or 10 same-category photos), photos returned per source, and the share of sources with
/// nothing returned, for the first `take(cosines)` results of every list.
fn image_row(lists: &[Vec<(f32, bool)>], take: &dyn Fn(&[f32]) -> usize) -> (f64, f64, f64, f64) {
    let (mut tp, mut ret, mut rel, mut empty) = (0usize, 0usize, 0usize, 0usize);
    for l in lists {
        let cos: Vec<f32> = l.iter().map(|x| x.0).collect();
        let k = take(&cos);
        tp += l[..k].iter().filter(|x| x.1).count();
        ret += k;
        rel += l.iter().filter(|x| x.1).count();
        empty += usize::from(k == 0);
    }
    let n = lists.len() as f64;
    (
        tp as f64 / ret.max(1) as f64,
        tp as f64 / rel as f64,
        ret as f64 / n,
        empty as f64 / n,
    )
}

/// The calibration of the "More like this" cut-off (`cutoff.rs`, `ImageRules`): each of the 209 photos is the source, the
/// other 208 are ranked by cosine to it, and a result is relevant when it is in the same category. Prints the cosine
/// distributions, a sweep of the strong rule's floor and relative distance, and what the shipped rules give.
#[test]
#[ignore = "the similar-photos calibration: needs the evaluation set and the models, prints tables"]
fn similar_bakeoff() {
    let Some(dir) = eval_dir() else {
        panic!("evaluation set not found: scripts/fetch-eval-set.sh, then SEARCH_EVAL_DIR=models/eval");
    };
    let photos = photos(&dir);
    for v in variants().into_iter().filter(|v| v.name.contains("shipped")) {
        let images = embed_photos(&v, &photos);
        let lists = image_lists(&photos, &images);
        println!("\n== {} ==", v.name);
        // Where same-category and other pairs sit.
        let (mut same, mut other): (Vec<f32>, Vec<f32>) = (Vec::new(), Vec::new());
        for l in &lists {
            for (c, r) in l {
                if *r { same.push(*c) } else { other.push(*c) }
            }
        }
        let q = |v: &mut Vec<f32>, p: f64| {
            v.sort_by(f32::total_cmp);
            v[((v.len() - 1) as f64 * p) as usize]
        };
        println!("cosine quantiles (5 / 25 / 50 / 75 / 95 %):");
        for (name, v) in [("same category", &mut same), ("other category", &mut other)] {
            println!(
                "  {name}: {:.3} {:.3} {:.3} {:.3} {:.3}",
                q(v, 0.05),
                q(v, 0.25),
                q(v, 0.5),
                q(v, 0.75),
                q(v, 0.95)
            );
        }
        let best: Vec<f32> = lists.iter().map(|l| l[0].0).collect();
        let mut b = best.clone();
        println!(
            "  best neighbour of each photo: {:.3} {:.3} {:.3} {:.3} {:.3}",
            q(&mut b, 0.05),
            q(&mut b, 0.25),
            q(&mut b, 0.5),
            q(&mut b, 0.75),
            q(&mut b, 0.95)
        );
        println!(
            "\nstrong rule sweep (floor x distance below the best; no gap rule): precision / recall / per source / empty"
        );
        let floors: Vec<f32> = [-0.06, -0.04, -0.02, 0.0, 0.02, 0.04, 0.06, 0.08, 0.1, 0.12, 0.15]
            .iter()
            .map(|d| q(&mut other.clone(), 0.99) + d)
            .collect();
        println!(
            "(floors are the 99th percentile of other-category cosines, {:.3}, plus -0.06 .. 0.15)",
            floors[0]
        );
        for delta in [0.03f32, 0.05, 0.08, 0.12, 1.0] {
            for &f in &floors {
                let rules = ImageRules {
                    strong_min: f,
                    strong_delta: delta,
                    gap: 1.0,
                    weak_min: f,
                    weak_delta: delta,
                };
                let (p, r, n, e) = image_row(&lists, &|c| image_tiers_with(rules, c).strong);
                println!("  floor {f:.3} delta {delta:.2}: {p:.3} / {r:.3} / {n:.1} / {e:.2}");
            }
        }
        let rules = v.model.image_cutoff_rules();
        println!("\ngap rule with the shipped floor and distance (precision / recall / per source):");
        for gap in [0.02f32, 0.03, 0.05, 1.0] {
            let r = ImageRules { gap, ..rules };
            let (p, rc, n, _) = image_row(&lists, &|c| image_tiers_with(r, c).strong);
            println!("  gap {gap:.2}: {p:.3} / {rc:.3} / {n:.1}");
        }
        println!("\nshipped rules {rules:?}:");
        let top = |k: usize| move |c: &[f32]| k.min(c.len());
        for (name, row) in [
            ("top 5", image_row(&lists, &top(5))),
            ("top 10", image_row(&lists, &top(10))),
            ("strong", image_row(&lists, &|c| image_tiers(v.model, c).strong)),
            ("strong + weaker", image_row(&lists, &|c| image_tiers(v.model, c).weak)),
        ] {
            println!(
                "  {name}: precision {:.3}, recall {:.3}, {:.1} per source, {:.2} empty",
                row.0, row.1, row.2, row.3
            );
        }
    }
}

/// A tiny deterministic generator (the calibration must print the same numbers every run).
struct Lcg(u64);
impl Lcg {
    /// Uniform in -1..1.
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }
}

/// One more frame of the same scene: the camera moved a little (shift and scale), the exposure drifted, as between two
/// shots of a burst. `amount` is the largest shift as a share of the side, the largest zoom, and (x 2) the largest
/// brightness change as a share of full scale.
fn burst_frame(src: &image::RgbImage, amount: f32, rng: &mut Lcg) -> image::RgbImage {
    use image::imageops::{FilterType, brighten, crop_imm, resize};
    let (w, h) = (src.width(), src.height());
    let zoom = 1.0 - amount * rng.next().abs();
    let (cw, ch) = (((w as f32 * zoom) as u32).max(8), ((h as f32 * zoom) as u32).max(8));
    let max_dx = (w - cw) as f32 + amount * w as f32;
    let max_dy = (h - ch) as f32 + amount * h as f32;
    let dx = (((w - cw) as f32 / 2.0 + rng.next() * max_dx).clamp(0.0, (w - cw) as f32)) as u32;
    let dy = (((h - ch) as f32 / 2.0 + rng.next() * max_dy).clamp(0.0, (h - ch) as f32)) as u32;
    let cropped = crop_imm(src, dx, dy, cw, ch).to_image();
    let resized = resize(&cropped, w, h, FilterType::Triangle);
    brighten(&resized, (rng.next() * amount * 2.0 * 255.0) as i32)
}

/// The calibration of burst grouping (`crates/cyanotype-core/src/bursts.rs`): how close two consecutive frames of one scene
/// are by embedding cosine against how close two different photos are. There is no real burst data in the evaluation set
/// (209 unrelated photos), so a burst is simulated: every third photo (70) gets 5 more frames with a small, a medium and a large camera
/// move and exposure drift. Prints the distributions and what the shipped threshold keeps and merges wrongly.
#[test]
#[ignore = "the burst calibration: needs the evaluation set and the models, prints tables"]
fn burst_bakeoff() {
    let Some(dir) = eval_dir() else {
        panic!("evaluation set not found: scripts/fetch-eval-set.sh, then SEARCH_EVAL_DIR=models/eval");
    };
    let photos = photos(&dir);
    let levels = [("small", 0.02f32), ("medium", 0.05), ("large", 0.10)];
    for v in variants().into_iter().filter(|v| v.name.contains("shipped")) {
        let images = embed_photos(&v, &photos);
        let encoder = ImageEncoder::load_file(v.model, &v.vision, 4).unwrap();
        // Different photos: every pair of the 209 (21,736 pairs).
        let mut distinct: Vec<f32> = Vec::new();
        for i in 0..images.len() {
            for j in i + 1..images.len() {
                distinct.push(cosine(&images[i], &images[j]));
            }
        }
        distinct.sort_by(f32::total_cmp);
        let at = |v: &[f32], p: f64| v[((v.len() - 1) as f64 * p) as usize];
        println!("\n== {} ==", v.name);
        println!(
            "different photos ({} pairs): median {:.3}, 99 % {:.3}, 99.9 % {:.3}, max {:.3}",
            distinct.len(),
            at(&distinct, 0.5),
            at(&distinct, 0.99),
            at(&distinct, 0.999),
            distinct[distinct.len() - 1]
        );
        let rules = v.model.burst_rules();
        let wrong = distinct.iter().filter(|c| **c >= rules.min_cosine).count();
        println!(
            "shipped min_cosine {:.3}: {wrong} of {} different pairs would merge ({:.4} %)",
            rules.min_cosine,
            distinct.len(),
            wrong as f64 * 100.0 / distinct.len() as f64
        );
        let mut per_level: Vec<(&str, Vec<f32>)> = Vec::new();
        for (name, amount) in levels {
            let mut rng = Lcg(7);
            // Consecutive frames of a burst of 6 (the original and 5 more, each moved independently from the original).
            let mut adjacent: Vec<f32> = Vec::new();
            let mut to_first: Vec<f32> = Vec::new();
            for p in photos.iter().step_by(3) {
                let mut frames = vec![p.pixels.clone()];
                frames.extend((0..5).map(|_| burst_frame(&p.pixels, amount, &mut rng)));
                let vecs: Vec<Vec<f32>> = frames
                    .iter()
                    .map(|f| {
                        let pre = preprocess_rgb8(v.model, f.width(), f.height(), f.as_raw()).unwrap();
                        encoder.embed(&[pre]).unwrap().remove(0)
                    })
                    .collect();
                adjacent.extend(vecs.windows(2).map(|w| cosine(&w[0], &w[1])));
                to_first.extend(vecs[1..].iter().map(|x| cosine(&vecs[0], x)));
            }
            adjacent.sort_by(f32::total_cmp);
            to_first.sort_by(f32::total_cmp);
            let kept = adjacent.iter().filter(|c| **c >= rules.min_cosine).count();
            per_level.push((name, adjacent.clone()));
            println!(
                "{name:<6} move ({:>4.0} % shift): adjacent frames min {:.3}, 1 % {:.3}, 5 % {:.3}, median {:.3}; kept at the shipped threshold {:.1} % (to the first frame: median {:.3})",
                amount * 100.0,
                adjacent[0],
                at(&adjacent, 0.01),
                at(&adjacent, 0.05),
                at(&adjacent, 0.5),
                kept as f64 * 100.0 / adjacent.len() as f64,
                at(&to_first, 0.5)
            );
        }
        println!(
            "\nthreshold sweep: different photos merged / consecutive burst frames kept (small, medium, large move)"
        );
        for t in [0.80f32, 0.83, 0.85, 0.87, 0.89, 0.91, 0.93] {
            let merged = distinct.iter().filter(|c| **c >= t).count();
            let kept: Vec<String> = per_level
                .iter()
                .map(|(_, a)| {
                    format!(
                        "{:.1} %",
                        a.iter().filter(|c| **c >= t).count() as f64 * 100.0 / a.len() as f64
                    )
                })
                .collect();
            println!(
                "  min cosine {t:.2}: {merged:>5} of {} pairs merged ({:.2} %); kept {}",
                distinct.len(),
                merged as f64 * 100.0 / distinct.len() as f64,
                kept.join(" / ")
            );
        }
    }
}

// ---- near-duplicates ------------------------------------------------------------------------------------------------------

/// Re-encodes and resizes of a photo, the way a copy comes about: a smaller export, a messaging app's recompression, a screenshot-size
/// copy, a copy with a thin border cropped off. Each is a (name, pixels) pair of the same picture.
fn duplicate_variants(p: &image::RgbImage) -> Vec<(&'static str, image::RgbImage)> {
    use image::{codecs::jpeg::JpegEncoder, imageops, imageops::FilterType};
    let jpeg = |img: &image::RgbImage, q: u8| -> image::RgbImage {
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, q)
            .encode_image(img)
            .expect("encode");
        image::load_from_memory_with_format(&out, image::ImageFormat::Jpeg)
            .expect("decode")
            .to_rgb8()
    };
    let scaled = |f: f32| {
        imageops::resize(
            p,
            ((p.width() as f32 * f) as u32).max(32),
            ((p.height() as f32 * f) as u32).max(32),
            FilterType::Lanczos3,
        )
    };
    let (w, h) = (p.width(), p.height());
    let (dx, dy) = (w * 3 / 100, h * 3 / 100);
    vec![
        ("half size", scaled(0.5)),
        ("quarter size", scaled(0.25)),
        ("same size, JPEG q70", jpeg(p, 70)),
        ("same size, JPEG q40", jpeg(p, 40)),
        ("half size, JPEG q60", jpeg(&scaled(0.5), 60)),
        (
            "3 % border cropped",
            imageops::crop_imm(p, dx, dy, w - 2 * dx, h - 2 * dy).to_image(),
        ),
    ]
}

/// The calibration of the near-duplicate threshold (`docs/build.md`, "Near-duplicate threshold"): copies of every photo of the
/// evaluation set (resized, recompressed, cropped) against pairs of different photos, and against the hardest different pairs (the
/// same category: other dogs, other beaches). Prints precision and recall for a sweep of cosine thresholds.
#[test]
#[ignore = "the near-duplicate calibration: needs the evaluation set and the models, prints tables"]
fn duplicate_bakeoff() {
    let Some(dir) = eval_dir() else {
        panic!("evaluation set not found: scripts/fetch-eval-set.sh, then SEARCH_EVAL_DIR=models/eval");
    };
    let photos = photos(&dir);
    // `DUP_MODEL=CLIP` (or `SigLIP`) runs one model; by default both shipped ones.
    let only = std::env::var("DUP_MODEL").unwrap_or_default();
    for v in variants()
        .into_iter()
        .filter(|v| v.name.contains("shipped") && v.name.contains(&only))
    {
        let enc = ImageEncoder::load_file(v.model, &v.vision, 4).unwrap();
        let embed = |img: &image::RgbImage| {
            let pre = preprocess_rgb8(v.model, img.width(), img.height(), img.as_raw()).unwrap();
            enc.embed(&[pre]).unwrap().remove(0)
        };
        let originals: Vec<Vec<f32>> = photos.iter().map(|p| embed(&p.pixels)).collect();
        // Positives: (original, copy) for every photo and kind of copy, and per kind.
        let mut kinds: BTreeMap<&'static str, Vec<f32>> = BTreeMap::new();
        for (i, p) in photos.iter().enumerate() {
            for (name, img) in duplicate_variants(&p.pixels) {
                kinds.entry(name).or_default().push(cosine(&originals[i], &embed(&img)));
            }
        }
        // Negatives: every pair of different photos; and the same-category ones among them.
        let (mut distinct, mut same_category): (Vec<f32>, Vec<f32>) = (Vec::new(), Vec::new());
        for i in 0..photos.len() {
            for j in i + 1..photos.len() {
                let c = cosine(&originals[i], &originals[j]);
                distinct.push(c);
                if photos[i].category == photos[j].category {
                    same_category.push(c);
                }
            }
        }
        let mut positives: Vec<f32> = kinds.values().flatten().copied().collect();
        for list in [&mut positives, &mut distinct, &mut same_category] {
            list.sort_by(f32::total_cmp);
        }
        let at = |v: &[f32], p: f64| v[((v.len() - 1) as f64 * p) as usize];
        println!("\n== {} ==", v.name);
        println!(
            "copies ({} pairs): min {:.4}, 1 % {:.4}, 5 % {:.4}, median {:.4}",
            positives.len(),
            positives[0],
            at(&positives, 0.01),
            at(&positives, 0.05),
            at(&positives, 0.5)
        );
        for (name, list) in &mut kinds {
            list.sort_by(f32::total_cmp);
            println!(
                "  {name:<22} min {:.4}, 1 % {:.4}, 5 % {:.4}, median {:.4}",
                list[0],
                at(list, 0.01),
                at(list, 0.05),
                at(list, 0.5)
            );
        }
        println!(
            "different photos ({} pairs): median {:.3}, 99 % {:.3}, 99.9 % {:.4}, max {:.4}",
            distinct.len(),
            at(&distinct, 0.5),
            at(&distinct, 0.99),
            at(&distinct, 0.999),
            distinct[distinct.len() - 1]
        );
        println!(
            "same-category pairs ({}): median {:.3}, 99 % {:.3}, max {:.4}",
            same_category.len(),
            at(&same_category, 0.5),
            at(&same_category, 0.99),
            same_category[same_category.len() - 1]
        );
        println!(
            "\nthreshold sweep over {} copies and {} different pairs",
            positives.len(),
            distinct.len()
        );
        println!("  min cosine | recall (copies found) | different pairs merged | precision | same-category merged");
        for t in [
            0.70f32, 0.75, 0.80, 0.85, 0.88, 0.90, 0.92, 0.94, 0.95, 0.96, 0.97, 0.98, 0.99,
        ] {
            let tp = positives.iter().filter(|c| **c >= t).count();
            let fp = distinct.iter().filter(|c| **c >= t).count();
            let fp_cat = same_category.iter().filter(|c| **c >= t).count();
            println!(
                "  {t:>9.3}  | {:>6.2} % ({tp:>4}/{}) | {fp:>5} ({:.3} %) | {:>6.2} % | {fp_cat}",
                tp as f64 * 100.0 / positives.len() as f64,
                positives.len(),
                fp as f64 * 100.0 / distinct.len() as f64,
                tp as f64 * 100.0 / (tp + fp).max(1) as f64
            );
        }
        // Which different pairs are at the top (are they the same picture twice?).
        let mut top: Vec<(f32, usize, usize)> = Vec::new();
        for i in 0..photos.len() {
            for j in i + 1..photos.len() {
                top.push((cosine(&originals[i], &originals[j]), i, j));
            }
        }
        top.sort_by(|a, b| b.0.total_cmp(&a.0));
        println!("\nthe closest different pairs:");
        for (c, i, j) in top.iter().take(8) {
            println!(
                "  {c:.4}  photo {i} ({}) and {j} ({})",
                photos[*i].category, photos[*j].category
            );
        }
    }
}
