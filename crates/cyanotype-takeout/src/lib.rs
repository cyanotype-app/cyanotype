//! Google Takeout, the parts that are the same everywhere: which JSON sidecar belongs to which media file
//! ([`plan`]), which folders are albums, and what a sidecar says ([`sidecar::parse`]).
//!
//! One implementation serves the command line (which walks a directory and calls it directly) and the server (which the
//! browser asks: it can read folders but would otherwise need a second copy of these rules in another language, and the
//! rules are the part that goes wrong). Everything here works on *names* (relative paths with `/`), so it never touches a
//! disk and a plan for fifty thousand files is a few milliseconds.
//!
//! What Takeout does to names (see `names.rs` for each rule, and its tests for real-world cases):
//! `IMG_1234.JPG` has `IMG_1234.JPG.json`, or since 2024 `IMG_1234.JPG.supplemental-metadata.json`; the whole sidecar name is
//! cut to 46 characters before `.json`, which can bite into `.supplemental-metadata` (`...JPG.supplemen.json`) or into a long
//! media name itself; a second file of the same name becomes `IMG_1234(1).JPG` whose sidecar is `IMG_1234.JPG(1).json`
//! (the counter moves behind the extension); an edited copy `IMG_1234-edited.jpg` has no sidecar of its own and shares the
//! original's; and the album of a photo is the folder it is in, except the per-year folders `Photos from 2019`.
pub mod names;
pub mod sidecar;

pub use names::{Kind, Plan, PlanItem, kind_of, plan};
pub use sidecar::{Meta, parse as parse_sidecar};
