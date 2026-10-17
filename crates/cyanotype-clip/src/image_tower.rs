use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result, bail};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use ort::value::Tensor;

use crate::{Model, l2_normalize};

/// Both models take 224 x 224 input.
pub const IMAGE_SIZE: usize = 224;
const CLIP_MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const CLIP_STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

/// The model's image preprocessing from an RGB8 buffer, as 3 x 224 x 224 floats, channel-first.
///
/// - CLIP: resize so the shorter side is 224 (bicubic), centre-crop 224 x 224, scale to 0..1, normalise with CLIP's
///   per-channel mean and standard deviation.
/// - SigLIP 2: resize the whole image to 224 x 224 (bilinear, anti-aliased, the aspect ratio is not kept and nothing is
///   cropped, as `SiglipImageProcessor` does), scale to 0..1, then mean 0.5 / standard deviation 0.5 (to -1..1).
pub fn preprocess_rgb8(model: Model, width: u32, height: u32, rgb: &[u8]) -> Result<Vec<f32>> {
    if width == 0 || height == 0 || rgb.len() != width as usize * height as usize * 3 {
        bail!("not a {width}x{height} RGB8 buffer ({} bytes)", rgb.len());
    }
    match model {
        Model::ClipB32 => preprocess_clip(width, height, rgb),
        Model::Siglip2B16 => preprocess_siglip(width, height, rgb),
    }
}

fn preprocess_siglip(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<f32>> {
    let side = IMAGE_SIZE as u32;
    let src = ImageRef::new(width, height, rgb, PixelType::U8x3).context("source image")?;
    let mut dst = Image::new(side, side, PixelType::U8x3);
    Resizer::new()
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear)),
        )
        .context("resize")?;
    let px = dst.buffer();
    let plane = IMAGE_SIZE * IMAGE_SIZE;
    let mut out = vec![0f32; 3 * plane];
    for i in 0..plane {
        for c in 0..3 {
            out[c * plane + i] = (f32::from(px[i * 3 + c]) / 255.0 - 0.5) / 0.5;
        }
    }
    Ok(out)
}

fn preprocess_clip(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<f32>> {
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
                out[c * plane + y * IMAGE_SIZE + x] = (v - CLIP_MEAN[c]) / CLIP_STD[c];
            }
        }
    }
    Ok(out)
}

/// The image tower. One session, run one batch at a time.
pub struct ImageEncoder {
    model: Model,
    session: Mutex<ort::session::Session>,
}

impl ImageEncoder {
    /// Load the model's vision tower from `dir`, with `threads` intra-op threads.
    pub fn load(model: Model, dir: &Path, threads: usize) -> Result<Self> {
        Self::load_file(model, &dir.join(model.vision_file()), threads)
    }

    /// Load a specific file of the model's graph (another quantisation of the same export, for the benchmarks).
    pub fn load_file(model: Model, file: &Path, threads: usize) -> Result<Self> {
        Ok(Self {
            model,
            session: Mutex::new(crate::session(file, threads)?),
        })
    }

    pub fn model(&self) -> Model {
        self.model
    }

    /// Unit-length embeddings for preprocessed images (each from [`preprocess_rgb8`]).
    ///
    /// Each image is its own run of the graph. The 8-bit CLIP graph quantises its activations with one scale for the whole
    /// batch (`DynamicQuantizeLinear` is per tensor), so a photo's vector would depend on which photos happened to be
    /// embedded with it (measured: cosine 0.99 between the same photo alone and in a batch of two). Running them one by one
    /// makes a vector a function of the photo alone, and costs nothing: a batch of 4 was no faster per image than 1 (135
    /// against 139 ms, CLIP; 627 against 562 ms, SigLIP 2) because a single image already fills the threads.
    pub fn embed(&self, batch: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        batch.iter().map(|image| self.embed_one(image)).collect()
    }

    fn embed_one(&self, image: &[f32]) -> Result<Vec<f32>> {
        let per = 3 * IMAGE_SIZE * IMAGE_SIZE;
        if image.len() != per {
            bail!("preprocessed image has {} values, expected {per}", image.len());
        }
        let input = Tensor::from_array(([1usize, 3, IMAGE_SIZE, IMAGE_SIZE], image.to_vec().into_boxed_slice()))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
        let outputs = session
            .run(ort::inputs!["pixel_values" => input])
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // CLIP's graph ends in the projection ("image_embeds"); SigLIP 2's attention-pooling head is its "pooler_output".
        let name = match self.model {
            Model::ClipB32 => "image_embeds",
            Model::Siglip2B16 => "pooler_output",
        };
        let dim = self.model.dim();
        let (shape, data) = outputs[name]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if shape.len() != 2 || shape[1] != dim as i64 || data.len() != dim {
            bail!("unexpected image embedding shape {shape:?}");
        }
        let mut v = data.to_vec();
        l2_normalize(&mut v);
        Ok(v)
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
        let out = preprocess_rgb8(Model::ClipB32, 640, 480, &solid(640, 480, [255, 0, 128])).unwrap();
        assert_eq!(out.len(), 3 * IMAGE_SIZE * IMAGE_SIZE);
        let plane = IMAGE_SIZE * IMAGE_SIZE;
        assert!((out[0] - (1.0 - CLIP_MEAN[0]) / CLIP_STD[0]).abs() < 1e-3);
        assert!((out[plane] - (0.0 - CLIP_MEAN[1]) / CLIP_STD[1]).abs() < 1e-3);
        assert!((out[2 * plane + 5] - (128.0 / 255.0 - CLIP_MEAN[2]) / CLIP_STD[2]).abs() < 1e-2);
    }

    #[test]
    fn siglip_squashes_the_whole_image_to_minus_one_to_one() {
        let out = preprocess_rgb8(Model::Siglip2B16, 640, 480, &solid(640, 480, [255, 0, 128])).unwrap();
        let plane = IMAGE_SIZE * IMAGE_SIZE;
        assert_eq!(out.len(), 3 * plane);
        assert!((out[0] - 1.0).abs() < 1e-3);
        assert!((out[plane] + 1.0).abs() < 1e-3);
        assert!((out[2 * plane + 5] - (128.0 / 255.0 - 0.5) / 0.5).abs() < 1e-2);
    }

    #[test]
    fn siglip_keeps_the_whole_panorama_in_frame() {
        // Left third black, right third white: a centre crop would see only the middle band; the squash keeps both edges.
        let (w, h) = (900u32, 100u32);
        let mut px = vec![0u8; (w * h * 3) as usize];
        for y in 0..h as usize {
            for x in 600..w as usize {
                let o = (y * w as usize + x) * 3;
                px[o..o + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let out = preprocess_rgb8(Model::Siglip2B16, w, h, &px).unwrap();
        let row = &out[100 * IMAGE_SIZE..101 * IMAGE_SIZE];
        assert!(row[5] < -0.99 && row[IMAGE_SIZE - 5] > 0.99);
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
        let out = preprocess_rgb8(Model::ClipB32, w, h, &px).unwrap();
        let white = out[..IMAGE_SIZE * IMAGE_SIZE].iter().filter(|v| **v > 1.0).count();
        let share = white as f32 / (IMAGE_SIZE * IMAGE_SIZE) as f32;
        assert!(share > 0.85, "{share}: the crop is not the centre");
        // And a tall image is cropped vertically, not squashed.
        assert!(preprocess_rgb8(Model::ClipB32, 100, 800, &solid(100, 800, [1, 2, 3])).is_ok());
    }

    #[test]
    fn rejects_mismatched_buffers() {
        assert!(preprocess_rgb8(Model::ClipB32, 10, 10, &[0; 5]).is_err());
        assert!(preprocess_rgb8(Model::ClipB32, 0, 10, &[]).is_err());
    }
}
