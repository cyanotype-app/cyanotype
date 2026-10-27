-- Dogs and cats in photos (docs/pets.md): which photos have a pet in them, and where, found by the worker's `pets` job (PETS_ENABLED,
-- on by default) in each photo's preview. Owners can filter, search and list photos by it. Telling individual pets apart is a later
-- feature; the boxes are stored for it.
--
--  * pet_detections: one row per animal found. `kind` is what it is (`dog` or `cat` today). It is data, not an enum: another kind is
--    one more row value and a line in the worker's model wrapper, never a change of shape. `box_*` are fractions (0..1) of the photo's
--    preview image (left, top, width, height), so they stay valid at any size it is drawn at. `score` is the detector's confidence.
--    `model` and `version` say what found it: a different model or version is a rescan, and the job replaces all of a photo's rows
--    (every model's) in one transaction, so running it again never duplicates an animal. `idx` orders a photo's animals, strongest first.
--  * pet_scans: one row per photo and model, written with the detections (also when there are none). That is what tells the sweep which
--    photos have been looked at, and what makes "scanned, no pets" different from "not scanned": a photo without pets has no
--    `pet_detections` row. `dog_share` and `cat_share` are what the search model said about the photo (0..1, docs/pets.md, "The
--    check"): the worker only looks for boxes in a photo where one of them is high enough, so these say why a photo has none.
--  * Both tables cascade from `assets` (a purged photo takes its detections with it) and `pet_detections` also from `users`.
-- New tables only: no row is rewritten and no job is queued here. With PETS_ENABLED off nothing writes to them.
CREATE TABLE pet_detections (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    asset_id   uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    owner_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind       text NOT NULL CHECK (kind <> ''),
    model      text NOT NULL,
    version    integer NOT NULL,
    idx        smallint NOT NULL,
    box_x      real NOT NULL,
    box_y      real NOT NULL,
    box_w      real NOT NULL CHECK (box_w > 0),
    box_h      real NOT NULL CHECK (box_h > 0),
    score      real NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (asset_id, model, idx)
);
-- "Which of this owner's photos have a dog": the Pets view, the filter and the facet.
CREATE INDEX pet_detections_owner_kind_idx ON pet_detections (owner_id, kind, asset_id);

CREATE TABLE pet_scans (
    asset_id    uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    model       text NOT NULL,
    version     integer NOT NULL,
    detections  integer NOT NULL,
    dog_share   real NOT NULL DEFAULT 0,
    cat_share   real NOT NULL DEFAULT 0,
    scanned_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (asset_id, model)
);
