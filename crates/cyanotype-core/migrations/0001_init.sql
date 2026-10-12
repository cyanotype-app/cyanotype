-- Multi-user from day one: every row hangs off owner_id / user_id.
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE users (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email         text NOT NULL,
    password_hash text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX users_email_lower_idx ON users (lower(email));

CREATE TABLE sessions (
    token_hash bytea PRIMARY KEY,           -- sha256 of the cookie value
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);
CREATE INDEX sessions_user_idx ON sessions (user_id);

CREATE TABLE api_keys (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       text NOT NULL,
    key_hash   bytea NOT NULL UNIQUE,       -- sha256 of the key
    created_at timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz
);

CREATE TABLE assets (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    sha256        bytea NOT NULL CHECK (octet_length(sha256) = 32),
    original_key  text NOT NULL,            -- S3 key of the immutable original
    original_name text NOT NULL,
    content_type  text NOT NULL,
    size_bytes    bigint NOT NULL,
    -- Filled by the worker's metadata job:
    width         integer,
    height        integer,
    taken_at      timestamptz,              -- falls back to created_at in queries
    latitude      double precision,
    longitude     double precision,
    camera_make   text,
    camera_model  text,
    lens_model    text,
    exif          jsonb,
    thumbhash     bytea,
    created_at    timestamptz NOT NULL DEFAULT now(),
    deleted_at    timestamptz,
    UNIQUE (owner_id, sha256)               -- dedupe key
);
CREATE INDEX assets_timeline_idx ON assets (owner_id, (coalesce(taken_at, created_at)) DESC) WHERE deleted_at IS NULL;

-- Postgres-backed queue: SELECT ... FOR UPDATE SKIP LOCKED.
CREATE TABLE jobs (
    id           bigserial PRIMARY KEY,
    kind         text NOT NULL,             -- 'metadata' | 'thumbnail' | 'geocode' | 'embed'
    asset_id     uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    version      integer NOT NULL DEFAULT 1, -- bump to re-run with a new pipeline/model
    state        text NOT NULL DEFAULT 'queued' CHECK (state IN ('queued','running','done','failed')),
    attempts     integer NOT NULL DEFAULT 0,
    run_after    timestamptz NOT NULL DEFAULT now(),
    locked_at    timestamptz,
    last_error   text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    finished_at  timestamptz,
    UNIQUE (kind, asset_id, version)        -- idempotent enqueue
);
CREATE INDEX jobs_claim_idx ON jobs (kind, run_after) WHERE state = 'queued';

-- Versioned embeddings so models can be swapped and re-run.
-- Dimension is fixed per table; the HNSW index is added with the search milestone
-- once a model is chosen.
CREATE TABLE asset_embeddings (
    asset_id   uuid NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    model      text NOT NULL,
    embedding  vector(512) NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (asset_id, model)
);

-- Pending direct-to-S3 uploads, so /complete can't be forged for another user's key.
CREATE TABLE uploads (
    upload_id     text PRIMARY KEY,         -- S3 multipart upload id
    owner_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    sha256        bytea NOT NULL,
    key           text NOT NULL,
    size_bytes    bigint NOT NULL,
    part_size     bigint NOT NULL,
    original_name text NOT NULL,
    content_type  text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);
