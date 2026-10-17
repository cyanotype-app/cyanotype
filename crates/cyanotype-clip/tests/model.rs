//! Parity and quality tests against the real model files. They need the ONNX files and tokenizer (see
//! `scripts/fetch-models.sh`) in `CLIP_MODEL_DIR`; without it they skip, unless `CYANOTYPE_REQUIRE_MODEL=1` (CI), where
//! a missing model is a failure.
//!
//! Reference values come from `tests/fixtures/make_reference.py` (Pillow + onnxruntime + tokenizers).
use std::{path::PathBuf, sync::Arc};

use cyanotype_clip::{DIM, ImageEncoder, TextEncoder, cosine, preprocess_rgb8};
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn model_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("CLIP_MODEL_DIR").map(PathBuf::from);
    let ok = dir.as_ref().is_some_and(|d| {
        [
            cyanotype_clip::VISION_FILE,
            cyanotype_clip::TEXT_FILE,
            cyanotype_clip::TOKENIZER_FILE,
        ]
        .iter()
        .all(|f| d.join(f).exists())
    });
    if ok {
        return dir;
    }
    assert!(
        std::env::var_os("CYANOTYPE_REQUIRE_MODEL").is_none(),
        "CYANOTYPE_REQUIRE_MODEL is set but CLIP_MODEL_DIR has no model files"
    );
    eprintln!("skipping: CLIP_MODEL_DIR does not hold the model files");
    None
}

fn floats(v: &Value) -> Vec<f32> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap() as f32)
        .collect()
}

fn reference() -> Value {
    serde_json::from_slice(&std::fs::read(fixtures().join("reference.json")).unwrap()).unwrap()
}

fn embed_fixture(enc: &ImageEncoder, name: &str) -> Vec<f32> {
    let img = image::open(fixtures().join(format!("images/{name}.jpg")))
        .unwrap()
        .to_rgb8();
    let pre = preprocess_rgb8(img.width(), img.height(), img.as_raw()).unwrap();
    enc.embed(&[pre]).unwrap().remove(0)
}

#[test]
fn tokenizer_matches_the_reference_ids_including_truncation() {
    let Some(dir) = model_dir() else { return };
    let text = TextEncoder::load(&dir, 1).unwrap();
    let r = reference();
    for (t, v) in r["texts"].as_object().unwrap() {
        let want: Vec<i64> = v["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_i64().unwrap())
            .collect();
        assert_eq!(text.tokenize(t).unwrap(), want, "token ids of {t:?}");
    }
    let want: Vec<i64> = r["long_text_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap())
        .collect();
    let got = text.tokenize(&"word ".repeat(100)).unwrap();
    assert_eq!(got.len(), cyanotype_clip::CONTEXT_LENGTH);
    assert_eq!(got, want);
    assert_eq!(*got.last().unwrap(), 49407);
}

#[test]
fn text_embeddings_match_the_reference() {
    let Some(dir) = model_dir() else { return };
    let text = TextEncoder::load(&dir, 1).unwrap();
    for (t, v) in reference()["texts"].as_object().unwrap() {
        let got = text.embed(t).unwrap();
        assert_eq!(got.len(), DIM);
        let want = floats(&v["embedding"]);
        let c = cosine(&got, &want);
        assert!(c > 0.998, "text {t:?}: cosine {c} against the reference");
    }
}

#[test]
fn image_embeddings_match_the_reference() {
    let Some(dir) = model_dir() else { return };
    let enc = ImageEncoder::load(&dir, 1).unwrap();
    for (name, want) in reference()["images"].as_object().unwrap() {
        let got = embed_fixture(&enc, name);
        let c = cosine(&got, &floats(want));
        // Same model and graph; the only difference is Pillow's 8-bit bicubic resampling against ours. The int8 model amplifies
        // that (it is also only ~0.96 cosine against the fp32 model), so this is a parity check, not an equality one.
        assert!(c > 0.985, "image {name}: cosine {c} against the reference");
    }
}

#[test]
fn identical_queries_come_from_the_cache() {
    let Some(dir) = model_dir() else { return };
    let text = TextEncoder::load(&dir, 1).unwrap();
    let a = text.embed("Dog  on the beach").unwrap();
    let b = text.embed("dog on the beach ").unwrap();
    assert!(
        Arc::ptr_eq(&a, &b),
        "case and whitespace variants share one cache entry"
    );
    assert!(!Arc::ptr_eq(&a, &text.embed("cat on the beach").unwrap()));
    assert!(text.embed("   ").is_err());
}

/// 20 CC0 photos (Wikimedia Commons, `images/SOURCES.json`) across clear categories, two queries each: the right photo
/// must be in the top 3 of 20. Prints the recall so a model swap shows what it did to search quality.
#[test]
fn text_queries_rank_the_right_photo_in_the_top_three() {
    let Some(dir) = model_dir() else { return };
    let enc = ImageEncoder::load(&dir, 2).unwrap();
    let text = TextEncoder::load(&dir, 2).unwrap();
    let queries: Value = serde_json::from_slice(&std::fs::read(fixtures().join("queries.json")).unwrap()).unwrap();
    let names: Vec<&String> = queries.as_object().unwrap().keys().collect();
    let images: Vec<Vec<f32>> = names.iter().map(|n| embed_fixture(&enc, n)).collect();
    let (mut top1, mut top3, mut total) = (0, 0, 0);
    let mut misses = Vec::new();
    for (i, name) in names.iter().enumerate() {
        for q in queries[name.as_str()].as_array().unwrap() {
            let q = q.as_str().unwrap();
            let t = text.embed(q).unwrap();
            let mut order: Vec<usize> = (0..images.len()).collect();
            order.sort_by(|&a, &b| cosine(&t, &images[b]).total_cmp(&cosine(&t, &images[a])));
            let rank = order.iter().position(|&o| o == i).unwrap();
            total += 1;
            top1 += usize::from(rank == 0);
            top3 += usize::from(rank < 3);
            if rank >= 3 {
                misses.push(format!("{q:?} ranked {name} #{}", rank + 1));
            }
        }
    }
    eprintln!("recall over {total} queries: top-1 {top1}/{total}, top-3 {top3}/{total}");
    assert!(misses.is_empty(), "missed the top 3: {misses:?}");
    assert!(top1 * 10 >= total * 9, "top-1 recall {top1}/{total} below 90%");
}
