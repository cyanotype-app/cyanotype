-- Semantic search (docs/library.md, "Search"): CLIP image embeddings, an approximate nearest-neighbour index over them,
-- and a priority for background jobs.

-- Job priority: lower is claimed first within a kind (default 0). The embedding backfill runs at 10, so a photo uploaded
-- while 50,000 others are being embedded is embedded first. The claim order is (priority, run_after, id).
ALTER TABLE jobs ADD COLUMN priority smallint NOT NULL DEFAULT 0;
DROP INDEX jobs_claim_idx;
CREATE INDEX jobs_claim_idx ON jobs (kind, priority, run_after, id) WHERE state = 'queued';

-- `model` is the model id and `version` the pipeline version (preprocessing, quantisation) the vector came from. A swap
-- changes either, the worker's sweep queues every photo that has no vector for the current pair, and the embed job
-- replaces the old vector. Search only reads vectors of the current pair.
ALTER TABLE asset_embeddings ADD COLUMN version integer NOT NULL DEFAULT 1;

-- Half precision: 1 KB a vector instead of 2 KB. At 2 KB a row no longer fits a page without being TOASTed (512 floats
-- plus the header are over the 2,032 byte limit), and the index would be twice the size for the same recall; on a
-- Postgres limited to a few hundred MB of cache that is the difference between the index being resident or not.
-- CLIP vectors are unit length, so fp16's ~3 significant digits are far finer than the gaps between neighbours.
-- (The table has never held a row: nothing wrote embeddings before this migration.)
ALTER TABLE asset_embeddings ALTER COLUMN embedding TYPE halfvec(512) USING embedding::halfvec(512);

-- Cosine distance, the metric CLIP is trained for. Unfiltered and broadly filtered searches walk this index (with
-- hnsw.iterative_scan, so a filter that rejects most candidates still returns enough rows); narrowly filtered
-- searches compute exact distances over the few photos that pass the filter instead (apps/assets, routes/search.rs).
CREATE INDEX asset_embeddings_hnsw_idx ON asset_embeddings USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Filters the search page offers. The place facet and filter read the geocoded names for one owner:
CREATE INDEX assets_place_idx ON assets (owner_id, place_country_code, place_region, place_city)
    WHERE deleted_at IS NULL AND primary_id IS NULL AND place_country_code IS NOT NULL;

-- The file type filter reads the original's extension. Computing it per row with a regex costs hundreds of milliseconds
-- over 50,000 photos (it is evaluated for every row the filter and its count touch), so keep it as a stored column.
-- (A rewrite of `assets`, a moment at this size; later inserts compute it themselves.)
ALTER TABLE assets ADD COLUMN file_ext text
    GENERATED ALWAYS AS (lower(substring(original_name from '\.([^./\\]+)$'))) STORED;

-- The camera as the facets and the filter name it: "Canon EOS R5" for make "Canon" and model "Canon EOS R5" (many cameras
-- repeat their make in the model), "Apple iPhone 15" for make "Apple" and model "iPhone 15". A stored column with an
-- index on its lowercase form, so the filter and its count read an index instead of evaluating this for every row.
ALTER TABLE assets ADD COLUMN camera_label text GENERATED ALWAYS AS (
    CASE WHEN camera_make IS NOT NULL AND camera_model IS NOT NULL AND lower(camera_model) LIKE lower(camera_make) || '%'
         THEN camera_model
         ELSE nullif(btrim(coalesce(camera_make, '') || ' ' || coalesce(camera_model, '')), '') END
) STORED;
CREATE INDEX assets_camera_label_idx ON assets (owner_id, (lower(camera_label)))
    WHERE deleted_at IS NULL AND primary_id IS NULL AND camera_label IS NOT NULL;
