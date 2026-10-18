//! The server side of semantic search besides the SQL: turning a query into a vector, and deciding which model answers.
//!
//! The image half runs in the worker (`embed` jobs); both use the models of `cyanotype-clip`, so the two vectors live in one
//! space and cosine distance compares them.
//!
//! A text tower is loaded on the first query that needs it (a deployment that never searches never pays for it) and then
//! stays resident (about 120 MB for SigLIP 2 with its embedding table in a side file, `docs/build.md`). At most one is
//! resident: during a model swap, when a query is answered by the model that is being replaced, the other is unloaded
//! first, so the service does not hold both. Recent queries are cached inside the encoder.
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Result;
use cyanotype_clip::{Model, TextEncoder};
use cyanotype_core::embeddings::{self, ModelCoverage};
use sqlx::PgPool;
use uuid::Uuid;

use crate::query_parse::{Place, PlaceKind};

/// Query text to vector for the given model. Blocking (model inference): call from `spawn_blocking`.
pub trait TextEmbedder: Send + Sync + 'static {
    /// Embed what the user typed (the model's own query preparation included).
    fn embed(&self, model: Model, query: &str) -> Result<Arc<Vec<f32>>>;
    /// Are the files for this model's text tower installed here?
    fn available(&self, model: Model) -> bool;
}

/// The search model is not installed on this deployment (see `scripts/fetch-models.sh`).
#[derive(Debug, thiserror::Error)]
#[error("search model files not found in {0}")]
pub struct ModelMissing(pub PathBuf);

pub struct ClipText {
    threads: usize,
    /// The one resident encoder.
    encoder: Mutex<Option<Arc<TextEncoder>>>,
}

impl ClipText {
    /// `threads`: intra-op threads of the text session (`SEARCH_THREADS`, default 2: a query is latency, not throughput).
    pub fn new(threads: usize) -> Self {
        Self {
            threads,
            encoder: Mutex::new(None),
        }
    }

    fn encoder(&self, model: Model) -> Result<Arc<TextEncoder>> {
        // Held across the load so concurrent first queries load once.
        let mut guard = self.encoder.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(e) = guard.as_ref().filter(|e| e.model() == model) {
            return Ok(e.clone());
        }
        let dir = model.dir();
        if !self.available(model) {
            return Err(ModelMissing(dir).into());
        }
        // Drop the other model's tower before loading this one.
        *guard = None;
        let started = Instant::now();
        let enc = Arc::new(TextEncoder::load(model, &dir, self.threads)?);
        tracing::info!(
            model = model.id(),
            threads = self.threads,
            load_ms = started.elapsed().as_millis() as u64,
            "text model loaded"
        );
        *guard = Some(enc.clone());
        Ok(enc)
    }
}

impl TextEmbedder for ClipText {
    fn embed(&self, model: Model, query: &str) -> Result<Arc<Vec<f32>>> {
        self.encoder(model)?.embed_query(query)
    }

    fn available(&self, model: Model) -> bool {
        let dir = model.dir();
        dir.join(model.text_file()).exists() && dir.join(model.tokenizer_file()).exists()
    }
}

/// Per owner: when it was read, and what.
type Cached<T> = HashMap<Uuid, (Instant, Arc<T>)>;

/// What search needs to know besides the database and the text tower.
pub struct SearchState {
    /// The model the worker embeds with (`SEARCH_MODEL`) and searches prefer.
    pub model: Model,
    /// Percent of a library the configured model must cover before it answers instead of the model it replaces.
    pub switch_percent: u8,
    ttl: Duration,
    places_ttl: Duration,
    coverage: Mutex<Cached<Vec<ModelCoverage>>>,
    places: Mutex<Cached<Vec<Place>>>,
}

/// Which model answers a search for one owner, and the state of the re-index behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Serving {
    pub model: Model,
    /// Set while the configured model is not yet answering: how far along it is.
    pub reindexing: Option<Reindexing>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Reindexing {
    /// The model being built (`SEARCH_MODEL`, e.g. "siglip2-b16").
    pub model: &'static str,
    pub indexed: i64,
    pub total: i64,
    pub percent: u8,
    /// The percentage at which it takes over.
    pub switch_at_percent: u8,
}

impl SearchState {
    pub fn new(model: Model, switch_percent: u8) -> Self {
        Self {
            model,
            switch_percent: switch_percent.clamp(1, 100),
            ttl: Duration::from_secs(30),
            places_ttl: Duration::from_secs(300),
            coverage: Mutex::default(),
            places: Mutex::default(),
        }
    }

    /// `SEARCH_MODEL` and `SEARCH_SWITCH_PERCENT` (default 95).
    pub fn from_env() -> Result<Self> {
        let percent = std::env::var("SEARCH_SWITCH_PERCENT")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(95);
        Ok(Self::new(Model::from_env()?, percent))
    }

    /// Tests: no caching.
    #[cfg(test)]
    pub fn uncached(model: Model, switch_percent: u8) -> Self {
        Self {
            ttl: Duration::ZERO,
            places_ttl: Duration::ZERO,
            ..Self::new(model, switch_percent)
        }
    }

    /// Coverage of every known model for `owner` (cached for a few seconds: it counts the library).
    pub async fn coverage(&self, db: &PgPool, text: &dyn TextEmbedder, owner: Uuid) -> Result<Arc<Vec<ModelCoverage>>> {
        if let Some((at, c)) = self.coverage.lock().unwrap_or_else(|p| p.into_inner()).get(&owner)
            && at.elapsed() < self.ttl
        {
            return Ok(c.clone());
        }
        let models: Vec<(&str, i32, bool)> = Model::ALL
            .iter()
            .map(|m| (m.id(), m.version(), text.available(*m)))
            .collect();
        let c = Arc::new(embeddings::coverage(db, owner, &models).await?);
        self.coverage
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(owner, (Instant::now(), c.clone()));
        Ok(c)
    }

    /// Which model answers searches for `owner` right now: the configured one once it covers `switch_percent` of the
    /// library, until then the installed model with the most photos indexed.
    pub async fn serving(&self, db: &PgPool, text: &dyn TextEmbedder, owner: Uuid) -> Result<Serving> {
        let coverage = self.coverage(db, text, owner).await?;
        let chosen = embeddings::choose_serving(&coverage, self.model.id(), self.switch_percent)
            .map(|i| {
                Model::ALL
                    .into_iter()
                    .find(|m| m.id() == coverage[i].model)
                    .unwrap_or(self.model)
            })
            .unwrap_or(self.model);
        let reindexing = (chosen != self.model)
            .then(|| coverage.iter().find(|c| c.model == self.model.id()))
            .flatten()
            .map(|c| Reindexing {
                model: self.model.name(),
                indexed: c.progress.indexed,
                total: c.progress.total,
                percent: c.percent(),
                switch_at_percent: self.switch_percent,
            });
        Ok(Serving {
            model: chosen,
            reindexing,
        })
    }

    /// The owner's place names (cities, regions, countries of their live photos), for the query parser.
    pub async fn places(&self, db: &PgPool, owner: Uuid) -> Result<Arc<Vec<Place>>> {
        if let Some((at, p)) = self.places.lock().unwrap_or_else(|p| p.into_inner()).get(&owner)
            && at.elapsed() < self.places_ttl
        {
            return Ok(p.clone());
        }
        let mut out = Vec::new();
        for (column, kind) in [
            ("place_city", PlaceKind::City),
            ("place_region", PlaceKind::Region),
            ("place_country", PlaceKind::Country),
        ] {
            let rows: Vec<(String,)> = sqlx::query_as(&format!(
                "SELECT DISTINCT {column} FROM assets
                 WHERE owner_id = $1 AND deleted_at IS NULL AND primary_id IS NULL AND {column} IS NOT NULL"
            ))
            .bind(owner)
            .fetch_all(db)
            .await?;
            out.extend(rows.into_iter().map(|(value,)| Place { value, kind }));
        }
        let places = Arc::new(out);
        self.places
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(owner, (Instant::now(), places.clone()));
        Ok(places)
    }
}
