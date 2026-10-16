# Task runner for Cyanotype. Install `just`: https://github.com/casey/just
#   cargo install just --locked | brew install just | apt install just | cargo binstall just
#   (or a prebuilt binary from https://github.com/casey/just/releases)
# Run `just` to list recipes.

set shell := ["bash", "-euo", "pipefail", "-c"]
# Load .env (if present) into recipes; variables already in the environment win. Values are never echoed.
set dotenv-load

# Bake tag for image builds, and the gateway URL for e2e.
tag :=  env_var_or_default("TAG", "latest")
base_url :=  env_var_or_default("BASE_URL", "http://localhost:3000")

# List recipes.
default:
    @just --list

# Format all code.
fmt:
    cargo fmt --all

# Check formatting without changing files.
fmt-check:
    cargo fmt --all --check

# Clippy with warnings as errors.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run tests. DATABASE_URL (env or .env) enables the DB integration tests; without it they are skipped.
test:
    @if [ -n "${DATABASE_URL:-}" ]; then echo "DATABASE_URL is set: running DB integration tests"; else echo "DATABASE_URL not set: DB integration tests are skipped"; fi
    cargo test --workspace

# Everything CI checks: format, lint, tests.
check: fmt-check lint test

# Apply migrations (needs DATABASE_URL, e.g. from .env).
migrate:
    cargo run -p cyanotype-cli -- migrate

# Run the importer from source (release build: hashing is CPU-bound). `just import ~/Pictures --dry-run`. See docs/import.md.
import *args:
    cargo run -q --release -p cyanotype-cli -- import {{args}}

# Dev loop: compose Postgres + SeaweedFS, migrate, run auth/assets/worker from source (Ctrl-C stops; :3001/:3002).
dev:
    docker compose up -d --wait postgres seaweedfs
    docker compose up seaweedfs-init
    cargo run -q -p cyanotype-cli -- migrate; \
    trap 'kill 0' EXIT INT TERM; \
    cargo run -q -p cyanotype-auth & \
    cargo run -q -p cyanotype-assets & \
    cargo run -q -p cyanotype-worker & \
    wait

# Web UI dev server (Vite, http://localhost:5173) proxying /auth, /assets and /healthz to the gateway (GATEWAY_URL, default :3000; `just up` first).
web-dev:
    cd apps/web && pnpm install --frozen-lockfile && pnpm dev

# Typecheck, lint and test the web UI (installs from the lockfile first).
web-check:
    cd apps/web && pnpm install --frozen-lockfile && pnpm check

# Production build of the web UI into apps/web/dist.
web-build:
    cd apps/web && pnpm install --frozen-lockfile && pnpm build

# Full stack in containers; API on http://localhost:3000.
up:
    docker compose up -d --build

# Stop the full stack (keeps volumes).
down:
    docker compose down

# Build all six images (auth, assets, worker, cli, backup, web) with buildx bake (the workspace compiles once). TAG=... to retag.
images:
    TAG={{tag}} docker buildx bake -f docker-bake.hcl

# End-to-end smoke test through the gateway (needs `just up` or equivalent). BASE_URL=... to override.
e2e:
    BASE_URL={{base_url}} scripts/e2e.sh

# End-to-end test of `cyanotype import` through the gateway (needs `just up`, REGISTRATION_MODE=open). BASE_URL=... to override.
e2e-import:
    BASE_URL={{base_url}} scripts/e2e-import.sh

# Full stack plus the opt-in Datadog Agent (needs DD_API_KEY in your environment; the value is never printed).
datadog-up:
    @test -n "${DD_API_KEY:-}" || { echo "DD_API_KEY is not set in the environment" >&2; exit 1; }
    docker compose --profile datadog up -d --build

# Local compose by default; for the prod stack set COMPOSE="docker compose --env-file .env.prod -f docker-compose.yml -f docker-compose.prod.yml".
# Postgres backup now: pg_dump -Fc into the bucket under backups/postgres/, then prune to BACKUP_KEEP.
backup-now:
    ${COMPOSE:-docker compose} run --rm backup now

# ---- Production (single VPS; see docs/deploy.md) ----------------------------------------------
# DEPLOY_HOST=deploy@203.0.113.10 (user@host, key auth; use the origin IP or a DNS-only name, not a Cloudflare-proxied
# one). Optional: DEPLOY_DIR (default ~/cyanotype), DEPLOY_PROFILES (e.g. "datadog"), DEPLOY_REF (git ref the bundle
# is built from; "worktree" ships your local tracked files). The server never runs git: scripts/deploy.sh ships a
# bundle (compose files, infra/caddy, VERSION) over SSH. Secrets stay in ~/cyanotype/.env.prod on the server.
# The images are private: either docker login ghcr.io once on the server (read:packages PAT), or export
# GHCR_TOKEN (+ GHCR_USER) and the token is piped over stdin for this deploy only. See docs/deploy.md.

prod_compose := "docker compose --env-file .env.prod -f docker-compose.yml -f docker-compose.prod.yml"

# Build the deploy bundle for a version, ship it to DEPLOY_HOST, then pull, migrate, up. version: sha-<short>, X.Y.Z or latest (default); older versions roll back, compose files included.
deploy version="latest":
    DEPLOY_SSH="${DEPLOY_HOST:?set DEPLOY_HOST (e.g. deploy@203.0.113.10)}" scripts/deploy.sh "{{version}}"

# Write deploy-bundle.tar.gz (what the server receives) for a version, from the working tree or REF=<git ref>.
bundle version="latest":
    scripts/make-deploy-bundle.sh -v "{{version}}" ${REF:+-r "$REF"}

# One Postgres backup now on DEPLOY_HOST (uploads to the bucket under backups/postgres/).
prod-backup-now:
    #!/usr/bin/env bash
    set -euo pipefail
    : "${DEPLOY_HOST:?set DEPLOY_HOST}"
    . scripts/lib-remote.sh
    dir="$(deploy_dir_normalize "${DEPLOY_DIR:-cyanotype}")"
    DEPLOY_SSH="$DEPLOY_HOST"; ssh_opts=(-o BatchMode=yes)
    remote_ssh bash -c 'cd "$1" && shift && exec "$@"' _ "$dir" {{prod_compose}} exec backup /usr/local/bin/backup.sh now

# Remove cyanotype-* images and dangling images, then prune BuildKit cache (the cache is host-wide).
clean-docker:
    -docker rmi cyanotype-auth:{{tag}} cyanotype-assets:{{tag}} cyanotype-worker:{{tag}} cyanotype-cli:{{tag}} cyanotype-backup:{{tag}} cyanotype-web:{{tag}}
    docker image prune -f
    docker builder prune -f
