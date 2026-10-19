//! Pairing Takeout media files with their sidecars, by name, and deciding which folder is an album.
use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

/// Takeout cuts a sidecar's name (before `.json`) to this many characters.
pub const TRUNCATE_AT: usize = 46;
const SUPPLEMENTAL: &str = "supplemental-metadata";

/// Sidecar-like files Takeout writes that belong to no photo.
const NOT_SIDECARS: &[&str] = &[
    "metadata.json",
    "print-subscriptions.json",
    "shared_album_comments.json",
    "user-generated-memory-titles.json",
];

/// The word Google Photos adds to an edited copy, in the languages it has been seen in (lower case; matched after the stem's
/// own `-`).
const EDITED: &[&str] = &[
    "edited",
    "bearbeitet",
    "modifié",
    "modifie",
    "editado",
    "modificato",
    "bewerkt",
    "edytowane",
    "redigerad",
    "redigeret",
    "muokattu",
    "düzenlendi",
    "отредактировано",
    "編集済み",
    "편집됨",
];

/// Folder names of the export itself, never albums.
const ROOT_FOLDERS: &[&str] = &[
    "takeout",
    "google photos",
    "google fotos",
    "google foto",
    "google foto's",
    "google photo",
    "google 相簿",
    "google フォト",
    "google 사진",
];

/// Where Google Photos keeps what was deleted: not imported.
const TRASH_FOLDERS: &[&str] = &[
    "trash",
    "bin",
    "papelera",
    "papierkorb",
    "corbeille",
    "cestino",
    "prullenbak",
    "lixeira",
];

/// The per-year folders (`Photos from 2019` in the language of the account): every photo is in one, and they are not albums.
const YEAR_PREFIXES: &[&str] = &[
    "photos from",
    "fotos von",
    "photos de",
    "fotos de",
    "foto del",
    "foto dal",
    "foto's uit",
    "fotos från",
    "fotos fra",
    "zdjęcia z",
    "fotografias de",
    "fotky z",
    "фото за",
    "fotografii din",
    "fotók",
    "kuvat vuodelta",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// An image Cyanotype imports.
    Photo,
    /// Video is out of scope: skipped (and counted).
    Video,
    Other,
}

const PHOTO_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "heic", "heif", "avif", "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2",
    "raf", "orf", "rw2", "rwl", "pef", "srw", "3fr", "iiq", "erf", "kdc", "mrw", "x3f", "crw", "tif", "tiff",
];
// `mp` is a Pixel "motion photo" video track; `mp~2` a duplicate of one.
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "m4v", "avi", "3gp", "3g2", "mkv", "webm", "mts", "m2ts", "mpg", "mpeg", "wmv", "flv", "mp", "mp~2",
];

pub fn kind_of(name: &str) -> Kind {
    let ext = split_ext(name).1.to_lowercase();
    if PHOTO_EXTS.contains(&ext.as_str()) {
        Kind::Photo
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        Kind::Video
    } else {
        Kind::Other
    }
}

/// `("IMG_1234", "JPG")` for `IMG_1234.JPG`; a leading dot is not an extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

/// `("IMG_1234", 1)` for `IMG_1234(1)`.
fn split_dup(stem: &str) -> (&str, u32) {
    if let Some(inner) = stem.strip_suffix(')')
        && let Some(open) = inner.rfind('(')
        && let Ok(n) = inner[open + 1..].parse::<u32>()
        && open > 0
        && !inner[open + 1..].is_empty()
    {
        return (&stem[..open], n);
    }
    (stem, 0)
}

/// `Some("IMG_1234")` for `IMG_1234-edited`, in any of the languages.
fn strip_edited(stem: &str) -> Option<&str> {
    let lower = stem.to_lowercase();
    for w in EDITED {
        let suffix = format!("-{w}");
        if lower.ends_with(&suffix) {
            // Cut by characters of the original (lower-casing can change byte lengths).
            let keep = stem.chars().count() - suffix.chars().count();
            let end = stem.char_indices().nth(keep).map_or(stem.len(), |(i, _)| i);
            return (end > 0).then(|| &stem[..end]);
        }
    }
    None
}

#[derive(Debug)]
struct Sidecar {
    path: String,
    /// The media file name it is for (possibly cut at [`TRUNCATE_AT`] characters), without the dup counter.
    base: String,
    dup: u32,
    /// The newer `.supplemental-metadata.json` form (any length of it).
    supplemental: bool,
}

/// Read a sidecar's name; None for anything that is not one.
fn parse_sidecar_name(path: &str, file: &str) -> Option<Sidecar> {
    let lower = file.to_lowercase();
    if !lower.ends_with(".json") || NOT_SIDECARS.contains(&lower.as_str()) {
        return None;
    }
    let stem = &file[..file.len() - ".json".len()];
    let (stem, dup) = split_dup(stem);
    // `.supplemental-metadata` as Takeout cut it: any prefix of it, up to the whole word, even just the dot.
    if let Some(i) = stem.rfind('.') {
        let tail = stem[i + 1..].to_lowercase();
        if SUPPLEMENTAL.starts_with(&tail) {
            return Some(Sidecar {
                path: path.to_string(),
                base: stem[..i].to_string(),
                dup,
                supplemental: true,
            });
        }
    }
    Some(Sidecar {
        path: path.to_string(),
        base: stem.to_string(),
        dup,
        supplemental: false,
    })
}

/// The sidecars of one folder.
struct Folder {
    sidecars: Vec<Sidecar>,
    exact: HashMap<(String, u32), Vec<usize>>,
    lower: HashMap<(String, u32), Vec<usize>>,
}

impl Folder {
    fn new(mut sidecars: Vec<Sidecar>) -> Self {
        // Deterministic whatever the order paths arrive in.
        sidecars.sort_by(|a, b| a.path.cmp(&b.path));
        let (mut exact, mut lower) = (HashMap::new(), HashMap::new());
        for (i, s) in sidecars.iter().enumerate() {
            exact.entry((s.base.clone(), s.dup)).or_insert_with(Vec::new).push(i);
            lower
                .entry((s.base.to_lowercase(), s.dup))
                .or_insert_with(Vec::new)
                .push(i);
        }
        Folder { sidecars, exact, lower }
    }

    /// Among sidecars that are equally good, the newer form first, then the name.
    fn best(&self, ixs: &[usize]) -> Option<usize> {
        ixs.iter()
            .copied()
            .max_by_key(|&i| (self.sidecars[i].supplemental, std::cmp::Reverse(&self.sidecars[i].path)))
    }

    /// A sidecar for the media name `name` with counter `dup`: exact, then cut at [`TRUNCATE_AT`], then ignoring case.
    fn lookup(&self, name: &str, dup: u32) -> Option<usize> {
        if let Some(i) = self.exact.get(&(name.to_string(), dup)).and_then(|v| self.best(v)) {
            return Some(i);
        }
        if name.chars().count() > TRUNCATE_AT {
            let cut: Vec<usize> = self
                .sidecars
                .iter()
                .enumerate()
                .filter(|(_, s)| s.dup == dup && s.base.chars().count() == TRUNCATE_AT && name.starts_with(&s.base))
                .map(|(i, _)| i)
                .collect();
            if let Some(i) = self.best(&cut) {
                return Some(i);
            }
        }
        self.lower.get(&(name.to_lowercase(), dup)).and_then(|v| self.best(v))
    }

    /// The sidecar of an edited copy's original when the original has another extension (an edited HEIC is exported as a JPEG):
    /// only when exactly one sidecar fits.
    fn lookup_any_ext(&self, stem: &str, dup: u32) -> Option<usize> {
        let fits: Vec<usize> = self
            .sidecars
            .iter()
            .enumerate()
            .filter(|(_, s)| s.dup == dup && split_ext(&s.base).0.eq_ignore_ascii_case(stem))
            .map(|(i, _)| i)
            .collect();
        let mut bases: Vec<&str> = fits.iter().map(|&i| self.sidecars[i].base.as_str()).collect();
        bases.sort_unstable();
        bases.dedup();
        if bases.len() == 1 { self.best(&fits) } else { None }
    }

    /// The sidecar of media file `name`, and whether it is an edited copy borrowing its original's.
    fn find(&self, name: &str) -> Option<(usize, bool)> {
        let (stem, ext) = split_ext(name);
        let (stem0, dup) = split_dup(stem);
        let join = |s: &str| {
            if ext.is_empty() {
                s.to_string()
            } else {
                format!("{s}.{ext}")
            }
        };
        // The name as it is (a file really called `Pic (2).jpg` has `Pic (2).jpg.json`), then with its counter moved.
        if let Some(i) = self.lookup(name, 0) {
            return Some((i, false));
        }
        if dup > 0
            && let Some(i) = self.lookup(&join(stem0), dup)
        {
            return Some((i, false));
        }
        if let Some(orig) = strip_edited(stem0) {
            if let Some(i) = self.lookup(&join(orig), dup) {
                return Some((i, true));
            }
            if let Some(i) = self.lookup_any_ext(orig, dup) {
                return Some((i, true));
            }
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanItem {
    /// The media file, as given.
    pub path: String,
    /// Its sidecar, when one was found.
    pub sidecar: Option<String>,
    /// The album it belongs to (the name of its folder), when that folder is an album.
    pub album: Option<String>,
    /// An edited copy (`-edited`) that shares its original's sidecar.
    pub edited: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// Every photo to import, in the order given.
    pub photos: Vec<PlanItem>,
    /// Videos, which are skipped.
    pub videos: Vec<String>,
    /// Files that are neither (a `.gif`, an `.html`): ignored.
    pub other: Vec<String>,
    /// Photos in the export's trash folder: skipped.
    pub trashed: Vec<String>,
    /// Photos per album folder.
    pub albums: BTreeMap<String, usize>,
    /// Sidecars no media file claimed.
    pub unused_sidecars: Vec<String>,
}

impl Plan {
    pub fn without_sidecar(&self) -> usize {
        self.photos.iter().filter(|p| p.sidecar.is_none()).count()
    }
}

enum Place {
    Album(String),
    /// A folder that is not an album (the root, a year, the archive).
    Plain,
    Trash,
}

fn is_year_folder(lower: &str) -> bool {
    let Some((head, year)) = lower.rsplit_once(' ') else {
        return false;
    };
    year.len() == 4
        && year.chars().all(|c| c.is_ascii_digit())
        && (1900..=2100).contains(&year.parse::<u32>().unwrap_or(0))
        && YEAR_PREFIXES.contains(&head.trim())
}

fn place_of(dir: &str) -> Place {
    let name = dir.rsplit('/').next().unwrap_or("").trim();
    let lower = name.to_lowercase();
    if name.is_empty() || ROOT_FOLDERS.contains(&lower.as_str()) || is_year_folder(&lower) {
        return Place::Plain;
    }
    // The archive is a place, not an album; its photos say `archived` in their sidecars.
    if matches!(
        lower.as_str(),
        "archive" | "archivo" | "archiv" | "archives" | "arquivo" | "archivio"
    ) {
        return Place::Plain;
    }
    if TRASH_FOLDERS.contains(&lower.as_str()) {
        return Place::Trash;
    }
    Place::Album(name.chars().take(200).collect())
}

fn is_junk(dir: &str, file: &str) -> bool {
    let lower = file.to_lowercase();
    dir.split('/').any(|s| s.starts_with('.') || s == "__MACOSX")
        || file.starts_with('.')
        || lower == "thumbs.db"
        || lower == "desktop.ini"
}

/// Pair every file of an export with its sidecar. `paths` are relative paths with `/` (the browser's
/// `webkitRelativePath`, or the directory walk's); any leading folders (`Takeout/Google Photos/`) are fine. Files of one folder
/// must all be in the same call.
pub fn plan<S: AsRef<str>>(paths: &[S]) -> Plan {
    let mut by_dir: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut order: Vec<(String, String)> = Vec::new();
    for p in paths {
        let p = p.as_ref().replace('\\', "/");
        let p = p.trim_start_matches("./").trim_start_matches('/').to_string();
        let (dir, file) = match p.rsplit_once('/') {
            Some((d, f)) => (d.to_string(), f.to_string()),
            None => (String::new(), p.clone()),
        };
        if file.is_empty() || is_junk(&dir, &file) {
            continue;
        }
        by_dir.entry(dir.clone()).or_default().push((p.clone(), file.clone()));
        order.push((dir, p));
    }
    let mut out = Plan::default();
    let mut folders: HashMap<String, Folder> = HashMap::new();
    let mut claimed: HashMap<String, bool> = HashMap::new();
    for (dir, files) in &by_dir {
        let sidecars: Vec<Sidecar> = files.iter().filter_map(|(p, f)| parse_sidecar_name(p, f)).collect();
        for s in &sidecars {
            claimed.insert(s.path.clone(), false);
        }
        folders.insert(dir.clone(), Folder::new(sidecars));
    }
    for (dir, path) in &order {
        let file = path.rsplit('/').next().unwrap_or(path);
        if parse_sidecar_name(path, file).is_some() {
            continue; // a sidecar, handled with the folder
        }
        if file.to_lowercase().ends_with(".json") {
            continue; // metadata.json and friends
        }
        let folder = &folders[dir];
        let found = folder
            .find(file)
            .map(|(i, edited)| (folder.sidecars[i].path.clone(), edited));
        if let Some((sp, _)) = &found {
            claimed.insert(sp.clone(), true);
        }
        match kind_of(file) {
            Kind::Video => out.videos.push(path.clone()),
            Kind::Other => out.other.push(path.clone()),
            Kind::Photo => match place_of(dir) {
                Place::Trash => out.trashed.push(path.clone()),
                place => {
                    let album = match place {
                        Place::Album(a) => Some(a),
                        _ => None,
                    };
                    if let Some(a) = &album {
                        *out.albums.entry(a.clone()).or_insert(0) += 1;
                    }
                    let (sidecar, edited) = match found {
                        Some((s, e)) => (Some(s), e),
                        None => (None, false),
                    };
                    out.photos.push(PlanItem {
                        path: path.clone(),
                        sidecar,
                        album,
                        edited,
                    });
                }
            },
        }
    }
    let mut unused: Vec<String> = claimed.into_iter().filter(|(_, c)| !c).map(|(p, _)| p).collect();
    unused.sort();
    out.unused_sidecars = unused;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str, n: usize) -> String {
        s.chars().take(n).collect()
    }

    /// One real-world naming case: the files of a folder, and which sidecar each media file must get (None: none).
    struct Case {
        what: &'static str,
        files: Vec<String>,
        want: Vec<(&'static str, Option<String>)>,
    }

    fn s(x: &str) -> String {
        x.to_string()
    }

    fn cases() -> Vec<Case> {
        // 41 characters: the supplemental word is cut after 5 more (46 in all).
        let m41 = "20190920_184501_Samsung_Galaxy_S9_pic.jpg";
        assert_eq!(m41.chars().count(), 41);
        let long = "2019-05-12_a_very_long_file_name_that_goes_on_forever_and_ever.jpg";
        let long_cut = chars(long, TRUNCATE_AT);
        let long2 = "2019-05-12_a_very_long_file_name_that_goes_on_forever_and_never.jpg";
        let umlaut = "Übergrößenträger_Fotoaufnahme_vom_Sommerfest_in_Köln_2019.jpg";
        let umlaut_cut = chars(umlaut, TRUNCATE_AT);
        vec![
            Case {
                what: "legacy sidecar: name plus .json",
                files: vec![s("IMG_1234.JPG"), s("IMG_1234.JPG.json")],
                want: vec![("IMG_1234.JPG", Some(s("IMG_1234.JPG.json")))],
            },
            Case {
                what: "2024+ supplemental-metadata sidecar",
                files: vec![s("IMG_1234.JPG"), s("IMG_1234.JPG.supplemental-metadata.json")],
                want: vec![("IMG_1234.JPG", Some(s("IMG_1234.JPG.supplemental-metadata.json")))],
            },
            Case {
                what: "supplemental word cut at 46 characters (5 characters of it left)",
                files: vec![s(m41), format!("{m41}.supp.json")],
                want: vec![(m41, Some(format!("{m41}.supp.json")))],
            },
            Case {
                what: "supplemental word cut to nothing but the dot",
                files: vec![
                    s("IMG_20190920_184501_Samsung_Galaxy_S9_p.jpg"),
                    s("IMG_20190920_184501_Samsung_Galaxy_S9_p.jpg..json"),
                ],
                want: vec![(
                    "IMG_20190920_184501_Samsung_Galaxy_S9_p.jpg",
                    Some(s("IMG_20190920_184501_Samsung_Galaxy_S9_p.jpg..json")),
                )],
            },
            Case {
                what: "supplemental word cut to its first letters (.s, .su, .supplemental-m)",
                files: vec![
                    s("a.jpg"),
                    s("a.jpg.s.json"),
                    s("b.jpg"),
                    s("b.jpg.su.json"),
                    s("c.jpg"),
                    s("c.jpg.supplemental-m.json"),
                ],
                want: vec![
                    ("a.jpg", Some(s("a.jpg.s.json"))),
                    ("b.jpg", Some(s("b.jpg.su.json"))),
                    ("c.jpg", Some(s("c.jpg.supplemental-m.json"))),
                ],
            },
            Case {
                what: "a media name over 46 characters: the sidecar is the name cut to 46",
                files: vec![s(long), format!("{long_cut}.json")],
                want: vec![(long, Some(format!("{long_cut}.json")))],
            },
            Case {
                what: "two long names sharing their first 46 characters do not both get one sidecar by accident",
                files: vec![s(long), s(long2), format!("{long_cut}.json")],
                // both share the cut name; Takeout would have numbered the second ((1)); without a counter both pair
                // with the one sidecar rather than guessing which is which.
                want: vec![
                    (long, Some(format!("{long_cut}.json"))),
                    (long2, Some(format!("{long_cut}.json"))),
                ],
            },
            Case {
                what: "a long name with non-ASCII characters is cut by characters, not bytes",
                files: vec![s(umlaut), format!("{umlaut_cut}.json")],
                want: vec![(umlaut, Some(format!("{umlaut_cut}.json")))],
            },
            Case {
                what: "the counter moves behind the extension: IMG(1).jpg has IMG.jpg(1).json",
                files: vec![
                    s("IMG_1234.jpg"),
                    s("IMG_1234.jpg.json"),
                    s("IMG_1234(1).jpg"),
                    s("IMG_1234.jpg(1).json"),
                ],
                want: vec![
                    ("IMG_1234.jpg", Some(s("IMG_1234.jpg.json"))),
                    ("IMG_1234(1).jpg", Some(s("IMG_1234.jpg(1).json"))),
                ],
            },
            Case {
                what: "the counter with the supplemental form: IMG.jpg.supplemental-metadata(1).json",
                files: vec![
                    s("IMG_1234.jpg"),
                    s("IMG_1234.jpg.supplemental-metadata.json"),
                    s("IMG_1234(1).jpg"),
                    s("IMG_1234.jpg.supplemental-metadata(1).json"),
                    s("IMG_1234(2).jpg"),
                    s("IMG_1234.jpg.supplemental-metadata(2).json"),
                ],
                want: vec![
                    ("IMG_1234.jpg", Some(s("IMG_1234.jpg.supplemental-metadata.json"))),
                    ("IMG_1234(1).jpg", Some(s("IMG_1234.jpg.supplemental-metadata(1).json"))),
                    ("IMG_1234(2).jpg", Some(s("IMG_1234.jpg.supplemental-metadata(2).json"))),
                ],
            },
            Case {
                what: "a counter is never matched to the sidecar without one",
                files: vec![s("IMG_1234.jpg.json"), s("IMG_1234(1).jpg")],
                want: vec![("IMG_1234(1).jpg", None)],
            },
            Case {
                what: "an edited copy shares the original's sidecar",
                files: vec![s("IMG_1234.jpg"), s("IMG_1234-edited.jpg"), s("IMG_1234.jpg.json")],
                want: vec![
                    ("IMG_1234.jpg", Some(s("IMG_1234.jpg.json"))),
                    ("IMG_1234-edited.jpg", Some(s("IMG_1234.jpg.json"))),
                ],
            },
            Case {
                what: "an edited copy of an original that has a supplemental sidecar",
                files: vec![s("IMG_1234-edited.jpg"), s("IMG_1234.jpg.supplemental-metadata.json")],
                want: vec![(
                    "IMG_1234-edited.jpg",
                    Some(s("IMG_1234.jpg.supplemental-metadata.json")),
                )],
            },
            Case {
                what: "edited copies in other languages",
                files: vec![
                    s("a-bearbeitet.jpg"),
                    s("a.jpg.json"),
                    s("b-modifié.jpg"),
                    s("b.jpg.json"),
                    s("c-editado.jpg"),
                    s("c.jpg.json"),
                    s("d-bewerkt.jpg"),
                    s("d.jpg.json"),
                    s("e-EDITED.JPG"),
                    s("e.JPG.json"),
                ],
                want: vec![
                    ("a-bearbeitet.jpg", Some(s("a.jpg.json"))),
                    ("b-modifié.jpg", Some(s("b.jpg.json"))),
                    ("c-editado.jpg", Some(s("c.jpg.json"))),
                    ("d-bewerkt.jpg", Some(s("d.jpg.json"))),
                    ("e-EDITED.JPG", Some(s("e.JPG.json"))),
                ],
            },
            Case {
                what: "an edited copy with a counter: IMG-edited(1).jpg is IMG.jpg(1).json's",
                files: vec![
                    s("IMG_1234-edited(1).jpg"),
                    s("IMG_1234.jpg(1).json"),
                    s("IMG_1234.jpg.json"),
                ],
                want: vec![("IMG_1234-edited(1).jpg", Some(s("IMG_1234.jpg(1).json")))],
            },
            Case {
                what: "an edited HEIC is exported as a JPEG; its sidecar is the original's, whatever the extension",
                files: vec![s("IMG_0007-edited.jpg"), s("IMG_0007.HEIC.supplemental-metadata.json")],
                want: vec![(
                    "IMG_0007-edited.jpg",
                    Some(s("IMG_0007.HEIC.supplemental-metadata.json")),
                )],
            },
            Case {
                what: "no guess when two originals fit an edited copy",
                files: vec![s("X-edited.jpg"), s("X.HEIC.json"), s("X.png.json")],
                want: vec![("X-edited.jpg", None)],
            },
            Case {
                what: "extension case differs between media and sidecar",
                files: vec![
                    s("img_1234.jpg"),
                    s("IMG_1234.JPG.json"),
                    s("Holiday.JPG"),
                    s("Holiday.jpg.json"),
                ],
                want: vec![("Holiday.JPG", Some(s("Holiday.jpg.json")))],
            },
            Case {
                what: "a file really named with (2): its own sidecar has no counter",
                files: vec![s("Pic (2).jpg"), s("Pic (2).jpg.json"), s("Pic.jpg"), s("Pic.jpg.json")],
                want: vec![
                    ("Pic (2).jpg", Some(s("Pic (2).jpg.json"))),
                    ("Pic.jpg", Some(s("Pic.jpg.json"))),
                ],
            },
            Case {
                what: "names with spaces, commas, brackets and unicode",
                files: vec![
                    s("Mein Foto, Sommer [2019] 日本.jpg"),
                    s("Mein Foto, Sommer [2019] 日本.jpg.json"),
                ],
                want: vec![(
                    "Mein Foto, Sommer [2019] 日本.jpg",
                    Some(s("Mein Foto, Sommer [2019] 日本.jpg.json")),
                )],
            },
            Case {
                what: "tilde copies (IMG~2.jpg) are ordinary names",
                files: vec![s("IMG_1234~2.jpg"), s("IMG_1234~2.jpg.json")],
                want: vec![("IMG_1234~2.jpg", Some(s("IMG_1234~2.jpg.json")))],
            },
            Case {
                what: "a photo with no sidecar",
                files: vec![s("IMG_1.jpg"), s("IMG_2.jpg"), s("IMG_2.jpg.json")],
                want: vec![("IMG_1.jpg", None), ("IMG_2.jpg", Some(s("IMG_2.jpg.json")))],
            },
            Case {
                what: "folder files that are not sidecars are never taken for one",
                files: vec![
                    s("IMG_1.jpg"),
                    s("metadata.json"),
                    s("print-subscriptions.json"),
                    s("shared_album_comments.json"),
                ],
                want: vec![("IMG_1.jpg", None)],
            },
            Case {
                what: "a PNG screenshot and a WebP",
                files: vec![
                    s("Screenshot_20190101-101010.png"),
                    s("Screenshot_20190101-101010.png.supplemental-metadata.json"),
                    s("sticker.webp"),
                    s("sticker.webp.json"),
                ],
                want: vec![
                    (
                        "Screenshot_20190101-101010.png",
                        Some(s("Screenshot_20190101-101010.png.supplemental-metadata.json")),
                    ),
                    ("sticker.webp", Some(s("sticker.webp.json"))),
                ],
            },
            Case {
                what: "when both forms exist the newer one is used",
                files: vec![s("a.jpg"), s("a.jpg.json"), s("a.jpg.supplemental-metadata.json")],
                want: vec![("a.jpg", Some(s("a.jpg.supplemental-metadata.json")))],
            },
        ]
    }

    #[test]
    fn real_world_naming_cases() {
        for case in cases() {
            let paths: Vec<String> = case
                .files
                .iter()
                .map(|f| format!("Takeout/Google Photos/Trip/{f}"))
                .collect();
            let p = plan(&paths);
            for (media, want) in &case.want {
                let item = p
                    .photos
                    .iter()
                    .find(|i| i.path.ends_with(&format!("/{media}")))
                    .unwrap_or_else(|| panic!("{}: {media} is not in the plan", case.what));
                let want = want.as_ref().map(|w| format!("Takeout/Google Photos/Trip/{w}"));
                assert_eq!(item.sidecar, want, "{}: {media}", case.what);
            }
        }
    }

    #[test]
    fn matching_does_not_depend_on_the_order_files_arrive_in() {
        let mut files = vec![
            "a/IMG_1234.jpg.json",
            "a/IMG_1234.jpg.supplemental-metadata.json",
            "a/IMG_1234.jpg",
            "a/IMG_1234(1).jpg",
            "a/IMG_1234.jpg(1).json",
            "a/IMG_1234-edited.jpg",
        ];
        let first = plan(&files);
        files.reverse();
        let mut second = plan(&files);
        let mut first = first;
        first.photos.sort_by(|a, b| a.path.cmp(&b.path));
        second.photos.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(first.photos, second.photos);
    }

    #[test]
    fn sidecars_are_found_next_to_the_media_only() {
        let p = plan(&["A/x.jpg", "B/x.jpg.json"]);
        assert_eq!(p.photos[0].sidecar, None);
        assert_eq!(p.unused_sidecars, vec!["B/x.jpg.json".to_string()]);
    }

    #[test]
    fn albums_are_named_folders_and_year_folders_and_the_root_are_not() {
        let p = plan(&[
            "Takeout/Google Photos/Photos from 2019/a.jpg",
            "Takeout/Google Photos/Fotos von 2020/b.jpg",
            "Takeout/Google Photos/Photos de 2021/c.jpg",
            "Takeout/Google Photos/Trip to Lisbon/d.jpg",
            "Takeout/Google Photos/Trip to Lisbon/e.jpg",
            "Takeout/Google Photos/Birthday 2019/f.jpg",
            "Takeout/Google Photos/Archive/g.jpg",
            "Takeout/Google Photos/loose.jpg",
            "Google Photos/Untitled(1)/h.jpg",
            "Takeout/Google Photos/Trash/gone.jpg",
        ]);
        let album = |name: &str| p.photos.iter().find(|i| i.path.ends_with(name)).unwrap().album.clone();
        assert_eq!(album("a.jpg"), None);
        assert_eq!(album("b.jpg"), None);
        assert_eq!(album("c.jpg"), None);
        assert_eq!(album("d.jpg").as_deref(), Some("Trip to Lisbon"));
        assert_eq!(
            album("f.jpg").as_deref(),
            Some("Birthday 2019"),
            "a year in an album's own name is still an album"
        );
        assert_eq!(album("g.jpg"), None, "the archive is not an album");
        assert_eq!(album("loose.jpg"), None);
        assert_eq!(album("h.jpg").as_deref(), Some("Untitled(1)"));
        assert_eq!(p.albums.get("Trip to Lisbon"), Some(&2));
        assert_eq!(p.albums.len(), 3);
        assert_eq!(p.trashed, vec!["Takeout/Google Photos/Trash/gone.jpg".to_string()]);
        assert_eq!(p.photos.len(), 9);
    }

    #[test]
    fn videos_and_other_files_are_set_aside_and_their_sidecars_are_not_unused() {
        let p = plan(&[
            "Trip/a.jpg",
            "Trip/a.jpg.json",
            "Trip/clip.MP4",
            "Trip/clip.MP4.supplemental-metadata.json",
            "Trip/IMG_1.MOV",
            "Trip/anim.gif",
            "Trip/notes.html",
            "Trip/metadata.json",
            "Trip/.DS_Store",
            "__MACOSX/Trip/._a.jpg",
            ".hidden/b.jpg",
        ]);
        assert_eq!(p.photos.len(), 1);
        assert_eq!(
            p.videos,
            vec!["Trip/clip.MP4".to_string(), "Trip/IMG_1.MOV".to_string()]
        );
        assert_eq!(
            p.other,
            vec!["Trip/anim.gif".to_string(), "Trip/notes.html".to_string()]
        );
        assert!(p.unused_sidecars.is_empty(), "{:?}", p.unused_sidecars);
        assert_eq!(p.without_sidecar(), 0);
    }

    #[test]
    fn windows_paths_and_a_bare_folder_work() {
        let p = plan(&[
            "Google Photos\\Trip\\a.jpg",
            "Google Photos\\Trip\\a.jpg.json",
            "./b.jpg",
        ]);
        assert_eq!(p.photos[0].sidecar.as_deref(), Some("Google Photos/Trip/a.jpg.json"));
        assert_eq!(p.photos[0].album.as_deref(), Some("Trip"));
        assert_eq!(p.photos[1].album, None);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of("a.JPG"), Kind::Photo);
        assert_eq!(kind_of("a.HEIC"), Kind::Photo);
        assert_eq!(kind_of("a.mov"), Kind::Video);
        assert_eq!(kind_of("a.MP"), Kind::Video);
        assert_eq!(kind_of("a.gif"), Kind::Other);
        assert_eq!(kind_of("noext"), Kind::Other);
        assert_eq!(kind_of(".jpg"), Kind::Other);
    }

    #[test]
    fn a_year_folder_needs_a_known_phrase_and_a_real_year() {
        assert!(is_year_folder("photos from 2019"));
        assert!(!is_year_folder("photos from 1850"));
        assert!(!is_year_folder("summer 2019"));
        assert!(!is_year_folder("photos from lisbon"));
    }
}
