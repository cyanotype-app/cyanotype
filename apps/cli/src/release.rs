//! The pure parts of self-update: version comparison, the `latest.json` the server publishes, SHA256SUMS parsing
//! and signature verification. No I/O here, so every rule is unit-tested.
//!
//! Trust chain (docs/releases.md): the CLI has the release public key compiled in. `SHA256SUMS` is signed with the
//! matching private key (an `ssh-keygen -Y sign` signature, namespace [`NAMESPACE`]) and lists the hash of every
//! file the server hands out, `latest.json` included. So a build is installed only if its hash is in a signed list.
use std::{cmp::Ordering, collections::BTreeMap, fmt};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};

/// SSH signature namespace: a signature made for anything else (a git commit, say) is not a release signature.
pub const NAMESPACE: &str = "cyanotype-cli";

/// This build's version (`0.3.1`, or `0.0.1-dev` for a build that is not a release).
pub const VERSION: &str = env!("CYANOTYPE_VERSION");
/// The Rust target triple this binary was built for; it names the right file in `latest.json`.
pub const TARGET: &str = env!("CYANOTYPE_TARGET");
/// `--version` output after the program name.
pub const LONG_VERSION: &str = concat!(env!("CYANOTYPE_VERSION"), " (", env!("CYANOTYPE_SHA"), ")");

pub fn is_release_build() -> bool {
    env!("CYANOTYPE_IS_RELEASE") == "true"
}

/// The release public key line compiled in from `apps/cli/release-key.pub` (None until the owner has set one up).
pub fn compiled_in_key() -> Option<&'static str> {
    include_str!("../release-key.pub")
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("ssh-ed25519 "))
}

// ---- versions -------------------------------------------------------------------------------------------------

/// `MAJOR.MINOR.PATCH` with an optional `-prerelease` (a pre-release sorts below its release). Build metadata
/// (`+...`) and a leading `v` or `cli-v` are accepted and ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(s: &str) -> Result<Version> {
        let orig = s;
        let s = s.trim();
        let s = s.strip_prefix("cli-").unwrap_or(s);
        let s = s.strip_prefix('v').unwrap_or(s);
        let s = s.split('+').next().unwrap_or(s);
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
            Some(_) => bail!("not a version: {orig:?}"),
            None => (s, None),
        };
        let mut parts = core.split('.');
        let mut num = || -> Result<u64> {
            let p = parts.next().with_context(|| format!("not a version: {orig:?}"))?;
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                bail!("not a version: {orig:?}");
            }
            p.parse().with_context(|| format!("not a version: {orig:?}"))
        };
        let (major, minor, patch) = (num()?, num()?, num()?);
        if parts.next().is_some() {
            bail!("not a version: {orig:?}");
        }
        Ok(Version {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(p) = &self.pre {
            write!(f, "-{p}")?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(o.major, o.minor, o.patch))
            .then_with(|| match (&self.pre, &o.pre) {
                (None, None) => Ordering::Equal,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(a), Some(b)) => cmp_pre(a, b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// SemVer pre-release precedence: dot-separated identifiers, numbers numerically and below text.
fn cmp_pre(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.split('.'), b.split('.'));
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

// ---- latest.json ----------------------------------------------------------------------------------------------

/// `/cli/latest.json`, written by `scripts/make-cli-dist.sh` into the web image.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Manifest {
    /// Version of this file's layout; a CLI that does not know it must not guess.
    pub schema: u32,
    pub version: String,
    pub git_sha: String,
    /// False for a dist built without the release key: such builds are never installed by `cyanotype update`.
    pub signed: bool,
    /// The oldest CLI this server accepts for `import`.
    pub min_cli_version: String,
    /// The server API level these CLIs need at least (reserved: 1 until the HTTP API gets a breaking change).
    pub min_server_api: u32,
    /// Rust target triple -> file.
    pub targets: BTreeMap<String, Artifact>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Artifact {
    pub file: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

pub const MANIFEST_SCHEMA: u32 = 1;

impl Manifest {
    pub fn parse(body: &str) -> Result<Manifest> {
        let m: Manifest = serde_json::from_str(body).context("latest.json is not valid")?;
        if m.schema != MANIFEST_SCHEMA {
            bail!(
                "latest.json uses layout {} but this cyanotype understands layout {MANIFEST_SCHEMA}; \
                 update by hand (re-run the install script)",
                m.schema
            );
        }
        Version::parse(&m.version).context("latest.json: version")?;
        Version::parse(&m.min_cli_version).context("latest.json: min_cli_version")?;
        for (t, a) in &m.targets {
            // The file name is joined onto the server URL and a path; refuse anything but a plain file name.
            if a.file.is_empty() || a.file.contains(['/', '\\']) || a.file.starts_with('.') {
                bail!("latest.json: bad file name for {t}");
            }
        }
        Ok(m)
    }

    pub fn version(&self) -> Version {
        Version::parse(&self.version).expect("validated in parse")
    }

    pub fn min_cli(&self) -> Version {
        Version::parse(&self.min_cli_version).expect("validated in parse")
    }
}

/// Whether a CLI at `current` may run `import` against a server that declares `min`. Development builds always pass.
pub fn meets_minimum(current: &Version, min: &Version) -> bool {
    current.pre.as_deref() == Some("dev") || current >= min
}

// ---- SHA256SUMS -----------------------------------------------------------------------------------------------

/// `sha256sum` output: `<64 hex>  <file>` (a `*` before the name marks binary mode). Returns file -> lowercase hex.
pub fn parse_sums(text: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (hash, name) = line
            .split_once(' ')
            .with_context(|| format!("bad SHA256SUMS line: {line:?}"))?;
        let name = name.trim_start_matches([' ', '*']);
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || name.is_empty() {
            bail!("bad SHA256SUMS line: {line:?}");
        }
        out.insert(name.to_string(), hash.to_ascii_lowercase());
    }
    Ok(out)
}

pub fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

// ---- SSH signatures (`ssh-keygen -Y sign -n cyanotype-cli`) ----------------------------------------------------

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Result<u32> {
        let (head, rest) = self.0.split_first_chunk::<4>().context("truncated signature")?;
        self.0 = rest;
        Ok(u32::from_be_bytes(*head))
    }
    fn string(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > self.0.len() {
            bail!("truncated signature");
        }
        let (s, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(s)
    }
}

fn put_string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s);
}

/// The 32-byte key out of an `ssh-ed25519 AAAA... comment` line.
pub fn parse_ssh_public_key(line: &str) -> Result<[u8; 32]> {
    let mut it = line.split_whitespace();
    if it.next() != Some("ssh-ed25519") {
        bail!("the release key must be an ssh-ed25519 public key");
    }
    let blob = STANDARD.decode(it.next().context("public key has no data")?)?;
    let mut r = Reader(&blob);
    if r.string()? != b"ssh-ed25519" {
        bail!("public key blob is not ssh-ed25519");
    }
    let key = r.string()?;
    key.try_into()
        .map_err(|_| anyhow::anyhow!("ed25519 public key must be 32 bytes"))
}

/// Check `armored` (the contents of `SHA256SUMS.sig`) as a signature over `message` made by `public_key`
/// (an `ssh-ed25519 ...` line) in [`NAMESPACE`].
pub fn verify_signature(public_key: &str, message: &[u8], armored: &str) -> Result<()> {
    let want_key = parse_ssh_public_key(public_key)?;
    let b64: String = armored
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("-----") && !l.is_empty())
        .collect();
    let blob = STANDARD.decode(b64).context("signature is not valid base64")?;
    let blob = blob.strip_prefix(b"SSHSIG").context("not an SSH signature")?;
    let mut r = Reader(blob);
    if r.u32()? != 1 {
        bail!("unsupported SSH signature version");
    }
    let signer = r.string()?;
    let namespace = r.string()?;
    let reserved = r.string()?;
    let hash_alg = r.string()?;
    let sig_blob = r.string()?;

    let mut kr = Reader(signer);
    if kr.string()? != b"ssh-ed25519" || kr.string()? != want_key {
        bail!("signed by a different key than the release key built into this program");
    }
    if namespace != NAMESPACE.as_bytes() {
        bail!("signature has the wrong namespace");
    }
    let digest: Vec<u8> = match hash_alg {
        b"sha512" => Sha512::digest(message).to_vec(),
        b"sha256" => Sha256::digest(message).to_vec(),
        _ => bail!("unsupported signature hash"),
    };
    let mut sr = Reader(sig_blob);
    if sr.string()? != b"ssh-ed25519" {
        bail!("signature is not ssh-ed25519");
    }
    let sig = sr.string()?;

    let mut signed = b"SSHSIG".to_vec();
    put_string(&mut signed, namespace);
    put_string(&mut signed, reserved);
    put_string(&mut signed, hash_alg);
    put_string(&mut signed, &digest);
    UnparsedPublicKey::new(&ED25519, &want_key)
        .verify(&signed, sig)
        .map_err(|_| anyhow::anyhow!("the signature does not match (the download is corrupt or was tampered with)"))
}

/// The whole check for one downloaded file: `sums` must carry a valid signature, and list `file` with the hash of `data`.
pub fn verify_download(public_key: &str, sums: &str, armored_sig: &str, file: &str, data: &[u8]) -> Result<()> {
    verify_signature(public_key, sums.as_bytes(), armored_sig)?;
    let listed = parse_sums(sums)?;
    let want = listed
        .get(file)
        .with_context(|| format!("{file} is not in the signed SHA256SUMS"))?;
    let got = sha256_hex(data);
    if &got != want {
        bail!("checksum mismatch for {file}: signed list says {want}, download is {got}");
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parse_and_display() {
        assert_eq!(v("1.2.3").to_string(), "1.2.3");
        assert_eq!(v("cli-v1.2.3"), v("1.2.3"));
        assert_eq!(v("v0.10.0+abc"), v("0.10.0"));
        assert_eq!(v("0.0.1-dev").to_string(), "0.0.1-dev");
        for bad in ["", "1", "1.2", "1.2.3.4", "a.b.c", "1.2.x", "1.2.3-", "-1.2.3", "1..3"] {
            assert!(Version::parse(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn ordering() {
        assert!(v("0.10.0") > v("0.9.9"), "numeric, not lexical");
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("1.2.4") > v("1.2.3"));
        assert_eq!(v("1.2.3").cmp(&v("1.2.3")), Ordering::Equal);
        assert!(v("1.0.0-dev") < v("1.0.0"));
        assert!(v("1.0.0-dev") > v("0.9.0"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.9"));
        assert!(v("1.0.0-1") < v("1.0.0-alpha"));
        assert!(v("1.0.0-alpha") < v("1.0.0-alpha.1"));
    }

    #[test]
    fn minimum_version_gate() {
        assert!(meets_minimum(&v("0.3.0"), &v("0.3.0")));
        assert!(meets_minimum(&v("0.4.0"), &v("0.3.0")));
        assert!(!meets_minimum(&v("0.2.9"), &v("0.3.0")));
        // A source build is never locked out.
        assert!(meets_minimum(&v("0.0.1-dev"), &v("9.0.0")));
    }

    const LATEST: &str = r#"{
      "schema": 1, "version": "0.3.1", "git_sha": "abc1234", "signed": true,
      "min_cli_version": "0.2.0", "min_server_api": 1,
      "targets": {
        "x86_64-unknown-linux-musl": {"file": "cyanotype-x86_64-unknown-linux-musl.gz", "sha256": "aa", "size": 5},
        "x86_64-pc-windows-gnu": {"file": "cyanotype-x86_64-pc-windows-gnu.exe.gz", "sha256": "bb"}
      },
      "future_field": true
    }"#;

    #[test]
    fn latest_json_parses() {
        let m = Manifest::parse(LATEST).unwrap();
        assert_eq!(m.version(), v("0.3.1"));
        assert_eq!(m.min_cli(), v("0.2.0"));
        assert!(m.signed);
        assert_eq!(m.targets["x86_64-unknown-linux-musl"].size, 5);
        assert_eq!(m.targets["x86_64-pc-windows-gnu"].size, 0);
    }

    #[test]
    fn latest_json_rejects_bad_input() {
        assert!(Manifest::parse("not json").is_err());
        assert!(Manifest::parse(&LATEST.replace("\"schema\": 1", "\"schema\": 2")).is_err());
        assert!(Manifest::parse(&LATEST.replace("0.3.1", "banana")).is_err());
        assert!(Manifest::parse(&LATEST.replace("\"0.2.0\"", "\"\"")).is_err());
        // A file name that would climb out of the download directory.
        assert!(Manifest::parse(&LATEST.replace("cyanotype-x86_64-unknown-linux-musl.gz", "../evil")).is_err());
        assert!(Manifest::parse(&LATEST.replace("\"signed\": true,", "")).is_err());
    }

    #[test]
    fn sums_parse() {
        let h = "a".repeat(64);
        let m = parse_sums(&format!("{h}  one.gz\n{}  *two.gz\n\n", "B".repeat(64))).unwrap();
        assert_eq!(m["one.gz"], h);
        assert_eq!(m["two.gz"], "b".repeat(64));
        assert!(parse_sums("short  x").is_err());
        assert!(parse_sums(&h).is_err());
    }

    // -- signatures: a throwaway key made right here, and a signer that follows PROTOCOL.sshsig. A fixture from the
    // real `ssh-keygen -Y sign` is checked in the test below, so a mistake shared by both sides would still show.

    pub struct TestKey {
        pair: Ed25519KeyPair,
    }

    impl TestKey {
        pub fn generate() -> TestKey {
            let rng = ring::rand::SystemRandom::new();
            let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
            TestKey {
                pair: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap(),
            }
        }

        fn public_blob(&self) -> Vec<u8> {
            let mut b = Vec::new();
            put_string(&mut b, b"ssh-ed25519");
            put_string(&mut b, self.pair.public_key().as_ref());
            b
        }

        pub fn public_line(&self) -> String {
            format!("ssh-ed25519 {} test", STANDARD.encode(self.public_blob()))
        }

        pub fn sign(&self, message: &[u8], namespace: &str) -> String {
            let digest = Sha512::digest(message);
            let mut signed = b"SSHSIG".to_vec();
            put_string(&mut signed, namespace.as_bytes());
            put_string(&mut signed, b"");
            put_string(&mut signed, b"sha512");
            put_string(&mut signed, &digest);
            let sig = self.pair.sign(&signed);
            let mut sig_blob = Vec::new();
            put_string(&mut sig_blob, b"ssh-ed25519");
            put_string(&mut sig_blob, sig.as_ref());

            let mut blob = b"SSHSIG".to_vec();
            blob.extend_from_slice(&1u32.to_be_bytes());
            put_string(&mut blob, &self.public_blob());
            put_string(&mut blob, namespace.as_bytes());
            put_string(&mut blob, b"");
            put_string(&mut blob, b"sha512");
            put_string(&mut blob, &sig_blob);
            let b64 = STANDARD.encode(blob);
            let mut out = String::from("-----BEGIN SSH SIGNATURE-----\n");
            for chunk in b64.as_bytes().chunks(70) {
                out.push_str(std::str::from_utf8(chunk).unwrap());
                out.push('\n');
            }
            out.push_str("-----END SSH SIGNATURE-----\n");
            out
        }
    }

    #[test]
    fn signature_roundtrip_and_tampering() {
        let key = TestKey::generate();
        let other = TestKey::generate();
        let msg = b"abc  file.gz\n";
        let sig = key.sign(msg, NAMESPACE);
        verify_signature(&key.public_line(), msg, &sig).unwrap();
        // Changed message, wrong key, wrong namespace, garbage, bit flip in the signature.
        assert!(verify_signature(&key.public_line(), b"abc  file.gz\n\n", &sig).is_err());
        assert!(verify_signature(&other.public_line(), msg, &sig).is_err());
        assert!(verify_signature(&key.public_line(), msg, &key.sign(msg, "git")).is_err());
        assert!(verify_signature(&key.public_line(), msg, "nonsense").is_err());
        assert!(verify_signature(&key.public_line(), msg, "").is_err());
        let mut raw = STANDARD
            .decode(sig.lines().filter(|l| !l.starts_with("-----")).collect::<String>())
            .unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        let flipped = format!(
            "-----BEGIN SSH SIGNATURE-----\n{}\n-----END SSH SIGNATURE-----\n",
            STANDARD.encode(raw)
        );
        assert!(verify_signature(&key.public_line(), msg, &flipped).is_err());
        // A public key that is not ed25519 is refused outright.
        assert!(verify_signature("ssh-rsa AAAA", msg, &sig).is_err());
    }

    #[test]
    fn download_needs_signed_sums_that_list_the_hash() {
        let key = TestKey::generate();
        let data = b"binary bytes";
        let sums = format!("{}  cyanotype-x.gz\n{}  other.gz\n", sha256_hex(data), "0".repeat(64));
        let sig = key.sign(sums.as_bytes(), NAMESPACE);
        verify_download(&key.public_line(), &sums, &sig, "cyanotype-x.gz", data).unwrap();
        // Tampered download.
        assert!(verify_download(&key.public_line(), &sums, &sig, "cyanotype-x.gz", b"evil").is_err());
        // Not listed.
        assert!(verify_download(&key.public_line(), &sums, &sig, "missing.gz", data).is_err());
        // Sums edited after signing (an attacker swaps in their hash).
        let forged = sums.replace(&sha256_hex(data), &sha256_hex(b"evil"));
        assert!(verify_download(&key.public_line(), &forged, &sig, "cyanotype-x.gz", b"evil").is_err());
    }

    /// Made by `ssh-keygen -t ed25519 -N '' -f k && ssh-keygen -Y sign -f k -n cyanotype-cli msg` with a throwaway key
    /// whose private half was discarded.
    #[test]
    fn real_ssh_keygen_signature_verifies() {
        let pubkey = include_str!("../tests/fixtures/test-key.pub");
        let msg = include_bytes!("../tests/fixtures/SHA256SUMS");
        let sig = include_str!("../tests/fixtures/SHA256SUMS.sig");
        verify_signature(pubkey, msg, sig).unwrap();
        let mut bad = msg.to_vec();
        bad[0] ^= 1;
        assert!(verify_signature(pubkey, &bad, sig).is_err());
    }
}
