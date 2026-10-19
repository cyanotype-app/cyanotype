-- Admin portal, part 3: per-account limits (docs/admin.md, "Limits").
--
-- NULL means unlimited, for each of the three. Nothing is set here, so every existing account (the owner, an admin) stays
-- unlimited until an admin sets a limit. Accounts created from now on get the server default (the `default_limits` entry of
-- `server_state`, set by an admin) and, when they registered with an invite, the invite's presets in front of it; the first
-- account of an instance (the admin) gets neither. One column added per limit, no row rewritten, no job queued.
--
--  * quota_bytes: what the account may store, originals plus trash (trash counts until it is purged).
--  * daily_upload_bytes: what it may upload in any rolling 24 hours.
--  * max_file_bytes: the largest single file it may upload.
ALTER TABLE users
    ADD COLUMN quota_bytes        bigint CHECK (quota_bytes IS NULL OR quota_bytes > 0),
    ADD COLUMN daily_upload_bytes bigint CHECK (daily_upload_bytes IS NULL OR daily_upload_bytes > 0),
    ADD COLUMN max_file_bytes     bigint CHECK (max_file_bytes IS NULL OR max_file_bytes > 0);
