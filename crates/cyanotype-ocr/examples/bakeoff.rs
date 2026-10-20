//! The OCR model bake-off (docs/build.md, "Text in photos"): runs one model variant over the 38 CC0 images of
//! `tests/bakeoff/images.tsv` and prints, per image, the share of the words a person can read that were found, the time,
//! and at the end the totals, the load time and the process's peak memory.
//!
//!   scripts/fetch-ocr-set.sh /root/repos/.cyanotype-eval/ocr
//!   OCR_VARIANT=v6 OCR_MODELS=/root/repos/.cyanotype-models/ocr-candidates OCR_SET=/root/repos/.cyanotype-eval/ocr \
//!     cargo run --release -p cyanotype-ocr --example bakeoff
//!
//! Variants: `v6` (PP-OCRv6 small detection + recognition, multilingual), `v3latin` (PP-OCRv3 mobile detection + PP-OCRv5
//! Latin recognition), `v5latin` (PP-OCRv5 detection + the same Latin recognition), `v3en` (v3 detection + English
//! recognition). OCR_DET_SIDE (default 960), OCR_THREADS (default 1), OCR_VERBOSE=1 prints what was read.
use std::{path::PathBuf, time::Instant};

use cyanotype_ocr::{Config, DetParams, Engine, Files, plain_text};

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn status_kb(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix(key).map(str::to_string)))
        .and_then(|v| v.split_whitespace().next().and_then(|n| n.parse().ok()))
        .unwrap_or(0)
}

/// Lower case with accents removed, for comparing what was read with what is written (Latin, Greek and Vietnamese marks).
fn fold(s: &str) -> String {
    const GROUPS: &[&str] = &[
        "aàáâãäåāăąạảấầẩẫậắằẳẵặ",
        "cçćĉċč",
        "dďđ",
        "eèéêëēĕėęěẹẻẽếềểễệ",
        "gĝğġģ",
        "hĥħ",
        "iìíîïĩīĭįıịỉ",
        "jĵ",
        "kķ",
        "lĺļľŀł",
        "nñńņňŉ",
        "oòóôõöøōŏőơọỏốồổỗộớờởỡợ",
        "rŕŗř",
        "sśŝşšș",
        "tţťŧț",
        "uùúûüũūŭůűųưụủứừửữự",
        "wŵ",
        "yýÿŷỳỵỷỹ",
        "zźżž",
        "αάἀἁ",
        "εέ",
        "ηή",
        "ιίϊΐ",
        "οό",
        "υύϋΰ",
        "ωώ",
    ];
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| {
            GROUPS
                .iter()
                .find(|g| g.chars().skip(1).any(|v| v == c))
                .and_then(|g| g.chars().next())
                .unwrap_or(c)
        })
        .collect()
}

fn edit_distance_le1(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (mut i, mut j, mut edits) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return false;
        }
        match a.len().cmp(&b.len()) {
            std::cmp::Ordering::Greater => i += 1,
            std::cmp::Ordering::Less => j += 1,
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
        }
    }
    edits + (a.len() - i) + (b.len() - j) <= 1
}

/// A word counts as found when it is a token of the folded text, or (five letters or more) inside it or one edit away from a token.
fn found(word: &str, text: &str) -> bool {
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || "|,;()[]\"'".contains(c))
        .filter(|t| !t.is_empty())
        .collect();
    let w = fold(word);
    if tokens.iter().any(|t| t.trim_matches(|c: char| ".:".contains(c)) == w) {
        return true;
    }
    w.chars().count() >= 5 && (text.contains(&w) || tokens.iter().any(|t| edit_distance_le1(t, &w)))
}

fn main() -> anyhow::Result<()> {
    let models = PathBuf::from(env("OCR_MODELS", "/root/repos/.cyanotype-models/ocr-candidates"));
    let set = PathBuf::from(env("OCR_SET", "/root/repos/.cyanotype-eval/ocr"));
    let variant = env("OCR_VARIANT", "v6");
    let threads: usize = env("OCR_THREADS", "1").parse()?;
    let side: u32 = env("OCR_DET_SIDE", "960").parse()?;
    let verbose = env("OCR_VERBOSE", "0") == "1";
    let (files, det) = match variant.as_str() {
        "v6" => (
            Files {
                det: models.join("v6/det.onnx"),
                rec: models.join("v6/rec.onnx"),
                dict: models.join("v6/rec.yml"),
            },
            DetParams {
                thresh: 0.2,
                box_thresh: 0.45,
                unclip: 1.4,
                ..DetParams::default()
            },
        ),
        "v3latin" => (
            Files {
                det: models.join("monkt/det_v3.onnx"),
                rec: models.join("monkt/latin_rec.onnx"),
                dict: models.join("monkt/latin_dict.txt"),
            },
            DetParams::default(),
        ),
        "v5latin" => (
            Files {
                det: models.join("monkt/det_v5.onnx"),
                rec: models.join("monkt/latin_rec.onnx"),
                dict: models.join("monkt/latin_dict.txt"),
            },
            DetParams {
                thresh: 0.3,
                box_thresh: 0.6,
                unclip: 1.5,
                ..DetParams::default()
            },
        ),
        "v3en" => (
            Files {
                det: models.join("monkt/det_v3.onnx"),
                rec: models.join("monkt/en_rec.onnx"),
                dict: models.join("monkt/en_dict.txt"),
            },
            DetParams::default(),
        ),
        other => anyhow::bail!("unknown OCR_VARIANT {other}"),
    };
    let cfg = Config {
        det: DetParams { side, ..det },
        threads,
        ..Config::default()
    };
    let rss0 = status_kb("VmRSS:");
    let t = Instant::now();
    let engine = Engine::load(&files, cfg)?;
    let load_ms = t.elapsed().as_millis();
    let rss_loaded = status_kb("VmRSS:");

    let tsv = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/bakeoff/images.tsv"))?;
    let mut by_cat: std::collections::BTreeMap<String, (usize, usize, f64, usize)> = Default::default();
    let (mut all_found, mut all_words, mut total_ms, mut n) = (0usize, 0usize, 0f64, 0usize);
    let mut times = Vec::new();
    let mut worst = (String::new(), 0u64);
    for line in tsv.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let (file, cat, words) = (f[0], f[1], f[5]);
        let img = image::open(set.join(file))?.to_rgb8();
        let (w, h) = img.dimensions();
        // Reset the high-water mark, so the peak below is this image's alone (what the worker's memory budget must cover).
        let _ = std::fs::write("/proc/self/clear_refs", "5");
        let t = Instant::now();
        let lines = engine.read(img.as_raw(), w, h)?;
        let peak = status_kb("VmHWM:") / 1024;
        if peak > worst.1 {
            worst = (file.to_string(), peak);
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let text = fold(&plain_text(&lines).replace('\n', " "));
        let want: Vec<&str> = words.split_whitespace().collect();
        let hit = want.iter().filter(|w| found(w, &text)).count();
        println!(
            "{file:<18} {cat:<10} {w:>4}x{h:<4} {:>3} lines {hit:>2}/{:<2} words {ms:>7.0} ms {peak:>4} MB",
            lines.len(),
            want.len()
        );
        if verbose {
            println!("    read: {}", plain_text(&lines).replace('\n', " | "));
            println!(
                "    missed: {}",
                want.iter()
                    .filter(|w| !found(w, &text))
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        let e = by_cat.entry(cat.to_string()).or_default();
        e.0 += hit;
        e.1 += want.len();
        e.2 += ms;
        e.3 += 1;
        all_found += hit;
        all_words += want.len();
        total_ms += ms;
        times.push(ms);
        n += 1;
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("\nvariant {variant}, det side {side}, {threads} thread(s)");
    for (cat, (hit, words, ms, k)) in &by_cat {
        println!(
            "  {cat:<10} recall {:>5.1}%  ({hit}/{words} words)  {:>6.0} ms/image",
            100.0 * *hit as f64 / *words as f64,
            ms / *k as f64
        );
    }
    println!(
        "  ALL        recall {:>5.1}%  ({all_found}/{all_words} words)  mean {:.0} ms, median {:.0} ms, slowest {:.0} ms",
        100.0 * all_found as f64 / all_words as f64,
        total_ms / n as f64,
        times[times.len() / 2],
        times[times.len() - 1]
    );
    println!(
        "  load {load_ms} ms, RSS before {} MB, after load {} MB, highest peak during one image {} MB ({})",
        rss0 / 1024,
        rss_loaded / 1024,
        worst.1,
        worst.0
    );
    qr_report(&set)?;
    Ok(())
}

/// QR codes in the real photos of `tests/bakeoff/qr.tsv`: how many of the codes that are there, and of the payloads that must
/// be read, the reader found.
fn qr_report(set: &std::path::Path) -> anyhow::Result<()> {
    let tsv = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/bakeoff/qr.tsv"))?;
    let (mut present, mut found_payloads, mut must, mut hit_must, mut ms_total) =
        (0usize, 0usize, 0usize, 0usize, 0f64);
    for line in tsv.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let img = image::open(set.join(f[0]))?.to_rgb8();
        let (w, h) = img.dimensions();
        let t = Instant::now();
        let codes = cyanotype_ocr::qr::detect(img.as_raw(), w, h);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let wanted: Vec<&str> = f.get(5).copied().unwrap_or("").split_whitespace().collect();
        let got: Vec<&str> = codes.iter().map(|c| c.text.as_str()).collect();
        let ok = wanted.iter().filter(|w| got.contains(w)).count();
        println!(
            "{:<16} {} present, {} read ({ok}/{} required) {ms:>6.0} ms",
            f[0],
            f[4],
            codes.len(),
            wanted.len()
        );
        present += f[4].parse::<usize>()?;
        found_payloads += codes.len();
        must += wanted.len();
        hit_must += ok;
        ms_total += ms;
    }
    println!(
        "QR: {found_payloads} of {present} codes read, {hit_must}/{must} required payloads, {ms_total:.0} ms in all"
    );
    Ok(())
}
