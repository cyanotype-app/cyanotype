-- Two-factor authentication (TOTP) and API key scopes and expiry (docs/admin.md, "Two-factor authentication"; docs/import.md,
-- "API keys").
--
--  * user_totp: one row per account that started enrolling. `secret_enc` is the TOTP secret encrypted with the server key
--    (AES-256-GCM, AUTH_TOTP_KEY; never stored in clear). `confirmed_at` is NULL while the enrolment is pending (the secret
--    was shown, no code entered yet), set once a code proves the authenticator works; only a confirmed row counts as
--    "2FA on". `last_step` is the newest 30 s step accepted, so a code cannot be replayed. `failed_attempts` and
--    `locked_until` are the per-account throttle on wrong codes.
--  * totp_recovery_codes: the single-use recovery codes, as a keyed hash only (HMAC-SHA256 with a key derived from the server
--    key), never the code. `used_at` marks a spent one.
--  * login_challenges: what a correct password gets when the account has 2FA on: a short-lived random token (stored as its
--    sha256) to present with the code. No session exists until the code is right.
--  * sessions.needs_mfa_enrol: the administrator requires 2FA and this account has none: such a session reaches only
--    /auth/me and the enrolment routes until the account enrols.
--  * api_keys.scope ('import', 'read', 'full') and api_keys.expires_at. Every existing key becomes 'full' with no expiry,
--    so nothing that works today stops working.
-- The "require 2FA" setting is a `server_state` row (`require_2fa`), made on first use. No row rewritten, no job queued.
CREATE TABLE user_totp (
    user_id         uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    secret_enc      bytea NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    confirmed_at    timestamptz,
    last_step       bigint,
    failed_attempts integer NOT NULL DEFAULT 0,
    locked_until    timestamptz
);

CREATE TABLE totp_recovery_codes (
    id        uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash bytea NOT NULL,
    used_at   timestamptz,
    UNIQUE (user_id, code_hash)
);

CREATE TABLE login_challenges (
    token_hash bytea PRIMARY KEY,
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);
CREATE INDEX login_challenges_expires_idx ON login_challenges (expires_at);

ALTER TABLE sessions ADD COLUMN needs_mfa_enrol boolean NOT NULL DEFAULT false;

ALTER TABLE api_keys
    ADD COLUMN scope text NOT NULL DEFAULT 'full' CHECK (scope IN ('import', 'read', 'full')),
    ADD COLUMN expires_at timestamptz;
