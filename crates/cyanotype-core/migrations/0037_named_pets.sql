-- Named pets (docs/pets.md, "Named pets"): the owner makes a pet ("Biscuit"), tags a few photos of it, and the app proposes more by how much
-- the picture of the animal looks like the tagged ones; the owner confirms or refuses, and every answer improves the next proposals. Nothing
-- here is automatic: no open model tells individual animals apart well enough to group them unattended, so a detection joins a pet only when
-- the owner says so. Everything is owner-scoped data, written only by the owner's own requests while PETS_ENABLED is on (on by default) and
-- read only by the owner's own session.
--
--  * pets: one row per named pet. `name` is always set (a pet exists because the owner named it). `kind` is `dog` or `cat` (data, not an
--    enum, like `pet_detections.kind`): only detections of that kind join it. `hidden` keeps a pet out of the lists, the filter and the
--    names read out of a search. `cover_detection_id` is the animal the owner chose for the round picture; empty means the best one is picked
--    when it is read. A name is personal data: it is never written to the bucket (the recovery manifests do not carry it), the audit trail
--    or a log.
--  * pet_detections.pet_id: the pet the owner put the animal in (`ON DELETE SET NULL`: deleting a pet frees its detections, which are not
--    deleted). Only the owner ever sets it.
--  * pet_detections.embedding / embed_model: the animal's fingerprint, a halfvec of the crop of its box made by the worker's `pets` job.
--    `embed_model` names the model that tried (and the crop recipe), so a different model is tried again and numbers of two models are never
--    compared; it is set with no `embedding` when the crop was too small to be worth one. Both null until tried (detections found before
--    this migration are filled in by the worker's sweep without being found again, so their ids and the decisions on them stay).
--  * pet_rejections: "this animal is not that pet", written when the owner takes an animal out of a pet, refuses a proposal, or moves it to
--    another pet. It is never proposed for that pet again. Both sides cascade: a purged photo takes its detections' rejections with it, and
--    so does a deleted pet.
--
-- A new scan of a photo replaces its detections (new ids, as before); `pets::store` hands what the owner decided on to the detections that
-- take the old ones' place, like faces do. New tables and columns only: no row is rewritten and no job is queued here.
CREATE TABLE pets (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id           uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name               text NOT NULL CHECK (btrim(name) <> '' AND char_length(name) <= 100),
    kind               text NOT NULL CHECK (kind <> ''),
    hidden             boolean NOT NULL DEFAULT false,
    cover_detection_id uuid REFERENCES pet_detections(id) ON DELETE SET NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX pets_owner_idx ON pets (owner_id);

ALTER TABLE pet_detections
    ADD COLUMN pet_id uuid REFERENCES pets(id) ON DELETE SET NULL,
    ADD COLUMN embedding halfvec,
    ADD COLUMN embed_model text;
-- A pet's animals (its photos, its centre of fingerprints).
CREATE INDEX pet_detections_pet_idx ON pet_detections (pet_id) WHERE pet_id IS NOT NULL;
-- The animals that could still be proposed: unassigned and fingerprinted, by owner and kind.
CREATE INDEX pet_detections_open_idx ON pet_detections (owner_id, kind) WHERE pet_id IS NULL AND embedding IS NOT NULL;

CREATE TABLE pet_rejections (
    detection_id uuid NOT NULL REFERENCES pet_detections(id) ON DELETE CASCADE,
    pet_id       uuid NOT NULL REFERENCES pets(id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (detection_id, pet_id)
);
CREATE INDEX pet_rejections_pet_idx ON pet_rejections (pet_id);
