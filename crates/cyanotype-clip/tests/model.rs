//! Parity and quality tests against the real model files, for each model. They need the ONNX files and tokenizers (see
//! `scripts/fetch-models.sh`) in `SEARCH_MODEL_DIR/<model>` (`clip`, `siglip2`); a model without files is skipped, unless
//! `CYANOTYPE_REQUIRE_MODEL=1` (CI), where a missing model is a failure.
//!
//! Reference values come from `tests/fixtures/make_reference.py` (CLIP) and `make_reference_siglip2.py` (SigLIP 2):
//! Pillow + onnxruntime + tokenizers, on the same quantised graphs, and on the fp32 graphs of the same exports.
//!
//! What is compared with what, and why (the CLIP image test once failed in CI with cosine 0.9828 against a 0.985
//! threshold):
//! - **Preprocessing** is compared as a tensor (a 16 x 16 grid per channel) with the reference's. That is plain arithmetic
//!   on pixels, identical on every CPU, so the tolerance is tight and says nothing about the model.
//! - **Quantised graphs** (int8 dynamic quantisation: MatMulInteger on u8 x s8; 4-bit weights with int8 activations) give
//!   results that depend a little on which kernel ONNX Runtime picks for the CPU: AVX2 (no VNNI) multiplies u8 x s8 with a
//!   saturating 16-bit step, AVX-512 VNNI does not. The reference was produced on one machine; CI runners are a mix. The
//!   threshold against it is set for that spread. The thread count is not a factor (every session here runs one thread and
//!   ONNX Runtime's results do not change with it: checked by `embedding_does_not_depend_on_the_thread_count`).
//! - **The fp32 graph of the same export** is the CPU-independent anchor: a quantised embedding must stay close to it
//!   (measured: CLIP int8 0.90 to 0.96, SigLIP 2 q4 0.91 to 0.97, text int8 0.97 to 0.99), which also catches a preprocessing mistake (a swapped
//!   channel order or a wrong mean drops the cosine below 0.8).
use std::{path::PathBuf, sync::Arc};

use cyanotype_clip::{ImageEncoder, Model, TextEncoder, cosine, preprocess_rgb8};
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The model's directory if its files are there (a failure instead of a skip under `CYANOTYPE_REQUIRE_MODEL`).
fn model_dir(model: Model) -> Option<PathBuf> {
    let dir = model.dir();
    if [model.vision_file(), model.text_file(), model.tokenizer_file()]
        .iter()
        .all(|f| dir.join(f).exists())
    {
        return Some(dir);
    }
    assert!(
        std::env::var_os("CYANOTYPE_REQUIRE_MODEL").is_none(),
        "CYANOTYPE_REQUIRE_MODEL is set but {} has no files for {}",
        dir.display(),
        model.name()
    );
    eprintln!(
        "skipping: {} does not hold the {} model files",
        dir.display(),
        model.name()
    );
    None
}

fn floats(v: &Value) -> Vec<f32> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap() as f32)
        .collect()
}

fn reference(model: Model) -> Value {
    let file = match model {
        Model::ClipB32 => "reference.json",
        Model::Siglip2B16 => "reference_siglip2.json",
    };
    serde_json::from_slice(&std::fs::read(fixtures().join(file)).unwrap()).unwrap()
}

fn fixture_rgb(name: &str) -> image::RgbImage {
    image::open(fixtures().join(format!("images/{name}.jpg")))
        .unwrap()
        .to_rgb8()
}

fn preprocess_fixture(model: Model, name: &str) -> Vec<f32> {
    let img = fixture_rgb(name);
    preprocess_rgb8(model, img.width(), img.height(), img.as_raw()).unwrap()
}

fn embed_fixture(enc: &ImageEncoder, name: &str) -> Vec<f32> {
    enc.embed(&[preprocess_fixture(enc.model(), name)]).unwrap().remove(0)
}

fn ids_of(v: &Value) -> Vec<i64> {
    v.as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect()
}

// ---- preprocessing ---------------------------------------------------------------------------------------------------

fn check_preprocessing(model: Model) {
    // (needs no model file: the tensors are plain arithmetic)
    let r = reference(model);
    for (name, want) in r["images"].as_object().unwrap() {
        let got = preprocess_fixture(model, name);
        let sample = floats(&want["tensor_sample"]);
        let plane = 224 * 224;
        let mut worst = 0f32;
        for (i, w) in sample.iter().enumerate() {
            let (c, rest) = (i / 256, i % 256);
            let (y, x) = (7 + 14 * (rest / 16), 7 + 14 * (rest % 16));
            worst = worst.max((got[c * plane + y * 224 + x] - w).abs());
        }
        // One 8-bit level is 2/255 = 0.008 for SigLIP and 1/255/0.27 = 0.015 for CLIP; the resamplers round differently
        // (Pillow's fixed-point kernel against ours) and the JPEG decoders may differ by a level: a few levels at most.
        let tolerance = match model {
            Model::ClipB32 => 0.12,
            Model::Siglip2B16 => 0.06,
        };
        assert!(
            worst < tolerance,
            "{model:?} {name}: tensor differs from the reference by {worst}"
        );
    }
}

#[test]
fn clip_preprocessing_matches_the_reference_tensor() {
    check_preprocessing(Model::ClipB32);
}

#[test]
fn siglip_preprocessing_matches_the_reference_tensor() {
    check_preprocessing(Model::Siglip2B16);
}

// ---- tokenizers ------------------------------------------------------------------------------------------------------

#[test]
fn clip_tokenizer_matches_the_reference_ids_including_truncation() {
    let model = Model::ClipB32;
    let Some(dir) = model_dir(model) else { return };
    let text = TextEncoder::load(model, &dir, 1).unwrap();
    let r = reference(model);
    for (t, v) in r["texts"].as_object().unwrap() {
        assert_eq!(text.tokenize(t).unwrap(), ids_of(&v["ids"]), "token ids of {t:?}");
    }
    let want = ids_of(&r["long_text_ids"]);
    let got = text.tokenize(&"word ".repeat(100)).unwrap();
    assert_eq!(got.len(), model.context_length());
    assert_eq!(got, want);
    assert_eq!(*got.last().unwrap(), 49407);
}

#[test]
fn siglip_tokenizer_matches_the_reference_ids_lowercase_truncated_and_padded() {
    let model = Model::Siglip2B16;
    let Some(dir) = model_dir(model) else { return };
    let text = TextEncoder::load(model, &dir, 1).unwrap();
    let r = reference(model);
    for (t, v) in r["texts"].as_object().unwrap() {
        let got = text.tokenize(t).unwrap();
        assert_eq!(got.len(), 64, "always padded to the context length: {t:?}");
        assert_eq!(got, ids_of(&v["ids"]), "token ids of {t:?}");
    }
    let got = text.tokenize(&"word ".repeat(100)).unwrap();
    assert_eq!(got.len(), 64);
    assert_eq!(got, ids_of(&r["long_text_ids"]));
    assert_eq!(*got.last().unwrap(), 1, "truncated text ends with <eos>");
    // Lower-casing happens before tokenising (the tokenizer file alone would not do it).
    assert_eq!(
        text.tokenize("Dog On The BEACH").unwrap(),
        text.tokenize("dog on the beach").unwrap()
    );
    // The padding is <pad> (0) after the single <eos>.
    let ids = text.tokenize("dog").unwrap();
    let eos = ids.iter().position(|&i| i == 1).unwrap();
    assert!(ids[eos + 1..].iter().all(|&i| i == 0) && eos < 5);
}

// ---- embeddings ------------------------------------------------------------------------------------------------------

fn check_text_embeddings(model: Model) {
    let Some(dir) = model_dir(model) else { return };
    let text = TextEncoder::load(model, &dir, 1).unwrap();
    for (t, v) in reference(model)["texts"].as_object().unwrap() {
        let got = text.embed(t).unwrap();
        assert_eq!(got.len(), model.dim());
        let c = cosine(&got, &floats(&v["embedding"]));
        assert!(c > 0.95, "text {t:?}: cosine {c} against the reference quantised graph");
        if let Some(fp32) = v.get("embedding_fp32") {
            let c = cosine(&got, &floats(fp32));
            assert!(c > 0.9, "text {t:?}: cosine {c} against the fp32 graph");
        }
    }
}

#[test]
fn clip_text_embeddings_match_the_reference() {
    check_text_embeddings(Model::ClipB32);
}

#[test]
fn siglip_text_embeddings_match_the_reference() {
    check_text_embeddings(Model::Siglip2B16);
}

fn check_image_embeddings(model: Model) {
    let Some(dir) = model_dir(model) else { return };
    let enc = ImageEncoder::load(model, &dir, 1).unwrap();
    for (name, want) in reference(model)["images"].as_object().unwrap() {
        let got = embed_fixture(&enc, name);
        let vs_quantised = cosine(&got, &floats(&want["embedding"]));
        let vs_fp32 = cosine(&got, &floats(&want["embedding_fp32"]));
        let cpu = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("model name")).map(str::to_string))
            .unwrap_or_default();
        // 0.95: room for the CPU-dependent spread of the quantised kernels (0.983 was seen for CLIP on a CI runner against a
        // reference made elsewhere; see the module comment). The fp32 bound below is the one that does not depend on the CPU.
        assert!(
            vs_quantised > 0.95,
            "{model:?} image {name}: cosine {vs_quantised} against the reference quantised graph ({cpu})"
        );
        assert!(
            vs_fp32 > 0.88,
            "{model:?} image {name}: cosine {vs_fp32} against the fp32 graph ({cpu})"
        );
    }
}

#[test]
fn clip_image_embeddings_match_the_reference() {
    check_image_embeddings(Model::ClipB32);
}

#[test]
fn siglip_image_embeddings_match_the_reference() {
    check_image_embeddings(Model::Siglip2B16);
}

/// The hypothesis behind the CI flake was thread nondeterminism; this is the check that it is not that: the same input
/// through sessions with 1, 2 and 4 threads gives the same vector to float precision, and a photo's vector does not
/// depend on the photos embedded in the same call (it did, before `ImageEncoder::embed` ran them one by one: the 8-bit
/// graph scales its activations per batch).
#[test]
fn embedding_does_not_depend_on_the_thread_count() {
    for model in Model::ALL {
        let Some(dir) = model_dir(model) else { continue };
        let pre = preprocess_fixture(model, "dog");
        let one = ImageEncoder::load(model, &dir, 1)
            .unwrap()
            .embed(std::slice::from_ref(&pre))
            .unwrap()
            .remove(0);
        for threads in [2, 4] {
            let many = ImageEncoder::load(model, &dir, threads)
                .unwrap()
                .embed(std::slice::from_ref(&pre))
                .unwrap()
                .remove(0);
            let c = cosine(&one, &many);
            assert!(
                c > 0.9999,
                "{model:?}: {threads} threads give cosine {c} against 1 thread"
            );
        }
        let again = ImageEncoder::load(model, &dir, 1)
            .unwrap()
            .embed(std::slice::from_ref(&pre))
            .unwrap()
            .remove(0);
        assert_eq!(
            one, again,
            "{model:?}: the same session settings are bit-for-bit repeatable"
        );
        let batch = ImageEncoder::load(model, &dir, 1)
            .unwrap()
            .embed(&[pre.clone(), preprocess_fixture(model, "cat")])
            .unwrap();
        assert_eq!(
            one, batch[0],
            "{model:?}: a photo's vector depends on what it was batched with"
        );
    }
}

#[test]
fn identical_queries_come_from_the_cache() {
    for model in Model::ALL {
        let Some(dir) = model_dir(model) else { continue };
        let text = TextEncoder::load(model, &dir, 1).unwrap();
        let a = text.embed("Dog  on the beach").unwrap();
        let b = text.embed("dog on the beach ").unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "case and whitespace variants share one cache entry"
        );
        assert!(!Arc::ptr_eq(&a, &text.embed("cat on the beach").unwrap()));
        assert!(text.embed("   ").is_err());
    }
}

/// The text tower with its embedding table moved to a side file (what the Docker image ships, `src/split.rs`) must give
/// the same vectors as the file as downloaded.
#[test]
fn a_split_text_model_gives_the_same_vectors() {
    let model = Model::Siglip2B16;
    let Some(dir) = model_dir(model) else { return };
    let tmp = std::env::temp_dir().join(format!("cyanotype-split-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let original = std::fs::read(dir.join(model.text_file())).unwrap();
    let name = model.text_file();
    let split = cyanotype_clip::split::split(&original, &format!("{name}.data"), 32 << 20).unwrap();
    assert_eq!(split.moved, 1, "the 256,000 x 768 embedding table");
    assert!(split.model.len() < original.len() / 2);
    std::fs::write(tmp.join(name), &split.model).unwrap();
    std::fs::write(tmp.join(format!("{name}.data")), &split.data).unwrap();
    std::fs::copy(dir.join(model.tokenizer_file()), tmp.join(model.tokenizer_file())).unwrap();
    let plain = TextEncoder::load(model, &dir, 1).unwrap();
    let moved = TextEncoder::load(model, &tmp, 1).unwrap();
    for q in ["dog on the beach", "perro en la playa", "a photo of two people hiking"] {
        let c = cosine(&plain.embed(q).unwrap(), &moved.embed(q).unwrap());
        assert!(c > 0.99999, "{q:?}: {c}");
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}

// ---- search quality on the 20-photo smoke set ------------------------------------------------------------------------

/// 20 CC0 photos (Wikimedia Commons, `images/SOURCES.json`) across clear categories, two queries each: the right photo
/// must be in the top 3 of 20. Prints the recall so a model swap shows what it did to search quality. (The 209-photo
/// comparison between models is `tests/bakeoff.rs`.)
fn smoke(model: Model) -> (usize, usize, usize) {
    let Some(dir) = model_dir(model) else { return (0, 0, 0) };
    let enc = ImageEncoder::load(model, &dir, 2).unwrap();
    let text = TextEncoder::load(model, &dir, 2).unwrap();
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
    eprintln!(
        "{} recall over {total} queries: top-1 {top1}/{total}, top-3 {top3}/{total}",
        model.name()
    );
    assert!(misses.is_empty(), "{}: missed the top 3: {misses:?}", model.name());
    (top1, top3, total)
}

#[test]
fn clip_ranks_the_right_photo_in_the_top_three() {
    let (top1, _, total) = smoke(Model::ClipB32);
    assert!(top1 * 10 >= total * 9, "top-1 recall {top1}/{total} below 90%");
}

#[test]
fn siglip_ranks_the_right_photo_in_the_top_three() {
    let (top1, _, total) = smoke(Model::Siglip2B16);
    assert!(top1 * 10 >= total * 9, "top-1 recall {top1}/{total} below 90%");
}

/// The calibrated cut-off on the smoke set: a query the 20 photos have no answer to comes back without strong matches, a
/// good query has its photo among them.
#[test]
fn siglip_cutoff_finds_the_photo_and_returns_nothing_for_an_absent_subject() {
    let model = Model::Siglip2B16;
    let Some(dir) = model_dir(model) else { return };
    let enc = ImageEncoder::load(model, &dir, 2).unwrap();
    let text = TextEncoder::load(model, &dir, 2).unwrap();
    let queries: Value = serde_json::from_slice(&std::fs::read(fixtures().join("queries.json")).unwrap()).unwrap();
    let names: Vec<&String> = queries.as_object().unwrap().keys().collect();
    let images: Vec<Vec<f32>> = names.iter().map(|n| embed_fixture(&enc, n)).collect();
    let ranked = |q: &str| {
        let t = text.embed_query(q).unwrap();
        let mut scored: Vec<(f32, usize)> = images.iter().enumerate().map(|(i, v)| (cosine(&t, v), i)).collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
    };
    for q in ["a giraffe", "a submarine", "an astronaut", "a bowl of soup"] {
        let r = ranked(q);
        eprintln!("{q}: best cosine {:.3}", r[0].0);
        let t = cyanotype_clip::tiers(model, &r.iter().map(|x| x.0).collect::<Vec<_>>());
        assert_eq!(
            t.strong, 0,
            "{q}: {} strong matches among 20 unrelated photos",
            t.strong
        );
    }
    let (mut strong_hits, mut weak_hits) = (0, 0);
    for (i, name) in names.iter().enumerate() {
        let r = ranked(queries[name.as_str()][0].as_str().unwrap());
        let t = cyanotype_clip::tiers(model, &r.iter().map(|x| x.0).collect::<Vec<_>>());
        strong_hits += usize::from(r[..t.strong].iter().any(|x| x.1 == i));
        weak_hits += usize::from(r[..t.weak].iter().any(|x| x.1 == i));
        eprintln!(
            "{name}: its photo's cosine {:.3} (best {:.3}); {} strong, {} with weak",
            r.iter().find(|x| x.1 == i).unwrap().0,
            r[0].0,
            t.strong,
            t.weak
        );
    }
    eprintln!(
        "the photo is among the strong matches for {strong_hits}/20 keyword queries, among strong or weak for {weak_hits}/20"
    );
    // A bare keyword is the hardest query for a model trained on captions: most, not all, clear the strong floor, and the
    // weak tier ("Show weaker matches") has the rest.
    assert!(
        strong_hits >= 17,
        "{strong_hits}/20 keyword queries find their photo among the strong matches"
    );
    assert_eq!(weak_hits, 20);
}

/// SigLIP 2 is multilingual (CLIP's English tokenizer is not): the right photo of the 20 is in the top 3 for single words
/// in Spanish, French and German, and a Spanish sentence finds a dog or a beach.
#[test]
fn siglip_finds_photos_for_queries_in_other_languages() {
    let model = Model::Siglip2B16;
    let Some(dir) = model_dir(model) else { return };
    let enc = ImageEncoder::load(model, &dir, 2).unwrap();
    let text = TextEncoder::load(model, &dir, 2).unwrap();
    let names: Vec<&str> = {
        let q: Value = serde_json::from_slice(&std::fs::read(fixtures().join("queries.json")).unwrap()).unwrap();
        q.as_object()
            .unwrap()
            .keys()
            .map(|k| Box::leak(k.clone().into_boxed_str()) as &str)
            .collect()
    };
    let images: Vec<Vec<f32>> = names.iter().map(|n| embed_fixture(&enc, n)).collect();
    let top3 = |q: &str| -> Vec<&str> {
        let t = text.embed_query(q).unwrap();
        let mut order: Vec<usize> = (0..images.len()).collect();
        order.sort_by(|&a, &b| cosine(&t, &images[b]).total_cmp(&cosine(&t, &images[a])));
        order.iter().take(3).map(|&i| names[i]).collect()
    };
    let mut misses = Vec::new();
    for (q, want) in [
        ("perro", "dog"),
        ("gato", "cat"),
        ("playa", "beach"),
        ("pizza", "pizza"),
        ("caballo", "horse"),
        ("guitarra", "guitar"),
        ("chien", "dog"),
        ("plage", "beach"),
        ("voiture", "car"),
        ("Hund", "dog"),
        ("Strand", "beach"),
        ("Gitarre", "guitar"),
        ("Pferd", "horse"),
    ] {
        let got = top3(q);
        if !got.contains(&want) {
            misses.push(format!("{q:?} wants {want}, top 3 {got:?}"));
        }
    }
    eprintln!("other languages: {} misses of 13: {misses:?}", misses.len());
    assert!(misses.len() <= 1, "{misses:?}");
    let got = top3("perro en la playa");
    assert!(got.contains(&"dog") || got.contains(&"beach"), "{got:?}");
}
