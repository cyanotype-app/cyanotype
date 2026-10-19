-- Admin portal, part 4: account actions (docs/admin.md, "Account actions").
--
--  * users.disabled_at: a disabled account cannot sign in, its sessions are deleted when it is disabled, and its API keys
--    (kept, so enabling restores the CLI) are refused by every service while it is disabled. NULL: enabled.
--  * password_resets: the one-time links an admin hands out. Only the sha256 of the token is stored; a link works once and
--    expires; using it sets the password and signs the account out everywhere.
--  * One account deletion at a time per account in `admin_tasks` (the background job that removes its objects and rows).
-- One nullable column and two empty tables; no row rewritten, no job queued.
ALTER TABLE users ADD COLUMN disabled_at timestamptz;

CREATE TABLE password_resets (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash bytea NOT NULL UNIQUE,
    created_by uuid REFERENCES users(id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    used_at    timestamptz
);
CREATE INDEX password_resets_user_idx ON password_resets (user_id);

CREATE UNIQUE INDEX admin_tasks_one_delete_per_account ON admin_tasks ((params ->> 'user_id'))
    WHERE kind = 'delete_account' AND state IN ('queued', 'running');
