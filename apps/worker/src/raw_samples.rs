//! Runs against real camera files when `CYANOTYPE_RAW_SAMPLES` points at a directory of them
//! (`scripts/fetch-raw-samples.sh` downloads CC0 samples from raw.pixls.us). Skipped otherwise, so
//! plain `cargo test` and CI need no large files.
use std::sync::Arc;

use crate::{
    formats::{self, Kind},
    raw::{self, Limits},
};

fn samples() -> Vec<(String, Arc<Vec<u8>>)> {
    let Ok(dir) = std::env::var("CYANOTYPE_RAW_SAMPLES") else {
        eprintln!("CYANOTYPE_RAW_SAMPLES not set: skipping real RAW samples");
        return Vec::new();
    };
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                Arc::new(std::fs::read(&p).unwrap()),
            )
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

const LIMITS: Limits = Limits {
    max_decode_bytes: 600 << 20,
    allow_develop: true,
};

#[test]
fn explore_samples() {
    for (name, bytes) in samples() {
        let t = std::time::Instant::now();
        eprintln!(
            "{name}: sensor_dims = {:?}",
            raw::sensor_dims(&rawler::rawsource::RawSource::new_from_shared_vec(bytes.clone()))
                .map_err(|e| format!("{e:#}"))
        );
        let kind = formats::sniff(&bytes, &name);
        match raw::inspect(&bytes, LIMITS) {
            Ok(h) => eprintln!(
                "{name}: kind={kind:?} sensor={:?} display={}x{} orient={} make={:?} model={:?} lens={:?} taken={:?} gps={:?},{:?} plan={:?} cost={}MB json_keys={} ({:?})",
                h.sensor,
                h.width,
                h.height,
                h.exif.orientation,
                h.exif.camera_make,
                h.exif.camera_model,
                h.exif.lens_model,
                h.exif.taken_at,
                h.exif.latitude,
                h.exif.longitude,
                h.plan,
                h.cost_bytes() >> 20,
                h.exif.json.as_ref().and_then(|j| j.as_object()).map_or(0, |o| o.len()),
                t.elapsed()
            ),
            Err(e) => eprintln!("{name}: kind={kind:?} INSPECT ERROR {e:#}"),
        }
    }
}

#[test]
fn every_sample_routes_to_raw_and_renders() {
    for (name, bytes) in samples() {
        assert_eq!(formats::sniff(&bytes, &name), Some(Kind::Raw), "{name} sniffs as RAW");
        let h = raw::inspect(&bytes, LIMITS).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert!(h.width > 0 && h.height > 0, "{name}");
        assert!(h.exif.camera_make.is_some(), "{name}: make");
        let d = raw::render(&bytes, &h).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!((d.width, d.height), (h.width, h.height), "{name}: reports sensor size");
        if let Ok(out) = std::env::var("CYANOTYPE_RAW_OUT") {
            std::fs::write(format!("{out}/{name}.thumb.webp"), &d.thumb_webp).unwrap();
            std::fs::write(format!("{out}/{name}.preview.webp"), &d.preview_webp).unwrap();
        }
    }
}
