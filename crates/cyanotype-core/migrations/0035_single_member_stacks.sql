-- A stack made by hand holds two or more live photos; the API now dissolves one that drops below that (a photo trashed, hidden
-- or taken out). Clean up the ones left over from before: every manual stack with fewer than two live photos loses its id and
-- cover flag on all members (trashed ones too), so they are ordinary photos again (back in their burst, if they were in one).
UPDATE assets
SET stack_id = NULL, stack_cover = false
WHERE stack_id IN (SELECT stack_id FROM assets WHERE stack_id IS NOT NULL
                   GROUP BY stack_id
                   HAVING count(*) FILTER (WHERE deleted_at IS NULL AND primary_id IS NULL) < 2);
