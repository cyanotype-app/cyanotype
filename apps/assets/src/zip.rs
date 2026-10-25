//! A streaming, store-only ZIP writer (the album download, docs/library.md "Downloading as a ZIP").
//!
//! Photos are already compressed, so entries are stored: the archive is the files plus a few dozen bytes each, and the
//! total size is known before the first byte is sent (`archive_len`), which lets the browser show a real progress bar.
//! Each entry is written the streaming way, with a data descriptor after its bytes (the CRC is only known once the bytes
//! have passed), so memory stays at one chunk whatever the file size.
//!
//! * ZIP64: an entry whose size is `>= 4 GiB - 1` carries the ZIP64 extra field and an 8-byte data descriptor; the central
//!   directory switches to ZIP64 offsets, and the end record to ZIP64, when the archive passes 4 GiB or 65,534 entries.
//!   Everything else is plain ZIP, which every unzip tool reads.
//! * Names are UTF-8 (flag bit 11) and made safe by [`entry_name`]/[`NameSet`]: no directories, no `..`, no control
//!   characters, unique case-insensitively (so unpacking on Windows or macOS cannot overwrite one photo with another).
//! * The Unix timestamp extra field (`UT`) carries the exact UTC time; the DOS fields carry the same, clamped to 1980-2107.
use std::collections::HashSet;

use crc32fast::Hasher;
use time::OffsetDateTime;

const U32_MAX: u64 = 0xFFFF_FFFF;
const U16_MAX: usize = 0xFFFF;
const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_DESCRIPTOR: u32 = 0x0807_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const SIG_EOCD64: u32 = 0x0606_4b50;
const SIG_LOCATOR64: u32 = 0x0706_4b50;
/// Bit 3 (sizes and CRC follow the data) and bit 11 (names are UTF-8).
const FLAGS: u16 = 0x0008 | 0x0800;
/// Unix, ZIP 4.5: `0o100644` regular file permissions in the external attributes.
const MADE_BY: u16 = (3 << 8) | 45;
const EXTERNAL_ATTRS: u32 = 0o100_644 << 16;

/// One file to put in the archive.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Already safe (see [`NameSet`]).
    pub name: String,
    /// Seconds since the Unix epoch (UTC), when the photo's capture time is known.
    pub mtime: Option<i64>,
    /// The number of bytes that will be written. [`ZipWriter::end`] fails if the data turned out different.
    pub size: u64,
}

impl Entry {
    fn zip64(&self) -> bool {
        self.size >= U32_MAX
    }
}

struct Done {
    name: String,
    mtime: Option<i64>,
    crc: u32,
    size: u64,
    zip64: bool,
    offset: u64,
}

struct Open {
    entry: Entry,
    offset: u64,
    hasher: Hasher,
    written: u64,
}

/// Produces the bytes of an archive piece by piece: [`begin`](Self::begin), any number of [`update`](Self::update)s,
/// [`end`](Self::end), for each entry, then [`finish`](Self::finish).
#[derive(Default)]
pub struct ZipWriter {
    offset: u64,
    done: Vec<Done>,
    open: Option<Open>,
    /// Tests: write the ZIP64 end record even for a small archive.
    #[cfg(test)]
    force_zip64_end: bool,
}

fn dos(mtime: Option<i64>) -> (u16, u16) {
    let t = mtime
        .and_then(|s| OffsetDateTime::from_unix_timestamp(s).ok())
        .filter(|t| (1980..=2107).contains(&t.year()))
        .map(|t| {
            let time = (t.hour() as u16) << 11 | (t.minute() as u16) << 5 | (t.second() as u16 / 2);
            let date = ((t.year() - 1980) as u16) << 9 | (u8::from(t.month()) as u16) << 5 | t.day() as u16;
            (time, date)
        });
    // 1980-01-01 00:00 when the time is unknown (or outside what DOS can say).
    t.unwrap_or((0, 0x0021))
}

/// The `UT` extra field: flags (mtime present) and the modification time, 32-bit.
fn unix_time_extra(mtime: Option<i64>) -> Vec<u8> {
    let Some(s) = mtime.and_then(|s| u32::try_from(s).ok()) else {
        return Vec::new();
    };
    let mut v = Vec::with_capacity(9);
    v.extend_from_slice(&0x5455u16.to_le_bytes());
    v.extend_from_slice(&5u16.to_le_bytes());
    v.push(1);
    v.extend_from_slice(&s.to_le_bytes());
    v
}

impl ZipWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The local file header of `entry`. Must not be called while another entry is open.
    pub fn begin(&mut self, entry: &Entry) -> Vec<u8> {
        assert!(self.open.is_none(), "begin: an entry is still open");
        let z64 = entry.zip64();
        let (dtime, ddate) = dos(entry.mtime);
        let mut extra = Vec::new();
        if z64 {
            // Both sizes, zero now: the data descriptor has the real ones.
            extra.extend_from_slice(&1u16.to_le_bytes());
            extra.extend_from_slice(&16u16.to_le_bytes());
            extra.extend_from_slice(&[0; 16]);
        }
        extra.extend(unix_time_extra(entry.mtime));
        let name = entry.name.as_bytes();
        let mut h = Vec::new();
        h.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        h.extend_from_slice(&(if z64 { 45u16 } else { 20 }).to_le_bytes());
        h.extend_from_slice(&FLAGS.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // stored
        h.extend_from_slice(&dtime.to_le_bytes());
        h.extend_from_slice(&ddate.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes()); // crc: in the descriptor
        let size_field = if z64 { U32_MAX as u32 } else { 0 };
        h.extend_from_slice(&size_field.to_le_bytes());
        h.extend_from_slice(&size_field.to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        h.extend_from_slice(name);
        h.extend_from_slice(&extra);
        self.open = Some(Open {
            entry: entry.clone(),
            offset: self.offset,
            hasher: Hasher::new(),
            written: 0,
        });
        self.offset += h.len() as u64;
        h
    }

    /// Account for a piece of the open entry's data (the caller sends the bytes itself).
    pub fn update(&mut self, chunk: &[u8]) {
        let o = self.open.as_mut().expect("update: no entry is open");
        o.hasher.update(chunk);
        o.written += chunk.len() as u64;
        self.offset += chunk.len() as u64;
    }

    /// The data descriptor that closes the open entry. Fails if the bytes written are not the announced size (the archive
    /// length was promised up front, so a different file must abort the download, not produce a wrong one).
    pub fn end(&mut self) -> Result<Vec<u8>, String> {
        let o = self.open.as_ref().expect("end: no entry is open");
        let (crc, written) = (o.hasher.clone().finalize(), o.written);
        if written != o.entry.size {
            return Err(format!(
                "{} is {written} bytes, expected {}",
                o.entry.name, o.entry.size
            ));
        }
        Ok(self.close(crc, written))
    }

    fn close(&mut self, crc: u32, size: u64) -> Vec<u8> {
        let o = self.open.take().expect("close: no entry is open");
        let z64 = o.entry.zip64();
        let mut d = Vec::with_capacity(24);
        d.extend_from_slice(&SIG_DESCRIPTOR.to_le_bytes());
        d.extend_from_slice(&crc.to_le_bytes());
        if z64 {
            d.extend_from_slice(&size.to_le_bytes());
            d.extend_from_slice(&size.to_le_bytes());
        } else {
            d.extend_from_slice(&(size as u32).to_le_bytes());
            d.extend_from_slice(&(size as u32).to_le_bytes());
        }
        self.offset += d.len() as u64;
        self.done.push(Done {
            name: o.entry.name,
            mtime: o.entry.mtime,
            crc,
            size,
            zip64: z64,
            offset: o.offset,
        });
        d
    }

    /// The central directory and the end record.
    pub fn finish(&mut self) -> Vec<u8> {
        assert!(self.open.is_none(), "finish: an entry is still open");
        let cd_start = self.offset;
        let mut out = Vec::new();
        for e in &self.done {
            let big_offset = e.offset >= U32_MAX;
            let (dtime, ddate) = dos(e.mtime);
            let mut extra = Vec::new();
            if e.zip64 || big_offset {
                let mut data = Vec::new();
                if e.zip64 {
                    data.extend_from_slice(&e.size.to_le_bytes());
                    data.extend_from_slice(&e.size.to_le_bytes());
                }
                if big_offset {
                    data.extend_from_slice(&e.offset.to_le_bytes());
                }
                extra.extend_from_slice(&1u16.to_le_bytes());
                extra.extend_from_slice(&(data.len() as u16).to_le_bytes());
                extra.extend(data);
            }
            extra.extend(unix_time_extra(e.mtime));
            let name = e.name.as_bytes();
            out.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
            out.extend_from_slice(&MADE_BY.to_le_bytes());
            out.extend_from_slice(&(if e.zip64 || big_offset { 45u16 } else { 20 }).to_le_bytes());
            out.extend_from_slice(&FLAGS.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&dtime.to_le_bytes());
            out.extend_from_slice(&ddate.to_le_bytes());
            out.extend_from_slice(&e.crc.to_le_bytes());
            let size_field = if e.zip64 { U32_MAX as u32 } else { e.size as u32 };
            out.extend_from_slice(&size_field.to_le_bytes());
            out.extend_from_slice(&size_field.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&(extra.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // comment
            out.extend_from_slice(&0u16.to_le_bytes()); // disk
            out.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            out.extend_from_slice(&EXTERNAL_ATTRS.to_le_bytes());
            let offset_field = if big_offset { U32_MAX as u32 } else { e.offset as u32 };
            out.extend_from_slice(&offset_field.to_le_bytes());
            out.extend_from_slice(name);
            out.extend_from_slice(&extra);
        }
        let cd_size = out.len() as u64;
        let n = self.done.len();
        #[cfg(test)]
        let force = self.force_zip64_end;
        #[cfg(not(test))]
        let force = false;
        let need64 = force || n >= U16_MAX || cd_start >= U32_MAX || cd_size >= U32_MAX;
        if need64 {
            let eocd64_at = cd_start + cd_size;
            out.extend_from_slice(&SIG_EOCD64.to_le_bytes());
            out.extend_from_slice(&44u64.to_le_bytes());
            out.extend_from_slice(&45u16.to_le_bytes());
            out.extend_from_slice(&45u16.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&(n as u64).to_le_bytes());
            out.extend_from_slice(&(n as u64).to_le_bytes());
            out.extend_from_slice(&cd_size.to_le_bytes());
            out.extend_from_slice(&cd_start.to_le_bytes());
            out.extend_from_slice(&SIG_LOCATOR64.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&eocd64_at.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
        }
        out.extend_from_slice(&SIG_EOCD.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        let count = if need64 { U16_MAX as u16 } else { n as u16 };
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&(if need64 { U32_MAX as u32 } else { cd_size as u32 }).to_le_bytes());
        out.extend_from_slice(&(if need64 { U32_MAX as u32 } else { cd_start as u32 }).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        self.offset += out.len() as u64;
        out
    }
}

/// The exact length of the archive of `entries`, computed by running the writer over empty data (so it cannot drift from
/// what is really written).
pub fn archive_len(entries: &[Entry]) -> u64 {
    let mut w = ZipWriter::new();
    let mut total = 0u64;
    for e in entries {
        total += w.begin(e).len() as u64;
        // `update` is for real bytes; the length only needs the offsets to advance by `size`.
        w.open.as_mut().expect("open").written = e.size;
        w.offset += e.size;
        total += e.size;
        total += w.close(0, e.size).len() as u64;
    }
    total + w.finish().len() as u64
}

/// A file name from an upload, made safe to put in an archive: only the last path component, no control characters, no
/// reserved names (`.`, `..`), and not empty. Characters Windows will not unpack (`<>:"|?*`) are replaced too.
pub fn entry_name(original: &str) -> String {
    let last = original.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned = clean(last);
    if cleaned.is_empty() {
        "photo".to_string()
    } else {
        cleaned
    }
}

/// Control characters and the ones Windows will not unpack replaced, outer dots and spaces trimmed, length bounded (names go in
/// a 16-bit length field; real file names are far shorter).
fn clean(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            c if c.is_control() => '_',
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c => c,
        })
        .collect();
    cleaned.trim_matches([' ', '.']).chars().take(180).collect()
}

/// Hands out unique names: `IMG_0001.jpg`, then `IMG_0001 (2).jpg`, `IMG_0001 (3).jpg` ... Uniqueness ignores case.
#[derive(Default)]
pub struct NameSet {
    used: HashSet<String>,
}

impl NameSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn unique(&mut self, original: &str) -> String {
        let name = entry_name(original);
        if self.used.insert(name.to_lowercase()) {
            return name;
        }
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => name.split_at(i),
            _ => (name.as_str(), ""),
        };
        for n in 2u32.. {
            let candidate = format!("{stem} ({n}){ext}");
            if self.used.insert(candidate.to_lowercase()) {
                return candidate;
            }
        }
        unreachable!("u32 counter exhausted")
    }
}

/// The download's file name from an album or link title: `<title>.zip`, safe for a header and a file system (slashes become
/// dashes, so "Summer / 2024" stays readable).
pub fn archive_name(title: Option<&str>) -> String {
    let stem = title
        .map(|t| clean(&t.replace(['/', '\\'], "-")))
        .map(|t| t.chars().take(100).collect::<String>())
        .map(|t| t.trim_matches([' ', '.']).to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "cyanotype-photos".to_string());
    format!("{stem}.zip")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal reader: finds the end record, follows ZIP64 when present, returns `(name, crc, size, offset, mtime)`.
    pub(crate) fn read_directory(z: &[u8]) -> Vec<(String, u32, u64, u64, Option<u32>)> {
        let le16 = |o: usize| u16::from_le_bytes(z[o..o + 2].try_into().unwrap());
        let le32 = |o: usize| u32::from_le_bytes(z[o..o + 4].try_into().unwrap());
        let le64 = |o: usize| u64::from_le_bytes(z[o..o + 8].try_into().unwrap());
        let eocd = z.len() - 22;
        assert_eq!(le32(eocd), SIG_EOCD);
        let (mut n, mut cd_off) = (le16(eocd + 10) as u64, le32(eocd + 16) as u64);
        if cd_off == U32_MAX || n == U16_MAX as u64 {
            let loc = eocd - 20;
            assert_eq!(le32(loc), SIG_LOCATOR64);
            let e64 = le64(loc + 8) as usize;
            assert_eq!(le32(e64), SIG_EOCD64);
            n = le64(e64 + 32);
            cd_off = le64(e64 + 48);
        }
        let mut p = cd_off as usize;
        let mut out = Vec::new();
        for _ in 0..n {
            assert_eq!(le32(p), SIG_CENTRAL);
            let crc = le32(p + 16);
            let (mut size, mut off) = (le32(p + 24) as u64, le32(p + 42) as u64);
            let (nl, el) = (le16(p + 28) as usize, le16(p + 30) as usize);
            let name = String::from_utf8(z[p + 46..p + 46 + nl].to_vec()).unwrap();
            let mut x = p + 46 + nl;
            let end = x + el;
            let mut mtime = None;
            while x < end {
                let (id, len) = (le16(x), le16(x + 2) as usize);
                let d = x + 4;
                match id {
                    1 => {
                        let mut q = d;
                        if size == U32_MAX {
                            size = le64(q);
                            q += 16;
                        }
                        if off == U32_MAX {
                            off = le64(q);
                        }
                    }
                    0x5455 => mtime = Some(le32(d + 1)),
                    _ => {}
                }
                x += 4 + len;
            }
            out.push((name, crc, size, off, mtime));
            p = end;
        }
        out
    }

    /// The bytes of the entry at `offset`, read through its local header.
    pub(crate) fn entry_bytes(z: &[u8], offset: u64, size: u64) -> &[u8] {
        let o = offset as usize;
        assert_eq!(u32::from_le_bytes(z[o..o + 4].try_into().unwrap()), SIG_LOCAL);
        let nl = u16::from_le_bytes(z[o + 26..o + 28].try_into().unwrap()) as usize;
        let el = u16::from_le_bytes(z[o + 28..o + 30].try_into().unwrap()) as usize;
        let start = o + 30 + nl + el;
        &z[start..start + size as usize]
    }

    fn build(files: &[(&str, Option<i64>, &[u8])], force64: bool) -> Vec<u8> {
        let entries: Vec<Entry> = files
            .iter()
            .map(|(n, t, d)| Entry {
                name: n.to_string(),
                mtime: *t,
                size: d.len() as u64,
            })
            .collect();
        let mut w = ZipWriter::new();
        w.force_zip64_end = force64;
        let mut z = Vec::new();
        for (e, (_, _, d)) in entries.iter().zip(files) {
            z.extend(w.begin(e));
            for c in d.chunks(7) {
                w.update(c);
                z.extend_from_slice(c);
            }
            z.extend(w.end().unwrap());
        }
        z.extend(w.finish());
        if !force64 {
            assert_eq!(z.len() as u64, archive_len(&entries), "predicted length is exact");
        }
        z
    }

    #[test]
    fn small_archive_reads_back() {
        let files: [(&str, Option<i64>, &[u8]); 3] = [
            ("a.jpg", Some(1_700_000_000), b"alpha bytes"),
            ("b.cr3", None, b""),
            ("c d.heic", Some(1_000_000_000), &[0u8; 100]),
        ];
        let z = build(&files, false);
        let dir = read_directory(&z);
        assert_eq!(dir.len(), 3);
        for ((name, crc, size, off, mtime), (n, t, d)) in dir.iter().zip(files) {
            assert_eq!(name, n);
            assert_eq!(*size, d.len() as u64);
            assert_eq!(*crc, crc32fast::hash(d));
            assert_eq!(entry_bytes(&z, *off, *size), d);
            assert_eq!(mtime.map(i64::from), t);
        }
    }

    #[test]
    fn zip64_end_record_reads_back() {
        let files: [(&str, Option<i64>, &[u8]); 2] = [("a.jpg", Some(1_700_000_000), b"one"), ("b.jpg", None, b"two")];
        let z = build(&files, true);
        let dir = read_directory(&z);
        assert_eq!(dir.len(), 2);
        assert_eq!(entry_bytes(&z, dir[1].3, dir[1].2), b"two");
    }

    #[test]
    fn big_entries_use_zip64_in_the_length() {
        // No bytes are produced: only the layout of a 5 GiB entry is checked, against the same entry made small.
        let entry = |size| Entry {
            name: "big.mov".into(),
            mtime: None,
            size,
        };
        let small = archive_len(&[entry(10)]) - 10;
        let big = archive_len(&[entry(5 << 30)]) - (5 << 30);
        // 16-byte ZIP64 extra in the local header (+4 for its header), 8 more descriptor bytes, the central ZIP64 extra
        // (4 + 16), and, as the directory starts past 4 GiB, the ZIP64 end record (56) and its locator (20).
        assert_eq!(big - small, 20 + 8 + 20 + 56 + 20);
    }

    /// A real ZIP64 archive: a 4 GiB + 1 entry of zeros written to disk and checked by `unzip -t`. Slow and needs 4 GiB of
    /// disk, so it only runs on request: `cargo test -p cyanotype-assets zip64_over_4_gib -- --ignored`.
    #[test]
    #[ignore]
    fn zip64_over_4_gib_is_read_by_unzip() {
        use std::io::Write;
        let size = (4u64 << 30) + 1;
        let entries = [
            Entry {
                name: "small.jpg".into(),
                mtime: Some(1_700_000_000),
                size: 3,
            },
            Entry {
                name: "big.bin".into(),
                mtime: None,
                size,
            },
            Entry {
                name: "after.jpg".into(),
                mtime: None,
                size: 2,
            },
        ];
        let path = std::env::temp_dir().join(format!("cyanotype-zip64-{}.zip", std::process::id()));
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let mut w = ZipWriter::new();
        let zeros = vec![0u8; 1 << 20];
        for (e, data) in entries.iter().zip([&b"abc"[..], &[][..], &b"zz"[..]]) {
            f.write_all(&w.begin(e)).unwrap();
            if e.size == size {
                let mut left = size;
                while left > 0 {
                    let n = left.min(zeros.len() as u64) as usize;
                    w.update(&zeros[..n]);
                    f.write_all(&zeros[..n]).unwrap();
                    left -= n as u64;
                }
            } else {
                w.update(data);
                f.write_all(data).unwrap();
            }
            f.write_all(&w.end().unwrap()).unwrap();
        }
        f.write_all(&w.finish()).unwrap();
        f.flush().unwrap();
        drop(f);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), archive_len(&entries));
        let out = std::process::Command::new("unzip")
            .arg("-t")
            .arg(&path)
            .output()
            .unwrap();
        std::fs::remove_file(&path).ok();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{text}");
        assert!(text.contains("No errors detected"), "{text}");
    }

    #[test]
    fn size_mismatch_is_an_error() {
        let mut w = ZipWriter::new();
        w.begin(&Entry {
            name: "a".into(),
            mtime: None,
            size: 5,
        });
        w.update(b"abc");
        assert!(w.end().is_err());
    }

    #[test]
    fn dos_time_is_clamped() {
        assert_eq!(dos(None), (0, 0x0021));
        assert_eq!(dos(Some(0)), (0, 0x0021)); // 1970
        let (t, d) = dos(Some(1_700_000_000)); // 2023-11-14 22:13:20 UTC
        assert_eq!(d, ((2023 - 1980) << 9) | (11 << 5) | 14);
        assert_eq!(t, (22 << 11) | (13 << 5) | 10);
    }

    #[test]
    fn names_are_safe_and_unique() {
        assert_eq!(entry_name("../../etc/passwd"), "passwd");
        assert_eq!(entry_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(entry_name("a/b/c.jpg"), "c.jpg");
        assert_eq!(entry_name(".."), "photo");
        assert_eq!(entry_name(""), "photo");
        assert_eq!(entry_name("a:b*c?.jpg"), "a_b_c_.jpg");
        assert_eq!(entry_name("tab\tname\n.jpg"), "tab_name_.jpg");
        assert_eq!(entry_name("Zoë Ünï.jpg"), "Zoë Ünï.jpg");
        let mut s = NameSet::new();
        assert_eq!(s.unique("IMG_0001.jpg"), "IMG_0001.jpg");
        assert_eq!(s.unique("IMG_0001.jpg"), "IMG_0001 (2).jpg");
        assert_eq!(s.unique("img_0001.JPG"), "img_0001 (3).JPG");
        assert_eq!(s.unique("IMG_0001 (2).jpg"), "IMG_0001 (2) (2).jpg");
        assert_eq!(s.unique("noext"), "noext");
        assert_eq!(s.unique("noext"), "noext (2)");
        assert_eq!(s.unique(".hidden"), "hidden");
    }

    #[test]
    fn archive_names() {
        assert_eq!(archive_name(Some("Summer / 2024")), "Summer - 2024.zip");
        assert_eq!(archive_name(Some("Alpha trip")), "Alpha trip.zip");
        assert_eq!(archive_name(Some("..")), "cyanotype-photos.zip");
        assert_eq!(archive_name(Some("   ")), "cyanotype-photos.zip");
        assert_eq!(archive_name(None), "cyanotype-photos.zip");
        assert!(archive_name(Some(&"x".repeat(500))).len() < 120);
    }
}
