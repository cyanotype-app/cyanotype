-- People (docs/faces.md, "People"): the faces of migration 0029 grouped into people the owner can name and correct. Everything here is
-- only ever written while FACES_ENABLED is on (it is off by default), and read only by the owner's own session.
--
--  * people: one row per person. `name` is empty until the owner gives one ("Unnamed person" in the app). `hidden` keeps a person out of
--    the People page's main list and out of a photo's chips (strangers in the background, a person the owner would rather not see). `cover_face_id`
--    is the face the owner chose for the round picture; empty means the best face is picked when it is read. Names are personal data: they
--    are never written to the bucket (the recovery manifests do not carry them) and never leave the owner's session.
--  * faces.person_id gets its foreign key now: deleting a person sets its faces free (ON DELETE SET NULL), they are not deleted. `assigned_by`
--    says who put a face in its person: 'auto' (the worker's grouping, which may be moved or undone) or 'user' (the owner said so: the
--    worker never moves such a face). It means nothing while `person_id` is empty.
--  * face_rejections: "this face is not this person", written when the owner takes a face out of a person or moves it to another. The worker
--    never assigns the face to that person again, and the app never suggests it. Both sides cascade: a purged photo takes its faces'
--    rejections with it, and so does a deleted person.
--  * people_runs: one row per owner, the request and the state of a grouping run, like `duplicate_scans` (0027). `requested_at` is set whenever
--    faces were stored for the owner's photos or the owner changed a person, and the worker runs when the owner's faces have stopped
--    arriving for a quiet spell, so a backfill of thousands of photos groups once, not once a photo.
--
-- No row is rewritten and no job is queued here. With FACES_ENABLED off nothing ever writes to these tables.
CREATE TABLE people (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name          text CHECK (name IS NULL OR (btrim(name) <> '' AND char_length(name) <= 100)),
    hidden        boolean NOT NULL DEFAULT false,
    cover_face_id uuid REFERENCES faces(id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX people_owner_idx ON people (owner_id);

ALTER TABLE faces
    ADD COLUMN assigned_by text NOT NULL DEFAULT 'auto' CHECK (assigned_by IN ('auto', 'user')),
    ADD CONSTRAINT faces_person_fk FOREIGN KEY (person_id) REFERENCES people(id) ON DELETE SET NULL;
-- The faces the worker may still group: unassigned and fingerprinted.
CREATE INDEX faces_ungrouped_idx ON faces (owner_id, model) WHERE person_id IS NULL AND embedding IS NOT NULL;

CREATE TABLE face_rejections (
    face_id    uuid NOT NULL REFERENCES faces(id) ON DELETE CASCADE,
    person_id  uuid NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (face_id, person_id)
);
CREATE INDEX face_rejections_person_idx ON face_rejections (person_id);

CREATE TABLE people_runs (
    owner_id     uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    requested_at timestamptz,
    started_at   timestamptz,
    finished_at  timestamptz,
    -- What the last run did: faces added to existing people, people made, faces in them.
    assigned     integer,
    created      integer,
    grouped      integer,
    error        text
);
CREATE INDEX people_runs_due_idx ON people_runs (requested_at) WHERE requested_at IS NOT NULL;
