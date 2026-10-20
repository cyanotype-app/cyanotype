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

/// The application segments (`marker`, body) of a JPEG up to the start of the image data. Nothing for a file that is not a JPEG;
/// a segment that is cut short ends the list.
fn jpeg_segments(bytes: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return out;
    }
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
        let Some(end) = i.checked_add(2 + len) else { break };
        if len < 2 || end > bytes.len() {
            break;
        }
        out.push((marker, &bytes[i + 4..end]));
        i = end;
    }
    out
}

/// The XMP packet of an image file, if there is one.
fn xmp_packet(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return jpeg_segments(bytes)
            .into_iter()
            .find(|(m, body)| *m == 0xE1 && body.starts_with(XMP_APP1))
            .map(|(_, body)| &body[XMP_APP1.len()..]);
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

/// The keywords a file carries: IPTC `Keywords` (a JPEG's Photoshop segment) and XMP `dc:subject`, in that order, as written (the
/// caller turns them into tag names and drops duplicates). At most [`MAX_KEYWORDS`] of them.
pub fn keywords(bytes: &[u8]) -> Vec<String> {
    let mut out = iptc_keywords(bytes);
    if let Some(packet) = xmp_packet(bytes) {
        out.extend(subject_in(&String::from_utf8_lossy(packet)));
    }
    out.truncate(MAX_KEYWORDS);
    out
}

/// The most keywords read from one file.
pub const MAX_KEYWORDS: usize = 200;

/// IPTC-IIM record 2, dataset 25 (`Keywords`) inside the Photoshop image resource 0x0404 of an APP13 segment.
fn iptc_keywords(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for (marker, body) in jpeg_segments(bytes) {
        let Some(mut rest) = (marker == 0xED)
            .then(|| body.strip_prefix(b"Photoshop 3.0\0"))
            .flatten()
        else {
            continue;
        };
        while rest.len() >= 12 && rest.starts_with(b"8BIM") {
            let id = u16::from_be_bytes([rest[4], rest[5]]);
            // A Pascal string, padded so that its length byte and text take an even number of bytes.
            let name_len = usize::from(rest[6]);
            let at = 7 + name_len + (1 + name_len) % 2;
            let Some(size_bytes) = rest.get(at..at + 4) else { break };
            let size = u32::from_be_bytes([size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3]]) as usize;
            let Some(data) = rest.get(at + 4..at + 4 + size) else {
                break;
            };
            if id == 0x0404 {
                iptc_records(data, &mut out);
            }
            let next = at + 4 + size + size % 2;
            rest = rest.get(next..).unwrap_or(&[]);
        }
    }
    out
}

fn iptc_records(data: &[u8], out: &mut Vec<String>) {
    let mut i = 0;
    while i + 5 <= data.len() && data[i] == 0x1C {
        let (record, dataset) = (data[i + 1], data[i + 2]);
        let size = usize::from(u16::from_be_bytes([data[i + 3], data[i + 4]]));
        if size & 0x8000 != 0 {
            return; // an extended dataset: not a keyword, and the length is not ours to read
        }
        let Some(value) = data.get(i + 5..i + 5 + size) else {
            return;
        };
        if record == 2 && dataset == 25 {
            // IPTC is UTF-8 only when it says so (dataset 1:90); older writers use Latin-1. UTF-8 first, since a valid
            // multi-byte sequence is almost never Latin-1 text.
            out.push(match std::str::from_utf8(value) {
                Ok(s) => s.to_string(),
                Err(_) => value.iter().map(|b| char::from(*b)).collect(),
            });
        }
        i += 5 + size;
    }
}

/// The entries of `<dc:subject><rdf:Bag><rdf:li>..</rdf:li>...`.
fn subject_in(xml: &str) -> Vec<String> {
    let Some(start) = xml.find("<dc:subject") else {
        return Vec::new();
    };
    let Some(open_end) = xml[start..].find('>') else {
        return Vec::new();
    };
    // `<dc:subject/>` has nothing in it.
    if xml[start..start + open_end].ends_with('/') {
        return Vec::new();
    }
    let body_start = start + open_end + 1;
    let body_end = xml[body_start..]
        .find("</dc:subject>")
        .map_or(xml.len(), |e| body_start + e);
    let mut out = Vec::new();
    let mut rest = &xml[body_start..body_end];
    while let Some(p) = rest.find("<rdf:li") {
        let after = &rest[p + "<rdf:li".len()..];
        // `<rdf:lig...`: some other element.
        if !after.starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace()) {
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        if after[..gt].ends_with('/') {
            rest = &after[gt + 1..];
            continue;
        }
        let text = &after[gt + 1..];
        let Some(close) = text.find("</rdf:li>") else { break };
        let entry = unescape(&text[..close]).trim().to_string();
        if !entry.is_empty() {
            out.push(entry);
        }
        rest = &text[close + "</rdf:li>".len()..];
    }
    out
}

/// Decode the five predefined XML entities and numeric references.
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

    /// An APP13 segment holding a Photoshop resource with IPTC keywords (and a caption that is not one).
    fn jpeg_with_iptc(keywords: &[&[u8]]) -> Vec<u8> {
        let mut iim = vec![0x1C, 0x02, 0x78, 0, 4];
        iim.extend_from_slice(b"Text"); // 2:120 caption
        for k in keywords {
            iim.extend_from_slice(&[0x1C, 0x02, 25]);
            iim.extend_from_slice(&(k.len() as u16).to_be_bytes());
            iim.extend_from_slice(k);
        }
        let mut res = b"Photoshop 3.0\0".to_vec();
        // An unrelated resource first, with a name, to walk past it (and the padding after its odd size).
        res.extend_from_slice(b"8BIM\x03\xED\x03abc\x00\x00\x00\x03xyz\x00");
        res.extend_from_slice(b"8BIM\x04\x04\x00\x00");
        res.extend_from_slice(&(iim.len() as u32).to_be_bytes());
        res.extend_from_slice(&iim);
        if iim.len() % 2 == 1 {
            res.push(0);
        }
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xED];
        out.extend_from_slice(&((res.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&res);
        out.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
        out
    }

    #[test]
    fn iptc_keywords_utf8_latin1_and_neighbours() {
        let j = jpeg_with_iptc(&[b"beach", "Café".as_bytes(), b"Caf\xe9 latin", b"trip/2024"]);
        assert_eq!(keywords(&j), ["beach", "Café", "Café latin", "trip/2024"]);
        assert!(keywords(&jpeg_with_iptc(&[])).is_empty());
    }

    #[test]
    fn xmp_subject_bag_with_entities_languages_and_empties() {
        let xml = r#"<x:xmpmeta><rdf:RDF><rdf:Description>
            <dc:subject><rdf:Bag><rdf:li>sun &amp; sea</rdf:li><rdf:li xml:lang="x-default">Lisboa</rdf:li><rdf:li/><rdf:li>  </rdf:li><rdf:li>last</rdf:li></rdf:Bag></dc:subject>
            <dc:title><rdf:Alt><rdf:li>Not a keyword</rdf:li></rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        assert_eq!(keywords(&jpeg_with_xmp(xml)), ["sun & sea", "Lisboa", "last"]);
        assert!(keywords(&jpeg_with_xmp("<x:xmpmeta><dc:subject/></x:xmpmeta>")).is_empty());
        assert!(keywords(&jpeg_with_xmp("<x:xmpmeta><dc:subject><rdf:Bag><rdf:li>cut")).is_empty());
        assert!(keywords(b"not an image").is_empty());
    }

    #[test]
    fn keyword_readers_survive_truncation_and_are_capped() {
        let j = jpeg_with_iptc(&[b"a", b"bb", b"ccc"]);
        for cut in 0..j.len() {
            let _ = keywords(&j[..cut]);
        }
        let many: Vec<String> = (0..300).map(|n| format!("k{n}")).collect();
        let refs: Vec<&[u8]> = many.iter().map(|s| s.as_bytes()).collect();
        assert_eq!(keywords(&jpeg_with_iptc(&refs)).len(), MAX_KEYWORDS);
    }

    #[test]
    fn entities() {
        assert_eq!(unescape("a &amp; b &lt;&#65;&#x42;&bogus; &"), "a & b <AB&bogus; &");
    }
}
