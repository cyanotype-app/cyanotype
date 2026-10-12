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

# Dev loop: compose Postgres + MinIO, migrate, run auth/assets/worker from source (Ctrl-C stops; :3001/:3002).
dev:
    docker compose up -d --wait postgres minio
    docker compose up minio-init
    cargo run -q -p cyanotype-cli -- migrate; \
    trap 'kill 0' EXIT INT TERM; \
    cargo run -q -p cyanotype-auth & \
    cargo run -q -p cyanotype-assets & \
    cargo run -q -p cyanotype-worker & \
    wait

# Full stack in containers; API on http://localhost:3000.
up:
    docker compose up -d --build

# Stop the full stack (keeps volumes).
down:
    docker compose down

# Build all four service images with buildx bake (the workspace compiles once). TAG=... to retag.
images:
    TAG={{tag}} docker buildx bake -f docker-bake.hcl

# End-to-end smoke test through the gateway (needs `just up` or equivalent). BASE_URL=... to override.
e2e:
    BASE_URL={{base_url}} scripts/e2e.sh

# Full stack plus the opt-in Datadog Agent (needs DD_API_KEY in your environment; the value is never printed).
datadog-up:
    @test -n "${DD_API_KEY:-}" || { echo "DD_API_KEY is not set in the environment" >&2; exit 1; }
    docker compose --profile datadog up -d --build

# Remove cyanotype-* images and dangling images, then prune BuildKit cache (the cache is host-wide).
clean-docker:
    -docker rmi cyanotype-auth:{{tag}} cyanotype-assets:{{tag}} cyanotype-worker:{{tag}} cyanotype-cli:{{tag}}
    docker image prune -f
    docker builder prune -f
