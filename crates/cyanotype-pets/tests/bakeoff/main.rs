//! The comparison behind `docs/pets.md`: which photos contain a dog or a cat, found by
//!
//!  * object detectors with COCO's dog and cat classes, alone: YOLOX-s, NanoDet-Plus, D-FINE-N and RT-DETR-R18,
//!  * zero-shot SigLIP 2, the search model's own image embedding read against text prompts (no new model, no boxes): the probability of
//!    the best prompt, the share among the prompts of the check (`cyanotype_clip::zeroshot`, as shipped), and two ablations of the
//!    negative prompts (what happens when the prompts do not know the evaluation set's other animals),
//!  * the shipped pipeline: the check says which kinds are in the photo, D-FINE-N puts the boxes on them (and runs only for those photos),
//!
//! on `tests/eval/pets.tsv`: Public domain and CC0 photos from Wikimedia Commons, labelled by eye. Prints precision and recall per
//! candidate, kind and threshold, the time per photo and the process's peak memory. Asserts nothing about the numbers (they depend on the
//! set): it is for looking at.
//!
//! ```text
//! scripts/fetch-pets-eval-set.sh                    # the photos -> models/pets-eval
//! scripts/fetch-models.sh                           # SigLIP 2 -> models/siglip2, D-FINE-N -> models/pets
//! LOCK=models/pets-bakeoff.lock scripts/fetch-models.sh models/pets-bakeoff
//! PETS_EVAL_DIR=models/pets-eval PETS_BAKEOFF_DIR=models/pets-bakeoff SEARCH_MODEL_DIR=$PWD/models \
//!   cargo test --release -p cyanotype-pets --test bakeoff -- --ignored --nocapture
//! ```
//!
//! `PETS_CANDIDATES=dfine,rtdetr` runs only some (yolox, nanodet, dfine, rtdetr, siglip, pipeline; the peak memory is the process's, so
//! measure one at a time); `PETS_THREADS` (default 2) sets the intra-op threads; `PETS_DUMP=file.jsonl` writes every photo's scores for
//! further analysis.
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::Result;
use candidates::{CAT, Candidate, DOG, Detr, NanoDet, Yolox};
use cyanotype_clip::{
    ImageEncoder, Model, TextEncoder, cosine, preprocess_rgb8,
    zeroshot::{self, PetCheck},
};
use cyanotype_pets::{Detector, DetectorConfig, Kind};

mod candidates;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Photo {
    file: String,
    size: (u32, u32),
    dog: bool,
    cat: bool,
    group: String,
    note: String,
}

fn photos() -> Vec<Photo> {
    let list = std::fs::read_to_string(repo().join("crates/cyanotype-pets/tests/eval/pets.tsv")).unwrap();
    list.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Photo {
                file: f[0].to_string(),
                size: (f[4].parse().unwrap(), f[5].parse().unwrap()),
                dog: matches!(f[1], "dog" | "both"),
                cat: matches!(f[1], "cat" | "both"),
                group: f[2].to_string(),
                note: if f[3] == "-" { String::new() } else { f[3].to_string() },
            }
        })
        .collect()
}

/// The photo as the worker's preview would be: upright (the file's EXIF orientation applied; the sizes in pets.tsv are of the upright
/// picture) and at most 1440 px on the long edge.
fn load(dir: &Path, file: &str, size: (u32, u32)) -> (Vec<u8>, u32, u32) {
    let open = || -> image::ImageResult<image::DynamicImage> {
        let mut decoder = image::ImageReader::open(dir.join(file))?
            .with_guessed_format()?
            .into_decoder()?;
        let orientation = image::ImageDecoder::orientation(&mut decoder)?;
        let mut img = image::DynamicImage::from_decoder(decoder)?;
        img.apply_orientation(orientation);
        Ok(img)
    };
    let img = open().unwrap_or_else(|e| panic!("{file}: {e}")).to_rgb8();
    let (w, h) = img.dimensions();
    assert_eq!((w, h), size, "{file} is not the pinned photo");
    let long = w.max(h);
    if long <= 1440 {
        return (img.into_raw(), w, h);
    }
    let s = 1440.0 / long as f32;
    let (nw, nh) = (
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
    );
    let rgb = candidates::resize_rgb(img.as_raw(), w, h, nw, nh, fast_image_resize::FilterType::Lanczos3).unwrap();
    (rgb, nw, nh)
}

fn peak_rss_mb() -> f64 {
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

fn threads() -> usize {
    std::env::var("PETS_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2)
}

fn size_mb(p: &Path) -> f64 {
    std::fs::metadata(p).map(|m| m.len() as f64 / 1048576.0).unwrap_or(0.0)
}

/// What a candidate says about a photo: the strongest dog and cat evidence, 0..1.
#[derive(Clone, Copy, Default)]
struct Verdict {
    dog: f32,
    cat: f32,
}

/// One photo, and what has been worked out about it so far.
struct Ctx {
    rgb: Vec<u8>,
    w: u32,
    h: u32,
    /// The search model's embedding of the photo: what the library already has when the pets job runs (so it is not part of any
    /// candidate's time; it is timed once, apart).
    embedding: Option<Vec<f32>>,
}

trait Scorer {
    fn name(&self) -> String;
    fn load_ms(&self) -> f64;
    fn file_mb(&self) -> f64;
    fn grid(&self) -> Vec<f32>;
    fn score(&self, c: &Ctx) -> Verdict;
    /// Called once between the warm-up run and the first measured one.
    fn reset(&self) {}
    /// Extra lines for the report, once all photos are scored.
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }
}

struct DetectorScorer {
    label: &'static str,
    inner: Box<dyn Candidate>,
    load_ms: f64,
    mb: f64,
}

impl Scorer for DetectorScorer {
    fn name(&self) -> String {
        self.label.to_string()
    }
    fn load_ms(&self) -> f64 {
        self.load_ms
    }
    fn file_mb(&self) -> f64 {
        self.mb
    }
    fn grid(&self) -> Vec<f32> {
        vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]
    }
    fn score(&self, c: &Ctx) -> Verdict {
        let dets = self.inner.run(&c.rgb, c.w, c.h).unwrap();
        let best = |class: usize| {
            dets.iter()
                .filter(|d| d.class == class)
                .map(|d| d.score)
                .fold(0.0, f32::max)
        };
        Verdict {
            dog: best(DOG),
            cat: best(CAT),
        }
    }
}

/// A set of prompts read against the photo's embedding.
struct Prompts {
    label: String,
    dog: Vec<Vec<f32>>,
    cat: Vec<Vec<f32>>,
    other: Vec<Vec<f32>>,
    /// false: the probability of the best positive prompt; true: the share among all the prompts.
    share: bool,
}

impl Scorer for Prompts {
    fn name(&self) -> String {
        self.label.clone()
    }
    fn load_ms(&self) -> f64 {
        0.0
    }
    fn file_mb(&self) -> f64 {
        0.0
    }
    fn grid(&self) -> Vec<f32> {
        if self.share {
            vec![0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.4, 0.5, 0.6, 0.8]
        } else {
            vec![0.0005, 0.001, 0.003, 0.01, 0.02, 0.05, 0.1, 0.2]
        }
    }
    fn score(&self, c: &Ctx) -> Verdict {
        let v = c.embedding.as_ref().unwrap();
        let cal = Model::Siglip2B16.calibration().unwrap();
        let cos = |t: &Vec<f32>| cosine(v, t);
        if !self.share {
            let best = |ts: &[Vec<f32>]| ts.iter().map(cos).fold(f32::MIN, f32::max);
            return Verdict {
                dog: cal.probability(best(&self.dog)),
                cat: cal.probability(best(&self.cat)),
            };
        }
        let all: Vec<f32> = self
            .dog
            .iter()
            .chain(&self.cat)
            .chain(&self.other)
            .map(|t| cal.scale * cos(t))
            .collect();
        let mx = all.iter().copied().fold(f32::MIN, f32::max);
        let sum: f32 = all.iter().map(|x| (x - mx).exp()).sum();
        let (nd, nc) = (self.dog.len(), self.cat.len());
        let part = |r: std::ops::Range<usize>| r.map(|i| (all[i] - mx).exp() / sum).sum::<f32>();
        Verdict {
            dog: part(0..nd),
            cat: part(nd..nd + nc),
        }
    }
}

/// The shipped check, through the shipped code.
struct Check(PetCheck);

impl Scorer for Check {
    fn name(&self) -> String {
        "SigLIP 2 zero-shot, share among the 20 prompts of the check (as shipped)".into()
    }
    fn load_ms(&self) -> f64 {
        0.0
    }
    fn file_mb(&self) -> f64 {
        0.0
    }
    fn grid(&self) -> Vec<f32> {
        vec![0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.4, 0.5, 0.6, 0.8]
    }
    fn score(&self, c: &Ctx) -> Verdict {
        let s = self.0.shares(c.embedding.as_ref().unwrap()).unwrap();
        Verdict { dog: s.dog, cat: s.cat }
    }
}

/// The shipped pipeline: the check, then the detector for the kinds it confirmed. The verdict is 1 for a kind with a box, else 0.
struct Pipeline {
    check: PetCheck,
    detector: Detector,
    load_ms: f64,
    mb: f64,
    /// Detector milliseconds of the photos it ran on, and how many flagged photos had no box.
    ran_ms: RefCell<Vec<f64>>,
    no_box: Cell<usize>,
    photos: Cell<usize>,
}

impl Scorer for Pipeline {
    fn name(&self) -> String {
        format!(
            "Pipeline as shipped: check (share >= {}) then D-FINE-N boxes (>= {})",
            zeroshot::MIN_SHARE,
            self.detector.config().min_score
        )
    }
    fn load_ms(&self) -> f64 {
        self.load_ms
    }
    fn file_mb(&self) -> f64 {
        self.mb
    }
    fn grid(&self) -> Vec<f32> {
        vec![0.5]
    }
    fn reset(&self) {
        self.ran_ms.borrow_mut().clear();
        self.no_box.set(0);
        self.photos.set(0);
    }
    fn score(&self, c: &Ctx) -> Verdict {
        self.photos.set(self.photos.get() + 1);
        let shares = self.check.shares(c.embedding.as_ref().unwrap()).unwrap();
        let (dog, cat) = shares.confirmed();
        if !dog && !cat {
            return Verdict::default();
        }
        let t = Instant::now();
        let found = self.detector.detect(&c.rgb, c.w, c.h).unwrap();
        self.ran_ms.borrow_mut().push(t.elapsed().as_secs_f64() * 1000.0);
        let has = |k: Kind, confirmed: bool| confirmed && found.iter().any(|d| d.kind == k);
        let v = Verdict {
            dog: f32::from(u8::from(has(Kind::Dog, dog))),
            cat: f32::from(u8::from(has(Kind::Cat, cat))),
        };
        if v.dog + v.cat == 0.0 {
            self.no_box.set(self.no_box.get() + 1);
        }
        v
    }
    fn notes(&self) -> Vec<String> {
        let mut ms = self.ran_ms.borrow().clone();
        ms.sort_by(f64::total_cmp);
        let n = self.photos.get().max(1);
        let mean = ms.iter().sum::<f64>() / ms.len().max(1) as f64;
        let p95 = ms
            .get(((ms.len() as f64 * 0.95) as usize).saturating_sub(1))
            .copied()
            .unwrap_or(0.0);
        vec![format!(
            "the check flagged {} of {n} photos ({:.0} %; this set is half pets, a family library has far fewer) and the detector ran on those: mean {mean:.0} ms, p95 {p95:.0} ms; {} flagged photos had no box and were dropped",
            ms.len(),
            100.0 * ms.len() as f64 / n as f64,
            self.no_box.get()
        )]
    }
}

struct Run {
    scorer: Box<dyn Scorer>,
    verdicts: Vec<Verdict>,
    ms: Vec<f64>,
}

/// Precision, recall, f1 at a threshold, and the counts.
fn prf(scores: &[(f32, bool)], t: f32) -> (usize, usize, usize, f64, f64, f64) {
    let (mut tp, mut fp, mut fneg) = (0, 0, 0);
    for (s, truth) in scores {
        match (*s >= t, *truth) {
            (true, true) => tp += 1,
            (true, false) => fp += 1,
            (false, true) => fneg += 1,
            _ => {}
        }
    }
    let p = if tp + fp == 0 {
        1.0
    } else {
        tp as f64 / (tp + fp) as f64
    };
    let r = if tp + fneg == 0 {
        1.0
    } else {
        tp as f64 / (tp + fneg) as f64
    };
    let f = if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) };
    (tp, fp, fneg, p, r, f)
}

/// Average precision of the ranking by score (threshold-free).
fn average_precision(scores: &[(f32, bool)]) -> f64 {
    let mut s = scores.to_vec();
    s.sort_by(|a, b| b.0.total_cmp(&a.0));
    let total = s.iter().filter(|x| x.1).count().max(1);
    let (mut hits, mut ap) = (0, 0.0);
    for (i, (_, t)) in s.iter().enumerate() {
        if *t {
            hits += 1;
            ap += hits as f64 / (i + 1) as f64;
        }
    }
    ap / total as f64
}

/// The threshold with the best F1 on `scores` (the candidates are the scores themselves).
fn best_threshold(scores: &[(f32, bool)]) -> f32 {
    scores
        .iter()
        .map(|(s, _)| *s)
        .filter(|s| *s > 0.0)
        .max_by(|a, b| prf(scores, *a).5.total_cmp(&prf(scores, *b).5))
        .unwrap_or(0.5)
}

fn wanted(only: &Option<Vec<String>>, key: &str) -> bool {
    only.as_ref().is_none_or(|o| o.iter().any(|x| x == key))
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let v = f();
    (v, t.elapsed().as_secs_f64() * 1000.0)
}

#[test]
#[ignore = "needs the evaluation photos and the models (see the header)"]
fn pets_bakeoff() {
    let eval = std::env::var_os("PETS_EVAL_DIR").map_or_else(|| repo().join("models/pets-eval"), PathBuf::from);
    let mdir = std::env::var_os("PETS_BAKEOFF_DIR").map_or_else(|| repo().join("models/pets-bakeoff"), PathBuf::from);
    let set = photos();
    assert!(
        eval.join(&set[0].file).exists(),
        "no evaluation photos in {}",
        eval.display()
    );
    let only: Option<Vec<String>> = std::env::var("PETS_CANDIDATES")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());
    let th = threads();

    let mut scorers: Vec<Box<dyn Scorer>> = Vec::new();
    let mut detector =
        |key: &str, label: &'static str, file: PathBuf, make: &dyn Fn(&Path) -> Result<Box<dyn Candidate>>| {
            if wanted(&only, key) && file.exists() {
                let (inner, load_ms) = timed(|| make(&file));
                match inner {
                    Ok(inner) => scorers.push(Box::new(DetectorScorer {
                        label,
                        inner,
                        load_ms,
                        mb: size_mb(&file),
                    })),
                    Err(e) => println!("{label}: does not load in ONNX Runtime ({e:#})"),
                }
            }
        };
    detector("yolox", "YOLOX-s fp32", mdir.join("yolox-s.onnx"), &|p| {
        Ok(Box::new(Yolox::load(p, th)?))
    });
    detector(
        "nanodet",
        "NanoDet-Plus-m-1.5x fp32",
        mdir.join("nanodet-plus.onnx"),
        &|p| Ok(Box::new(NanoDet::load(p, th)?)),
    );
    detector(
        "dfine",
        "D-FINE-N fp32 (as shipped)",
        cyanotype_pets::model_dir().join(cyanotype_pets::DETECTOR_FILE),
        &|p| Ok(Box::new(Detr::load(p, th)?)),
    );
    detector("rtdetr", "RT-DETR-R18vd fp32", mdir.join("rtdetr-r18vd.onnx"), &|p| {
        Ok(Box::new(Detr::load(p, th)?))
    });

    // The search model's image tower (only to make the embeddings the library would already hold) and its text tower (only for the
    // prompts of the ablations; the shipped check reads its committed vectors).
    let siglip = Model::Siglip2B16;
    let wants_sig = wanted(&only, "siglip") || wanted(&only, "pipeline");
    let encoder = wants_sig.then(|| ImageEncoder::load(siglip, &siglip.dir(), th).unwrap());
    if wanted(&only, "siglip") {
        let text = TextEncoder::load(siglip, &siglip.dir(), th).unwrap();
        let emb = |ps: &[&str]| -> Vec<Vec<f32>> { ps.iter().map(|p| (*text.embed(p).unwrap()).clone()).collect() };
        let prompts = |label: &str, other: &[&str], share: bool| -> Box<dyn Scorer> {
            Box::new(Prompts {
                label: label.to_string(),
                dog: emb(zeroshot::DOG_PROMPTS),
                cat: emb(zeroshot::CAT_PROMPTS),
                other: emb(other),
                share,
            })
        };
        scorers.push(prompts(
            "SigLIP 2 zero-shot, probability of the best dog or cat prompt",
            &[],
            false,
        ));
        scorers.push(Box::new(Check(PetCheck::for_model(siglip).unwrap())));
        // Ablations: the check's other prompts, with less and less of what this evaluation set holds.
        let generic = [
            "a photo of a person",
            "a photo of a landscape",
            "a photo of a street",
            "a photo of a room",
            "a photo of food",
        ];
        scorers.push(prompts(
            "ablation: share among the dog and cat prompts and only 5 generic ones (person, landscape, street, room, food)",
            &generic,
            true,
        ));
        let no_art: Vec<&str> = zeroshot::OTHER_PROMPTS
            .iter()
            .copied()
            .filter(|p| !p.contains("toy") && !p.contains("statue") && !p.contains("painting"))
            .collect();
        scorers.push(prompts(
            "ablation: share without the toy, statue and painting prompts",
            &no_art,
            true,
        ));
    }
    if wanted(&only, "pipeline") {
        let dir = cyanotype_pets::model_dir();
        let (detector, load_ms) = timed(|| Detector::load(&dir, th, DetectorConfig::default()).unwrap());
        scorers.push(Box::new(Pipeline {
            check: PetCheck::for_model(siglip).unwrap(),
            detector,
            load_ms,
            mb: size_mb(&dir.join(cyanotype_pets::DETECTOR_FILE)),
            ran_ms: Default::default(),
            no_box: Default::default(),
            photos: Default::default(),
        }));
    }
    let mut runs: Vec<Run> = scorers
        .into_iter()
        .map(|scorer| Run {
            scorer,
            verdicts: Vec::new(),
            ms: Vec::new(),
        })
        .collect();

    // The photos one at a time (so the peak memory is not the evaluation set's), a first untimed run per scorer to warm it up.
    let mut embed_ms: Vec<f64> = Vec::new();
    let mut warm = vec![false; runs.len()];
    for p in &set {
        let (rgb, w, h) = load(&eval, &p.file, p.size);
        let mut ctx = Ctx {
            rgb,
            w,
            h,
            embedding: None,
        };
        if let Some(enc) = &encoder {
            let pre = preprocess_rgb8(siglip, w, h, &ctx.rgb).unwrap();
            let (v, ms) = timed(|| enc.embed(&[pre]).unwrap().remove(0));
            ctx.embedding = Some(v);
            embed_ms.push(ms);
        }
        for (i, run) in runs.iter_mut().enumerate() {
            if !warm[i] {
                run.scorer.score(&ctx);
                run.scorer.reset();
                warm[i] = true;
            }
            let (v, ms) = timed(|| run.scorer.score(&ctx));
            run.ms.push(ms);
            run.verdicts.push(v);
        }
    }
    let peak = peak_rss_mb();

    if let Some(path) = std::env::var_os("PETS_DUMP") {
        let mut out = String::new();
        for (i, p) in set.iter().enumerate() {
            let mut scores = serde_json::Map::new();
            for r in &runs {
                scores.insert(
                    r.scorer.name(),
                    serde_json::json!([r.verdicts[i].dog, r.verdicts[i].cat]),
                );
            }
            out += &serde_json::json!({"file": p.file, "dog": p.dog, "cat": p.cat, "group": p.group, "note": p.note, "scores": scores})
                .to_string();
            out.push('\n');
        }
        std::fs::write(path, out).unwrap();
    }

    let n_dog = set.iter().filter(|p| p.dog).count();
    let n_cat = set.iter().filter(|p| p.cat).count();
    let n_pet = set.iter().filter(|p| p.dog || p.cat).count();
    println!(
        "\n{} photos: {n_dog} with a dog, {n_cat} with a cat, {n_pet} with either, {} with neither. Threads {}. Process peak RSS {peak:.0} MB.",
        set.len(),
        set.len() - n_pet,
        th
    );
    if !embed_ms.is_empty() {
        let mean = embed_ms.iter().sum::<f64>() / embed_ms.len() as f64;
        println!(
            "The search model's image embedding of each photo (already in the library when the check runs, not part of any time below): mean {mean:.0} ms."
        );
    }
    for r in &runs {
        let mut ms = r.ms.clone();
        ms.sort_by(f64::total_cmp);
        let mean = ms.iter().sum::<f64>() / ms.len() as f64;
        println!(
            "\n== {} ({:.1} MB on disk, load {:.0} ms; per photo mean {mean:.2} ms, median {:.2}, p95 {:.2})",
            r.scorer.name(),
            r.scorer.file_mb(),
            r.scorer.load_ms(),
            ms[ms.len() / 2],
            ms[(ms.len() as f64 * 0.95) as usize - 1]
        );
        for note in r.scorer.notes() {
            println!("   {note}");
        }
        type Target<'a> = Box<dyn Fn(usize) -> (f32, bool) + 'a>;
        let targets: [(&str, Target); 3] = [
            ("dog", Box::new(|i| (r.verdicts[i].dog, set[i].dog))),
            ("cat", Box::new(|i| (r.verdicts[i].cat, set[i].cat))),
            (
                "pet",
                Box::new(|i| (r.verdicts[i].dog.max(r.verdicts[i].cat), set[i].dog || set[i].cat)),
            ),
        ];
        let scored = |t: &Target| -> Vec<(f32, bool)> { (0..set.len()).map(t).collect() };
        println!(
            "   average precision: dog {:.3}  cat {:.3}  pet {:.3}",
            average_precision(&scored(&targets[0].1)),
            average_precision(&scored(&targets[1].1)),
            average_precision(&scored(&targets[2].1))
        );
        println!("   threshold |      dog P / R |      cat P / R |      pet P / R  (precision, recall)");
        for t in r.scorer.grid() {
            let mut line = format!("   {t:>9.4} |");
            for (_, f) in &targets {
                let (_, _, _, p, rc, _) = prf(&scored(f), t);
                line += &format!(" {:>5.1}% / {:>5.1}% |", p * 100.0, rc * 100.0);
            }
            println!("{line}");
        }
        // The threshold with the best F1 for the pet question, the groups it gets wrong, and how it holds up on photos it was not chosen on.
        let s = scored(&targets[2].1);
        let best = best_threshold(&s);
        let (tp, fp, fneg, p, rc, f) = prf(&s, best);
        println!(
            "   best F1 for pet at {best:.4}: tp {tp} fp {fp} fn {fneg}  P {:.1}% R {:.1}% F1 {f:.3}",
            p * 100.0,
            rc * 100.0
        );
        let even: Vec<(f32, bool)> = s.iter().copied().step_by(2).collect();
        let odd: Vec<(f32, bool)> = s.iter().copied().skip(1).step_by(2).collect();
        let held = (prf(&odd, best_threshold(&even)).5 + prf(&even, best_threshold(&odd)).5) / 2.0;
        println!("   F1 with the threshold chosen on one half and scored on the other (mean of both ways): {held:.3}");
        let mut by_group: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for (i, p) in set.iter().enumerate() {
            let (score, truth) = s[i];
            if (score >= best) != truth {
                let e = by_group.entry(p.group.as_str()).or_default();
                if truth { e.0 += 1 } else { e.1 += 1 }
            }
        }
        println!("   mistakes by group at that threshold (missed, false alarms): {by_group:?}");
        let mut by_note: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for (i, p) in set.iter().enumerate() {
            if p.note.is_empty() {
                continue;
            }
            let e = by_note.entry(p.note.as_str()).or_default();
            e.1 += 1;
            if (s[i].0 >= best) == s[i].1 {
                e.0 += 1;
            }
        }
        println!("   photos with a note, right at that threshold (right, of): {by_note:?}");
    }
}
