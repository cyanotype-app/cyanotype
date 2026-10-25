-- "Not a face" (docs/faces.md, "Not a face"): the owner marks a detection that is no face at all (a pattern, a reflection, a poster) so it
-- is left out of everything that counts or proposes faces. It is not "Not this person" (`face_rejections`): that says whose face it is not,
-- this says there is none.
--
--  * faces.not_face: set by the owner, cleared by the owner (undo, or "Bring back" in the viewer). A marked face never has a person (the
--    check), is skipped by the grouping run and by "Is this X?" suggestions, is not counted anywhere, and is not shown as a chip or a region
--    in the viewer. A new scan of the photo (a rotate or crop, a new model version) hands the mark on to the face that takes its place, like
--    a rejection (`faces::store`).
--
-- No row is rewritten and no job is queued here. With FACES_ENABLED off nothing ever writes to the column.
ALTER TABLE faces
    ADD COLUMN not_face boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT faces_not_face_unassigned CHECK (NOT not_face OR person_id IS NULL);
