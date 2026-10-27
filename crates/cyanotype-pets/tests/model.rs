//! The real detector on real photos (the search tests' fixtures: a dog, a cat, a horse, a car, a bird, a beach): an animal is found where it
//! is, with the right kind, and nothing is found where there is none. Needs the model file (`scripts/fetch-models.sh`; `PETS_MODEL_DIR`
//! or `SEARCH_MODEL_DIR`); skipped without it unless `CYANOTYPE_REQUIRE_MODEL` is set (CI sets it).
use std::path::PathBuf;

use cyanotype_pets::{Detection, Detector, DetectorConfig, Kind, model_dir, models_installed};

fn detector() -> Option<Detector> {
    let dir = model_dir();
    if !models_installed(&dir) {
        assert!(
            std::env::var_os("CYANOTYPE_REQUIRE_MODEL").is_none(),
            "CYANOTYPE_REQUIRE_MODEL is set but {} has no pet model",
            dir.display()
        );
        return None;
    }
    Some(Detector::load(&dir, 1, DetectorConfig::default()).unwrap())
}

fn photo(name: &str) -> (Vec<u8>, u32, u32) {
    let p =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../cyanotype-clip/tests/fixtures/images/{name}.jpg"));
    let img = image::open(p).unwrap().to_rgb8();
    let (w, h) = img.dimensions();
    (img.into_raw(), w, h)
}

fn find(d: &Detector, name: &str) -> (Vec<Detection>, u32, u32) {
    let (rgb, w, h) = photo(name);
    (d.detect(&rgb, w, h).unwrap(), w, h)
}

#[test]
fn a_dog_is_found_where_it_is_and_called_a_dog() {
    let Some(d) = detector() else { return };
    let (found, w, h) = find(&d, "dog");
    let best = found.first().expect("a dog in the dog photo");
    assert_eq!(best.kind, Kind::Dog, "{found:?}");
    assert!(best.score > 0.3, "{best:?}");
    let [x, y, bw, bh] = best.bbox;
    // Inside the picture, and a good part of it: it is a portrait of a dog.
    assert!(
        x >= 0.0 && y >= 0.0 && x + bw <= w as f32 + 1e-3 && y + bh <= h as f32 + 1e-3,
        "{best:?}"
    );
    assert!(bw > w as f32 * 0.25 && bh > h as f32 * 0.25, "{best:?} in {w}x{h}");
    // Strongest first, and no cat made of the same dog.
    assert!(found.windows(2).all(|p| p[0].score >= p[1].score));
    assert!(
        found.iter().all(|f| f.kind == Kind::Dog || f.score < best.score),
        "{found:?}"
    );
}

#[test]
fn a_cat_is_found_and_called_a_cat() {
    let Some(d) = detector() else { return };
    let (found, w, h) = find(&d, "cat");
    let best = found.first().expect("a cat in the cat photo");
    assert_eq!(best.kind, Kind::Cat, "{found:?}");
    assert!(best.score > 0.3, "{best:?}");
    assert!(
        best.bbox[2] > w as f32 * 0.25 && best.bbox[3] > h as f32 * 0.25,
        "{best:?} in {w}x{h}"
    );
}

#[test]
fn photos_without_a_dog_or_a_cat_have_none_even_with_other_animals_in_them() {
    let Some(d) = detector() else { return };
    for name in ["horse", "bird", "car", "beach", "pizza", "mountain"] {
        let (found, _, _) = find(&d, name);
        assert!(found.is_empty(), "{name}: {found:?}");
    }
}

#[test]
fn the_same_photo_gives_the_same_boxes_and_a_different_size_gives_about_the_same_ones() {
    let Some(d) = detector() else { return };
    let (a, w, h) = find(&d, "dog");
    let (b, _, _) = find(&d, "dog");
    assert_eq!(a, b, "deterministic");
    // The boxes are of the picture, not of the model's input: the same dog in a picture twice the size is the same box, twice the size.
    let (rgb, _, _) = photo("dog");
    let big = image::imageops::resize(
        &image::RgbImage::from_raw(w, h, rgb).unwrap(),
        w * 2,
        h * 2,
        image::imageops::FilterType::Triangle,
    );
    let doubled = d.detect(big.as_raw(), w * 2, h * 2).unwrap();
    let (x, y, bw, bh) = (
        a[0].bbox[0] * 2.0,
        a[0].bbox[1] * 2.0,
        a[0].bbox[2] * 2.0,
        a[0].bbox[3] * 2.0,
    );
    let got = doubled[0].bbox;
    let tolerance = 0.06 * (w * 2) as f32;
    assert!(
        (got[0] - x).abs() < tolerance
            && (got[1] - y).abs() < tolerance
            && (got[2] - bw).abs() < tolerance
            && (got[3] - bh).abs() < tolerance,
        "{got:?} against {:?}",
        [x, y, bw, bh]
    );
}

#[test]
fn a_buffer_that_is_not_the_picture_it_says_is_refused() {
    let Some(d) = detector() else { return };
    assert!(d.detect(&[0u8; 10], 4, 4).is_err());
    assert!(d.detect(&[], 0, 0).is_err());
}
