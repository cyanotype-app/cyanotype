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

use cyanotype_clip::{ImageEncoder, Model, TextEncoder, cosine, preprocess_rgb8, tiers};
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
