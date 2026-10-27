//! Draw what the pets job would store on photos, to look at: `draw OUT.jpg COLS TILE a.jpg b.jpg ...` writes a contact sheet where each
//! photo has a red box on every dog and a blue one on every cat the job keeps (the search model's check first, then the detector's boxes of
//! the kinds it confirmed: `SEARCH_MODEL_DIR` and `PETS_MODEL_DIR`), and the photo's file number in yellow. A photo the check passes over
//! has a thin grey frame. A tool for looking at the boxes, not part of the product.
use anyhow::Result;
use cyanotype_clip::{ImageEncoder, Model, zeroshot::PetCheck};
use cyanotype_pets::{Detector, DetectorConfig, Kind};
use image::{Rgb, RgbImage, imageops::FilterType};

fn rect(img: &mut RgbImage, b: [f32; 4], color: Rgb<u8>, thick: i32) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (x0, y0) = (b[0] as i32, b[1] as i32);
    let (x1, y1) = ((b[0] + b[2]) as i32, (b[1] + b[3]) as i32);
    for t in 0..thick {
        for x in x0..=x1 {
            for y in [y0 + t, y1 - t] {
                if (0..w).contains(&x) && (0..h).contains(&y) {
                    img.put_pixel(x as u32, y as u32, color);
                }
            }
        }
        for y in y0..=y1 {
            for x in [x0 + t, x1 - t] {
                if (0..w).contains(&x) && (0..h).contains(&y) {
                    img.put_pixel(x as u32, y as u32, color);
                }
            }
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (out, cols, tile) = (&args[0], args[1].parse::<u32>()?, args[2].parse::<u32>()?);
    let files = &args[3..];
    let m = Model::Siglip2B16;
    let enc = ImageEncoder::load(m, &m.dir(), 2)?;
    let check = PetCheck::for_model(m).unwrap();
    let detector = Detector::load(&cyanotype_pets::model_dir(), 2, DetectorConfig::default())?;
    let rows = (files.len() as u32).div_ceil(cols);
    let mut sheet = RgbImage::from_pixel(cols * tile, rows * tile, Rgb([40, 40, 40]));
    for (i, f) in files.iter().enumerate() {
        let img = image::open(f)?;
        let img = if img.width().max(img.height()) > 1440 {
            img.resize(1440, 1440, FilterType::Triangle)
        } else {
            img
        };
        let rgb = img.to_rgb8();
        let (w, h) = rgb.dimensions();
        let pre = cyanotype_clip::preprocess_rgb8(m, w, h, rgb.as_raw())?;
        let shares = check.shares(&enc.embed(&[pre])?[0]).unwrap();
        let (dog, cat) = shares.confirmed();
        let mut tile_img = img.resize(tile - 6, tile - 6, FilterType::Triangle).to_rgb8();
        let scale = tile_img.width() as f32 / w as f32;
        if dog || cat {
            for d in detector.detect(rgb.as_raw(), w, h)? {
                if (d.kind == Kind::Dog && dog) || (d.kind == Kind::Cat && cat) {
                    let b = d.bbox.map(|v| v * scale);
                    rect(
                        &mut tile_img,
                        b,
                        if d.kind == Kind::Dog {
                            Rgb([255, 0, 0])
                        } else {
                            Rgb([0, 120, 255])
                        },
                        3,
                    );
                }
            }
        }
        let (ox, oy) = (
            (i as u32 % cols) * tile + 3 + (tile - 6 - tile_img.width()) / 2,
            (i as u32 / cols) * tile + 3 + (tile - 6 - tile_img.height()) / 2,
        );
        image::imageops::replace(&mut sheet, &tile_img, i64::from(ox), i64::from(oy));
        if !(dog || cat) {
            rect(
                &mut sheet,
                [
                    ox as f32 - 2.0,
                    oy as f32 - 2.0,
                    tile_img.width() as f32 + 3.0,
                    tile_img.height() as f32 + 3.0,
                ],
                Rgb([120, 120, 120]),
                1,
            );
        }
    }
    sheet.save(out)?;
    Ok(())
}
