//! Local hash cache: `(absolute path, size, mtime)` -> sha256, so a resumed import of a 100k-file
//! library does not re-read every byte.
//!
//! One JSON object per line, append-only. Each entry is written with a single `write` so a kill
//! mid-import loses at most the last line, which is ignored on load. Later lines win; the file is
//! compacted on open when it carries many superseded entries.
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    path: String,
    size: u64,
    mtime_ns: i128,
}

impl Key {
    /// None when the path is not valid UTF-8 (such files are simply never cached).
    pub fn new(path: &Path, size: u64, mtime: Option<SystemTime>) -> Option<Key> {
        Some(Key {
            path: path.to_str()?.to_string(),
            size,
            mtime_ns: mtime_ns(mtime?),
        })
    }
}

pub fn mtime_ns(t: SystemTime) -> i128 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

#[derive(Serialize, Deserialize)]
struct Line {
    p: String,
    s: u64,
    m: String, // i128 as a string: JSON numbers cannot hold it
    h: String,
}

pub struct HashCache {
    map: Mutex<HashMap<Key, String>>,
    file: Mutex<Option<File>>,
    path: Option<PathBuf>,
}

impl HashCache {
    /// A cache that remembers nothing across runs (used by tests and `--no-cache`).
    pub fn in_memory() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            file: Mutex::new(None),
            path: None,
        }
    }

    /// Open (creating if needed) the cache file. `rehash` ignores existing entries but keeps writing.
    pub fn open(path: &Path, rehash: bool) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let mut map = HashMap::new();
        let mut lines = 0usize;
        if !rehash && let Ok(f) = File::open(path) {
            for l in BufReader::new(f).lines() {
                let Ok(l) = l else { break };
                lines += 1;
                let Ok(e) = serde_json::from_str::<Line>(&l) else {
                    continue;
                };
                let (Ok(m), true) = (e.m.parse::<i128>(), e.h.len() == 64) else {
                    continue;
                };
                map.insert(
                    Key {
                        path: e.p,
                        size: e.s,
                        mtime_ns: m,
                    },
                    e.h,
                );
            }
        }
        let cache = Self {
            map: Mutex::new(map),
            file: Mutex::new(None),
            path: Some(path.to_path_buf()),
        };
        if rehash || lines > 1000 && lines > cache.len() * 2 {
            cache.rewrite()?;
        }
        // A torn last line (killed mid-write) must not swallow the next entry we append.
        let torn = fs::read(path)
            .map(|b| b.last().is_some_and(|c| *c != b'\n'))
            .unwrap_or(false);
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening hash cache {}", path.display()))?;
        if torn {
            f.write_all(b"\n")?;
        }
        *cache.file.lock().unwrap() = Some(f);
        Ok(cache)
    }

    /// Replace the file with just the live entries (atomic via a temp file).
    fn rewrite(&self) -> Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let tmp = path.with_extension("jsonl.tmp");
        let mut f = File::create(&tmp)?;
        for (k, h) in self.map.lock().unwrap().iter() {
            f.write_all(line(k, h).as_bytes())?;
        }
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    pub fn get(&self, key: &Key) -> Option<String> {
        self.map.lock().unwrap().get(key).cloned()
    }

    pub fn put(&self, key: Key, sha256_hex: String) {
        if let Some(f) = self.file.lock().unwrap().as_mut() {
            // Best effort: a failed cache write only costs a re-hash next time.
            let _ = f.write_all(line(&key, &sha256_hex).as_bytes());
        }
        self.map.lock().unwrap().insert(key, sha256_hex);
    }
}

fn line(k: &Key, h: &str) -> String {
    let l = Line {
        p: k.path.clone(),
        s: k.size,
        m: k.mtime_ns.to_string(),
        h: h.to_string(),
    };
    format!("{}\n", serde_json::to_string(&l).expect("serialisable"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn key(p: &str, size: u64, secs: u64) -> Key {
        Key::new(Path::new(p), size, Some(UNIX_EPOCH + Duration::from_secs(secs))).unwrap()
    }

    const H1: &str = "11111111111111111111111111111111111111111111111111111111111111aa";
    const H2: &str = "22222222222222222222222222222222222222222222222222222222222222bb";

    #[test]
    fn hit_requires_same_path_size_and_mtime() {
        let c = HashCache::in_memory();
        c.put(key("/a/b.jpg", 10, 100), H1.into());
        assert_eq!(c.get(&key("/a/b.jpg", 10, 100)).as_deref(), Some(H1));
        assert_eq!(c.get(&key("/a/b.jpg", 11, 100)), None, "size changed");
        assert_eq!(c.get(&key("/a/b.jpg", 10, 101)), None, "mtime changed");
        assert_eq!(c.get(&key("/a/c.jpg", 10, 100)), None, "other path");
    }

    #[test]
    fn persists_across_runs_and_survives_a_torn_last_line() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c").join("hashes.jsonl");
        {
            let c = HashCache::open(&p, false).unwrap();
            c.put(key("/x.jpg", 1, 5), H1.into());
            c.put(key("/y.jpg", 2, 6), H2.into());
        }
        // Simulate a kill in the middle of writing a third entry.
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(br#"{"p":"/z.jpg","s":3,"m":"7000"#).unwrap();
        drop(f);
        let c = HashCache::open(&p, false).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(&key("/x.jpg", 1, 5)).as_deref(), Some(H1));
        assert_eq!(c.get(&key("/y.jpg", 2, 6)).as_deref(), Some(H2));
        assert_eq!(c.get(&key("/z.jpg", 3, 7)), None);
        // Appending after a torn line must not corrupt the next entry: later entries still load.
        c.put(key("/w.jpg", 4, 8), H1.into());
        drop(c);
        let c = HashCache::open(&p, false).unwrap();
        assert_eq!(c.get(&key("/w.jpg", 4, 8)).as_deref(), Some(H1));
    }

    #[test]
    fn newer_entry_wins_and_rehash_discards_old_ones() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("hashes.jsonl");
        let c = HashCache::open(&p, false).unwrap();
        c.put(key("/x.jpg", 1, 5), H1.into());
        c.put(key("/x.jpg", 1, 5), H2.into());
        drop(c);
        let c = HashCache::open(&p, false).unwrap();
        assert_eq!(c.get(&key("/x.jpg", 1, 5)).as_deref(), Some(H2));
        drop(c);
        let c = HashCache::open(&p, true).unwrap();
        assert_eq!(c.len(), 0);
        drop(c);
        assert_eq!(fs::read_to_string(&p).unwrap(), "");
    }

    #[test]
    fn compacts_when_mostly_superseded() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("hashes.jsonl");
        let c = HashCache::open(&p, false).unwrap();
        for i in 0..1200u64 {
            c.put(key("/same.jpg", 1, 5), if i % 2 == 0 { H1.into() } else { H2.into() });
        }
        drop(c);
        let c = HashCache::open(&p, false).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(fs::read_to_string(&p).unwrap().lines().count(), 1);
    }

    #[test]
    fn non_utf8_paths_are_not_cached() {
        #[cfg(unix)]
        {
            use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
            let p = Path::new(OsStr::from_bytes(b"/bad/\xff.jpg"));
            assert!(Key::new(p, 1, Some(UNIX_EPOCH)).is_none());
        }
    }
}
