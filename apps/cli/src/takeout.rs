//! `cyanotype import --takeout <dir>`: an unzipped Google Takeout folder. The pairing of photos with their JSON sidecars, the
//! albums and what is skipped come from the same crate the server uses for the browser (`cyanotype-takeout`); this file only
//! walks the folder and reads the sidecars. The sidecar text travels with each photo's check call and the server applies it
//! after the photo is processed (docs/import.md, "Moving from Google Photos"), so stopping the import loses nothing.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use walkdir::WalkDir;

use crate::{api::TakeoutField, walk};

/// What a Takeout folder holds, for the summary the person sees before anything is uploaded.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub photos: usize,
    pub videos: usize,
    pub other: usize,
    pub trashed: usize,
    pub albums: usize,
    pub without_sidecar: usize,
    pub unused_sidecars: usize,
}

pub struct Found {
    pub scan: walk::Scan,
    /// The metadata to send with each photo, by path.
    pub fields: HashMap<PathBuf, TakeoutField>,
    pub summary: Summary,
}

/// Walk `dir` and pair its files. `albums` and `favorites` are the options the person chose.
pub fn scan(dir: &Path, albums: bool, favorites: bool) -> Result<Found> {
    let meta = std::fs::metadata(dir).with_context(|| format!("cannot read {}", dir.display()))?;
    if !meta.is_dir() {
        bail!(
            "{} is not a folder. --takeout takes the folder a Google Takeout .zip unpacks to (unzip it first).",
            dir.display()
        );
    }
    // Relative names start with the folder's own name, as the browser's do, so choosing an album folder directly still names it.
    let top = dir
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let mut rels: Vec<String> = Vec::new();
    let mut errors = Vec::new();
    for e in WalkDir::new(dir).follow_links(false) {
        match e {
            Ok(e) if e.file_type().is_file() => {
                if let Ok(rel) = e.path().strip_prefix(dir) {
                    let rel = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    rels.push(if top.is_empty() { rel } else { format!("{top}/{rel}") });
                }
            }
            Ok(_) => {}
            Err(err) => errors.push((
                err.path().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf()),
                err.to_string(),
            )),
        }
    }
    let plan = cyanotype_takeout::plan(&rels);
    let on_disk = |rel: &str| -> PathBuf {
        let inner = rel.strip_prefix(&format!("{top}/")).unwrap_or(rel);
        dir.join(inner)
    };
    let mut scan = walk::Scan {
        errors,
        ..walk::Scan::default()
    };
    let mut fields = HashMap::new();
    for item in &plan.photos {
        let path = on_disk(&item.path);
        let Ok(m) = std::fs::metadata(&path) else {
            scan.errors.push((path, "cannot stat".into()));
            continue;
        };
        let sidecar = item.sidecar.as_ref().and_then(|s| {
            let p = on_disk(s);
            match std::fs::metadata(&p) {
                Ok(m) if m.len() <= cyanotype_takeout::sidecar::MAX_SIDECAR_BYTES as u64 => {
                    std::fs::read_to_string(&p).ok()
                }
                _ => None,
            }
        });
        fields.insert(
            path.clone(),
            TakeoutField {
                sidecar,
                album: if albums { item.album.clone() } else { None },
                favorites,
            },
        );
        scan.files.push(walk::Candidate {
            path,
            size: m.len(),
            mtime: m.modified().ok(),
            kind: walk::Kind::Image,
        });
    }
    for (why, n) in [
        ("video (not imported)", plan.videos.len()),
        ("not a photo", plan.other.len()),
        ("in the Takeout trash folder", plan.trashed.len()),
    ] {
        if n > 0 {
            scan.skipped.insert(why, n as u64);
        }
    }
    let summary = Summary {
        photos: plan.photos.len(),
        videos: plan.videos.len(),
        other: plan.other.len(),
        trashed: plan.trashed.len(),
        albums: plan.albums.len(),
        without_sidecar: plan.without_sidecar(),
        unused_sidecars: plan.unused_sidecars.len(),
    };
    Ok(Found { scan, fields, summary })
}

impl Summary {
    pub fn print(&self, albums: bool) {
        eprintln!("Google Takeout: {} photos to import.", self.photos);
        eprintln!(
            "  {} without a sidecar (they import as they are), {} videos skipped, {} other files ignored{}.",
            self.without_sidecar,
            self.videos,
            self.other,
            if self.trashed > 0 {
                format!(", {} in the Takeout trash skipped", self.trashed)
            } else {
                String::new()
            }
        );
        if albums {
            eprintln!("  {} albums will be created from named folders.", self.albums);
        } else {
            eprintln!("  albums are not created (--takeout-no-albums).");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn pairs_a_synthetic_export_and_sends_the_sidecar_text_and_album() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Takeout");
        let gp = "Google Photos";
        put(&root, &format!("{gp}/Photos from 2019/IMG_1.jpg"), "a");
        put(
            &root,
            &format!("{gp}/Photos from 2019/IMG_1.jpg.supplemental-metadata.json"),
            r#"{"favorited":true}"#,
        );
        put(&root, &format!("{gp}/Trip/IMG_2.jpg"), "b");
        put(&root, &format!("{gp}/Trip/IMG_2.jpg.json"), r#"{"description":"x"}"#);
        put(&root, &format!("{gp}/Trip/IMG_3.png"), "c");
        put(&root, &format!("{gp}/Trip/clip.mp4"), "v");
        put(&root, &format!("{gp}/Trip/metadata.json"), "{}");
        let f = scan(&root, true, true).unwrap();
        assert_eq!(f.scan.files.len(), 3);
        assert_eq!(
            f.summary,
            Summary {
                photos: 3,
                videos: 1,
                other: 0,
                trashed: 0,
                albums: 1,
                without_sidecar: 1,
                unused_sidecars: 0
            }
        );
        let by = |n: &str| f.fields.iter().find(|(p, _)| p.ends_with(n)).unwrap().1;
        assert_eq!(by("IMG_1.jpg").album, None);
        assert!(by("IMG_1.jpg").sidecar.as_deref().unwrap().contains("favorited"));
        assert_eq!(by("IMG_2.jpg").album.as_deref(), Some("Trip"));
        assert_eq!(by("IMG_3.png").sidecar, None);
        // The options turn albums off, and the favourite flag off.
        let f = scan(&root, false, false).unwrap();
        assert!(f.fields.values().all(|x| x.album.is_none() && !x.favorites));
    }

    #[test]
    fn a_file_or_a_missing_folder_is_refused_with_a_hint() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "x.zip", "z");
        let e = scan(&tmp.path().join("x.zip"), true, true).err().unwrap().to_string();
        assert!(e.contains("unzip"), "{e}");
        assert!(scan(&tmp.path().join("nope"), true, true).is_err());
    }

    #[test]
    fn choosing_an_album_folder_directly_names_the_album_after_it() {
        let tmp = tempfile::tempdir().unwrap();
        put(tmp.path(), "Trip to Lisbon/IMG_1.jpg", "a");
        let f = scan(&tmp.path().join("Trip to Lisbon"), true, true).unwrap();
        assert_eq!(
            f.fields.values().next().unwrap().album.as_deref(),
            Some("Trip to Lisbon")
        );
    }
}
