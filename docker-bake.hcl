# Build all service images from the single shared builder stage in ./Dockerfile:
#   docker buildx bake              # group "default": auth, assets, worker, cli
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

# Set CACHE=gha in CI (GitHub Actions cache; needs ACTIONS_CACHE_URL/ACTIONS_RUNTIME_TOKEN, which
# docker/build-push-action-style runners or crazy-max/ghaction-github-runtime export).
# Set CACHE=local to keep a portable cache in ./.buildcache (useful for CI that cannot use gha).
# Unset (default): rely on the local BuildKit layer cache only.
variable "CACHE" {
  default = ""
}

# One shared cache scope: every target contains the same builder stage, so they share cache entries.
function "cache_from" {
  params = []
  result = CACHE == "gha" ? ["type=gha,scope=cyanotype"] : CACHE == "local" ? ["type=local,src=.buildcache"] : []
}

function "cache_to" {
  params = []
  result = CACHE == "gha" ? ["type=gha,scope=cyanotype,mode=max"] : CACHE == "local" ? ["type=local,dest=.buildcache,mode=max"] : []
}

group "default" {
  targets = ["auth", "assets", "worker", "cli", "backup"]
}

target "_common" {
  context    = "."
  dockerfile = "Dockerfile"
  cache-from = cache_from()
  cache-to   = cache_to()
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
}

target "assets" {
  inherits = ["_common"]
  target   = "assets"
  tags     = tags("assets")
}

target "worker" {
  inherits = ["_common"]
  target   = "worker"
  tags     = tags("worker")
}

target "cli" {
  inherits = ["_common"]
  target   = "cli"
  tags     = tags("cli")
}

# Nightly Postgres backup job (pg_dump + rclone); its own tiny Dockerfile, no Rust.
target "backup" {
  inherits   = ["_common"]
  context    = "infra/backup"
  dockerfile = "Dockerfile"
  tags       = tags("backup")
}
