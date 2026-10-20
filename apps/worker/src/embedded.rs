//! What a photo says about itself that is not EXIF proper: the XMP packet (Lightroom, digiKam, darktable, Windows and
//! most phones write `xmp:Rating` there). Pure logic, no I/O.
//!
//! The packet is found in a JPEG by its APP1 segment; in every other container (HEIC, PNG, WebP, TIFF-based RAW) by looking for
//! the `<x:xmpmeta` text in the first [`SCAN_BYTES`], where such a packet lives. There is no XML parser here on purpose: the
//! two things read out are a rating attribute or element and the entries of one list, and anything the scanner does not
//! understand is simply "not there" (a rating is a default, never something to fail a job over).
use std::borrow::Cow;

/// How far into a non-JPEG file the XMP packet is looked for.
const SCAN_BYTES: usize = 4 << 20;
/// The largest XMP packet read (the format caps one at 64 KiB per segment; extended packets are not read).
const MAX_PACKET: usize = 1 << 20;
const XMP_APP1: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// The star rating the file carries, 0 to 5, if it carries one that is a rating (`-1` means "rejected" in XMP and is not a
/// rating; unparseable values are none). 0 is returned as `Some(0)`: "the file says unrated".
pub fn xmp_rating(bytes: &[u8]) -> Option<u8> {
    let packet = xmp_packet(bytes)?;
    let text = String::from_utf8_lossy(packet);
    rating_in(&text)
}

/// The XMP packet of an image file, if there is one.
fn xmp_packet(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 4 <= bytes.len() && bytes[i] == 0xFF {
            let marker = bytes[i + 1];
            if marker == 0xFF {
                i += 1; // fill byte
                continue;
            }
            // Markers without a length, and the start of the entropy-coded data: nothing more to read.
            if marker == 0xDA || marker == 0xD9 || (0xD0..=0xD8).contains(&marker) || marker == 0x01 {
                break;
            }
            let len = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
            let end = i.checked_add(2 + len)?;
            if len < 2 || end > bytes.len() {
                break;
            }
            let body = &bytes[i + 4..end];
            if marker == 0xE1 && body.starts_with(XMP_APP1) {
                return Some(&body[XMP_APP1.len()..]);
            }
            i = end;
        }
        return None;
    }
    let head = &bytes[..bytes.len().min(SCAN_BYTES)];
    let start = find(head, b"<x:xmpmeta")?;
    let rest = &head[start..];
    let end = find(rest, b"</x:xmpmeta>").map_or(rest.len().min(MAX_PACKET), |e| e + b"</x:xmpmeta>".len());
    Some(&rest[..end.min(MAX_PACKET)])
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let first = *needle.first()?;
    let mut from = 0;
    while let Some(p) = hay[from..].iter().position(|b| *b == first) {
        let at = from + p;
        if hay[at..].starts_with(needle) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// `xmp:Rating="4"`, `xmp:Rating='4'` or `<xmp:Rating>4</xmp:Rating>` (also the old `xap:` prefix).
fn rating_in(xml: &str) -> Option<u8> {
    for prefix in ["xmp:Rating", "xap:Rating"] {
        let mut from = 0;
        while let Some(p) = xml[from..].find(prefix) {
            let after = from + p + prefix.len();
            from = after;
            let rest = &xml[after..];
            // `xmp:RatingPercent` is a different property.
            if rest.starts_with(|c: char| c.is_ascii_alphanumeric()) {
                continue;
            }
            let value = if let Some(r) = rest.trim_start().strip_prefix('=') {
                let r = r.trim_start();
                let quote = r.chars().next().filter(|c| *c == '"' || *c == '\'')?;
                r[1..].split(quote).next()
            } else if let Some(r) = rest.strip_prefix('>') {
                r.split('<').next()
            } else {
                None
            };
            if let Some(v) = value {
                return parse_rating(v);
            }
        }
    }
    None
}

fn parse_rating(v: &str) -> Option<u8> {
    let n: f64 = v.trim().parse().ok()?;
    if !n.is_finite() || !(0.0..=5.0).contains(&n.round()) {
        return None; // -1 is "rejected"; anything else is not a rating
    }
    Some(n.round() as u8)
}

/// EXIF `Rating` (0x4746, 0 to 5) or, failing that, `RatingPercent` (0x4749: 1, 25, 50, 75 and 99 are 1 to 5 stars).
pub fn exif_rating(rating: Option<u32>, percent: Option<u32>) -> Option<u8> {
    if let Some(r) = rating {
        return (r <= 5).then_some(r as u8);
    }
    percent.and_then(|p| match p {
        0 => Some(0),
        1..=12 => Some(1),
        13..=37 => Some(2),
        38..=62 => Some(3),
        63..=87 => Some(4),
        88..=100 => Some(5),
        _ => None,
    })
}

/// Decode the five predefined XML entities and numeric references.
#[allow(dead_code)]
pub fn unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        match rest.find(';') {
            Some(e) if e <= 10 => {
                let ent = &rest[1..e];
                let ch = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => ent
                        .strip_prefix("#x")
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                        .and_then(char::from_u32),
                };
                match ch {
                    Some(c) => {
                        out.push(c);
                        rest = &rest[e + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &rest[1..];
                    }
                }
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg_with_xmp(xml: &str) -> Vec<u8> {
        let mut seg = XMP_APP1.to_vec();
        seg.extend_from_slice(xml.as_bytes());
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&((seg.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&seg);
        out.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
        out
    }

    #[test]
    fn rating_as_attribute_element_and_in_other_containers() {
        let attr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF><rdf:Description xmp:Rating="4" xmp:Label="x"/></rdf:RDF></x:xmpmeta>"#;
        assert_eq!(xmp_rating(&jpeg_with_xmp(attr)), Some(4));
        let elem = "<x:xmpmeta><xmp:Rating>2</xmp:Rating></x:xmpmeta>";
        assert_eq!(xmp_rating(&jpeg_with_xmp(elem)), Some(2));
        // A PNG / HEIC / RAW: the packet is found by its text.
        let mut other = b"\x89PNG\r\n....iTXtXML:com.adobe.xmp".to_vec();
        other.extend_from_slice(attr.as_bytes());
        assert_eq!(xmp_rating(&other), Some(4));
        assert_eq!(xmp_rating(b"nothing here"), None);
    }

    #[test]
    fn rejected_garbage_and_lookalikes_are_not_ratings() {
        assert_eq!(
            xmp_rating(&jpeg_with_xmp(r#"<x:xmpmeta><d xmp:Rating="-1"/></x:xmpmeta>"#)),
            None
        );
        assert_eq!(
            xmp_rating(&jpeg_with_xmp(r#"<x:xmpmeta><d xmp:Rating="9"/></x:xmpmeta>"#)),
            None
        );
        assert_eq!(
            xmp_rating(&jpeg_with_xmp(r#"<x:xmpmeta><d xmp:Rating="lots"/></x:xmpmeta>"#)),
            None
        );
        assert_eq!(
            xmp_rating(&jpeg_with_xmp(r#"<x:xmpmeta><d xmp:RatingPercent="75"/></x:xmpmeta>"#)),
            None
        );
        assert_eq!(
            xmp_rating(&jpeg_with_xmp(r#"<x:xmpmeta><d xmp:Rating="0"/></x:xmpmeta>"#)),
            Some(0)
        );
        assert_eq!(
            xmp_rating(&jpeg_with_xmp(r#"<x:xmpmeta><d xmp:Rating='3.0'/></x:xmpmeta>"#)),
            Some(3)
        );
    }

    #[test]
    fn exif_rating_and_percent() {
        assert_eq!(exif_rating(Some(3), None), Some(3));
        assert_eq!(exif_rating(Some(9), Some(99)), None);
        assert_eq!(exif_rating(None, Some(75)), Some(4));
        assert_eq!(exif_rating(None, Some(99)), Some(5));
        assert_eq!(exif_rating(None, Some(1)), Some(1));
        assert_eq!(exif_rating(None, None), None);
    }

    #[test]
    fn truncated_jpeg_segments_do_not_panic() {
        let mut j = jpeg_with_xmp(r#"<x:xmpmeta><d xmp:Rating="5"/></x:xmpmeta>"#);
        for cut in 0..j.len() {
            let _ = xmp_rating(&j[..cut]);
        }
        j[4] = 0xFF;
        j[5] = 0xFF;
        let _ = xmp_rating(&j);
    }

    #[test]
    fn entities() {
        assert_eq!(unescape("a &amp; b &lt;&#65;&#x42;&bogus; &"), "a & b <AB&bogus; &");
    }
}
