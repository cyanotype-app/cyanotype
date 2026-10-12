//! Which decoder handles a file. Routing is by magic bytes, not by the stored content type:
//! the CLI uploader may send `application/octet-stream` (or a wrong type) for unusual RAW files.
use cyanotype_core::keys;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Png,
    Webp,
    /// HEIC / HEIF / AVIF, decoded by libheif.
    Heif,
    /// Camera RAW (CR2/CR3, NEF/NRW, ARW, RAF, ORF, RW2, PEF, DNG, SRW, ...).
    Raw,
}

/// File extensions of the RAW formats we route to the RAW pipeline.
const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "rwl", "pef", "ptx", "dng", "srw",
    "3fr", "fff", "iiq", "erf", "kdc", "dcr", "mef", "mos", "mrw", "raw", "rwz", "x3f", "gpr", "cap", "eip",
];

const HEIF_EXTS: &[&str] = &["heic", "heif", "hif", "avif"];

fn base_type(content_type: &str) -> String {
    content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// Cheap pre-download check: could this upload possibly be an image we handle? Anything clearly
/// unrelated (video, PDF, ...) is skipped without fetching it. A "maybe" is settled by [`sniff`].
pub fn is_candidate(content_type: &str, filename: &str) -> bool {
    let ct = base_type(content_type);
    let ext = keys::sanitize_ext(filename);
    let ext = ext.as_str();
    match ct.as_str() {
        "image/jpeg"
        | "image/jpg"
        | "image/png"
        | "image/webp"
        | "image/heic"
        | "image/heif"
        | "image/heic-sequence"
        | "image/heif-sequence"
        | "image/avif"
        | "image/tiff" => true,
        "" | "application/octet-stream" | "binary/octet-stream" => {
            matches!(ext, "jpg" | "jpeg" | "png" | "webp") || HEIF_EXTS.contains(&ext) || RAW_EXTS.contains(&ext)
        }
        // image/x-canon-cr2, image/x-nikon-nef, image/x-adobe-dng, image/x-raw, ...
        t if t.starts_with("image/") => {
            t.contains("-raw") || t.contains("x-") || RAW_EXTS.contains(&ext) || HEIF_EXTS.contains(&ext)
        }
        _ => false,
    }
}

/// Decide the decoder from the file's leading bytes (plus the filename to tell RAW from a plain
/// TIFF). `None` means "not something we decode".
pub fn sniff(bytes: &[u8], filename: &str) -> Option<Kind> {
    let ext = keys::sanitize_ext(filename);
    let ext = ext.as_str();
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Kind::Jpeg);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Kind::Png);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(Kind::Webp);
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return match ftyp_family(bytes) {
            Ftyp::Heif => Some(Kind::Heif),
            Ftyp::CanonRaw => Some(Kind::Raw),
            Ftyp::Other => None,
        };
    }
    // Non-TIFF RAW containers with their own magic.
    if bytes.starts_with(b"FUJIFILMCCD-RAW")      // RAF
        || bytes.starts_with(b"\x00MRM")            // MRW
        || bytes.starts_with(b"FOVb")               // X3F
        || (bytes.len() > 14 && &bytes[6..14] == b"HEAPCCDR")  // CRW
        || bytes.starts_with(b"IIU\0")              // RW2 / RWL (Panasonic)
        || bytes.starts_with(b"IIRO")
        || bytes.starts_with(b"IIRS")
        || bytes.starts_with(b"MMOR")
    // ORF
    {
        return Some(Kind::Raw);
    }
    // TIFF-based RAW (CR2, NEF, NRW, ARW, PEF, DNG, SRW, ...) share the plain TIFF magic.
    // A genuine .tif/.tiff is not RAW; anything else with TIFF magic goes to the RAW pipeline,
    // which declines it cleanly if the camera is unknown and no preview exists.
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return if matches!(ext, "tif" | "tiff") {
            None
        } else {
            Some(Kind::Raw)
        };
    }
    None
}

enum Ftyp {
    Heif,
    CanonRaw,
    Other,
}

/// Classify an ISO-BMFF `ftyp` box by its major and compatible brands.
fn ftyp_family(bytes: &[u8]) -> Ftyp {
    let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let end = size.clamp(16, bytes.len().min(256));
    let brands = std::iter::once(&bytes[8..12]).chain(bytes[16..end].chunks_exact(4));
    let mut heif = false;
    for b in brands {
        match b {
            b"crx " => return Ftyp::CanonRaw,
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"hevm" | b"hevs" | b"mif1" | b"msf1"
            | b"avif" | b"avis" | b"mif2" => heif = true,
            _ => {}
        }
    }
    if heif { Ftyp::Heif } else { Ftyp::Other }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftyp(major: &[u8; 4], compat: &[&[u8; 4]]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(((16 + 4 * compat.len()) as u32).to_be_bytes());
        b.extend(b"ftyp");
        b.extend(major);
        b.extend([0, 0, 0, 0]);
        for c in compat {
            b.extend(*c);
        }
        b
    }

    #[test]
    fn sniff_by_magic_ignores_name() {
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0], "IMG.HEIC"), Some(Kind::Jpeg));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nxxxx", "a.jpg"), Some(Kind::Png));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 ", "a"), Some(Kind::Webp));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt ", "a.webp"), None);
    }

    #[test]
    fn sniff_heif_family() {
        assert_eq!(sniff(&ftyp(b"heic", &[b"mif1", b"heic"]), "a.bin"), Some(Kind::Heif));
        assert_eq!(sniff(&ftyp(b"mif1", &[b"avif"]), "a"), Some(Kind::Heif));
        assert_eq!(sniff(&ftyp(b"avif", &[]), "a"), Some(Kind::Heif));
        assert_eq!(sniff(&ftyp(b"isom", &[b"mp42"]), "a.heic"), None); // an mp4 renamed
        assert_eq!(sniff(&ftyp(b"qt  ", &[]), "a.mov"), None);
    }

    #[test]
    fn sniff_raw() {
        assert_eq!(sniff(&ftyp(b"crx ", &[b"isom"]), "a.cr3"), Some(Kind::Raw));
        assert_eq!(sniff(b"FUJIFILMCCD-RAW 0201", "a.raf"), Some(Kind::Raw));
        assert_eq!(sniff(b"IIU\0\x18\0\0\0", "a.rw2"), Some(Kind::Raw));
        assert_eq!(sniff(b"IIRO\x08\0\0\0", "a.orf"), Some(Kind::Raw));
        assert_eq!(sniff(b"II*\0\x08\0\0\0", "a.nef"), Some(Kind::Raw));
        assert_eq!(sniff(b"MM\0*\0\0\0\x08", "a.dng"), Some(Kind::Raw));
        // generic TIFF is not RAW
        assert_eq!(sniff(b"II*\0\x08\0\0\0", "scan.tif"), None);
        // octet-stream upload with no usable name still routes by magic
        assert_eq!(sniff(b"II*\0\x08\0\0\0", "upload"), Some(Kind::Raw));
        assert_eq!(sniff(b"%PDF-1.7", "a.pdf"), None);
        assert_eq!(sniff(b"", "a.jpg"), None);
    }

    #[test]
    fn candidates() {
        assert!(is_candidate("image/jpeg", "a.jpg"));
        assert!(is_candidate("image/heic", "a"));
        assert!(is_candidate("application/octet-stream", "IMG_0001.CR3"));
        assert!(is_candidate("application/octet-stream", "x.HEIC"));
        assert!(is_candidate("image/x-canon-cr2", "a"));
        assert!(is_candidate("", "a.nef"));
        assert!(!is_candidate("application/octet-stream", "notes.txt"));
        assert!(!is_candidate("video/mp4", "a.mp4"));
        assert!(!is_candidate("application/pdf", "a.pdf"));
    }
}
