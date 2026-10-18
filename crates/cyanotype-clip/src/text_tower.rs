use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail};
use ort::value::Tensor;
use tokenizers::Tokenizer;

use crate::{Model, l2_normalize};

/// CLIP's end-of-text token; SigLIP 2's (Gemma's) `<eos>` and `<pad>`.
const CLIP_END_OF_TEXT: i64 = 49407;
const GEMMA_EOS: i64 = 1;
const GEMMA_PAD: i64 = 0;
const CACHE_ENTRIES: usize = 256;

/// The text tower plus a small LRU of recent query embeddings (typing the same query again, paging, changing only a
/// filter: all skip the model).
pub struct TextEncoder {
    model: Model,
    session: Mutex<ort::session::Session>,
    tokenizer: Tokenizer,
    cache: Mutex<Lru>,
}

#[derive(Default)]
struct Lru {
    tick: u64,
    entries: HashMap<String, (u64, Arc<Vec<f32>>)>,
}

impl Lru {
    fn get(&mut self, key: &str) -> Option<Arc<Vec<f32>>> {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut(key).map(|e| {
            e.0 = tick;
            e.1.clone()
        })
    }

    fn put(&mut self, key: String, value: Arc<Vec<f32>>) {
        self.tick += 1;
        if self.entries.len() >= CACHE_ENTRIES
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self.entries.iter().min_by_key(|(_, e)| e.0).map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, (self.tick, value));
    }
}

/// Queries are compared case- and whitespace-insensitively (the tokenizer lowercases and collapses whitespace anyway),
/// so "Dog  on the beach" and "dog on the beach" share a cache entry.
fn normalize_query(q: &str) -> String {
    q.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

impl TextEncoder {
    /// Load the tokenizer and the model's text tower from `dir`, with `threads` intra-op threads.
    pub fn load(model: Model, dir: &Path, threads: usize) -> Result<Self> {
        Self::load_file(model, dir, &dir.join(model.text_file()), threads)
    }

    /// Load a specific file of the model's text graph (another quantisation of the same export, for the benchmarks).
    pub fn load_file(model: Model, dir: &Path, file: &Path, threads: usize) -> Result<Self> {
        let tokenizer = Tokenizer::from_file(dir.join(model.tokenizer_file()))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("load tokenizer.json")?;
        Ok(Self {
            model,
            session: Mutex::new(crate::session(file, threads)?),
            tokenizer,
            cache: Mutex::default(),
        })
    }

    pub fn model(&self) -> Model {
        self.model
    }

    /// Token ids for `text`, as the model's reference processor produces them.
    ///
    /// - CLIP: start marker, BPE tokens, end marker, at most 77 (longer text keeps its beginning and ends with the
    ///   end marker). Not padded: the graph pools at the end-of-text token.
    /// - SigLIP 2: lowercased (the model was trained on lowercase text and the tokenizer does not do it itself), Gemma
    ///   tokens, `<eos>`, padded with `<pad>` to exactly 64 (the graph pools the last position); longer text keeps its
    ///   first 63 tokens and `<eos>`.
    pub fn tokenize(&self, text: &str) -> Result<Vec<i64>> {
        let context = self.model.context_length();
        match self.model {
            Model::ClipB32 => {
                let enc = self.tokenizer.encode(text, true).map_err(|e| anyhow::anyhow!("{e}"))?;
                let mut ids: Vec<i64> = enc.get_ids().iter().map(|&i| i64::from(i)).collect();
                if ids.len() > context {
                    ids.truncate(context - 1);
                    ids.push(CLIP_END_OF_TEXT);
                }
                Ok(ids)
            }
            Model::Siglip2B16 => {
                let enc = self
                    .tokenizer
                    .encode(text.to_lowercase(), true)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                // (The tokenizer file would pad to 64 itself; padding is done here so the length is not a property of a
                // downloaded file.)
                let mut ids: Vec<i64> = enc
                    .get_ids()
                    .iter()
                    .map(|&i| i64::from(i))
                    .filter(|&i| i != GEMMA_PAD)
                    .collect();
                if ids.len() > context {
                    ids.truncate(context - 1);
                    ids.push(GEMMA_EOS);
                }
                ids.resize(context, GEMMA_PAD);
                Ok(ids)
            }
        }
    }

    /// Unit-length embedding of what a user typed: [`Model::prepare_query`] (SigLIP 2's caption template for short queries)
    /// and then [`TextEncoder::embed`].
    pub fn embed_query(&self, query: &str) -> Result<Arc<Vec<f32>>> {
        self.embed(&self.model.prepare_query(query))
    }

    /// Unit-length embedding of exactly this text (lower-cased and whitespace-normalised).
    pub fn embed(&self, query: &str) -> Result<Arc<Vec<f32>>> {
        let key = normalize_query(query);
        if key.is_empty() {
            bail!("empty query");
        }
        if let Some(hit) = self.cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return Ok(hit);
        }
        let ids = self.tokenize(&key)?;
        let input =
            Tensor::from_array(([1usize, ids.len()], ids.into_boxed_slice())).map_err(|e| anyhow::anyhow!("{e}"))?;
        let name = match self.model {
            Model::ClipB32 => "text_embeds",
            Model::Siglip2B16 => "pooler_output",
        };
        let mut v = {
            let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
            let outputs = session
                .run(ort::inputs!["input_ids" => input])
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let (shape, data) = outputs[name]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if data.len() != self.model.dim() {
                bail!("unexpected text embedding shape {shape:?}");
            }
            data.to_vec()
        };
        l2_normalize(&mut v);
        let v = Arc::new(v);
        self.cache.lock().unwrap_or_else(|p| p.into_inner()).put(key, v.clone());
        Ok(v)
    }
}
