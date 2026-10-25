-- Signing the command line in through the browser (docs/import.md, "Logging in"). Two short-lived tables; the API key a
-- successful login produces is an ordinary `api_keys` row, so the key list, revoke and the admin view need nothing new.
--
--  * cli_auth_codes: the loopback flow (a PKCE authorization code). The browser, once the person pressed Allow, sends the CLI
--    a random single-use code; the CLI trades it plus its PKCE verifier for the key. Only the sha256 of the code is stored,
--    the row is deleted when the code is used (or by the next login attempt after it expires, 60 s), and `redirect_uri`
--    must be given again at the exchange.
--  * cli_device_codes: the device flow (RFC 8628, for SSH sessions and headless machines). `device_code_hash` is the secret
--    the CLI polls with, `user_code_hash` the short code typed on /activate (both only as sha256). `status` is what the
--    person decided; the key is made at the poll that finds `approved`, which also deletes the row. `interval_secs` and
--    `last_polled_at` implement `slow_down`.
CREATE TABLE cli_auth_codes (
    code_hash      bytea PRIMARY KEY,
    user_id        uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_challenge text NOT NULL,
    redirect_uri   text NOT NULL,
    device_name    text NOT NULL,
    scope          text NOT NULL CHECK (scope IN ('import', 'read', 'full')),
    created_at     timestamptz NOT NULL DEFAULT now(),
    expires_at     timestamptz NOT NULL
);
CREATE INDEX cli_auth_codes_expires_idx ON cli_auth_codes (expires_at);

CREATE TABLE cli_device_codes (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    device_code_hash bytea NOT NULL UNIQUE,
    user_code_hash   bytea NOT NULL UNIQUE,
    device_name      text NOT NULL,
    scope            text NOT NULL CHECK (scope IN ('import', 'read', 'full')),
    status           text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'denied')),
    user_id          uuid REFERENCES users(id) ON DELETE CASCADE,
    created_at       timestamptz NOT NULL DEFAULT now(),
    expires_at       timestamptz NOT NULL,
    last_polled_at   timestamptz,
    interval_secs    integer NOT NULL DEFAULT 5
);
CREATE INDEX cli_device_codes_expires_idx ON cli_device_codes (expires_at);
