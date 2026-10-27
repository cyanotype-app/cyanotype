//! Zero-shot reading of a photo's stored image embedding: is a dog or a cat in it? (`docs/pets.md`, "The check".) No new model: the
//! embedding the search job already made is compared with the text tower's embedding of a few prompts, and the answer is how much
//! of the model's attention the dog prompts and the cat prompts get among all the prompts.
//!
//! The prompts are three lists: [`DOG_PROMPTS`], [`CAT_PROMPTS`] and [`OTHER_PROMPTS`], the things a dog or a cat photo is told apart
//! from (other animals, a toy, a statue, a painting, and what photos are otherwise of). Their embeddings are computed once, with the
//! text tower, and kept in `data/pets-prompts-siglip2-b16.f32` (`cargo run --release -p cyanotype-clip --example pet_prompts`
//! writes it; `tests/model.rs` checks that the file still matches the model), so the worker needs neither the text tower (283 MB) nor
//! the tokenizer. The measured accuracy, and the limits of the evaluation, are in `docs/pets.md`.
//!
//! A *share* is a softmax over the prompts of `logit_scale * cosine` (SigLIP's own scale, [`Model::calibration`]), summed over the
//! dog prompts or the cat prompts. It is 0..1, and unlike a cosine it says "this and not that": a horse, a statue of a dog or a
//! plush toy gives the dog prompts a low share because another prompt fits better.
use crate::{Model, cosine};

pub const DOG_PROMPTS: &[&str] = &["a photo of a dog", "a photo of a puppy"];
pub const CAT_PROMPTS: &[&str] = &["a photo of a cat", "a photo of a kitten"];
pub const OTHER_PROMPTS: &[&str] = &[
    "a photo of a horse",
    "a photo of a sheep",
    "a photo of a fox",
    "a photo of a wolf",
    "a photo of a rabbit",
    "a photo of a bird",
    "a photo of a cow",
    "a photo of a wild animal",
    "a photo of a toy",
    "a photo of a statue",
    "a painting of an animal",
    "a photo of a person",
    "a photo of a landscape",
    "a photo of a street",
    "a photo of a room",
    "a photo of food",
];

/// Every prompt, in the order of the vectors in the data file: dogs, cats, the rest.
pub fn all_prompts() -> impl Iterator<Item = &'static str> {
    DOG_PROMPTS.iter().chain(CAT_PROMPTS).chain(OTHER_PROMPTS).copied()
}

/// A dog or cat share at least this big says the animal is in the photo. Chosen on the evaluation set (`docs/pets.md`): the best F1
/// for "has a pet" over the thresholds, in the middle of a plateau (0.20 to 0.35 are within 2 points).
pub const MIN_SHARE: f32 = 0.25;

/// The prompt vectors of a model, as written by the `pet_prompts` example: f32, little endian, [`Model::dim`] values each.
fn prompt_file(model: Model) -> Option<&'static [u8]> {
    match model {
        Model::Siglip2B16 => Some(include_bytes!("../data/pets-prompts-siglip2-b16.f32")),
        Model::ClipB32 => None,
    }
}

/// How much of the attention the dog prompts and the cat prompts get, 0..1 each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PetShares {
    pub dog: f32,
    pub cat: f32,
}

impl PetShares {
    /// The kinds whose share reaches [`MIN_SHARE`].
    pub fn confirmed(self) -> (bool, bool) {
        (self.dog >= MIN_SHARE, self.cat >= MIN_SHARE)
    }
}

/// The prompts of one model, ready to read embeddings with.
pub struct PetCheck {
    dim: usize,
    scale: f32,
    /// One vector per prompt, in [`all_prompts`] order.
    vectors: Vec<Vec<f32>>,
}

impl PetCheck {
    /// The check for the vectors of `model`; `None` for a model without prompts (only SigLIP 2 has them: the thresholds are its own).
    pub fn for_model(model: Model) -> Option<Self> {
        let file = prompt_file(model)?;
        let dim = model.dim();
        let n = all_prompts().count();
        assert_eq!(
            file.len(),
            n * dim * 4,
            "the prompt file does not hold {n} vectors of {dim} values: run the pet_prompts example"
        );
        let vectors = file
            .chunks_exact(dim * 4)
            .map(|c| c.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
            .collect();
        Some(Self {
            dim,
            scale: model.calibration()?.scale,
            vectors,
        })
    }

    /// The prompt vectors, in [`all_prompts`] order (for the test that checks them against the text tower).
    pub fn prompt_vectors(&self) -> &[Vec<f32>] {
        &self.vectors
    }

    /// The shares for an image embedding of the model (any length but [`Model::dim`] is `None`).
    pub fn shares(&self, embedding: &[f32]) -> Option<PetShares> {
        if embedding.len() != self.dim {
            return None;
        }
        let logits: Vec<f32> = self.vectors.iter().map(|p| self.scale * cosine(embedding, p)).collect();
        let max = logits.iter().copied().fold(f32::MIN, f32::max);
        let exp: Vec<f32> = logits.iter().map(|l| (l - max).exp()).collect();
        let total: f32 = exp.iter().sum();
        let (nd, nc) = (DOG_PROMPTS.len(), CAT_PROMPTS.len());
        Some(PetShares {
            dog: exp[..nd].iter().sum::<f32>() / total,
            cat: exp[nd..nd + nc].iter().sum::<f32>() / total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit vector along axis `i` of `dim`.
    fn axis(dim: usize, i: usize) -> Vec<f32> {
        let mut v = vec![0.0; dim];
        v[i] = 1.0;
        v
    }

    fn check_with(vectors: Vec<Vec<f32>>) -> PetCheck {
        PetCheck {
            dim: vectors[0].len(),
            scale: 112.668_9,
            vectors,
        }
    }

    #[test]
    fn the_shipped_file_holds_every_prompt() {
        let c = PetCheck::for_model(Model::Siglip2B16).expect("siglip 2 has prompts");
        assert_eq!(c.vectors.len(), all_prompts().count());
        for v in &c.vectors {
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "prompt vectors are unit length, got {norm}");
        }
        assert!(PetCheck::for_model(Model::ClipB32).is_none(), "CLIP has no prompts");
    }

    #[test]
    fn an_embedding_on_a_dog_prompt_is_a_dog_and_one_on_another_prompt_is_not() {
        // Orthogonal prompts, one per axis: the first two are dogs, the next two cats, the rest others.
        let n = all_prompts().count();
        let c = check_with((0..n).map(|i| axis(n, i)).collect());
        let on_dog = c.shares(&axis(n, 1)).unwrap();
        assert!(on_dog.dog > 0.99 && on_dog.cat < 0.01, "{on_dog:?}");
        assert_eq!(on_dog.confirmed(), (true, false));
        let on_cat = c.shares(&axis(n, 3)).unwrap();
        assert_eq!(on_cat.confirmed(), (false, true));
        let on_horse = c.shares(&axis(n, 5)).unwrap();
        assert!(on_horse.dog < 0.01 && on_horse.cat < 0.01, "{on_horse:?}");
        assert_eq!(on_horse.confirmed(), (false, false));
    }

    #[test]
    fn a_photo_between_a_dog_and_a_horse_splits_its_share_and_needs_to_lean_to_the_dog() {
        let n = all_prompts().count();
        let c = check_with((0..n).map(|i| axis(n, i)).collect());
        // Equal parts of a dog prompt and the horse prompt: the dog prompts get half of the attention.
        let mut mix = axis(n, 0);
        mix[4] = 1.0;
        let s = c.shares(&mix).unwrap();
        assert!(s.dog > 0.4 && s.dog < 0.6, "{s:?}");
        // A little horse and mostly dog is a dog, a little dog and mostly horse is not.
        let mut mostly_dog = axis(n, 0);
        mostly_dog[4] = 0.8;
        assert!(c.shares(&mostly_dog).unwrap().confirmed().0);
        let mut mostly_horse = axis(n, 4);
        mostly_horse[0] = 0.8;
        assert!(!c.shares(&mostly_horse).unwrap().confirmed().0);
    }

    #[test]
    fn an_embedding_of_the_wrong_size_is_refused() {
        let c = PetCheck::for_model(Model::Siglip2B16).unwrap();
        assert!(c.shares(&[0.5; 512]).is_none());
        assert!(c.shares(&[]).is_none());
    }
}
