-- Text in photos (docs/library.md, "Text in photos"): what the worker's `ocr` job reads off a photo's preview.
--
--  * asset_text: one row per photo. `text` is the recognised lines joined by newlines (what search and "Copy" use); `lines` is
--    the same with a confidence, a quadrilateral and the words of each line, all in coordinates 0..1 of the image (so valid at
--    any size the viewer draws them); `qr` is the QR codes found (payload and corners). A photo with no text has a row with an
--    empty `text` (so it is known to have been read, and is not read again). `model` and `version` say what read it: a
--    different model or version is a re-run (the old text keeps answering searches until the new one replaces it).
--  * `tsv` is a generated `simple`-configuration tsvector: no stemming and no stop words, so it works for any script and any
--    mix of languages a photo has; GIN-indexed. `has_text` is generated too, for the "has text" filter.
--  * A trigram index on `text` makes `ILIKE '%part of a number%'` an index scan: the way to find half of a receipt total or a
--    word's middle that whole-word matching cannot (pg_trgm ships with Postgres; it is a trusted extension, the database's
--    owner may create it).
-- One new empty table; no row rewritten. The worker's sweep queues an `ocr` job (low priority) for every photo that has a
-- preview and no row for the current model: that is the backfill of an existing library, a few thousand photos an hour.
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE TABLE asset_text (
    asset_id   uuid PRIMARY KEY REFERENCES assets(id) ON DELETE CASCADE,
    model      text NOT NULL,
    version    integer NOT NULL,
    text       text NOT NULL,
    lines      jsonb NOT NULL DEFAULT '[]',
    qr         jsonb NOT NULL DEFAULT '[]',
    has_text   boolean GENERATED ALWAYS AS (btrim(text) <> '') STORED,
    tsv        tsvector GENERATED ALWAYS AS (to_tsvector('simple', text)) STORED,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX asset_text_tsv_idx ON asset_text USING gin (tsv);
CREATE INDEX asset_text_trgm_idx ON asset_text USING gin (text gin_trgm_ops);
CREATE INDEX asset_text_has_text_idx ON asset_text (asset_id) WHERE has_text;
