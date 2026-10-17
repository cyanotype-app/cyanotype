-- Search with more than one model (docs/library.md, "Search", and docs/build.md, "Semantic search model").
--
-- Until now the table held one model's vectors as halfvec(512) under one HNSW index. A second model (SigLIP 2, 768
-- dimensions) has to be able to fill the table while the first one keeps serving searches, so:
--  * `embedding` takes any dimension (a `model` always has its own: 512 for CLIP B/32, 768 for SigLIP 2 base);
--  * each model gets its own HNSW index, a partial index on `(embedding::halfvec(N)) WHERE model = '<id>'`, so an
--    index only ever holds one model's vectors and its dimension is checked when a row is written (a vector of the
--    wrong size for its model fails the insert);
--  * old vectors are no longer deleted when a photo gets a vector from a newer model: the worker removes them once the
--    whole library is covered by the configured model (crates/cyanotype-core/src/embeddings.rs, `prune_superseded`).
--    Until then search keeps answering from the old model.
-- Searches must repeat the index's expression and predicate (`e.model = '<id>'` and `e.embedding::halfvec(N) <=> ...`)
-- for the planner to use the index.

DROP INDEX asset_embeddings_hnsw_idx;

ALTER TABLE asset_embeddings ALTER COLUMN embedding TYPE halfvec;

-- Cosine distance, as before. Re-created for CLIP here (the library's existing vectors stay where they are; building
-- the index takes seconds for tens of thousands of photos), created empty for SigLIP 2, which the worker fills.
CREATE INDEX asset_embeddings_clip_b32_hnsw ON asset_embeddings
    USING hnsw ((embedding::halfvec(512)) halfvec_cosine_ops) WITH (m = 16, ef_construction = 64)
    WHERE model = 'clip-vit-b32-openai-int8';

CREATE INDEX asset_embeddings_siglip2_b16_hnsw ON asset_embeddings
    USING hnsw ((embedding::halfvec(768)) halfvec_cosine_ops) WITH (m = 16, ef_construction = 64)
    WHERE model = 'siglip2-base-patch16-224-q4';

-- The embed backfill of a new model is the worker's sweep (it queues a job per photo without a vector for the
-- configured model and version), so no jobs are inserted here: a deploy that keeps CLIP has nothing to do, and one
-- that switches models starts the re-index on its own.
