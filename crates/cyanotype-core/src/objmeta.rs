//! Self-describing originals. Every original is stored with S3 user metadata (`x-amz-meta-*`) so that a bucket
//! scan can rebuild an asset row without the database: the original filename, the owning user, the upload time and
//! the client-reported file mtime. The content type is the object's ordinary `Content-Type`.
//!
//! S3 metadata values must be printable ASCII and the user-defined part of a request is limited to 2 KiB in total,
//! so the filename is percent-encoded (every byte outside `A-Za-z0-9._~-`, which also covers quotes, control
//! characters and non-ASCII) and clipped to [`MAX_NAME_ENCODED`] encoded bytes, keeping the extension.
//! Keys are written without the `x-amz-meta-` prefix; S3 lower-cases them.
use std::collections::HashMap;

use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

pub const K_VERSION: &str = "cyanotype-v";
pub const K_NAME: &str = "cyanotype-name";
pub const K_OWNER: &str = "cyanotype-owner";
pub const K_CREATED: &str = "cyanotype-created";
pub const K_MTIME: &str = "cyanotype-mtime";
/// Current metadata layout version.
pub const VERSION: &str = "1";
/// Longest percent-encoded filename stored. Leaves ample room under the 2 KiB metadata limit.
pub const MAX_NAME_ENCODED: usize = 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OriginalMeta {
    pub name: Option<String>,
    pub owner: Option<Uuid>,
    pub created_at: Option<OffsetDateTime>,
    pub file_modified_at: Option<OffsetDateTime>,
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-')
}

fn encoded_len(c: char) -> usize {
    let mut buf = [0u8; 4];
    c.encode_utf8(&mut buf)
        .bytes()
        .map(|b| if is_unreserved(b) { 1 } else { 3 })
        .sum()
}

/// Percent-encode `s` (UTF-8 bytes outside `A-Za-z0-9._~-`).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Inverse of [`percent_encode`]. Malformed escapes are kept literally; invalid UTF-8 is replaced.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Clip `name` so its encoding fits `max_encoded` bytes, keeping a short extension. Whole characters only.
pub fn clip_name(name: &str, max_encoded: usize) -> String {
    if percent_encode(name).len() <= max_encoded {
        return name.to_string();
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e)
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric()))
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let stem_budget = max_encoded.saturating_sub(ext.len());
    let mut used = 0;
    let mut out = String::new();
    for c in name[..name.len() - ext.len()].chars() {
        let n = encoded_len(c);
        if used + n > stem_budget {
            break;
        }
        used += n;
        out.push(c);
    }
    out.push_str(&ext);
    out
}

fn fmt_time(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn parse_time(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// The user metadata to send with `CreateMultipartUpload` / `CopyObject`.
pub fn encode(m: &OriginalMeta) -> HashMap<String, String> {
    let mut out = HashMap::new();
    out.insert(K_VERSION.to_string(), VERSION.to_string());
    if let Some(n) = &m.name {
        out.insert(K_NAME.into(), percent_encode(&clip_name(n, MAX_NAME_ENCODED)));
    }
    if let Some(o) = m.owner {
        out.insert(K_OWNER.into(), o.to_string());
    }
    if let Some(t) = m.created_at {
        out.insert(K_CREATED.into(), fmt_time(t));
    }
    if let Some(t) = m.file_modified_at {
        out.insert(K_MTIME.into(), fmt_time(t));
    }
    out
}

/// Read what [`encode`] wrote. Unknown or malformed fields are None; objects from before this feature decode to
/// an all-None value. Key matching is case-insensitive.
pub fn decode(map: &HashMap<String, String>) -> OriginalMeta {
    let get = |k: &str| {
        map.iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    };
    OriginalMeta {
        name: get(K_NAME).map(percent_decode).filter(|n| !n.is_empty()),
        owner: get(K_OWNER).and_then(|v| Uuid::parse_str(v).ok()),
        created_at: get(K_CREATED).and_then(parse_time),
        file_modified_at: get(K_MTIME).and_then(parse_time),
    }
}

/// Whether the object carries this feature's metadata (what `--backfill-metadata` looks for).
pub fn is_described(map: &HashMap<String, String>) -> bool {
    map.keys().any(|k| k.eq_ignore_ascii_case(K_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn percent_round_trip() {
        for s in [
            "IMG_0001.JPG",
            "Café déjà vu 日本語 🌅.heic",
            "quote\"s and 'apos' and \\ and %41 and +plus",
            "tab\tnew\nline\r\0nul",
            "",
            "a b",
        ] {
            let e = percent_encode(s);
            assert!(
                e.bytes().all(|b| b.is_ascii_alphanumeric() || b"._~-%".contains(&b)),
                "{e}"
            );
            assert_eq!(percent_decode(&e), s);
        }
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
    }

    #[test]
    fn encode_decode_round_trip() {
        let m = OriginalMeta {
            name: Some("Süd-Tirol \"Urlaub\" 2019.JPG".into()),
            owner: Some(Uuid::new_v4()),
            created_at: Some(datetime!(2026-05-01 12:34:56 UTC)),
            file_modified_at: Some(datetime!(2019-08-01 07:00:01 +02:00)),
        };
        let enc = encode(&m);
        assert!(
            enc.values()
                .all(|v| v.is_ascii() && v.bytes().all(|b| (0x20..0x7f).contains(&b)))
        );
        assert_eq!(enc[K_VERSION], VERSION);
        assert_eq!(decode(&enc), m);
        assert!(is_described(&enc));
        // S3 may hand keys back in another case.
        let upper: HashMap<_, _> = enc.iter().map(|(k, v)| (k.to_ascii_uppercase(), v.clone())).collect();
        assert_eq!(decode(&upper), m);
    }

    #[test]
    fn legacy_and_garbage_decode_to_nothing() {
        assert_eq!(decode(&HashMap::new()), OriginalMeta::default());
        assert!(!is_described(&HashMap::new()));
        let junk = HashMap::from([
            (K_OWNER.to_string(), "nope".to_string()),
            (K_CREATED.to_string(), "yesterday".to_string()),
            (K_NAME.to_string(), String::new()),
        ]);
        assert_eq!(decode(&junk), OriginalMeta::default());
    }

    #[test]
    fn very_long_names_are_clipped_on_a_character_boundary_and_keep_the_extension() {
        let name = format!("{}.jpeg", "日".repeat(300)); // 9 encoded bytes per character
        let m = OriginalMeta {
            name: Some(name),
            ..Default::default()
        };
        let enc = encode(&m);
        assert!(enc[K_NAME].len() <= MAX_NAME_ENCODED, "{}", enc[K_NAME].len());
        let back = decode(&enc).name.unwrap();
        assert!(back.ends_with(".jpeg") && back.starts_with('日'));
        assert!(back.chars().count() > 100);
        // Short names are untouched.
        assert_eq!(clip_name("a.jpg", MAX_NAME_ENCODED), "a.jpg");
        // No extension at all.
        let c = clip_name(&"x".repeat(5000), 100);
        assert_eq!(c.len(), 100);
        // The whole metadata block stays well under S3's 2 KiB.
        let total: usize = enc.iter().map(|(k, v)| k.len() + v.len()).sum();
        assert!(total < 1500, "{total}");
    }
}
