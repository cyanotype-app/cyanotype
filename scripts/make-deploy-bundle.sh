#!/usr/bin/env bash
# Build the deploy bundle: the files the production server needs, and nothing else (no source, no secrets).
# The server never runs git; scripts/deploy.sh ships this tarball over SSH.
#
# Contents: docker-compose.yml, docker-compose.prod.yml, infra/caddy/** (Caddyfiles, tls/ modes,
# cloudflare-ips.caddy), .env.prod.example, scripts/update-cloudflare-ips.sh, scripts/fetch-basemap.sh (run on the server: docs/deploy.md, "Map"), and VERSION (the image tag).
# Never included: .env*, certs/, source code.
#
# Usage: scripts/make-deploy-bundle.sh [-o OUT.tar.gz] [-r GIT_REF] [-v VERSION]
#   -r GIT_REF   bundle the files as of that commit/tag/branch (git archive); default: the working tree
#   -v VERSION   image tag written to VERSION (sha-<short>, X.Y.Z, latest); default sha-<short> of HEAD/GIT_REF
#   -o OUT       output path; default deploy-bundle.tar.gz in the current directory
set -euo pipefail

out="deploy-bundle.tar.gz"; ref=""; version=""
while getopts "o:r:v:" opt; do
  case "$opt" in
    o) out="$OPTARG" ;;
    r) ref="$OPTARG" ;;
    v) version="$OPTARG" ;;
    *) sed -n '2,13p' "$0" >&2; exit 2 ;;
  esac
done

cd "$(dirname "$0")/.."
# Managed paths (relative to the repo root). A path missing at an older ref is simply skipped.
paths=(docker-compose.yml docker-compose.prod.yml infra/caddy .env.prod.example scripts/update-cloudflare-ips.sh scripts/fetch-basemap.sh)

if [ -z "$version" ]; then version="sha-$(git rev-parse --short=7 "${ref:-HEAD}^{commit}")"; fi
[[ "$version" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "bad version: $version" >&2; exit 2; }

stage="$(mktemp -d)"; trap 'rm -rf "$stage"' EXIT
mkdir "$stage/cyanotype"
if [ -n "$ref" ]; then
  present=()
  for p in "${paths[@]}"; do
    if git cat-file -e "$ref:$p" 2>/dev/null; then present+=("$p"); fi
  done
  git archive "$ref" -- "${present[@]}" | tar -x -C "$stage/cyanotype"
else
  # Tracked files only, so stray local files (certs, scratch) can never leak in.
  git ls-files -z -- "${paths[@]}" | tar --null -T - -c | tar -x -C "$stage/cyanotype"
fi
# Belt and braces: secrets must never ship.
find "$stage/cyanotype" \( -name '.env' -o -name '.env.*' -o -name certs \) ! -name '.env.prod.example' -prune -exec rm -rf {} +
[ -f "$stage/cyanotype/docker-compose.yml" ] && [ -f "$stage/cyanotype/docker-compose.prod.yml" ] \
  || { echo "compose files missing at ${ref:-the working tree}" >&2; exit 1; }
printf '%s\n' "$version" > "$stage/cyanotype/VERSION"

tar -C "$stage/cyanotype" --sort=name --owner=0 --group=0 --numeric-owner -czf "$out" .
echo "wrote $out (version $version, ${ref:-working tree})" >&2
