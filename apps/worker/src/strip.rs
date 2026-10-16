//! Location-free JPEG copies for share links. Pure logic, no I/O.
//!
//! [`strip_jpeg`] rewrites a JPEG at the segment level, so the compressed image data is copied byte for byte
//! (no re-encode, no quality loss) while everything that can describe where, when or with what the photo was
//! taken is dropped: the whole EXIF block (GPS, camera serial number, embedded thumbnail, maker notes), XMP, IPTC /
//! Photoshop resources, comments, multi-picture (MPF) data and every other application segment, plus anything
//! stored after the end-of-image marker (motion-photo videos, extra pictures). What stays is what is needed to
//! display the picture the same way: the ICC colour profile, the Adobe colour-transform marker, and a freshly
//! written EXIF block that holds nothing but the orientation.
use anyhow::{Result, bail};

const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOS: u8 = 0xDA;

/// The segments (marker and payload, without the `FF xx` prefix and the length) worth keeping.
fn keep(marker: u8, payload: &[u8]) -> bool {
    match marker {
        // JFIF header (not the JFXX extension, which can embed a thumbnail).
        0xE0 => payload.starts_with(b"JFIF\0"),
        // ICC colour profile (other APP2 payloads are MPF / FlashPix extensions).
        0xE2 => payload.starts_with(b"ICC_PROFILE\0"),
        // Adobe: tells decoders whether the data is YCCK / RGB, without it some images change colour.
        0xEE => payload.starts_with(b"Adobe"),
        // Every other APPn (EXIF and XMP in APP1, IPTC in APP13, vendor data) and comments.
        0xE1 | 0xE3..=0xED | 0xEF | 0xFE => false,
        // Tables, frame headers, restart intervals: needed to decode.
        _ => true,
    }
}

/// A minimal APP1 segment with only the Orientation tag (big-endian TIFF, one IFD entry).
fn orientation_app1(orientation: u8) -> Vec<u8> {
    let mut v = vec![0xFF, 0xE1, 0x00, 34];
    v.extend(b"Exif\0\0");
    v.extend(b"MM\0*\0\0\0\x08"); // TIFF header, first IFD at offset 8
    v.extend([0, 1]); // one entry
    v.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, orientation, 0, 0]); // Orientation, SHORT, count 1
    v.extend([0, 0, 0, 0]); // no next IFD
    v
}

/// `jpeg` without metadata, keeping `orientation` (EXIF values 2..=8; 1 and anything else write no EXIF block).
pub fn strip_jpeg(jpeg: &[u8], orientation: u8) -> Result<Vec<u8>> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != SOI {
        bail!("not a JPEG (no SOI marker)");
    }
    let mut out = Vec::with_capacity(jpeg.len());
    out.extend([0xFF, SOI]);
    let mut orientation_written = !(2..=8).contains(&orientation);
    let mut pos = 2;
    let mut saw_sos = false;
    loop {
        // A marker: 0xFF, optionally repeated as fill, then the marker byte.
        if pos >= jpeg.len() {
            bail!("JPEG ends without an end-of-image marker");
        }
        if jpeg[pos] != 0xFF {
            bail!("corrupt JPEG: expected a marker at byte {pos}");
        }
        while pos < jpeg.len() && jpeg[pos] == 0xFF {
            pos += 1;
        }
        let Some(&marker) = jpeg.get(pos) else {
            bail!("JPEG ends inside a marker");
        };
        pos += 1;
        match marker {
            EOI => {
                if !saw_sos {
                    bail!("JPEG has no image data");
                }
                out.extend([0xFF, EOI]);
                return Ok(out);
            }
            // Standalone markers (no length): restart markers and TEM.
            0x00 | 0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        let Some(len) = jpeg
            .get(pos..pos + 2)
            .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
        else {
            bail!("JPEG ends inside a segment header");
        };
        if len < 2 || pos + len > jpeg.len() {
            bail!("corrupt JPEG: segment of {len} bytes at byte {pos} runs past the end");
        }
        let payload = &jpeg[pos + 2..pos + len];
        if keep(marker, payload) {
            // The orientation block goes right after the JFIF header, before the first other segment.
            if !orientation_written && marker != 0xE0 {
                out.extend(orientation_app1(orientation));
                orientation_written = true;
            }
            out.extend([0xFF, marker]);
            out.extend(&jpeg[pos..pos + len]);
        }
        pos += len;
        if marker == SOS {
            saw_sos = true;
            // Entropy-coded data: copy up to the next real marker (FF followed by neither 00 stuffing, a restart
            // marker nor fill).
            let start = pos;
            while pos < jpeg.len() {
                if jpeg[pos] == 0xFF
                    && let Some(&n) = jpeg.get(pos + 1)
                    && n != 0x00
                    && n != 0xFF
                    && !(0xD0..=0xD7).contains(&n)
                {
                    break;
                }
                pos += 1;
            }
            out.extend(&jpeg[start..pos]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif::{self, testutil::*};

    fn tiny_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_fn(16, 8, |x, y| image::Rgb([(x * 15) as u8, (y * 30) as u8, 90]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    }

    fn gps_app1() -> Vec<u8> {
        let gps = [
            (0x0001, 2, ascii("N")),
            (0x0002, 5, rationals(&[(48, 1), (51, 1), (30, 1)])),
            (0x0003, 2, ascii("E")),
            (0x0004, 5, rationals(&[(2, 1), (17, 1), (40, 1)])),
        ];
        exif_app1(
            &[
                (0x010F, 2, ascii("SECRETMAKE")),
                (0x0112, 3, vec![6, 0]),
                (0xA431, 2, ascii("SERIAL123")),
            ],
            &[],
            &gps,
        )
    }

    fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, marker];
        v.extend((payload.len() as u16 + 2).to_be_bytes());
        v.extend(payload);
        v
    }

    /// A JPEG that carries location in every place it can hide.
    fn leaky() -> Vec<u8> {
        let mut j = with_app1(&tiny_jpeg(), &gps_app1());
        let xmp = segment(
            0xE1,
            b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>GPSLatitude=48.85</x:xmpmeta>",
        );
        let iptc = segment(0xED, b"Photoshop 3.0\0SECRETCITY");
        let com = segment(0xFE, b"taken at SECRETPLACE");
        let mpf = segment(0xE2, b"MPF\0SECRETMPF");
        j.splice(2..2, [xmp, iptc, com, mpf].concat());
        // Something appended after the image (a motion-photo video, a second picture with its own EXIF).
        j.extend(b"TRAILER-SECRETTRAIL");
        j.extend(with_app1(&tiny_jpeg(), &gps_app1()));
        j
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn the_fixture_really_has_a_location() {
        let i = exif::parse(&leaky());
        assert!(i.latitude.is_some() && i.longitude.is_some());
        assert_eq!(i.camera_make.as_deref(), Some("SECRETMAKE"));
    }

    #[test]
    fn strips_location_and_all_other_metadata() {
        let src = leaky();
        let out = strip_jpeg(&src, exif::parse(&src).orientation).unwrap();
        let i = exif::parse(&out);
        assert_eq!((i.latitude, i.longitude), (None, None));
        assert_eq!(i.camera_make, None);
        assert_eq!(i.taken_at, None);
        for needle in [
            &b"SECRET"[..],
            b"GPS",
            b"xmpmeta",
            b"Photoshop",
            b"MPF",
            b"SERIAL123",
            b"TRAILER",
        ] {
            assert!(!contains(&out, needle), "leaked {:?}", String::from_utf8_lossy(needle));
        }
        // The EXIF block that is left holds the orientation and no GPS IFD pointer (tag 0x8825).
        assert_eq!(i.orientation, 6);
        let app1 = out.windows(6).position(|w| w == b"Exif\0\0").unwrap();
        let tiff = &out[app1 + 6..app1 + 6 + 26];
        assert_eq!(tiff.len(), 26);
        assert!(!contains(tiff, &[0x88, 0x25]) && !contains(tiff, &[0x25, 0x88]));
        // Ends at the first EOI, and the pixels are untouched (same dimensions, same pixels).
        assert_eq!(&out[out.len() - 2..], &[0xFF, 0xD9]);
        let (a, b) = (
            image::load_from_memory(&src).unwrap(),
            image::load_from_memory(&out).unwrap(),
        );
        assert_eq!(a.to_rgb8().as_raw(), b.to_rgb8().as_raw());
    }

    #[test]
    fn upright_photos_get_no_exif_block_at_all() {
        let out = strip_jpeg(&tiny_jpeg(), 1).unwrap();
        assert!(!contains(&out, b"Exif"));
        assert_eq!(exif::parse(&out).orientation, 1);
        image::load_from_memory(&out).unwrap();
    }

    #[test]
    fn icc_profile_and_jfif_survive() {
        let mut j = tiny_jpeg();
        let icc = segment(0xE2, b"ICC_PROFILE\0\x01\x01profile-bytes");
        j.splice(2..2, icc);
        let out = strip_jpeg(&j, 1).unwrap();
        assert!(contains(&out, b"ICC_PROFILE\0"));
        if contains(&tiny_jpeg(), b"JFIF\0") {
            assert!(contains(&out, b"JFIF\0"));
        }
    }

    #[test]
    fn rejects_things_that_are_not_complete_jpegs() {
        assert!(strip_jpeg(b"not a jpeg at all", 1).is_err());
        let j = tiny_jpeg();
        assert!(strip_jpeg(&j[..j.len() / 2], 1).is_err());
        assert!(strip_jpeg(&[0xFF, 0xD8, 0xFF, 0xD9], 1).is_err());
    }
}
