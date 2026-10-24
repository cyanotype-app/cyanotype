//! The real models on real portraits: a face is found where it is, its landmarks are in the order alignment expects, and
//! fingerprints of one person are closer than fingerprints of two. Needs the model files (`scripts/fetch-models.sh`; `FACES_MODEL_DIR`
//! or `SEARCH_MODEL_DIR`); skipped without them unless `CYANOTYPE_REQUIRE_MODEL` is set (CI sets it).
use std::path::PathBuf;

use cyanotype_faces::{Engine, EngineConfig, Face, cosine, model_dir, models_installed};

fn engine() -> Option<Engine> {
    let dir = model_dir();
    if !models_installed(&dir) {
        assert!(
            std::env::var_os("CYANOTYPE_REQUIRE_MODEL").is_none(),
            "CYANOTYPE_REQUIRE_MODEL is set but {} has no face models",
            dir.display()
        );
        return None;
    }
    Some(Engine::load(&dir, EngineConfig::default()).unwrap())
}

fn portrait(name: &str) -> (Vec<u8>, u32, u32) {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let img = image::open(p).unwrap().to_rgb8();
    let (w, h) = img.dimensions();
    (img.into_raw(), w, h)
}

fn one_face(engine: &Engine, name: &str) -> (Face, u32, u32) {
    let (rgb, w, h) = portrait(name);
    let mut faces = engine.analyse(&rgb, w, h).unwrap();
    assert_eq!(faces.len(), 1, "{name}: {faces:?}");
    (faces.remove(0), w, h)
}

#[test]
fn a_portrait_has_one_face_with_landmarks_in_the_expected_places() {
    let Some(engine) = engine() else { return };
    for name in ["person_a.jpg", "person_b1.jpg", "person_b2.jpg"] {
        let (f, w, h) = one_face(&engine, name);
        let d = f.detection;
        assert!(d.score > 0.8, "{name}: {}", d.score);
        // The face is a good part of a portrait and inside the picture.
        assert!(
            d.bbox[2] > w as f32 * 0.15 && d.bbox[3] > h as f32 * 0.1,
            "{name}: {:?}",
            d.bbox
        );
        assert!(
            d.bbox[0] >= 0.0
                && d.bbox[1] >= 0.0
                && d.bbox[0] + d.bbox[2] <= w as f32
                && d.bbox[1] + d.bbox[3] <= h as f32
        );
        let [re, le, nose, rm, lm] = d.landmarks;
        // Right eye (on the left of the picture) before the left eye, eyes above the nose above the mouth, mouth corners in order.
        assert!(
            re[0] < nose[0] && nose[0] < le[0],
            "{name}: eyes {re:?} {le:?} nose {nose:?}"
        );
        assert!(
            re[1].max(le[1]) < nose[1] && nose[1] < rm[1].min(lm[1]),
            "{name}: {:?}",
            d.landmarks
        );
        assert!(rm[0] < lm[0], "{name}: mouth {rm:?} {lm:?}");
        // All five points are inside the box.
        for p in d.landmarks {
            assert!(
                p[0] >= d.bbox[0]
                    && p[0] <= d.bbox[0] + d.bbox[2]
                    && p[1] >= d.bbox[1]
                    && p[1] <= d.bbox[1] + d.bbox[3]
            );
        }
        let e = f.embedding.expect("a face this size is fingerprinted");
        assert_eq!(e.len(), cyanotype_faces::DIM);
        assert!((e.iter().map(|x| x * x).sum::<f32>().sqrt() - 1.0).abs() < 1e-4);
    }
}

#[test]
fn the_same_person_is_closer_than_another_and_a_second_run_gives_the_same_numbers() {
    let Some(engine) = engine() else { return };
    let emb = |n: &str| one_face(&engine, n).0.embedding.unwrap();
    let (a, b1, b2) = (emb("person_a.jpg"), emb("person_b1.jpg"), emb("person_b2.jpg"));
    let same = cosine(&b1, &b2);
    let different = cosine(&a, &b1).max(cosine(&a, &b2));
    assert!(same > different + 0.15, "same {same}, different {different}");
    assert!(same > 0.363, "below the published SFace threshold: {same}");
    assert!(different < 0.363, "{different}");
    let again = emb("person_b1.jpg");
    assert!(cosine(&b1, &again) > 0.9999);
}

#[test]
fn a_picture_without_a_face_finds_none() {
    let Some(engine) = engine() else { return };
    // A smooth gradient with a little texture: nothing a detector should call a face.
    let (w, h) = (640u32, 480u32);
    let rgb: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [(x / 3) as u8, (y / 2) as u8, ((x ^ y) % 64 + 100) as u8]
        })
        .collect();
    assert!(engine.analyse(&rgb, w, h).unwrap().is_empty());
}

#[test]
fn a_face_below_the_minimum_size_is_reported_without_a_fingerprint() {
    let Some(_) = engine() else { return };
    let strict = Engine::load(
        &model_dir(),
        EngineConfig {
            min_face_px: 10_000.0,
            ..EngineConfig::default()
        },
    )
    .unwrap();
    let (f, _, _) = one_face(&strict, "person_b1.jpg");
    assert!(f.embedding.is_none());
}
