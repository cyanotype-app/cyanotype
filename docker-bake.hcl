# Build all service images from the single shared builder stage in ./Dockerfile:
#   docker buildx bake              # group "default": auth, assets, worker, cli
#   TAG=v1 docker buildx bake
#   docker buildx bake --print      # show the resolved plan
#
# BuildKit dedupes the identical `builder` stage across targets, so the workspace compiles once.

variable "TAG" {
  default = "latest"
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
  targets = ["auth", "assets", "worker", "cli"]
}

target "_common" {
  context    = "."
  dockerfile = "Dockerfile"
  cache-from = cache_from()
  cache-to   = cache_to()
}

target "auth" {
  inherits = ["_common"]
  target   = "auth"
  tags     = ["cyanotype-auth:${TAG}"]
}

target "assets" {
  inherits = ["_common"]
  target   = "assets"
  tags     = ["cyanotype-assets:${TAG}"]
}

target "worker" {
  inherits = ["_common"]
  target   = "worker"
  tags     = ["cyanotype-worker:${TAG}"]
}

target "cli" {
  inherits = ["_common"]
  target   = "cli"
  tags     = ["cyanotype-cli:${TAG}"]
}
