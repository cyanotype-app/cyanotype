-- Admin portal, part 1: roles, invites and the audit log (docs/admin.md).
--
-- Roles: every account is a `member` unless it is an `admin`. The earliest-created account becomes the admin here, so an
-- instance that has run until now with a single owner (REGISTRATION_MODE=first_user) upgrades with that owner as its
-- admin and nobody locked out. On an empty database nothing is promoted; the first account to register is made admin by
-- the auth service. One row is rewritten; no job is queued.
ALTER TABLE users ADD COLUMN role text NOT NULL DEFAULT 'member' CHECK (role IN ('admin', 'member'));
UPDATE users SET role = 'admin' WHERE id = (SELECT id FROM users ORDER BY created_at, id LIMIT 1);

-- Invites: a single-use registration link made by an admin. Only the sha256 of the token is stored, so the link can be
-- shown once (at creation or when it is re-issued, which replaces the token), never read back from the database.
-- The three limit columns are presets the registration copies onto the new account (applied by the limits work);
-- NULL means "no preset".
CREATE TABLE invites (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash         bytea NOT NULL UNIQUE,
    created_by         uuid REFERENCES users(id) ON DELETE SET NULL,
    note               text,
    created_at         timestamptz NOT NULL DEFAULT now(),
    expires_at         timestamptz NOT NULL,
    revoked_at         timestamptz,
    used_at            timestamptz,
    used_by            uuid REFERENCES users(id) ON DELETE SET NULL,
    -- The address the invite was used with, kept when the account is deleted later.
    used_email         text,
    quota_bytes        bigint CHECK (quota_bytes IS NULL OR quota_bytes > 0),
    daily_upload_bytes bigint CHECK (daily_upload_bytes IS NULL OR daily_upload_bytes > 0),
    max_file_bytes     bigint CHECK (max_file_bytes IS NULL OR max_file_bytes > 0)
);
CREATE INDEX invites_created_idx ON invites (created_at DESC);

-- What admins did, newest first. The actor's email is kept beside the id so the entry still reads after the account is
-- gone. `detail` is a small JSON object (never a secret, a token or a photo name).
CREATE TABLE audit_log (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at          timestamptz NOT NULL DEFAULT now(),
    actor_id    uuid REFERENCES users(id) ON DELETE SET NULL,
    actor_email text,
    action      text NOT NULL,
    target_type text,
    target_id   text,
    detail      jsonb NOT NULL DEFAULT '{}'
);
CREATE INDEX audit_log_at_idx ON audit_log (at DESC, id DESC);
CREATE INDEX audit_log_action_idx ON audit_log (action, at DESC);
CREATE INDEX audit_log_actor_idx ON audit_log (actor_id, at DESC);
