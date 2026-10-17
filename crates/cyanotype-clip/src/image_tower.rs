use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result, bail};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use ort::value::Tensor;

use crate::{DIM, VISION_FILE, l2_normalize};

/// CLIP ViT-B/32 input side.
pub const IMAGE_SIZE: usize = 224;
const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

/// CLIP's image preprocessing from an RGB8 buffer: resize so the shorter side is 224 (bicubic), centre-crop 224x224,
/// scale to 0..1, normalise per channel, channel-first. Returns 3 x 224 x 224 floats.
pub fn preprocess_rgb8(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<f32>> {
    if width == 0 || height == 0 || rgb.len() != width as usize * height as usize * 3 {
        bail!("not a {width}x{height} RGB8 buffer ({} bytes)", rgb.len());
    }
    let side = IMAGE_SIZE as u32;
    // Shorter side to 224, the other rounded as the reference (PIL / transformers) does: int(side * long / short).
    let (nw, nh) = if width <= height {
        (
            side,
            ((u64::from(side) * u64::from(height) / u64::from(width)) as u32).max(side),
        )
    } else {
        (
            ((u64::from(side) * u64::from(width) / u64::from(height)) as u32).max(side),
            side,
        )
    };
    let src = ImageRef::new(width, height, rgb, PixelType::U8x3).context("source image")?;
    let mut dst = Image::new(nw, nh, PixelType::U8x3);
    Resizer::new()
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::CatmullRom)),
        )
        .context("resize")?;
    let (left, top) = ((nw - side) / 2, (nh - side) / 2);
    let px = dst.buffer();
    let plane = IMAGE_SIZE * IMAGE_SIZE;
    let mut out = vec![0f32; 3 * plane];
    for y in 0..IMAGE_SIZE {
        let row = ((top as usize + y) * nw as usize + left as usize) * 3;
        for x in 0..IMAGE_SIZE {
            for c in 0..3 {
                let v = f32::from(px[row + x * 3 + c]) / 255.0;
                out[c * plane + y * IMAGE_SIZE + x] = (v - MEAN[c]) / STD[c];
            }
        }
    }
    Ok(out)
}

/// The image tower. One session, run one batch at a time.
pub struct ImageEncoder {
    session: Mutex<ort::session::Session>,
}

impl ImageEncoder {
    /// Load `vision_model_int8.onnx` from `dir`, with `threads` intra-op threads.
    pub fn load(dir: &Path, threads: usize) -> Result<Self> {
        Ok(Self {
            session: Mutex::new(crate::session(&dir.join(VISION_FILE), threads)?),
        })
    }

    /// Unit-length embeddings for preprocessed images (each from [`preprocess_rgb8`]).
    pub fn embed(&self, batch: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let per = 3 * IMAGE_SIZE * IMAGE_SIZE;
        let mut flat = Vec::with_capacity(batch.len() * per);
        for b in batch {
            if b.len() != per {
                bail!("preprocessed image has {} values, expected {per}", b.len());
            }
            flat.extend_from_slice(b);
        }
        let input = Tensor::from_array(([batch.len(), 3, IMAGE_SIZE, IMAGE_SIZE], flat.into_boxed_slice()))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
        let outputs = session
            .run(ort::inputs!["pixel_values" => input])
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let (shape, data) = outputs["image_embeds"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if shape.len() != 2 || shape[1] != DIM as i64 || data.len() != batch.len() * DIM {
            bail!("unexpected image embedding shape {shape:?}");
        }
        Ok(data
            .as_chunks::<DIM>()
            .0
            .iter()
            .map(|c| {
                let mut v = c.to_vec();
                l2_normalize(&mut v);
                v
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, px: [u8; 3]) -> Vec<u8> {
        px.iter().copied().cycle().take(w as usize * h as usize * 3).collect()
    }

    #[test]
    fn output_is_channel_first_and_normalised() {
        let out = preprocess_rgb8(640, 480, &solid(640, 480, [255, 0, 128])).unwrap();
        assert_eq!(out.len(), 3 * IMAGE_SIZE * IMAGE_SIZE);
        let plane = IMAGE_SIZE * IMAGE_SIZE;
        assert!((out[0] - (1.0 - MEAN[0]) / STD[0]).abs() < 1e-3);
        assert!((out[plane] - (0.0 - MEAN[1]) / STD[1]).abs() < 1e-3);
        assert!((out[2 * plane + 5] - (128.0 / 255.0 - MEAN[2]) / STD[2]).abs() < 1e-2);
    }

    #[test]
    fn centre_crop_takes_the_middle_of_a_panorama() {
        // 8 bands of 100 px: black, black, black, white, white, black, black, black. After the shorter side is scaled
        // to 224 only the middle 224 px survive: the crop must be mostly the two white bands.
        let (w, h) = (800u32, 100u32);
        let mut px = vec![0u8; (w * h * 3) as usize];
        for y in 0..h as usize {
            for x in 300..500usize {
                let o = (y * w as usize + x) * 3;
                px[o..o + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let out = preprocess_rgb8(w, h, &px).unwrap();
        let white = out[..IMAGE_SIZE * IMAGE_SIZE].iter().filter(|v| **v > 1.0).count();
        let share = white as f32 / (IMAGE_SIZE * IMAGE_SIZE) as f32;
        assert!(share > 0.85, "{share}: the crop is not the centre");
        // And a tall image is cropped vertically, not squashed.
        assert!(preprocess_rgb8(100, 800, &solid(100, 800, [1, 2, 3])).is_ok());
    }

    #[test]
    fn rejects_mismatched_buffers() {
        assert!(preprocess_rgb8(10, 10, &[0; 5]).is_err());
        assert!(preprocess_rgb8(0, 10, &[]).is_err());
    }
}
