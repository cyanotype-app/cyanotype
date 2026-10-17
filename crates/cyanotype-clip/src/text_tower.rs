use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail};
use ort::value::Tensor;
use tokenizers::Tokenizer;

use crate::{DIM, TEXT_FILE, TOKENIZER_FILE, l2_normalize};

/// CLIP's text context: 77 tokens including the start and end markers.
pub const CONTEXT_LENGTH: usize = 77;
const END_OF_TEXT: i64 = 49407;
const CACHE_ENTRIES: usize = 256;

/// The text tower plus a small LRU of recent query embeddings (typing the same query again, paging, changing only a
/// filter: all skip the model).
pub struct TextEncoder {
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
    /// Load the tokenizer and `text_model_int8.onnx` from `dir`, with `threads` intra-op threads.
    pub fn load(dir: &Path, threads: usize) -> Result<Self> {
        let tokenizer = Tokenizer::from_file(dir.join(TOKENIZER_FILE))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("load tokenizer.json")?;
        Ok(Self {
            session: Mutex::new(crate::session(&dir.join(TEXT_FILE), threads)?),
            tokenizer,
            cache: Mutex::default(),
        })
    }

    /// Token ids for `text`: start marker, BPE tokens, end marker, at most [`CONTEXT_LENGTH`] (longer text keeps its
    /// beginning and ends with the end marker, as the reference truncation does).
    pub fn tokenize(&self, text: &str) -> Result<Vec<i64>> {
        let enc = self.tokenizer.encode(text, true).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut ids: Vec<i64> = enc.get_ids().iter().map(|&i| i64::from(i)).collect();
        if ids.len() > CONTEXT_LENGTH {
            ids.truncate(CONTEXT_LENGTH - 1);
            ids.push(END_OF_TEXT);
        }
        Ok(ids)
    }

    /// Unit-length embedding of a query. The graph pools at the end-of-text token, so the sequence is not padded.
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
        let mut v = {
            let mut session = self.session.lock().unwrap_or_else(|p| p.into_inner());
            let outputs = session
                .run(ort::inputs!["input_ids" => input])
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let (shape, data) = outputs["text_embeds"]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if data.len() != DIM {
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
