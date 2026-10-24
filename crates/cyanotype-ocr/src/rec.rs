//! Text recognition: crops, batches and CTC decoding.
use std::sync::Mutex;

use anyhow::{Result, bail};
use ort::value::Tensor;

use crate::{
    Config, Line, Word,
    geom::{self, Rect},
};

/// A batch of crops is padded to at least this wide (PaddleOCR's reference width), so short lines are seen as the model was
/// trained to see them.
const MIN_BATCH_WIDTH: usize = 320;

pub(crate) struct Out {
    pub data: Vec<f32>,
    /// Time steps and classes.
    pub t: usize,
    pub c: usize,
}

/// Run the recognition graph on `imgs` (each `3 * h * w` floats, channel-first, already padded to `w`).
pub(crate) fn run_batch(session: &Mutex<ort::session::Session>, imgs: &[Vec<f32>], h: usize, w: usize) -> Result<Out> {
    let n = imgs.len();
    let mut flat = Vec::with_capacity(n * 3 * h * w);
    for i in imgs {
        if i.len() != 3 * h * w {
            bail!("a crop has {} values, expected {}", i.len(), 3 * h * w);
        }
        flat.extend_from_slice(i);
    }
    let tensor =
        Tensor::from_array(([n, 3usize, h, w], flat.into_boxed_slice())).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut s = session.lock().unwrap_or_else(|e| e.into_inner());
    let name = s.inputs()[0].name().to_string();
    let outputs = s
        .run(ort::inputs![name.as_str() => tensor])
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let (shape, data) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if shape.len() != 3 || shape[0] as usize != n {
        bail!("unexpected recognition output shape {shape:?}");
    }
    let (t, c) = (shape[1] as usize, shape[2] as usize);
    let mut data = data.to_vec();
    // The exports end in a softmax; if one does not, apply it here.
    if data.len() >= c {
        let sum: f32 = data[..c].iter().sum();
        if !(0.98..=1.02).contains(&sum) {
            for row in data.chunks_mut(c) {
                let m = row.iter().copied().fold(f32::MIN, f32::max);
                let mut z = 0.0;
                for v in row.iter_mut() {
                    *v = (*v - m).exp();
                    z += *v;
                }
                for v in row.iter_mut() {
                    *v /= z;
                }
            }
        }
    }
    Ok(Out { data, t, c })
}

/// Cut `rect` out of the image, upright, `h` pixels tall; returns the channel-first floats and the width.
pub(crate) fn crop(rgb: &[u8], w: usize, h_img: usize, rect: &Rect, h: usize, max_w: usize) -> (Vec<f32>, usize) {
    let ratio = rect.len_u / rect.len_v.max(1.0);
    let cw = ((h as f32 * ratio).ceil() as usize).clamp(8, max_w);
    let mut out = vec![0f32; 3 * h * cw];
    for i in 0..h {
        let b = ((i as f32 + 0.5) / h as f32 - 0.5) * rect.len_v;
        for j in 0..cw {
            let a = ((j as f32 + 0.5) / cw as f32 - 0.5) * rect.len_u;
            let (x, y) = (
                rect.center.0 + rect.u.0 * a + rect.v.0 * b,
                rect.center.1 + rect.u.1 * a + rect.v.1 * b,
            );
            let px = geom::sample_bgr(rgb, w, h_img, x, y);
            for (c, v) in px.iter().enumerate() {
                out[c * h * cw + i * cw + j] = (v / 255.0 - 0.5) / 0.5;
            }
        }
    }
    (out, cw)
}

/// Pad a crop with zeros on the right to `w`.
fn pad(crop: &[f32], cw: usize, h: usize, w: usize) -> Vec<f32> {
    let mut out = vec![0f32; 3 * h * w];
    for c in 0..3 {
        for i in 0..h {
            let (src, dst) = (c * h * cw + i * cw, c * h * w + i * w);
            out[dst..dst + cw].copy_from_slice(&crop[src..src + cw]);
        }
    }
    out
}

/// Split crops (sorted by width) into batches of at most `max_n`, closing a batch when it would be wider than `max_px` in
/// total (padded to its widest, which is the last, as they are sorted) or narrower crops would make it short of the minimum.
fn batches<T>(crops: &[(usize, T, usize)], max_n: usize, max_px: usize) -> Vec<&[(usize, T, usize)]> {
    let (mut out, mut start) = (Vec::new(), 0);
    for i in 0..crops.len() {
        let n = i + 1 - start;
        let width = crops[i].2.max(MIN_BATCH_WIDTH) * n;
        if n > 1 && (n > max_n || width > max_px) {
            out.push(&crops[start..i]);
            start = i;
        }
    }
    if start < crops.len() {
        out.push(&crops[start..]);
    }
    out
}

/// What CTC decoding found in one row of the output.
#[derive(Debug, PartialEq)]
pub(crate) struct Decoded {
    pub text: String,
    pub score: f32,
    /// For each character of `text` (by `chars()` index of the pieces): the time step it was read at.
    pub steps: Vec<(String, usize)>,
}

/// CTC greedy decoding of one sample: `probs` is `t * c` probabilities, class 0 is the blank, the dictionary follows, and a
/// trailing class (when the model has `dict.len() + 2`) is a space.
pub(crate) fn decode(probs: &[f32], t: usize, c: usize, dict: &[String]) -> Decoded {
    let (mut text, mut steps, mut sum, mut prev) = (String::new(), Vec::new(), 0f32, 0usize);
    for step in 0..t {
        let row = &probs[step * c..(step + 1) * c];
        let (mut best, mut p) = (0usize, row[0]);
        for (k, v) in row.iter().enumerate() {
            if *v > p {
                best = k;
                p = *v;
            }
        }
        if best != 0 && best != prev {
            let ch = dict.get(best - 1).map_or(" ", String::as_str);
            text.push_str(ch);
            steps.push((ch.to_string(), step));
            sum += p;
        }
        prev = best;
    }
    let score = if steps.is_empty() {
        0.0
    } else {
        sum / steps.len() as f32
    };
    Decoded { text, score, steps }
}

/// The words of a decoded line as spans of time steps `(text, first, last)`.
pub(crate) fn word_spans(d: &Decoded) -> Vec<(String, usize, usize)> {
    let mut out: Vec<(String, usize, usize)> = Vec::new();
    let mut cur: Option<(String, usize, usize)> = None;
    for (ch, t) in &d.steps {
        if ch.trim().is_empty() {
            if let Some(w) = cur.take() {
                out.push(w);
            }
        } else {
            match &mut cur {
                Some(w) => {
                    w.0.push_str(ch);
                    w.2 = *t;
                }
                None => cur = Some((ch.clone(), *t, *t)),
            }
        }
    }
    out.extend(cur);
    out
}

/// A point along the line's long axis (`f` from 0 to 1) and across it (`g`, 0 top to 1 bottom), normalised to the image.
fn at(r: &Rect, f: f32, g: f32, size: (u32, u32)) -> [f32; 2] {
    let (a, b) = ((f - 0.5) * r.len_u, (g - 0.5) * r.len_v);
    [
        ((r.center.0 + r.u.0 * a + r.v.0 * b) / size.0 as f32).clamp(0.0, 1.0),
        ((r.center.1 + r.u.1 * a + r.v.1 * b) / size.1 as f32).clamp(0.0, 1.0),
    ]
}

pub(crate) fn read_all(
    session: &Mutex<ort::session::Session>,
    cfg: &Config,
    dict: &[String],
    classes: usize,
    rgb: &[u8],
    size: (u32, u32),
    rects: &[Rect],
) -> Result<Vec<Line>> {
    let h = cfg.rec_height as usize;
    let (w, hi) = (size.0 as usize, size.1 as usize);
    let mut crops: Vec<(usize, Vec<f32>, usize)> = rects
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let (c, cw) = crop(rgb, w, hi, r, h, cfg.rec_max_width as usize);
            (i, c, cw)
        })
        .collect();
    crops.sort_by_key(|(_, _, cw)| *cw);
    let mut lines: Vec<Option<Line>> = vec![None; rects.len()];
    for batch in batches(&crops, cfg.rec_batch.max(1), cfg.rec_batch_px) {
        let bw = batch
            .iter()
            .map(|(_, _, cw)| *cw)
            .max()
            .unwrap_or(0)
            .max(MIN_BATCH_WIDTH);
        let imgs: Vec<Vec<f32>> = batch.iter().map(|(_, c, cw)| pad(c, *cw, h, bw)).collect();
        let out = run_batch(session, &imgs, h, bw)?;
        if out.c != classes {
            bail!(
                "the recognition model changed its class count ({} then {})",
                classes,
                out.c
            );
        }
        for (k, (idx, _, cw)) in batch.iter().enumerate() {
            let d = decode(
                &out.data[k * out.t * out.c..(k + 1) * out.t * out.c],
                out.t,
                out.c,
                dict,
            );
            if d.steps.is_empty() {
                continue;
            }
            let r = &rects[*idx];
            // The crop fills the first `cw` of `bw` columns; the time steps span all `bw`.
            let span = bw as f32 / *cw as f32;
            let frac = |t: f32| (t / out.t as f32 * span).clamp(0.0, 1.0);
            let words = word_spans(&d)
                .into_iter()
                .map(|(text, a, b)| {
                    let (f0, f1) = (frac(a as f32), frac(b as f32 + 1.0));
                    Word {
                        text,
                        quad: [
                            at(r, f0, 0.0, size),
                            at(r, f1, 0.0, size),
                            at(r, f1, 1.0, size),
                            at(r, f0, 1.0, size),
                        ],
                    }
                })
                .collect();
            let c = r.corners();
            let norm = |p: (f32, f32)| {
                [
                    (p.0 / size.0 as f32).clamp(0.0, 1.0),
                    (p.1 / size.1 as f32).clamp(0.0, 1.0),
                ]
            };
            lines[*idx] = Some(Line {
                text: d.text.trim().to_string(),
                score: d.score,
                quad: [norm(c[0]), norm(c[1]), norm(c[2]), norm(c[3])],
                words,
            });
        }
    }
    Ok(lines.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Vec<String> {
        ["a", "b", "c", " "].iter().map(|s| s.to_string()).collect()
    }

    /// One-hot rows over 5 classes (blank, a, b, c, space) for the given class per time step.
    fn rows(classes: &[usize]) -> Vec<f32> {
        classes
            .iter()
            .flat_map(|&k| (0..5).map(move |i| if i == k { 0.9 } else { 0.025 }))
            .collect()
    }

    #[test]
    fn batches_close_when_the_padded_width_would_pass_the_budget() {
        let c = |w: usize| (0usize, (), w);
        let crops: Vec<_> = [100, 120, 200, 300, 900, 1500, 3200].iter().map(|w| c(*w)).collect();
        let b: Vec<Vec<usize>> = batches(&crops, 8, 1600)
            .iter()
            .map(|b| b.iter().map(|x| x.2).collect())
            .collect();
        // Short crops are padded to 320, so five of them fill 1600; a wide one is alone.
        assert_eq!(b, vec![vec![100, 120, 200, 300], vec![900], vec![1500], vec![3200]]);
        assert_eq!(batches(&crops[..2], 1, 1600).len(), 2, "never more than max_n");
        assert!(batches::<()>(&[], 8, 1600).is_empty());
    }

    #[test]
    fn ctc_collapses_repeats_and_drops_blanks_and_a_repeat_after_a_blank_counts() {
        // a a _ a b b _ c -> "aabc": the second a is a new character because a blank came between.
        let d = decode(&rows(&[1, 1, 0, 1, 2, 2, 0, 3]), 8, 5, &dict());
        assert_eq!(d.text, "aabc");
        assert!((d.score - 0.9).abs() < 1e-6);
        assert_eq!(d.steps.iter().map(|(_, t)| *t).collect::<Vec<_>>(), vec![0, 3, 4, 7]);
        assert_eq!(decode(&rows(&[0, 0, 0]), 3, 5, &dict()).steps.len(), 0);
    }

    #[test]
    fn the_class_past_the_dictionary_is_a_space_and_words_split_on_it() {
        // dict has 4 entries (a b c ' '), class 4 is the last of them; a model with dict+2 classes has one more: a space.
        let d = decode(&rows(&[1, 2, 0, 4, 0, 3, 3]), 7, 5, &dict());
        assert_eq!(d.text, "ab c");
        let spans = word_spans(&d);
        assert_eq!(spans, vec![("ab".to_string(), 0, 1), ("c".to_string(), 5, 5)]);
    }

    #[test]
    fn unknown_classes_read_as_a_space_not_a_panic() {
        let probs = vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let d = decode(&probs, 1, 6, &dict());
        assert_eq!(d.text, " ");
    }

    #[test]
    fn crop_is_upright_for_a_rotated_rectangle_and_padding_keeps_columns() {
        // A 20 x 10 image, left half dark, right half light; a rectangle over the whole image reads dark then light.
        let (w, hh) = (20usize, 10usize);
        let mut rgb = vec![0u8; w * hh * 3];
        for y in 0..hh {
            for x in 10..w {
                rgb[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let r = Rect {
            center: (10.0, 5.0),
            u: (1.0, 0.0),
            v: (0.0, 1.0),
            len_u: 20.0,
            len_v: 10.0,
        };
        let (c, cw) = crop(&rgb, w, hh, &r, 8, 3200);
        assert_eq!(cw, 16, "8 px tall and twice as long as tall");
        assert!(c[0] < -0.9 && c[cw - 1] > 0.9, "{} {}", c[0], c[cw - 1]);
        let padded = pad(&c, cw, 8, 32);
        assert_eq!(padded.len(), 3 * 8 * 32);
        assert_eq!(padded[0], c[0]);
        assert_eq!(padded[31], 0.0, "zeros on the right");
        // The same rectangle read the other way round (rotated 180 degrees) sees light first.
        let flipped = Rect {
            u: (-1.0, 0.0),
            v: (0.0, -1.0),
            ..r
        };
        let (c2, _) = crop(&rgb, w, hh, &flipped, 8, 3200);
        assert!(c2[0] > 0.9);
    }
}
