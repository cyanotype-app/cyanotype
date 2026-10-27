//! Write the prompt vectors of the pet check (`crates/cyanotype-clip/src/zeroshot.rs`, `docs/pets.md`): the text tower's embedding of
//! every prompt, f32 little endian, in the order of `zeroshot::all_prompts`.
//!
//! ```text
//! scripts/fetch-models.sh
//! SEARCH_MODEL_DIR=models cargo run --release -p cyanotype-clip --example pet_prompts
//! ```
//!
//! writes `crates/cyanotype-clip/data/pets-prompts-siglip2-b16.f32` (or the path given as the argument). Run it when a prompt or the
//! text tower changes; `tests/model.rs` fails when the committed file no longer matches the model.
use anyhow::Result;
use cyanotype_clip::{Model, TextEncoder, zeroshot};

fn main() -> Result<()> {
    let model = Model::Siglip2B16;
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("{}/data/pets-prompts-siglip2-b16.f32", env!("CARGO_MANIFEST_DIR")));
    let text = TextEncoder::load(model, &model.dir(), 1)?;
    let mut bytes = Vec::new();
    for prompt in zeroshot::all_prompts() {
        // Exactly the prompt, not the search's caption template: these already are captions.
        let v = text.embed(prompt)?;
        assert_eq!(v.len(), model.dim());
        bytes.extend(v.iter().flat_map(|x| x.to_le_bytes()));
    }
    std::fs::write(&out, &bytes)?;
    println!("wrote {} prompt vectors to {out}", zeroshot::all_prompts().count());
    Ok(())
}
