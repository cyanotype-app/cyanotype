//! The face-model bake-off behind docs/faces.md: how well do the candidate models tell people apart, and how small a face can the
//! detector still find and the recognizer still match?
//!
//!   scripts/fetch-face-eval-set.sh models/face-eval        # the 8 public-domain photos per person (tests/eval/faces.tsv)
//!   MODELS=faces scripts/fetch-models.sh                   # the shipped models; LOCK=models/faces-bakeoff.lock for the other candidates
//!   FACES_MODEL_DIR=models/faces FACES_EVAL_DIR=models/face-eval \
//!     cargo test --release -p cyanotype-faces --test bakeoff -- --ignored --nocapture
//!
//! Three measurements, each printed as a table:
//!  1. Verification: the largest face of every photo is fingerprinted; every pair of photos of one person is a genuine pair and
//!     every pair of different people an impostor pair. The true-accept rate (TAR) is read at a fixed false-match rate (FMR).
//!  2. Small faces: portraits are cut down to a face of a chosen size, pasted onto 1440 x 1080 canvases (the size of a stored
//!     preview) with a synthetic textured background, and the detector is asked to find them at several input sizes. Recall is
//!     measured against the box the same detector found on the full-size photo, so what it says is "how much does shrinking the
//!     face cost", and each found face is fingerprinted and compared with the full-size photo's fingerprint.
//!  3. Speed per call and false detections on canvases without faces are in `examples/bench.rs`.
//!
//! Skipped (the test passes at once) unless both directories are set.
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use cyanotype_faces::{Detection, Detector, DetectorConfig, Recognizer, align, cosine, iou};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};

const LIST: &str = include_str!("eval/faces.tsv");
/// The longest side a photo is analysed at: the evaluation photos are stored at their originals, the app stores previews of up
/// to 1440 px; 960 is what the Commons thumbnails the set was chosen with were.
const PHOTO_LONG_EDGE: u32 = 960;

struct Photo {
    file: String,
    person: String,
    w: u32,
    h: u32,
    rgb: Vec<u8>,
}

fn resize(rgb: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let src = ImageRef::new(w, h, rgb, PixelType::U8x3).unwrap();
    let mut dst = Image::new(nw, nh, PixelType::U8x3);
    let alg = if nw < w {
        ResizeAlg::Convolution(FilterType::Bilinear)
    } else {
        ResizeAlg::Convolution(FilterType::CatmullRom)
    };
    Resizer::new()
        .resize(&src, &mut dst, &ResizeOptions::new().resize_alg(alg))
        .unwrap();
    dst.into_vec()
}

fn load_photos(dir: &Path) -> Vec<Photo> {
    let mut out = Vec::new();
    for line in LIST.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let img = image::open(dir.join(f[0]))
            .unwrap_or_else(|e| panic!("{}: {e}", f[0]))
            .to_rgb8();
        let (w, h) = img.dimensions();
        let long = w.max(h);
        let (rgb, w, h) = if long > PHOTO_LONG_EDGE {
            let s = PHOTO_LONG_EDGE as f32 / long as f32;
            let (nw, nh) = (
                ((w as f32 * s).round() as u32).max(1),
                ((h as f32 * s).round() as u32).max(1),
            );
            (resize(img.as_raw(), w, h, nw, nh), nw, nh)
        } else {
            (img.into_raw(), w, h)
        };
        out.push(Photo {
            file: f[0].to_string(),
            person: f[1].to_string(),
            w,
            h,
            rgb,
        });
    }
    out
}

/// The strongest, largest face.
fn main_face(dets: &[Detection]) -> Option<&Detection> {
    dets.iter()
        .max_by(|a, b| (a.score * a.area()).total_cmp(&(b.score * b.area())))
}

/// The smallest threshold-free summary: the TAR at a false-match rate, with the threshold that gives it.
fn tar_at_fmr(genuine: &[f32], impostor: &[f32], fmr: f64) -> (f64, f32) {
    let mut imp = impostor.to_vec();
    imp.sort_by(|a, b| b.total_cmp(a));
    let allowed = (fmr * imp.len() as f64).floor() as usize;
    // At most `allowed` impostor pairs may score above the threshold.
    let thr = imp.get(allowed).copied().unwrap_or(f32::NEG_INFINITY);
    let tar = genuine.iter().filter(|g| **g > thr).count() as f64 / genuine.len() as f64;
    (tar, thr)
}

fn auc(genuine: &[f32], impostor: &[f32]) -> f64 {
    let mut imp = impostor.to_vec();
    imp.sort_by(f32::total_cmp);
    let mut wins = 0.0;
    for g in genuine {
        let below = imp.partition_point(|x| x < g);
        let upto = imp.partition_point(|x| x <= g);
        wins += below as f64 + 0.5 * (upto - below) as f64;
    }
    wins / (genuine.len() as f64 * imp.len() as f64)
}

struct Pairs {
    genuine: Vec<f32>,
    impostor: Vec<f32>,
}

fn pairs(people: &[&str], emb: &[Vec<f32>]) -> Pairs {
    let (mut genuine, mut impostor) = (Vec::new(), Vec::new());
    for i in 0..emb.len() {
        for j in i + 1..emb.len() {
            let c = cosine(&emb[i], &emb[j]);
            if people[i] == people[j] {
                genuine.push(c);
            } else {
                impostor.push(c);
            }
        }
    }
    Pairs { genuine, impostor }
}

// ---- synthetic canvases --------------------------------------------------------------------------------------------------

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}

/// A smooth, cluttered, face-free background: coarse random colour cells interpolated up, plus fine noise.
fn background(w: u32, h: u32, seed: u64) -> Vec<u8> {
    let mut rng = Lcg(seed);
    let (gw, gh) = (36u32, 27u32);
    let coarse: Vec<u8> = (0..gw * gh * 3).map(|_| (50.0 + rng.next() * 150.0) as u8).collect();
    let mut px = resize(&coarse, gw, gh, w, h);
    for v in px.iter_mut() {
        *v = (f32::from(*v) + (rng.next() - 0.5) * 24.0).clamp(0.0, 255.0) as u8;
    }
    px
}

fn blit(canvas: &mut [u8], cw: u32, patch: &[u8], pw: u32, ph: u32, x: u32, y: u32) {
    for row in 0..ph {
        let dst = ((y + row) * cw + x) as usize * 3;
        let src = (row * pw) as usize * 3;
        canvas[dst..dst + pw as usize * 3].copy_from_slice(&patch[src..src + pw as usize * 3]);
    }
}

struct Source<'a> {
    photo: &'a Photo,
    det: Detection,
    embedding: Vec<f32>,
}

struct Pasted {
    source: usize,
    gt: [f32; 4],
}

const CANVAS_W: u32 = 1440;
const CANVAS_H: u32 = 1080;

/// A canvas with up to six faces of shortest side `size` px, one in each cell of a 3 x 2 grid, at a random place in its cell.
fn canvas(sources: &[Source], picks: &[usize], size: f32, seed: u64) -> (Vec<u8>, Vec<Pasted>) {
    let mut rng = Lcg(seed);
    let mut px = background(CANVAS_W, CANVAS_H, seed ^ 0x9e37);
    let mut pasted = Vec::new();
    let (cell_w, cell_h) = (CANVAS_W / 3, CANVAS_H / 2);
    for (cell, &si) in picks.iter().enumerate().take(6) {
        let s = &sources[si];
        let b = s.det.bbox;
        // The face and as much of its surroundings as the photo has, up to the face's own size on each side.
        let (x0, y0) = ((b[0] - b[2]).max(0.0), (b[1] - b[3]).max(0.0));
        let (x1, y1) = (
            (b[0] + 2.0 * b[2]).min(s.photo.w as f32),
            (b[1] + 2.0 * b[3]).min(s.photo.h as f32),
        );
        let (rw, rh) = ((x1 - x0).floor() as u32, (y1 - y0).floor() as u32);
        let scale = size / b[2].min(b[3]);
        let (pw, ph) = (
            ((rw as f32 * scale).round() as u32).max(2),
            ((rh as f32 * scale).round() as u32).max(2),
        );
        if pw >= cell_w || ph >= cell_h {
            continue;
        }
        let mut region = Vec::with_capacity((rw * rh * 3) as usize);
        for y in 0..rh {
            let o = ((y0 as u32 + y) * s.photo.w + x0 as u32) as usize * 3;
            region.extend_from_slice(&s.photo.rgb[o..o + rw as usize * 3]);
        }
        let patch = resize(&region, rw, rh, pw, ph);
        let (cx, cy) = ((cell as u32 % 3) * cell_w, (cell as u32 / 3) * cell_h);
        let px_x = cx + (rng.next() * (cell_w - pw) as f32) as u32;
        let px_y = cy + (rng.next() * (cell_h - ph) as f32) as u32;
        blit(&mut px, CANVAS_W, &patch, pw, ph, px_x, px_y);
        pasted.push(Pasted {
            source: si,
            gt: [
                (b[0] - x0) * scale + px_x as f32,
                (b[1] - y0) * scale + px_y as f32,
                b[2] * scale,
                b[3] * scale,
            ],
        });
    }
    (px, pasted)
}

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

#[test]
#[ignore = "needs the models and the evaluation photos (see the top of this file)"]
fn bakeoff() {
    let (Some(eval), models) = (
        std::env::var_os("FACES_EVAL_DIR").map(PathBuf::from),
        cyanotype_faces::model_dir(),
    ) else {
        eprintln!("FACES_EVAL_DIR is not set: skipped");
        return;
    };
    if !cyanotype_faces::models_installed(&models) {
        eprintln!("no models in {}: skipped", models.display());
        return;
    }
    let photos = load_photos(&eval);
    let mut people: Vec<&str> = photos.iter().map(|p| p.person.as_str()).collect();
    people.sort_unstable();
    people.dedup();
    println!(
        "\n== evaluation set: {} photos of {} people",
        photos.len(),
        people.len()
    );

    let rss0 = rss_kb();
    let detector = Detector::load(&models.join("yunet.onnx"), 2, DetectorConfig::default()).unwrap();
    let mut variants: Vec<(String, Recognizer)> = Vec::new();
    for file in ["sface.onnx", "sface_int8.onnx"] {
        if models.join(file).exists() {
            variants.push((file.to_string(), Recognizer::load(&models.join(file), 2).unwrap()));
        }
    }
    println!(
        "resident memory: {} MB before loading, {} MB with the models loaded",
        rss0 / 1024,
        rss_kb() / 1024
    );

    // Detect once; every recognizer variant sees the same crops.
    let mut kept: Vec<(usize, Detection)> = Vec::new();
    let mut missed = Vec::new();
    for (i, p) in photos.iter().enumerate() {
        let dets = detector.detect(&p.rgb, p.w, p.h).unwrap();
        match main_face(&dets) {
            Some(d) => kept.push((i, *d)),
            None => missed.push(p.file.clone()),
        }
    }
    println!(
        "faces found in {} of {} photos (missed: {:?})",
        kept.len(),
        photos.len(),
        missed
    );
    let crops: Vec<Vec<u8>> = kept
        .iter()
        .map(|(i, d)| align::align(&photos[*i].rgb, photos[*i].w, photos[*i].h, &d.landmarks).unwrap())
        .collect();
    let who: Vec<&str> = kept.iter().map(|(i, _)| photos[*i].person.as_str()).collect();

    println!("\n== 1. verification (largest face per photo, aligned to the ArcFace template)");
    println!(
        "{:<22} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "recognizer", "genuine", "impostor", "TAR@1e-2", "TAR@1e-3", "thr@1e-3", "AUC"
    );
    let mut operating: Option<f32> = None;
    let mut embeddings_first: Vec<Vec<f32>> = Vec::new();
    for (name, rec) in &variants {
        let t = Instant::now();
        let emb: Vec<Vec<f32>> = crops.iter().map(|c| rec.embed(c).unwrap()).collect();
        let ms = t.elapsed().as_secs_f64() * 1000.0 / crops.len() as f64;
        let pr = pairs(&who, &emb);
        let (t2, _) = tar_at_fmr(&pr.genuine, &pr.impostor, 1e-2);
        let (t3, thr3) = tar_at_fmr(&pr.genuine, &pr.impostor, 1e-3);
        println!(
            "{:<22} {:>9} {:>9} {:>8.1}% {:>8.1}% {:>9.3} {:>9.4}   ({ms:.1} ms per face)",
            name,
            pr.genuine.len(),
            pr.impostor.len(),
            t2 * 100.0,
            t3 * 100.0,
            thr3,
            auc(&pr.genuine, &pr.impostor)
        );
        // At OpenCV's published SFace cosine threshold:
        let hit = |v: &[f32], thr: f32| v.iter().filter(|x| **x >= thr).count() as f64 / v.len() as f64;
        println!(
            "{:<22} at the published threshold 0.363: TAR {:.1}%, FMR {:.3}%",
            "",
            hit(&pr.genuine, 0.363) * 100.0,
            hit(&pr.impostor, 0.363) * 100.0
        );
        if operating.is_none() {
            // The worst genuine pairs and the best impostor pairs, by file: a mislabelled photo shows up here.
            let files: Vec<&str> = kept.iter().map(|(i, _)| photos[*i].file.as_str()).collect();
            let mut genu = Vec::new();
            let mut imp = Vec::new();
            for i in 0..emb.len() {
                for j in i + 1..emb.len() {
                    let c = cosine(&emb[i], &emb[j]);
                    if who[i] == who[j] {
                        genu.push((c, files[i], files[j]))
                    } else {
                        imp.push((c, files[i], files[j]))
                    }
                }
            }
            genu.sort_by(|a, b| a.0.total_cmp(&b.0));
            imp.sort_by(|a, b| b.0.total_cmp(&a.0));
            println!("  lowest genuine pairs: {:?}", &genu[..6.min(genu.len())]);
            println!("  highest impostor pairs: {:?}", &imp[..6.min(imp.len())]);
            operating = Some(thr3);
            embeddings_first = emb.clone();
        }
        // Channel-order sanity check: the same crops fed as BGR must be clearly worse if RGB is right.
        if name == "sface.onnx" {
            let swapped: Vec<Vec<f32>> = crops
                .iter()
                .map(|c| {
                    let mut s = c.clone();
                    s.as_chunks_mut::<3>().0.iter_mut().for_each(|p| p.swap(0, 2));
                    rec.embed(&s).unwrap()
                })
                .collect();
            let sp = pairs(&who, &swapped);
            let (t3s, _) = tar_at_fmr(&sp.genuine, &sp.impostor, 1e-3);
            println!(
                "{:<22} (check: the same crops as BGR give TAR@1e-3 {:.1}%)",
                "",
                t3s * 100.0
            );
        }
    }
    let thr = operating.unwrap_or(0.363);

    println!(
        "\n== 2. small faces on 1440 x 1080 canvases (six faces each; threshold for \"same person\" = {thr:.3}, the first recognizer's @1e-3)"
    );
    // Sources: photos whose face is sharp and confident at full size.
    let (rec0_name, rec0) = &variants[0];
    let sources: Vec<Source> = kept
        .iter()
        .zip(&embeddings_first)
        .filter(|((_, d), _)| d.score >= 0.9)
        .map(|((i, d), e)| Source {
            photo: &photos[*i],
            det: *d,
            embedding: e.clone(),
        })
        .collect();
    println!(
        "{} source faces (recognizer {rec0_name}); detector input sizes are the long edge of the canvas",
        sources.len()
    );
    let configs: Vec<(&str, DetectorConfig)> = vec![
        (
            "640",
            DetectorConfig {
                long_edge: 640,
                ..DetectorConfig::default()
            },
        ),
        (
            "960",
            DetectorConfig {
                long_edge: 960,
                ..DetectorConfig::default()
            },
        ),
        (
            "1280",
            DetectorConfig {
                long_edge: 1280,
                ..DetectorConfig::default()
            },
        ),
        (
            "1440",
            DetectorConfig {
                long_edge: 1440,
                ..DetectorConfig::default()
            },
        ),
        (
            "1920 (x1.33 up)",
            DetectorConfig {
                long_edge: 1920,
                max_scale: 2.0,
                ..DetectorConfig::default()
            },
        ),
    ];
    let detectors: Vec<Detector> = configs
        .iter()
        .map(|(_, c)| Detector::load(&models.join("yunet.onnx"), 2, *c).unwrap())
        .collect();
    let sizes = [12.0f32, 16.0, 20.0, 24.0, 32.0, 40.0, 48.0, 64.0, 96.0];
    let mut recall_rows = Vec::new();
    let mut match_rows = Vec::new();
    let mut ms_per_cfg = vec![0.0f64; configs.len()];
    let mut canvases_run = 0usize;
    for &size in &sizes {
        let mut found = vec![0usize; configs.len()];
        let mut total = vec![0usize; configs.len()];
        let mut false_dets = vec![0usize; configs.len()];
        let mut canv = 0usize;
        // Cosines of the found faces with their own full-size fingerprint, for the 960 configuration.
        let mut cos: Vec<f32> = Vec::new();
        // Sources whose face is big enough that shrinking it to `size` is a shrink.
        let usable: Vec<usize> = (0..sources.len())
            .filter(|i| sources[*i].det.bbox[2].min(sources[*i].det.bbox[3]) >= size)
            .collect();
        for (k, chunk) in usable.chunks(6).enumerate() {
            let (px, pasted) = canvas(&sources, chunk, size, 1000 + k as u64 + size as u64 * 7919);
            canv += 1;
            for (ci, det) in detectors.iter().enumerate() {
                let t = Instant::now();
                let dets = det.detect(&px, CANVAS_W, CANVAS_H).unwrap();
                ms_per_cfg[ci] += t.elapsed().as_secs_f64() * 1000.0;
                let mut used = vec![false; dets.len()];
                for p in &pasted {
                    total[ci] += 1;
                    let best = dets
                        .iter()
                        .enumerate()
                        .map(|(j, d)| (j, iou(&d.bbox, &p.gt)))
                        .filter(|(_, v)| *v >= 0.3)
                        .max_by(|a, b| a.1.total_cmp(&b.1));
                    if let Some((j, _)) = best {
                        found[ci] += 1;
                        used[j] = true;
                        if configs[ci].0 == "960"
                            && let Some(crop) = align::align(&px, CANVAS_W, CANVAS_H, &dets[j].landmarks)
                        {
                            let e = rec0.embed(&crop).unwrap();
                            cos.push(cosine(&e, &sources[p.source].embedding));
                        }
                    }
                }
                false_dets[ci] += used.iter().filter(|u| !**u).count();
            }
        }
        canvases_run += canv * configs.len();
        recall_rows.push((size, total[0], found.clone(), false_dets.clone(), canv));
        let n = cos.len().max(1) as f32;
        let mean = cos.iter().sum::<f32>() / n;
        let above = cos.iter().filter(|c| **c >= thr).count() as f32 / n;
        match_rows.push((size, cos.len(), mean, above));
    }
    let _ = canvases_run;
    print!("{:>7} {:>7}", "face px", "faces");
    for (n, _) in &configs {
        print!(" {:>16}", format!("recall @ {n}"));
    }
    println!();
    for (size, total, found, _, _) in &recall_rows {
        print!("{:>7} {:>7}", size, total);
        for f in found {
            print!(" {:>15.1}%", *f as f64 * 100.0 / (*total).max(1) as f64);
        }
        println!();
    }
    print!("{:>15}", "false dets/canv");
    for (ci, _) in configs.iter().enumerate() {
        let (f, c): (usize, usize) = recall_rows.iter().fold((0, 0), |a, r| (a.0 + r.3[ci], a.1 + r.4));
        print!(" {:>16.2}", f as f64 / c.max(1) as f64);
    }
    println!();
    print!("{:>15}", "ms per canvas");
    for (ci, _) in configs.iter().enumerate() {
        let c: usize = recall_rows.iter().map(|r| r.4).sum();
        print!(" {:>16.0}", ms_per_cfg[ci] / c.max(1) as f64);
    }
    println!("   (2 threads, includes this run's contention)");
    println!("\nfingerprint of a found small face against the same face at full size (detector input 960):");
    println!(
        "{:>7} {:>7} {:>10} {:>14}",
        "face px",
        "faces",
        "mean cos",
        format!("share >= {thr:.2}")
    );
    for (size, n, mean, above) in &match_rows {
        println!("{:>7} {:>7} {:>10.3} {:>13.1}%", size, n, mean, above * 100.0);
    }
    println!("\nresident memory at the end: {} MB", rss_kb() / 1024);
}
