-- The owner's word on what an animal is (docs/pets.md, "A wrong dog or cat"). `pet_detections.kind` is the detector's guess until the owner
-- says otherwise: putting an animal in a pet of the other kind, or choosing "It's a dog" / "It's a cat" in the viewer, rewrites `kind` itself
-- and sets `kind_by_owner`, so every reader of `kind` (the Pets view and its summary, the `pets` filter and facet, the smart-album rule,
-- the viewer's labels, the proposals) counts the correction without knowing about it. The flag is what a new scan reads: it lets the
-- animal take the old row's place whatever kind the detector now says, and carries the owner's kind over (`pets::store`).
-- A new column with a default only: no row is rewritten in a way that changes what it means, and no job is queued.
ALTER TABLE pet_detections ADD COLUMN kind_by_owner boolean NOT NULL DEFAULT false;
