//! TOTP (RFC 6238) and the server-side protection of its secrets, with nothing but `ring`.
//!
//! * The code: HMAC-SHA1 (the algorithm every authenticator app supports; SHA-1 is safe here because a TOTP secret is a
//!   random key, not a collision target), 30 second steps, 6 digits, and a window of one step either side of "now" for clock
//!   drift. A step is accepted once (`last_step`), so a code seen over a shoulder or on the wire cannot be replayed.
//! * At rest: a secret is sealed with AES-256-GCM under a key derived from `AUTH_TOTP_KEY`, with the account id as
//!   associated data (a sealed secret cannot be moved to another account's row). Recovery codes are stored only as an
//!   HMAC-SHA256 under another key derived from the same one: unlike a bare hash, a copy of the database alone cannot be
//!   brute-forced offline to recover a code.
//! * Nothing is ever stored in clear, and when the key is unset nothing can be sealed, so enrolment is refused.
use anyhow::{Result, anyhow, bail};
use rand::RngCore;
use ring::{aead, hmac};
use uuid::Uuid;

pub const STEP_SECS: u64 = 30;
pub const DIGITS: u32 = 6;
/// Steps accepted either side of the current one.
pub const WINDOW: i64 = 1;
/// Bytes of a new secret: 160 bits, the size RFC 4226 recommends.
pub const SECRET_BYTES: usize = 20;
pub const RECOVERY_CODES: usize = 10;

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32 without padding (what authenticator apps take as the manual-entry secret).
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(B32[((buf >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let (mut buf, mut bits) = (0u32, 0u32);
    let mut out = Vec::new();
    for c in s.bytes().filter(|c| *c != b'=' && !c.is_ascii_whitespace()) {
        let v = B32.iter().position(|b| *b == c.to_ascii_uppercase())? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push((buf >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

/// RFC 4226 HOTP with SHA-1: `digits` decimal digits of the dynamically truncated MAC.
pub fn hotp(secret: &[u8], counter: u64, digits: u32) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let mac = hmac::sign(&key, &counter.to_be_bytes());
    let mac = mac.as_ref();
    let offset = (mac[mac.len() - 1] & 0x0f) as usize;
    let bin = u32::from_be_bytes([mac[offset] & 0x7f, mac[offset + 1], mac[offset + 2], mac[offset + 3]]);
    bin % 10u32.pow(digits)
}

/// The code for the step containing `unix`.
#[cfg(test)]
pub fn code_at(secret: &[u8], unix: u64) -> String {
    format!(
        "{:0width$}",
        hotp(secret, unix / STEP_SECS, DIGITS),
        width = DIGITS as usize
    )
}

/// The step `code` is valid for around `unix`, if any, and newer than `after_step` (the last one this account used).
/// All window steps are compared in constant time, whether or not an earlier one matched.
pub fn verify(secret: &[u8], code: &str, unix: u64, after_step: Option<i64>) -> Option<i64> {
    let code = code.trim();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let now = (unix / STEP_SECS) as i64;
    let mut found = None;
    for step in now - WINDOW..=now + WINDOW {
        if step < 0 {
            continue;
        }
        let expect = format!("{:0width$}", hotp(secret, step as u64, DIGITS), width = DIGITS as usize);
        if ct_eq(expect.as_bytes(), code.as_bytes()) && after_step.is_none_or(|a| step > a) {
            found = Some(step);
        }
    }
    found
}

/// Equality without an early exit on the first differing byte (both are short ASCII strings of the same length here).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn new_secret() -> Vec<u8> {
    let mut b = vec![0u8; SECRET_BYTES];
    rand::thread_rng().fill_bytes(&mut b);
    b
}

/// The `otpauth://` URI authenticator apps read from a QR code.
pub fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
        pct(issuer),
        pct(account),
        base32_encode(secret),
        pct(issuer)
    )
}

fn pct(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// The server key (`AUTH_TOTP_KEY`) and the two keys derived from it.
#[derive(Clone)]
pub struct TotpKey {
    seal: aead::LessSafeKey,
    recovery: hmac::Key,
}

impl std::fmt::Debug for TotpKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TotpKey(..)")
    }
}

impl TotpKey {
    /// From the 32 raw bytes of the key.
    pub fn new(master: &[u8; 32]) -> Self {
        let m = hmac::Key::new(hmac::HMAC_SHA256, master);
        let derive = |label: &[u8]| hmac::sign(&m, label);
        let seal = aead::UnboundKey::new(&aead::AES_256_GCM, derive(b"cyanotype totp secret v1").as_ref())
            .expect("a 32 byte key");
        Self {
            seal: aead::LessSafeKey::new(seal),
            recovery: hmac::Key::new(hmac::HMAC_SHA256, derive(b"cyanotype totp recovery v1").as_ref()),
        }
    }

    /// Parse `AUTH_TOTP_KEY`: 32 bytes as base64 (`openssl rand -base64 32`) or 64 hex characters.
    pub fn parse(v: &str) -> Result<Self> {
        use base64::Engine;
        let v = v.trim();
        let bytes = if v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()) {
            (0..32)
                .map(|i| u8::from_str_radix(&v[i * 2..i * 2 + 2], 16))
                .collect::<Result<Vec<u8>, _>>()?
        } else {
            base64::engine::general_purpose::STANDARD
                .decode(v)
                .map_err(|_| anyhow!("AUTH_TOTP_KEY must be 32 random bytes as base64 or 64 hex characters"))?
        };
        let arr: [u8; 32] = bytes.try_into().map_err(|_| {
            anyhow!("AUTH_TOTP_KEY must be exactly 32 bytes (generate one with `openssl rand -base64 32`)")
        })?;
        Ok(Self::new(&arr))
    }

    /// `nonce (12) || ciphertext || tag`, bound to `user`.
    pub fn seal(&self, user: Uuid, secret: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; aead::NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let mut buf = secret.to_vec();
        self.seal
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(user.as_bytes()),
                &mut buf,
            )
            .expect("sealing in memory cannot fail");
        let mut out = nonce.to_vec();
        out.extend(buf);
        out
    }

    pub fn open(&self, user: Uuid, sealed: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < aead::NONCE_LEN + 16 {
            bail!("sealed secret is too short");
        }
        let (nonce, rest) = sealed.split_at(aead::NONCE_LEN);
        let mut buf = rest.to_vec();
        let nonce = aead::Nonce::try_assume_unique_for_key(nonce).map_err(|_| anyhow!("bad nonce"))?;
        let plain = self
            .seal
            .open_in_place(nonce, aead::Aad::from(user.as_bytes()), &mut buf)
            .map_err(|_| anyhow!("cannot open the sealed secret (the server key changed?)"))?;
        Ok(plain.to_vec())
    }

    /// What is stored for a recovery code (after [`normalise_recovery`]).
    pub fn recovery_hash(&self, code: &str) -> Vec<u8> {
        hmac::sign(&self.recovery, code.as_bytes()).as_ref().to_vec()
    }
}

/// A new recovery code: 16 characters of base32 (80 bits) in groups of four, lower case: `abcd-efgh-2345-ijkl`.
pub fn new_recovery_code() -> String {
    let mut b = [0u8; 10];
    rand::thread_rng().fill_bytes(&mut b);
    let s = base32_encode(&b).to_ascii_lowercase();
    s.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("-")
}

/// What a person typed, reduced to what is hashed: lower case, without dashes or spaces.
pub fn normalise_recovery(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A TOTP code as typed: digits, maybe with a space in the middle (`123 456`).
pub fn looks_like_totp(s: &str) -> bool {
    let d: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    d.len() == DIGITS as usize && d.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 appendix B: the SHA-1 secret is the ASCII string "12345678901234567890"; the RFC lists 8-digit codes.
    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc6238_sha1_vectors() {
        for (t, code8) in [
            (59u64, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ] {
            assert_eq!(format!("{:08}", hotp(RFC_SECRET, t / STEP_SECS, 8)), code8, "t = {t}");
            // The 6-digit code is the last six digits of the 8-digit one.
            assert_eq!(code_at(RFC_SECRET, t), code8[2..], "t = {t}");
        }
    }

    #[test]
    fn rfc4226_hotp_vectors() {
        let want = [
            "755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583", "399871", "520489",
        ];
        for (i, w) in want.iter().enumerate() {
            assert_eq!(format!("{:06}", hotp(RFC_SECRET, i as u64, 6)), *w);
        }
    }

    #[test]
    fn window_is_one_step_either_side() {
        let t = 1_700_000_010u64;
        let at = |t: u64| code_at(RFC_SECRET, t);
        let step = (t / STEP_SECS) as i64;
        assert_eq!(verify(RFC_SECRET, &at(t), t, None), Some(step));
        assert_eq!(verify(RFC_SECRET, &at(t - 30), t, None), Some(step - 1));
        assert_eq!(verify(RFC_SECRET, &at(t + 30), t, None), Some(step + 1));
        assert_eq!(verify(RFC_SECRET, &at(t - 60), t, None), None);
        assert_eq!(verify(RFC_SECRET, &at(t + 60), t, None), None);
    }

    #[test]
    fn a_step_is_accepted_once() {
        let t = 1_700_000_010u64;
        let c = code_at(RFC_SECRET, t);
        let step = verify(RFC_SECRET, &c, t, None).unwrap();
        assert_eq!(verify(RFC_SECRET, &c, t, Some(step)), None);
        // A code from an earlier step than the last used one is also refused.
        assert_eq!(verify(RFC_SECRET, &code_at(RFC_SECRET, t - 30), t, Some(step)), None);
        assert!(verify(RFC_SECRET, &c, t, Some(step - 1)).is_some());
    }

    #[test]
    fn malformed_codes_never_match() {
        let t = 1_700_000_010u64;
        for bad in ["", "12345", "1234567", "12345a", "      ", "１２３４５６"] {
            assert_eq!(verify(RFC_SECRET, bad, t, None), None, "{bad:?}");
        }
        assert!(looks_like_totp("123 456") && looks_like_totp("000000") && !looks_like_totp("abcd-efgh"));
    }

    #[test]
    fn base32_round_trips_and_matches_rfc4648() {
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32_encode(b"fooba"), "MZXW6YTB");
        assert_eq!(base32_encode(b"f"), "MY");
        assert_eq!(base32_decode("mzxw 6ytb oi").unwrap(), b"foobar");
        assert_eq!(base32_decode("MZXW6YTBOI======").unwrap(), b"foobar");
        assert!(base32_decode("1").is_none());
        let s = new_secret();
        assert_eq!(base32_decode(&base32_encode(&s)).unwrap(), s);
    }

    #[test]
    fn otpauth_uri_is_what_apps_expect() {
        let u = otpauth_uri("Cyanotype", "a+b@example.com", RFC_SECRET);
        assert_eq!(
            u,
            "otpauth://totp/Cyanotype:a%2Bb%40example.com?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Cyanotype&algorithm=SHA1&digits=6&period=30"
        );
    }

    #[test]
    fn sealed_secrets_round_trip_and_are_bound_to_the_account() {
        let k = TotpKey::new(&[7u8; 32]);
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let sealed = k.seal(a, RFC_SECRET);
        assert!(
            !sealed.windows(RFC_SECRET.len()).any(|w| w == RFC_SECRET),
            "no plaintext in the blob"
        );
        assert_eq!(k.open(a, &sealed).unwrap(), RFC_SECRET);
        assert!(k.open(b, &sealed).is_err(), "another account's id cannot open it");
        assert!(
            TotpKey::new(&[8u8; 32]).open(a, &sealed).is_err(),
            "another key cannot open it"
        );
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(k.open(a, &tampered).is_err());
        // Two seals of the same secret differ (fresh nonce).
        assert_ne!(k.seal(a, RFC_SECRET), sealed);
    }

    #[test]
    fn key_parsing() {
        let hex = "00".repeat(32);
        assert!(TotpKey::parse(&hex).is_ok());
        assert!(TotpKey::parse("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").is_ok());
        assert!(TotpKey::parse("tooshort").is_err());
        assert!(TotpKey::parse("AAAA").is_err());
        assert!(TotpKey::parse(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn recovery_codes_are_distinct_hashed_and_forgiving_to_type() {
        let k = TotpKey::new(&[1u8; 32]);
        let c = new_recovery_code();
        assert_eq!(c.len(), 19);
        assert_eq!(c.matches('-').count(), 3);
        assert_ne!(c, new_recovery_code());
        let typed = format!("  {} ", c.to_ascii_uppercase().replace('-', " "));
        assert_eq!(
            k.recovery_hash(&normalise_recovery(&typed)),
            k.recovery_hash(&normalise_recovery(&c))
        );
        assert_ne!(k.recovery_hash("abcd"), TotpKey::new(&[2u8; 32]).recovery_hash("abcd"));
    }
}
