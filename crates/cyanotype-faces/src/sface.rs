//! Face recognition with SFace (OpenCV zoo, `face_recognition_sface_2021dec.onnx`, Apache 2.0): a MobileFaceNet trained with
//! the sigmoid-constrained hypersphere loss. It takes an aligned 112 x 112 face (RGB, 0..255, not normalised) and returns a
//! 128-dimension embedding; two faces are the same person when the cosine of their embeddings is high.
use std::{path::Path, sync::Mutex};

use anyhow::{Result, bail};
use ort::value::Tensor;

use crate::{align::SIZE, l2_normalize};

/// Embedding length.
pub const DIM: usize = 128;

/// An aligned crop (112 x 112 RGB8) as the network's input: channel-first floats 0..255.
pub fn preprocess(crop: &[u8]) -> Result<Vec<f32>> {
    if crop.len() != SIZE * SIZE * 3 {
        bail!("an aligned face is {SIZE}x{SIZE} RGB8, got {} bytes", crop.len());
    }
    let plane = SIZE * SIZE;
    let mut out = vec![0f32; 3 * plane];
    for i in 0..plane {
        for c in 0..3 {
            out[c * plane + i] = f32::from(crop[i * 3 + c]);
        }
    }
    Ok(out)
}

pub struct Recognizer {
    session: Mutex<ort::session::Session>,
}

impl Recognizer {
    pub fn load(file: &Path, threads: usize) -> Result<Self> {
        Ok(Self {
            session: Mutex::new(crate::session(file, threads)?),
        })
    }

    /// The unit-length embedding of an aligned 112 x 112 RGB8 face.
    pub fn embed(&self, crop: &[u8]) -> Result<Vec<f32>> {
        let data = preprocess(crop)?;
        let input = Tensor::from_array(([1usize, 3, SIZE, SIZE], data.into_boxed_slice()))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
        let outputs = session
            .run(ort::inputs!["data" => input])
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let (shape, v) = outputs["fc1"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if v.len() != DIM || shape.len() != 2 {
            bail!("unexpected face embedding shape {shape:?}");
        }
        let mut v = v.to_vec();
        l2_normalize(&mut v);
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crop_becomes_channel_first_floats_in_rgb_order() {
        let mut crop = vec![0u8; SIZE * SIZE * 3];
        crop[0..3].copy_from_slice(&[1, 2, 3]);
        crop[3..6].copy_from_slice(&[4, 5, 6]);
        let t = preprocess(&crop).unwrap();
        let plane = SIZE * SIZE;
        assert_eq!([t[0], t[1]], [1.0, 4.0]);
        assert_eq!([t[plane], t[plane + 1]], [2.0, 5.0]);
        assert_eq!([t[2 * plane], t[2 * plane + 1]], [3.0, 6.0]);
    }

    #[test]
    fn a_crop_of_the_wrong_size_is_refused() {
        assert!(preprocess(&[0; 10]).is_err());
    }
}
