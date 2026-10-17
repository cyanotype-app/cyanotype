-- Synthetic library for measuring search: 50,000 photos of one owner with clustered 512-d unit vectors (a few hundred
-- "topics", the way real photo embeddings clump), spread over 8 years, 3 cameras, 40 places, GPS on half of them,
-- JPEG/HEIC/RAW-paired, 10% favorites, a 40-photo album. Run against a migrated scratch database:
--   psql -U cyanotype -d cy_perf -f scripts/search-perf-seed.sql
-- (docs/library.md, "Search": the measured numbers). Not for a real library.
\timing on
INSERT INTO users (id, email, password_hash) VALUES ('00000000-0000-0000-0000-0000000000a1', 'perf@example.com', 'x');

CREATE TEMP TABLE centers AS
SELECT g AS c, (SELECT array_agg(random() - 0.5 ORDER BY i) FROM generate_series(1, 512) i) AS v
FROM generate_series(1, 400) g;

INSERT INTO assets (owner_id, sha256, original_key, original_name, content_type, size_bytes, width, height, taken_at,
                    latitude, longitude, camera_make, camera_model, favorite, thumbhash, place_city, place_region,
                    place_country, place_country_code)
SELECT '00000000-0000-0000-0000-0000000000a1', sha256(g::text::bytea), 'originals/perf/' || g, 'IMG_' || g || (ARRAY['.JPG','.HEIC','.JPG','.CR3'])[1 + g % 4],
       'image/jpeg', 3000000, 4000, 3000, now() - (random() * 365 * 8) * interval '1 day',
       CASE WHEN g % 2 = 0 THEN 38 + random() ELSE NULL END, CASE WHEN g % 2 = 0 THEN -9 + random() ELSE NULL END,
       (ARRAY['Canon', 'Apple', 'SONY'])[1 + g % 3], (ARRAY['Canon EOS R5', 'iPhone 14 Pro', 'ILCE-7M4'])[1 + g % 3],
       g % 10 = 0, '\x01',
       CASE WHEN g % 2 = 0 THEN 'City ' || (g % 40) END, CASE WHEN g % 2 = 0 THEN 'Region ' || (g % 8) END,
       CASE WHEN g % 2 = 0 THEN 'Country ' || (g % 3) END, CASE WHEN g % 2 = 0 THEN 'C' || (g % 3) END
FROM generate_series(1, 50000) g;

INSERT INTO asset_embeddings (asset_id, model, version, embedding)
SELECT a.id, 'clip-vit-b32-openai-int8', 1, l2_normalize((SELECT array_agg(c.v[i] + 0.35 * (random() - 0.5) ORDER BY i)
                                                           FROM generate_series(1, 512) i)::real[]::vector::halfvec)
FROM assets a JOIN centers c ON c.c = 1 + (abs(hashtext(a.id::text)) % 400);

INSERT INTO albums (id, owner_id, title) VALUES ('00000000-0000-0000-0000-0000000000b1', '00000000-0000-0000-0000-0000000000a1', 'Perf album');
INSERT INTO album_assets (album_id, asset_id)
SELECT '00000000-0000-0000-0000-0000000000b1', id FROM assets ORDER BY id LIMIT 40;
ANALYZE;
SELECT count(*) AS photos, pg_size_pretty(pg_total_relation_size('asset_embeddings')) AS embeddings_total,
       pg_size_pretty(pg_relation_size('asset_embeddings_hnsw_idx')) AS hnsw_index FROM assets;
