//! QR reading on synthetic codes (module matrices made by the web app's QR generator, `tests/fixtures/qr-*.txt`), drawn at
//! several sizes, shifted into a larger noisy picture and turned.
use cyanotype_ocr::qr::{self, web_link};

fn matrix(name: &str) -> Vec<Vec<bool>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| l.chars().map(|c| c == '#').collect())
        .collect()
}

/// An RGB8 picture of `m` at `scale` pixels per module with a four-module quiet zone, on a `w` x `h` background, at (x, y).
fn draw(m: &[Vec<bool>], scale: usize, (w, h): (usize, usize), (x0, y0): (usize, usize), noise: bool) -> Vec<u8> {
    let mut img = vec![235u8; w * h * 3];
    let mut seed = 12345u32;
    if noise {
        for px in img.iter_mut() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *px = 200 + (seed >> 24) as u8 % 50;
        }
    }
    let n = m.len();
    let quiet = 4 * scale;
    for y in 0..(n * scale + 2 * quiet) {
        for x in 0..(n * scale + 2 * quiet) {
            let (px, py) = (x0 + x, y0 + y);
            if px >= w || py >= h {
                continue;
            }
            let dark = x >= quiet && y >= quiet && {
                let (mx, my) = ((x - quiet) / scale, (y - quiet) / scale);
                mx < n && my < n && m[my][mx]
            };
            let v = if dark { 20 } else { 250 };
            img[(py * w + px) * 3..(py * w + px) * 3 + 3].copy_from_slice(&[v, v, v]);
        }
    }
    img
}

fn rotate90(img: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; img.len()];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = (h - 1 - y, x);
            out[(ny * h + nx) * 3..(ny * h + nx) * 3 + 3].copy_from_slice(&img[(y * w + x) * 3..(y * w + x) * 3 + 3]);
        }
    }
    out
}

#[test]
fn reads_a_web_address_at_several_sizes() {
    let m = matrix("qr-https.txt");
    for scale in [3usize, 5, 8, 12] {
        let side = (m.len() + 8) * scale;
        let img = draw(&m, scale, (side, side), (0, 0), false);
        let found = qr::detect(&img, side as u32, side as u32);
        assert_eq!(found.len(), 1, "scale {scale}");
        assert_eq!(found[0].text, "https://example.org/p?id=42");
        assert_eq!(web_link(&found[0].text), Some("https://example.org/p?id=42"));
        // The corners are normalised to the picture.
        assert!(found[0].quad.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
    }
}

#[test]
fn finds_a_small_code_inside_a_big_noisy_picture_and_where_it_is() {
    let m = matrix("qr-https.txt");
    let (w, h) = (1400usize, 1000usize);
    let img = draw(&m, 6, (w, h), (800, 500), true);
    let found = qr::detect(&img, w as u32, h as u32);
    assert_eq!(found.len(), 1);
    let (cx, cy) = (
        found[0].quad.iter().map(|p| p[0]).sum::<f32>() / 4.0,
        found[0].quad.iter().map(|p| p[1]).sum::<f32>() / 4.0,
    );
    let (ex, ey) = (
        (800.0 + (m.len() as f32 + 8.0) * 3.0) / w as f32,
        (500.0 + (m.len() as f32 + 8.0) * 3.0) / h as f32,
    );
    assert!(
        (cx - ex).abs() < 0.03 && (cy - ey).abs() < 0.03,
        "{cx} {cy} against {ex} {ey}"
    );
}

#[test]
fn reads_a_turned_code_and_keeps_non_web_payloads_as_text() {
    let m = matrix("qr-wifi.txt");
    let side = (m.len() + 8) * 6;
    let img = rotate90(&draw(&m, 6, (side, side), (0, 0), false), side, side);
    let found = qr::detect(&img, side as u32, side as u32);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].text, "WIFI:S:net;T:WPA;P:secret;;");
    assert_eq!(web_link(&found[0].text), None, "a Wi-Fi login is text, never a link");
}

#[test]
fn two_codes_and_the_same_one_twice() {
    let a = matrix("qr-https.txt");
    let b = matrix("qr-wifi.txt");
    let (w, h) = (700usize, 350usize);
    let mut img = draw(&a, 6, (w, h), (0, 0), false);
    let right = draw(&b, 6, (w, h), (350, 0), false);
    // Paste the second code over the right half.
    for y in 0..h {
        for x in 350..w {
            img[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&right[(y * w + x) * 3..(y * w + x) * 3 + 3]);
        }
    }
    let mut texts: Vec<String> = qr::detect(&img, w as u32, h as u32)
        .into_iter()
        .map(|c| c.text)
        .collect();
    texts.sort();
    assert_eq!(
        texts,
        vec![
            "WIFI:S:net;T:WPA;P:secret;;".to_string(),
            "https://example.org/p?id=42".to_string()
        ]
    );
}
