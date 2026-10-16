-- Map view and offline reverse geocoding (docs/library.md, "Map" and "Places").

-- Geotagged live photos for the map: the clustering query and the viewport list filter a latitude band and then a
-- longitude range for one owner, so (owner, latitude, longitude) answers both from the index, and hidden RAWs and
-- trashed photos are not in it. Photos without GPS (the vast majority of a typical library) are not in it either.
CREATE INDEX assets_geo_idx ON assets (owner_id, latitude, longitude)
    WHERE deleted_at IS NULL AND primary_id IS NULL AND latitude IS NOT NULL AND longitude IS NOT NULL;

-- Where a photo was taken, from the nearest populated place in `places` (the `geocode` job). All NULL when the photo
-- has no coordinates or nothing in the dataset is close enough (open sea, polar regions).
ALTER TABLE assets
    ADD COLUMN place_city         text,
    ADD COLUMN place_region       text,
    ADD COLUMN place_country      text,
    ADD COLUMN place_country_code text;

-- GeoNames populated places (CC BY 4.0, https://www.geonames.org/), loaded by `cyanotype migrate` from the snapshot
-- bundled in the CLI image (apps/cli/data/places.tsv.gz), so reverse geocoding needs no external service and no
-- download on deploy. Plain doubles and a btree: the lookup narrows by a latitude/longitude box around the photo
-- and ranks the few dozen rows inside it by great-circle distance, which needs neither PostGIS nor cube.
CREATE TABLE places (
    name         text             NOT NULL,
    region       text,
    country      text             NOT NULL,
    country_code text             NOT NULL,
    latitude     double precision NOT NULL,
    longitude    double precision NOT NULL,
    population   integer          NOT NULL DEFAULT 0
);
CREATE INDEX places_lat_lon_idx ON places (latitude, longitude);

-- Which snapshot is loaded (one row), so `migrate` reloads only when the bundled one is different.
CREATE TABLE place_dataset (
    only_row  boolean PRIMARY KEY DEFAULT true CHECK (only_row),
    version   text        NOT NULL,
    places    integer     NOT NULL,
    loaded_at timestamptz NOT NULL DEFAULT now()
);

-- Backfill: one 'geocode' job per existing geotagged photo (trashed ones too, so restoring one needs nothing more).
-- Re-running is harmless (ON CONFLICT). The worker drains them in the background; each is one indexed lookup.
INSERT INTO jobs (kind, asset_id, version)
SELECT 'geocode', id, 1 FROM assets
WHERE latitude IS NOT NULL AND longitude IS NOT NULL
ON CONFLICT DO NOTHING;
