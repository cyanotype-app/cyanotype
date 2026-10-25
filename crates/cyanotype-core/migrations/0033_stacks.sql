-- Stacks: photos shown as one tile in the timeline and in search (docs/library.md, "Stacks"). A stack is either an automatic
-- burst (`burst_id`, computed by the worker, migration 0014) or one the owner made by hand. What the owner decided is kept in
-- columns the worker never writes, so no regroup or sweep can undo it:
--  * `stack_id`: the manual stack a photo is in (a random id shared by its members, NULL for none). It wins over the burst.
--  * `stack_unlinked`: the owner took the photo out of its stack or dissolved it. The photo is never again part of an
--    automatic burst (its `burst_id` is still kept up to date, and ignored).
--  * `stack_cover`: the owner chose this photo as the stack's cover (at most one per stack; without one, the cover is the
--    first favorite of the stack, else its earliest photo).
-- A photo's stack is therefore `coalesce(stack_id, CASE WHEN stack_unlinked THEN NULL ELSE burst_id END)`: the id of the stack, shared
-- by its members, owned by one account (every query also filters on `owner_id`). A stack never crosses owners.
-- Constant defaults on a new column: nothing is rewritten, so this is instant at any size.
ALTER TABLE assets
    ADD COLUMN stack_id uuid,
    ADD COLUMN stack_unlinked boolean NOT NULL DEFAULT false,
    ADD COLUMN stack_cover boolean NOT NULL DEFAULT false;

-- A manual stack's members (the burst index, `assets_burst_idx`, serves the automatic ones).
CREATE INDEX assets_stack_idx ON assets (owner_id, stack_id) WHERE stack_id IS NOT NULL;

-- The per-user switch: stacks collapsed to their cover in the timeline (the default), or every photo shown.
ALTER TABLE user_prefs ADD COLUMN stacks_collapsed boolean NOT NULL DEFAULT true;
