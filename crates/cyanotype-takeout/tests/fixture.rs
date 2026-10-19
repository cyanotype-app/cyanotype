//! The tiny synthetic Takeout the Playwright flow uploads (`apps/web/e2e/fixtures/takeout-synthetic`) and the plan the real
//! matcher makes of it. The browser test answers `/assets/takeout/plan` with the stored plan, so this test is what keeps that
//! mock honest: it fails when the matcher's answer for the fixture changes. `UPDATE_FIXTURE=1 cargo test -p cyanotype-takeout`
//! rewrites the stored plan after a deliberate change.
use std::path::{Path, PathBuf};

use cyanotype_takeout::plan;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/e2e/fixtures")
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = format!("{prefix}/{name}");
        if e.file_type().unwrap().is_dir() {
            walk(&e.path(), &rel, out);
        } else {
            out.push(rel);
        }
    }
}

#[test]
fn the_synthetic_takeout_is_planned_as_the_browser_test_expects() {
    let mut paths = Vec::new();
    walk(&fixtures().join("takeout-synthetic/Takeout"), "Takeout", &mut paths);
    assert_eq!(paths.len(), 13, "{paths:#?}");
    let p = plan(&paths);
    let mut json = serde_json::to_string_pretty(&p).unwrap();
    json.push('\n');
    let stored = fixtures().join("takeout-plan.json");
    if std::env::var("UPDATE_FIXTURE").is_ok() {
        std::fs::write(&stored, &json).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&stored).unwrap(),
        json,
        "run with UPDATE_FIXTURE=1 if this change is intended"
    );
    // What the fixture is for.
    assert_eq!(p.photos.len(), 6);
    assert_eq!(p.videos.len(), 1);
    assert_eq!(p.without_sidecar(), 1);
    assert_eq!(p.albums.keys().collect::<Vec<_>>(), vec!["Trip to Lisbon"]);
    assert!(p.photos.iter().any(|i| i.edited));
}
