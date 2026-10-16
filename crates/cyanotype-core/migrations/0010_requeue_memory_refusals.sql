-- Photos the worker refused to decode for memory ("decode refused") become processable with the reduced-size decoders
-- (DCT-scaled JPEG, tile-streamed HEIC, streamed PNG): the memory a decode needs no longer grows with the image's
-- size, so requeue exactly those failures, with a fresh set of attempts. The worker wakes on the jobs_notify trigger.
--
-- Only the refusal the thumbnail job writes for an image that was too big to decode within the memory budget is
-- matched ("decoding a WxH image needs about N MB, over the worker memory budget (M MB available to this job)").
-- Other failures stay failed (corrupt files, originals larger than the budget itself, RAW without a preview, ...);
-- "Retry failed" in the app (POST /assets/processing/retry) requeues those on request. If the new limits still do not
-- fit an image, the job fails again with the same message, so running this twice, or on a database where nothing
-- matches, is harmless: a migration only runs once, and a requeued job is no longer 'failed'.
UPDATE jobs
SET state = 'queued', attempts = 0, run_after = now(), locked_at = NULL, finished_at = NULL, last_error = NULL
WHERE state = 'failed'
  AND kind = 'thumbnail'
  AND last_error LIKE 'decoding a %x% image needs about % MB, over the worker memory budget%';
