//! DINOv2-small (a self-supervised vision transformer, Apache 2.0) as an embedding of an animal's crop: the candidate that `docs/pets.md`
//! compares with the search model's own image tower for telling individuals apart.
use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result, bail};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};
use ort::value::Tensor;

use crate::crop::Rgb;

/// The model takes 224 x 224 pictures.
pub const SIDE: usize = 224;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// A crop as the model reads it: squashed to 224 x 224 (bicubic; the crop is already as square as its picture allows) and
/// normalised with ImageNet's mean and deviation, channel-first.
pub fn preprocess(img: &Rgb) -> Result<Vec<f32>> {
    if img.width == 0 || img.height == 0 || img.data.len() != img.width as usize * img.height as usize * 3 {
        bail!(
            "not a {}x{} RGB8 buffer ({} bytes)",
            img.width,
            img.height,
            img.data.len()
        );
    }
    let src = ImageRef::new(img.width, img.height, &img.data, PixelType::U8x3).context("source image")?;
    let mut dst = Image::new(SIDE as u32, SIDE as u32, PixelType::U8x3);
    Resizer::new()
        .resize(
            &src,
            &mut dst,
            &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::CatmullRom)),
        )
        .context("resize")?;
    let plane = SIDE * SIDE;
    let mut out = vec![0f32; 3 * plane];
    for (i, px) in dst.buffer().as_chunks::<3>().0.iter().enumerate() {
        for c in 0..3 {
            out[c * plane + i] = (f32::from(px[c]) / 255.0 - MEAN[c]) / STD[c];
        }
    }
    Ok(out)
}

/// Which numbers of the model's output are the embedding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pooling {
    /// The class token (384 numbers).
    Cls,
    /// The mean of the patch tokens (384).
    PatchMean,
    /// Both, each unit length, joined (768).
    Both,
}

pub struct DinoEncoder {
    session: Mutex<ort::session::Session>,
}

impl DinoEncoder {
    pub fn load(file: &Path, threads: usize) -> Result<Self> {
        Ok(Self {
            session: Mutex::new(crate::session(file, threads)?),
        })
    }

    /// Embeddings of a preprocessed crop, one for each pooling asked, each of unit length.
    pub fn embed(&self, image: &[f32], poolings: &[Pooling]) -> Result<Vec<Vec<f32>>> {
        if image.len() != 3 * SIDE * SIDE {
            bail!("preprocessed image has {} values", image.len());
        }
        let input = Tensor::from_array(([1usize, 3, SIDE, SIDE], image.to_vec().into_boxed_slice()))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
        let outputs = session
            .run(ort::inputs!["pixel_values" => input])
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let (shape, data) = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if shape.len() != 3 || shape[0] != 1 || shape[1] < 2 {
            bail!("unexpected output shape {shape:?}");
        }
        let (tokens, dim) = (shape[1] as usize, shape[2] as usize);
        let cls = &data[..dim];
        let mut mean = vec![0f32; dim];
        for t in 1..tokens {
            for (m, v) in mean.iter_mut().zip(&data[t * dim..(t + 1) * dim]) {
                *m += v / (tokens - 1) as f32;
            }
        }
        Ok(poolings
            .iter()
            .map(|p| match p {
                Pooling::Cls => unit(cls.to_vec()),
                Pooling::PatchMean => unit(mean.clone()),
                Pooling::Both => unit(cls.to_vec()).into_iter().chain(unit(mean.clone())).collect(),
            })
            .collect())
    }
}

fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}
