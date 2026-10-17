-- Backfill: one low-priority 'embed' job per existing photo whose thumbnail exists (the embed job reads preview.webp, so
-- a photo without derived images is embedded when its thumbnail job finishes, which enqueues the embed job itself).
-- Hidden RAW files are left out: the visible photo of the pair is the one that is searched. Version 1 is the model
-- cyanotype-clip ships first; a later model swap bumps the version in code and the worker's sweep queues the rest.
-- Re-running is harmless (ON CONFLICT), like the pairing and geocode backfills. Priority 10 puts these behind the
-- thumbnails and embeds of anything uploaded while this drains.
INSERT INTO jobs (kind, asset_id, version, priority)
SELECT 'embed', id, 1, 10 FROM assets
WHERE thumbhash IS NOT NULL AND primary_id IS NULL AND purging_at IS NULL
ON CONFLICT DO NOTHING;
