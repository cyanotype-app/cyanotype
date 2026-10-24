//! QR codes, with `rxing` (a Rust port of ZXing: pure Rust, no native code). What is found is data: it is shown as text, a link
//! when it is one, and never opened by anything on the server.
//!
//! Why `rxing` and not the smaller `rqrr`: on real photos (a code on a sticker, one printed in a street sign's corner, a
//! stylised code with round dots) `rqrr` found none of five where `rxing` found the codes in three (docs/build.md, "Text in
//! photos"). Photos are read at their own size and then at twice it, which finds smaller codes (a second pass at 3x found two
//! codes on a sign where 1x found one).
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use rxing::{BarcodeFormat, DecodeHints};

/// The longest payload kept (QR codes hold up to about 4,300 characters).
pub const MAX_CHARS: usize = 4096;
/// The 2x pass is skipped when the long side is already over this (the preview is at most 1440).
const UPSCALE_UP_TO: u32 = 1800;

#[derive(Debug, Clone, PartialEq)]
pub struct QrCode {
    pub text: String,
    /// The four corners of a box around the code (top-left, top-right, bottom-right, bottom-left), normalised to the image.
    pub quad: [[f32; 2]; 4],
}

fn luma(rgb: &[u8]) -> Vec<u8> {
    rgb.as_chunks::<3>()
        .0
        .iter()
        .map(|p| ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u8)
        .collect()
}

fn scan(gray: Vec<u8>, w: u32, h: u32) -> Vec<QrCode> {
    let mut hints = DecodeHints {
        PossibleFormats: Some([BarcodeFormat::QR_CODE].into_iter().collect()),
        TryHarder: Some(true),
        ..DecodeHints::default()
    };
    let Ok(results) = rxing::helpers::detect_multiple_in_luma_with_hints(gray, w, h, &mut hints) else {
        return Vec::new();
    };
    results
        .into_iter()
        .filter_map(|r| {
            let text = r.getText().to_string();
            if text.is_empty() || text.chars().count() > MAX_CHARS || text.contains('\0') {
                return None;
            }
            let pts = r.getPoints();
            if pts.is_empty() {
                return None;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for p in pts {
                x0 = x0.min(p.x);
                y0 = y0.min(p.y);
                x1 = x1.max(p.x);
                y1 = y1.max(p.y);
            }
            // The points are the finder patterns' centres, three and a half modules inside the code's corners: grow the box.
            let pad = 0.12 * (x1 - x0).max(y1 - y0).max(8.0);
            let (fw, fh) = (w as f32, h as f32);
            let n = |x: f32, y: f32| [(x / fw).clamp(0.0, 1.0), (y / fh).clamp(0.0, 1.0)];
            Some(QrCode {
                text,
                quad: [
                    n(x0 - pad, y0 - pad),
                    n(x1 + pad, y0 - pad),
                    n(x1 + pad, y1 + pad),
                    n(x0 - pad, y1 + pad),
                ],
            })
        })
        .collect()
}

/// QR codes in an RGB8 image: a pass at full size and, unless the picture is already large, one at twice the size. The same
/// text found twice is reported once.
pub fn detect(rgb: &[u8], width: u32, height: u32) -> Vec<QrCode> {
    if width == 0 || height == 0 || rgb.len() != width as usize * height as usize * 3 {
        return Vec::new();
    }
    let gray = luma(rgb);
    let mut found = scan(gray.clone(), width, height);
    if width.max(height) <= UPSCALE_UP_TO
        && let Ok(src) = ImageRef::new(width, height, &gray, PixelType::U8)
    {
        let mut dst = Image::new(width * 2, height * 2, PixelType::U8);
        if Resizer::new()
            .resize(
                &src,
                &mut dst,
                &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
            )
            .is_ok()
        {
            found.extend(scan(dst.into_vec(), width * 2, height * 2));
        }
    }
    let mut seen = std::collections::HashSet::new();
    found.retain(|c| seen.insert(c.text.clone()));
    found
}

/// A payload that is a web address (`http` or `https`) the viewer may offer as a link. Anything else (a Wi-Fi login, a
/// payment request, plain text, `javascript:` or `data:` URLs) is shown as text only.
pub fn web_link(text: &str) -> Option<&str> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    ((lower.starts_with("http://") || lower.starts_with("https://"))
        && !t.chars().any(|c| c.is_whitespace() || c.is_control())
        && t.len() <= 2048)
        .then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_web_addresses_are_links() {
        assert_eq!(web_link("https://example.org/a?b=c"), Some("https://example.org/a?b=c"));
        assert_eq!(web_link("  HTTP://Example.org "), Some("HTTP://Example.org"));
        for no in [
            "javascript:alert(1)",
            "data:text/html,x",
            "WIFI:S:net;T:WPA;P:pw;;",
            "tel:+123",
            "ftp://x",
            "https://a b",
            "just text",
            "https://x\n.evil",
        ] {
            assert_eq!(web_link(no), None, "{no}");
        }
    }

    #[test]
    fn a_blank_or_wrong_sized_image_has_no_codes() {
        assert!(detect(&vec![255u8; 100 * 100 * 3], 100, 100).is_empty());
        assert!(detect(&[0u8; 5], 100, 100).is_empty());
        assert!(detect(&[], 0, 0).is_empty());
    }
}
