//! S3 key layout. Originals are immutable and content-addressed.
use uuid::Uuid;

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse_sha256(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

/// Lowercased alphanumeric extension, or "bin".
pub fn sanitize_ext(filename: &str) -> String {
    filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "bin".into())
}

pub fn original(user: Uuid, sha256: &[u8; 32], ext: &str) -> String {
    let h = hex(sha256);
    format!("originals/{user}/{}/{h}.{ext}", &h[..2])
}

pub fn thumb(user: Uuid, asset: Uuid) -> String {
    format!("derived/{user}/{asset}/thumb.webp")
}

pub fn preview(user: Uuid, asset: Uuid) -> String {
    format!("derived/{user}/{asset}/preview.webp")
}

/// Prefix of everything derived from one asset (thumbnail, preview, and whatever later stages add).
/// Purging an asset deletes all versions of every key under it.
pub fn derived_prefix(user: Uuid, asset: Uuid) -> String {
    format!("derived/{user}/{asset}/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_roundtrip() {
        let h = "00ff".repeat(16);
        assert_eq!(hex(&parse_sha256(&h).unwrap()), h);
        assert!(parse_sha256("zz").is_none());
    }

    #[test]
    fn ext() {
        assert_eq!(sanitize_ext("IMG_0001.HEIC"), "heic");
        assert_eq!(sanitize_ext("noext"), "bin");
        assert_eq!(sanitize_ext("a.b/../c"), "bin");
    }

    #[test]
    fn derived_keys_live_under_the_derived_prefix() {
        let (u, a) = (Uuid::new_v4(), Uuid::new_v4());
        let p = derived_prefix(u, a);
        assert!(thumb(u, a).starts_with(&p) && preview(u, a).starts_with(&p));
    }

    #[test]
    fn original_layout() {
        let u = Uuid::nil();
        let k = original(u, &[0xab; 32], "jpg");
        assert!(k.starts_with(&format!("originals/{u}/ab/abab")) && k.ends_with(".jpg"));
    }
}
