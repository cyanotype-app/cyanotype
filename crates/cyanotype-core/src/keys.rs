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

/// Everything under this prefix is an immutable original (see [`original`]).
pub const ORIGINALS_PREFIX: &str = "originals/";
/// Everything under this prefix is derived and regenerable (see [`thumb`], [`preview`]).
pub const DERIVED_PREFIX: &str = "derived/";
/// Per-user recovery manifests (`manifest` module); regenerable from the database, useful when it is lost.
pub const MANIFESTS_PREFIX: &str = "manifests/";

/// A parsed `originals/{user}/{hash[0:2]}/{hash}.{ext}` key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedOriginal {
    pub user: Uuid,
    pub sha256: [u8; 32],
    /// Extension exactly as it appears in the key (1 to 8 ASCII letters or digits).
    pub ext: String,
}

/// Inverse of [`original`]. Strict: anything this code could not have produced (other depth, a non-canonical
/// user id, a shard directory that is not the hash's first two characters, a bad extension) is None. A hash or
/// extension in upper case is accepted, since the key is kept verbatim and the hash is compared as bytes.
pub fn parse_original(key: &str) -> Option<ParsedOriginal> {
    let rest = key.strip_prefix(ORIGINALS_PREFIX)?;
    let mut parts = rest.split('/');
    let (user, shard, file) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let uid = Uuid::parse_str(user).ok()?;
    if uid.hyphenated().to_string() != user {
        return None;
    }
    let (hash, ext) = file.split_once('.')?;
    if ext.is_empty() || ext.len() > 8 || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let sha256 = parse_sha256(hash)?;
    if !hash[..2].eq_ignore_ascii_case(shard) || shard.len() != 2 {
        return None;
    }
    Some(ParsedOriginal {
        user: uid,
        sha256,
        ext: ext.to_string(),
    })
}

/// Prefix of all of a user's manifests.
pub fn manifest_prefix(user: Uuid) -> String {
    format!("{MANIFESTS_PREFIX}{user}/")
}

/// One day's manifest of a user: `manifests/{user}/originals-YYYYMMDD.jsonl`.
pub fn manifest(user: Uuid, yyyymmdd: &str) -> String {
    format!("{}originals-{yyyymmdd}.jsonl", manifest_prefix(user))
}

/// The asset id of a `derived/{user}/{asset}/thumb.webp` or `preview.webp` key, with the file name.
pub fn parse_derived(key: &str) -> Option<(Uuid, Uuid, &str)> {
    let rest = key.strip_prefix(DERIVED_PREFIX)?;
    let mut parts = rest.split('/');
    let (user, asset, file) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some((Uuid::parse_str(user).ok()?, Uuid::parse_str(asset).ok()?, file))
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
    fn parse_original_round_trips_and_rejects_foreign_keys() {
        let u = Uuid::new_v4();
        let sha = [0xab; 32];
        for ext in ["jpg", "bin", "heic", "dng", "a1b2c3d4"] {
            let p = parse_original(&original(u, &sha, ext)).unwrap();
            assert_eq!((p.user, p.sha256, p.ext.as_str()), (u, sha, ext));
        }
        let good = original(u, &sha, "jpg");
        // Uppercase extension and hash are accepted (and kept verbatim by the caller).
        assert_eq!(parse_original(&good.replace(".jpg", ".JPG")).unwrap().ext, "JPG");
        assert!(parse_original(&good.to_ascii_uppercase().replace("ORIGINALS", "originals")).is_none()); // uuid case
        let upper_hash = format!("originals/{u}/AB/{}.jpg", "AB".repeat(32));
        assert_eq!(parse_original(&upper_hash).unwrap().sha256, sha);
        for bad in [
            "".to_string(),
            "originals/".to_string(),
            format!("derived/{u}/ab/{}.jpg", "ab".repeat(32)),
            format!("originals/{u}/ab/{}", "ab".repeat(32)), // no extension
            format!("originals/{u}/ab/{}.", "ab".repeat(32)), // empty extension
            format!("originals/{u}/ab/{}.jpeg2000x", "ab".repeat(32)), // 9 chars
            format!("originals/{u}/ab/{}.j-g", "ab".repeat(32)),
            format!("originals/{u}/cd/{}.jpg", "ab".repeat(32)), // wrong shard
            format!("originals/{u}/ab/{}.jpg", "ab".repeat(31)), // short hash
            format!("originals/{u}/ab/{}.jpg", "zz".repeat(32)),
            format!("originals/{u}/ab/x/{}.jpg", "ab".repeat(32)), // too deep
            format!("originals/{}/ab/{}.jpg", u.simple(), "ab".repeat(32)), // non-hyphenated id
            format!("originals/not-a-uuid/ab/{}.jpg", "ab".repeat(32)),
            format!("originals/{u}/ab/{}.tar.gz", "ab".repeat(32)),
        ] {
            assert!(parse_original(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn manifest_and_derived_keys() {
        let (u, a) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(
            manifest(u, "20260101"),
            format!("manifests/{u}/originals-20260101.jsonl")
        );
        assert_eq!(parse_derived(&thumb(u, a)), Some((u, a, "thumb.webp")));
        assert_eq!(parse_derived(&preview(u, a)), Some((u, a, "preview.webp")));
        assert!(parse_derived(&original(u, &[1; 32], "jpg")).is_none());
    }

    #[test]
    fn original_layout() {
        let u = Uuid::nil();
        let k = original(u, &[0xab; 32], "jpg");
        assert!(k.starts_with(&format!("originals/{u}/ab/abab")) && k.ends_with(".jpg"));
    }
}
