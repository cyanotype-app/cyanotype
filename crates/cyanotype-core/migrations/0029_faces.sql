-- Faces in photos (docs/faces.md): where the faces in a photo are, and a fingerprint (embedding) of each, found by the worker's `faces`
-- job when FACES_ENABLED is on (it is off by default). Nothing groups them into people yet, and nothing in the API or the web
-- app reads these tables: they are the data a later "People" feature is built on.
--
--  * faces: one row per face found. `box_*` and `landmarks` are fractions (0..1) of the photo's preview image, so they stay
--    valid at any size it is drawn at: the box is left/top/width/height, `landmarks` the five points (right eye, left eye, nose,
--    right mouth corner, left mouth corner) as x0,y0,...,x4,y4. `score` is the detector's confidence. `embedding` is the
--    face's fingerprint (unit length, cosine distance); it is NULL for a face too small to fingerprint reliably. `model` and
--    `version` say what found it: a different model or version is a re-run, and the job replaces a photo's rows for the
--    model it runs (so running it again never duplicates a face). `idx` orders a photo's faces, strongest first, and with the
--    model makes a face unique within its photo.
--    `person_id` is empty for now: grouping faces into people will fill it (and add the table it points to).
--  * face_scans: one row per photo and model, written with the faces (also when there are none). That is what tells the sweep
--    which photos have been looked at: a photo without faces has no `faces` row.
--  * Each model gets its own HNSW index, a partial index on `(embedding::halfvec(N)) WHERE model = '<id>'`, as the search
--    embeddings do (migration 0013): an index only ever holds one model's vectors and the dimension is checked when a row is
--    written. A later model adds its own index in its own migration. Queries must repeat the expression and the predicate.
--  * Both tables cascade from `assets` (a purged photo takes its faces with it) and `faces` also from `users`.
-- New tables only: no row is rewritten and no job is queued here. With FACES_ENABLED off nothing ever writes to them.
CREATE TABLE faces (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    asset_id   uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    owner_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    model      text NOT NULL,
    version    integer NOT NULL,
    idx        smallint NOT NULL,
    box_x      real NOT NULL,
    box_y      real NOT NULL,
    box_w      real NOT NULL CHECK (box_w > 0),
    box_h      real NOT NULL CHECK (box_h > 0),
    landmarks  real[] NOT NULL CHECK (cardinality(landmarks) = 10),
    score      real NOT NULL,
    embedding  halfvec,
    person_id  uuid,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (asset_id, model, idx)
);
CREATE INDEX faces_owner_idx ON faces (owner_id, model);
CREATE INDEX faces_person_idx ON faces (person_id) WHERE person_id IS NOT NULL;
CREATE INDEX faces_yunet2026may_sface2021dec_hnsw ON faces
    USING hnsw ((embedding::halfvec(128)) halfvec_cosine_ops) WITH (m = 16, ef_construction = 64)
    WHERE model = 'yunet2026may-sface2021dec';

CREATE TABLE face_scans (
    asset_id   uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    model      text NOT NULL,
    version    integer NOT NULL,
    faces      integer NOT NULL,
    scanned_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (asset_id, model)
);
