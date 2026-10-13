//! Finding the files to import: recursive walk, type detection, filters.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
}

/// `(extension, mime, kind)`. Extensions are lowercase, without the dot.
const TYPES: &[(&str, &str, Kind)] = &[
    ("jpg", "image/jpeg", Kind::Image),
    ("jpeg", "image/jpeg", Kind::Image),
    ("png", "image/png", Kind::Image),
    ("webp", "image/webp", Kind::Image),
    ("heic", "image/heic", Kind::Image),
    ("heif", "image/heif", Kind::Image),
    ("avif", "image/avif", Kind::Image),
    ("tif", "image/tiff", Kind::Image),
    ("tiff", "image/tiff", Kind::Image),
    ("dng", "image/x-adobe-dng", Kind::Image),
    ("cr2", "image/x-canon-cr2", Kind::Image),
    ("cr3", "image/x-canon-cr3", Kind::Image),
    ("nef", "image/x-nikon-nef", Kind::Image),
    ("nrw", "image/x-nikon-nrw", Kind::Image),
    ("arw", "image/x-sony-arw", Kind::Image),
    ("srf", "image/x-sony-srf", Kind::Image),
    ("sr2", "image/x-sony-sr2", Kind::Image),
    ("raf", "image/x-fuji-raf", Kind::Image),
    ("orf", "image/x-olympus-orf", Kind::Image),
    ("rw2", "image/x-panasonic-rw2", Kind::Image),
    ("pef", "image/x-pentax-pef", Kind::Image),
    ("srw", "image/x-samsung-srw", Kind::Image),
    ("x3f", "image/x-sigma-x3f", Kind::Image),
    ("3fr", "image/x-hasselblad-3fr", Kind::Image),
    ("iiq", "image/x-phaseone-iiq", Kind::Image),
    ("rwl", "image/x-leica-rwl", Kind::Image),
    ("mov", "video/quicktime", Kind::Video),
    ("mp4", "video/mp4", Kind::Video),
    ("m4v", "video/x-m4v", Kind::Video),
];

/// Sidecar files that travel with photos but are not assets themselves.
const SIDECAR_EXTS: &[&str] = &["xmp", "aae", "thm"];
/// Names of OS/NAS junk that is not dot-prefixed.
const JUNK_NAMES: &[&str] = &[
    "thumbs.db",
    "ehthumbs.db",
    "desktop.ini",
    "__macosx",
    "$recycle.bin",
    "system volume information",
    "@eadir",
    "lost+found",
    "icon\r",
];

fn lower_ext(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

pub fn classify(path: &Path) -> Option<(Kind, &'static str)> {
    let ext = lower_ext(path)?;
    TYPES.iter().find(|t| t.0 == ext).map(|t| (t.2, t.1))
}

/// MIME type by extension; `application/octet-stream` when unknown.
pub fn content_type(path: &Path) -> &'static str {
    classify(path).map_or("application/octet-stream", |(_, m)| m)
}

fn is_junk_name(name: &str) -> bool {
    name.starts_with('.') || JUNK_NAMES.contains(&name.to_ascii_lowercase().as_str())
}

#[derive(Debug, Clone)]
pub struct WalkOptions {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub videos: bool,
    pub follow_symlinks: bool,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: Option<SystemTime>,
    pub kind: Kind,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub files: Vec<Candidate>,
    /// Why files were left out, with counts ("sidecar", "hidden/junk", "video (--no-videos)", ...).
    pub skipped: BTreeMap<&'static str, u64>,
    /// Paths that could not be read at all.
    pub errors: Vec<(PathBuf, String)>,
}

fn globset(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g: Glob = GlobBuilder::new(p)
            .case_insensitive(true)
            .literal_separator(false)
            .build()
            .with_context(|| format!("bad glob {p:?}"))?;
        b.add(g);
    }
    Ok(Some(b.build()?))
}

/// A pattern matches when it matches the file name, or the path relative to the walked root, or the full path.
fn matches(set: &GlobSet, name: &str, rel: &Path, full: &Path) -> bool {
    set.is_match(name) || set.is_match(rel) || set.is_match(full)
}

pub fn scan(roots: &[PathBuf], opts: &WalkOptions) -> Result<Scan> {
    let include = globset(&opts.include)?;
    let exclude = globset(&opts.exclude)?;
    let mut out = Scan::default();
    let mut seen = std::collections::HashSet::new();

    for root in roots {
        let meta = match std::fs::metadata(root) {
            Ok(m) => m,
            Err(e) => {
                out.errors.push((root.clone(), e.to_string()));
                continue;
            }
        };
        let single_file = meta.is_file();
        let base = if single_file {
            root.parent().unwrap_or(Path::new("")).to_path_buf()
        } else {
            root.clone()
        };
        let walker = WalkDir::new(root)
            .follow_links(opts.follow_symlinks)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                // Prune junk and excluded directories before descending. The root itself is always entered.
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                if e.file_type().is_dir() {
                    if is_junk_name(&name) {
                        return false;
                    }
                    if let Some(ex) = &exclude {
                        let rel = e.path().strip_prefix(&base).unwrap_or(e.path());
                        if matches(ex, &name, rel, e.path()) {
                            return false;
                        }
                    }
                }
                true
            });
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    let p = e.path().map(Path::to_path_buf).unwrap_or_else(|| root.clone());
                    out.errors.push((p, e.to_string()));
                    continue;
                }
            };
            if entry.file_type().is_dir() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let mut skip = |why: &'static str| *out.skipped.entry(why).or_insert(0) += 1;
            if entry.path_is_symlink() && !opts.follow_symlinks {
                skip("symlink (use --follow-symlinks)");
                continue;
            }
            if !entry.file_type().is_file() {
                skip("not a regular file");
                continue;
            }
            // Dotfiles and junk are never imported, even when named on the command line.
            if is_junk_name(&name) {
                skip("hidden/junk");
                continue;
            }
            if lower_ext(path).is_some_and(|e| SIDECAR_EXTS.contains(&e.as_str())) {
                skip("sidecar");
                continue;
            }
            let Some((kind, _)) = classify(path) else {
                skip("unsupported type");
                continue;
            };
            if kind == Kind::Video && !opts.videos {
                skip("video (--no-videos)");
                continue;
            }
            let rel = path.strip_prefix(&base).unwrap_or(path);
            if let Some(inc) = &include
                && !matches(inc, &name, rel, path)
            {
                skip("not matched by --include");
                continue;
            }
            if let Some(ex) = &exclude
                && matches(ex, &name, rel, path)
            {
                skip("excluded by --exclude");
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    out.errors.push((path.to_path_buf(), e.to_string()));
                    continue;
                }
            };
            if meta.len() == 0 {
                skip("empty file");
                continue;
            }
            // The same file reachable through two roots (or a followed symlink) is imported once.
            let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            if !seen.insert(canon) {
                skip("duplicate path");
                continue;
            }
            out.files.push(Candidate {
                path: std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
                size: meta.len(),
                mtime: meta.modified().ok(),
                kind,
            });
        }
    }
    out.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn touch(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    fn opts() -> WalkOptions {
        WalkOptions {
            include: vec![],
            exclude: vec![],
            videos: true,
            follow_symlinks: false,
        }
    }

    fn names(s: &Scan) -> Vec<String> {
        s.files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn tree() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for f in [
            "a.jpg",
            "B.JPEG",
            "sub/c.png",
            "sub/deep/d.CR3",
            "sub/deep/e.heic",
            "raw/f.nef",
            "clip.MOV",
            "a.jpg.xmp",
            "IMG_1.AAE",
            "x.thm",
            ".hidden.jpg",
            "._a.jpg",
            ".git/inner.jpg",
            "Thumbs.db",
            "__MACOSX/g.jpg",
            "notes.txt",
            "noext",
        ] {
            touch(r, f, b"data");
        }
        touch(r, "empty.jpg", b"");
        d
    }

    #[test]
    fn walks_recursively_and_skips_sidecars_dotfiles_and_junk() {
        let d = tree();
        let s = scan(&[d.path().to_path_buf()], &opts()).unwrap();
        let mut got = names(&s);
        got.sort();
        assert_eq!(
            got,
            ["B.JPEG", "a.jpg", "c.png", "clip.MOV", "d.CR3", "e.heic", "f.nef"]
        );
        assert_eq!(s.skipped["sidecar"], 3);
        assert_eq!(s.skipped["unsupported type"], 2); // notes.txt, noext
        assert_eq!(s.skipped["hidden/junk"], 3); // .hidden.jpg, ._a.jpg, Thumbs.db (.git and __MACOSX are pruned)
        assert_eq!(s.skipped["empty file"], 1);
    }

    #[test]
    fn videos_can_be_turned_off() {
        let d = tree();
        let s = scan(
            &[d.path().to_path_buf()],
            &WalkOptions {
                videos: false,
                ..opts()
            },
        )
        .unwrap();
        assert!(!names(&s).contains(&"clip.MOV".to_string()));
        assert_eq!(s.skipped["video (--no-videos)"], 1);
    }

    #[test]
    fn include_narrows_and_exclude_removes_files_and_directories() {
        let d = tree();
        let root = vec![d.path().to_path_buf()];
        let s = scan(
            &root,
            &WalkOptions {
                include: vec!["*.cr3".into(), "*.NEF".into()],
                ..opts()
            },
        )
        .unwrap();
        let mut got = names(&s);
        got.sort();
        assert_eq!(got, ["d.CR3", "f.nef"]);

        let s = scan(
            &root,
            &WalkOptions {
                exclude: vec!["sub/**".into(), "*.mov".into()],
                ..opts()
            },
        )
        .unwrap();
        let mut got = names(&s);
        got.sort();
        assert_eq!(got, ["B.JPEG", "a.jpg", "f.nef"]);

        // A bare directory name prunes that directory wherever it is.
        let s = scan(
            &root,
            &WalkOptions {
                exclude: vec!["deep".into()],
                ..opts()
            },
        )
        .unwrap();
        assert!(!names(&s).contains(&"d.CR3".to_string()));
        assert!(names(&s).contains(&"c.png".to_string()));
    }

    #[test]
    fn symlinks_need_the_flag() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        touch(outside.path(), "real.jpg", b"x");
        touch(outside.path(), "dir/in.png", b"x");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path().join("real.jpg"), d.path().join("link.jpg")).unwrap();
            std::os::unix::fs::symlink(outside.path().join("dir"), d.path().join("linkdir")).unwrap();
            touch(d.path(), "plain.jpg", b"x");
            let s = scan(&[d.path().to_path_buf()], &opts()).unwrap();
            assert_eq!(names(&s), ["plain.jpg"]);
            assert_eq!(s.skipped["symlink (use --follow-symlinks)"], 2); // the file link and the directory link
            let s = scan(
                &[d.path().to_path_buf()],
                &WalkOptions {
                    follow_symlinks: true,
                    ..opts()
                },
            )
            .unwrap();
            let mut got = names(&s);
            got.sort();
            assert_eq!(got, ["in.png", "link.jpg", "plain.jpg"]);
        }
    }

    #[test]
    fn explicit_files_dedupe_and_hidden_roots_work() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), ".staging/a.jpg", b"x");
        touch(d.path(), "b.jpg", b"x");
        // A hidden *root* directory is fine (only hidden entries inside are skipped); the same file
        // given twice, or via its parent too, is imported once.
        let s = scan(
            &[
                d.path().join(".staging"),
                d.path().join("b.jpg"),
                d.path().join("b.jpg"),
                d.path().to_path_buf(),
            ],
            &opts(),
        )
        .unwrap();
        assert_eq!(names(&s), ["a.jpg", "b.jpg"]);
        // Dotfiles named explicitly are still refused.
        let s = scan(&[d.path().join(".staging").join("a.jpg")], &opts()).unwrap();
        assert_eq!(names(&s), ["a.jpg"]);
        touch(d.path(), ".dot.jpg", b"x");
        let s = scan(&[d.path().join(".dot.jpg")], &opts()).unwrap();
        assert!(s.files.is_empty());
        // Unreadable roots are reported, not fatal.
        let s = scan(&[d.path().join("missing")], &opts()).unwrap();
        assert_eq!(s.errors.len(), 1);
    }

    #[test]
    fn content_types() {
        let ct = |n: &str| content_type(Path::new(n));
        assert_eq!(ct("a.JPG"), "image/jpeg");
        assert_eq!(ct("a.jpeg"), "image/jpeg");
        assert_eq!(ct("a.heic"), "image/heic");
        assert_eq!(ct("a.HEIF"), "image/heif");
        assert_eq!(ct("IMG_0001.CR3"), "image/x-canon-cr3");
        assert_eq!(ct("a.cr2"), "image/x-canon-cr2");
        assert_eq!(ct("a.nef"), "image/x-nikon-nef");
        assert_eq!(ct("a.arw"), "image/x-sony-arw");
        assert_eq!(ct("a.dng"), "image/x-adobe-dng");
        assert_eq!(ct("a.tiff"), "image/tiff");
        assert_eq!(ct("a.mov"), "video/quicktime");
        assert_eq!(ct("a.mp4"), "video/mp4");
        assert_eq!(ct("a.xyz"), "application/octet-stream");
        assert_eq!(ct("noext"), "application/octet-stream");
        // Every RAW extension the importer advertises has its own vendor type.
        for e in [
            "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "srw", "x3f", "3fr", "iiq",
            "rwl",
        ] {
            let m = ct(&format!("a.{e}"));
            assert!(m.starts_with("image/x-"), "{e} -> {m}");
        }
    }
}
