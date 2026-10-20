-- Rotate and crop, non-destructively (docs/library.md, "Rotate and crop"). Additive: no row is rewritten and no job is queued.
--
-- recipe              the photo's edit recipe (crate::recipe::Recipe as JSON), NULL when the photo is shown as the file is.
--                     The bucket's edits/ object is the source of truth; reindex restores this column from it.
-- width_orig/height_orig
--                     the file's own (displayed) size while a recipe is set; width/height then hold the size of the edited
--                     picture, so the grid's tile shapes and the "resolution" shown follow what the photo looks like now.
--                     NULL when there is no recipe.
-- edited_copy_at/_bytes
--                     the full-size JPEG of the edited picture ("Download edited", share links), written by the worker on
--                     demand to derived/{owner}/{asset}/edited.jpg; cleared whenever the recipe changes.
ALTER TABLE assets
    ADD COLUMN recipe jsonb,
    ADD COLUMN width_orig integer,
    ADD COLUMN height_orig integer,
    ADD COLUMN edited_copy_at timestamptz,
    ADD COLUMN edited_copy_bytes bigint;
