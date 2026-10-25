# Build all service images from the single shared builder stage in ./Dockerfile:
#   docker buildx bake              # group "default": auth, assets, worker, cli, backup, web
#   TAG=v1 docker buildx bake
#   docker buildx bake --print      # show the resolved plan
#
# BuildKit dedupes the identical `builder` stage across targets, so the workspace compiles once.

variable "TAG" {
  default = "latest"
}

# Publishing (CI): REGISTRY is a prefix such as "ghcr.io/cyanotype-app/" (note the trailing slash) and
# TAGS a comma-separated tag list, e.g. "sha-1a2b3c4,latest". Both default to local-only naming, in which
# case the only tag is cyanotype-<svc>:$TAG.
variable "REGISTRY" {
  default = ""
}

variable "TAGS" {
  default = ""
}

# Commit sha recorded in the image labels (CI sets it).
variable "REVISION" {
  default = ""
}

function "tags" {
  params = [name]
  result = [for t in split(",", TAGS != "" ? TAGS : TAG) : "${REGISTRY}cyanotype-${name}:${t}"]
}

# Version of the downloadable CLI (the cli-vX.Y.Z tag, without the prefix; docs/releases.md). Empty = a development build.
variable "CLI_VERSION" {
  default = ""
}

# Parallel compile jobs for the CLI cross-builds (Dockerfile stage cli-deps). 2 keeps a small builder from running out of
# memory; the GitHub runners (4 CPUs, 16 GB) use 4. CI and publish must use the same value, or they invalidate each other's
# cached dependency layers.
variable "CARGO_BUILD_JOBS" {
  default = "2"
}

# Set CACHE=gha in CI (GitHub Actions cache; needs ACTIONS_CACHE_URL/ACTIONS_RUNTIME_TOKEN, which
# docker/build-push-action-style runners or crazy-max/ghaction-github-runtime export).
# Set CACHE=local to keep a portable cache in ./.buildcache (useful for CI that cannot use gha).
# Unset (default): rely on the local BuildKit layer cache only.
variable "CACHE" {
  default = ""
}

# Whether this run may write the cache. CI sets it to "false" for pull requests: they read what the last build of main stored and
# add nothing, so the 10 GB repository cache budget is not spent on per-PR copies of a multi-GB dependency layer (which evict
# main's entries) and the PR run does not wait for an export. Publish (main) writes.
variable "CACHE_WRITE" {
  default = "true"
}

function "cache_from_scope" {
  params = [scope, dir]
  result = CACHE == "gha" ? ["type=gha,scope=${scope}"] : CACHE == "local" ? ["type=local,src=${dir}"] : []
}

function "cache_to_scope" {
  params = [scope, dir]
  result = CACHE_WRITE != "true" ? [] : CACHE == "gha" ? ["type=gha,scope=${scope},mode=max"] : CACHE == "local" ? ["type=local,dest=${dir},mode=max"] : []
}

group "default" {
  targets = ["auth", "assets", "worker", "cli", "backup", "web"]
}

target "_common" {
  context    = "."
  dockerfile = "Dockerfile"
  # Each image has its own scope (set per target below): an export replaces its scope's manifest, so images sharing one
  # replaced each other's model layers. Layers are stored once by digest, so the shared builder layers are not duplicated.
  labels = {
    "org.opencontainers.image.source"   = "https://github.com/cyanotype-app/cyanotype"
    "org.opencontainers.image.licenses" = "AGPL-3.0-or-later"
    "org.opencontainers.image.revision" = REVISION
  }
}

target "auth" {
  inherits = ["_common"]
  target   = "auth"
  tags     = tags("auth")
  cache-from = cache_from_scope("cyanotype-auth", ".buildcache-auth")
  cache-to   = cache_to_scope("cyanotype-auth", ".buildcache-auth")
}

target "assets" {
  inherits = ["_common"]
  target   = "assets"
  tags     = tags("assets")
  cache-from = cache_from_scope("cyanotype-assets", ".buildcache-assets")
  cache-to   = cache_to_scope("cyanotype-assets", ".buildcache-assets")
}

target "worker" {
  inherits = ["_common"]
  target   = "worker"
  tags     = tags("worker")
  cache-from = cache_from_scope("cyanotype-worker", ".buildcache-worker")
  cache-to   = cache_to_scope("cyanotype-worker", ".buildcache-worker")
}

target "cli" {
  inherits = ["_common"]
  target   = "cli"
  tags     = tags("cli")
  cache-from = cache_from_scope("cyanotype-cli", ".buildcache-cli")
  cache-to   = cache_to_scope("cyanotype-cli", ".buildcache-cli")
}

# Nightly Postgres backup job (pg_dump + rclone); its own tiny Dockerfile, no Rust.
target "backup" {
  inherits   = ["_common"]
  context    = "infra/backup"
  dockerfile = "Dockerfile"
  tags       = tags("backup")
  # Its own scope: in the shared one its (tiny) export could replace the Rust layers.
  cache-from = cache_from_scope("cyanotype-backup", ".buildcache-backup")
  cache-to   = cache_to_scope("cyanotype-backup", ".buildcache-backup")
}

# Static web UI: Vite build (node + pnpm) served by Caddy; its own Dockerfile, no Rust.
target "web" {
  inherits   = ["_common"]
  context    = "apps/web"
  dockerfile = "Dockerfile"
  target     = "web"
  tags       = tags("web")
  # The shared Rust cache scope is not useful here.
  cache-from = cache_from_scope("cyanotype-web", ".buildcache-web")
  cache-to   = cache_to_scope("cyanotype-web", ".buildcache-web")
  # The CLI downloads the server hands out (/cli/*) are built by the cli-dist target and copied in.
  contexts = { "cli-dist-ctx" = "target:cli-dist" }
  args     = { CLI_DIST = "cli-dist-ctx" }
}

# The downloadable CLI for every platform (Dockerfile stage cli-dist; docs/releases.md). Cross-compiled, so it has its own
# cache scope: a change to the server crates that the CLI does not use never invalidates it, and vice versa.
#   docker buildx bake cli-dist     # unsigned dist into ./cli-dist-out
# Signing: pass the release key as a BuildKit secret, never as an argument:
#   CLI_RELEASE_KEY=... docker buildx bake cli-dist --set '*.secrets=id=cli_release_key,env=CLI_RELEASE_KEY'
target "cli-dist" {
  inherits   = ["_common"]
  target     = "cli-dist"
  output     = ["type=local,dest=cli-dist-out"]
  args       = { CYANOTYPE_CLI_VERSION = CLI_VERSION, CYANOTYPE_GIT_SHA = REVISION, CARGO_BUILD_JOBS = CARGO_BUILD_JOBS }
  cache-from = cache_from_scope("cyanotype-cli-dist", ".buildcache-cli")
  cache-to   = cache_to_scope("cyanotype-cli-dist", ".buildcache-cli")
}
