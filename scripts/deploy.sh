#!/usr/bin/env bash
# Deploy a version to the production server over SSH. Used by the Deploy workflow and `just deploy`.
# The server never runs git: this builds the deploy bundle locally (scripts/make-deploy-bundle.sh), streams it
# over SSH, swaps only the managed files into ~/cyanotype, then pull -> migrate -> up.
#
# Usage: scripts/deploy.sh [version]      version: sha-<short> | X.Y.Z | latest (default latest)
# Env:
#   DEPLOY_SSH       ssh destination (user@host or a Host alias). Required.
#   DEPLOY_DIR       directory on the server (default cyanotype, relative to the login home)
#   DEPLOY_REF       git ref the bundle is built from. Default: sha-X -> X, X.Y.Z -> vX.Y.Z, latest -> origin/main
#                    (or main). Use "worktree" to ship the local working tree (tracked files).
#   DEPLOY_PROFILES  extra compose profiles, space separated (datadog is auto-enabled when DD_API_KEY is set
#                    in the server's .env.prod)
#   GHCR_TOKEN, GHCR_USER  optional: if set, the token is piped over stdin to `docker login ghcr.io` on the
#                    server for the duration of the deploy and `docker logout` runs afterwards. Never on a
#                    command line. If unset, the server must already be logged in (docker login ghcr.io).
# Never touched on the server: .env.prod (only CYANOTYPE_VERSION is updated), certs/, volumes/data.
set -euo pipefail

: "${DEPLOY_SSH:?set DEPLOY_SSH (user@host or ssh alias)}"
version="${1:-latest}"
dir="${DEPLOY_DIR:-cyanotype}"
profiles="${DEPLOY_PROFILES:-}"
[[ "$version" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "bad version: $version" >&2; exit 2; }
[[ "$dir" =~ ^[A-Za-z0-9._/~-]+$ ]] || { echo "bad DEPLOY_DIR" >&2; exit 2; }
here="$(cd "$(dirname "$0")" && pwd)"

ref="${DEPLOY_REF:-}"
if [ -z "$ref" ]; then
  case "$version" in
    sha-*)  ref="${version#sha-}" ;;
    [0-9]*) ref="v$version" ;;
    *)      if git -C "$here" rev-parse -q --verify origin/main >/dev/null; then ref="origin/main"; else ref="main"; fi ;;
  esac
fi
ref_args=()
if [ "$ref" != "worktree" ]; then
  git -C "$here" rev-parse -q --verify "$ref^{commit}" >/dev/null \
    || { echo "cannot resolve git ref '$ref' for version $version (fetch tags/history first)" >&2; exit 1; }
  ref_args=(-r "$ref")
fi

bundle="$(mktemp)"; trap 'rm -f "$bundle"' EXIT
"$here/make-deploy-bundle.sh" "${ref_args[@]}" -v "$version" -o "$bundle"

ssh_opts=(-o BatchMode=yes)
loggedin=0
logout() { if [ "$loggedin" = 1 ]; then ssh "${ssh_opts[@]}" "$DEPLOY_SSH" "docker logout ghcr.io" >/dev/null 2>&1 || true; fi; }
trap 'logout; rm -f "$bundle"' EXIT

if [ -n "${GHCR_TOKEN:-}" ]; then
  # The token travels on stdin only; the remote command line carries just the (non-secret) user name.
  printf '%s' "$GHCR_TOKEN" | ssh "${ssh_opts[@]}" "$DEPLOY_SSH" "docker login ghcr.io -u '${GHCR_USER:-github-actions}' --password-stdin"
  loggedin=1
fi

echo "shipping $version ($ref) to $DEPLOY_SSH:$dir"
# 1) Bundle -> a staging dir on the server (stdin; nothing sensitive).
ssh "${ssh_opts[@]}" "$DEPLOY_SSH" "set -e; mkdir -p $dir; cd $dir; rm -rf .incoming; mkdir .incoming; tar -xzf - --no-same-owner -C .incoming" < "$bundle"

# 2) Install managed files and roll out. The script comes in on stdin, so it is not in the process list.
ssh "${ssh_opts[@]}" "$DEPLOY_SSH" bash -s -- "$version" "$dir" "$profiles" <<'REMOTE'
set -euo pipefail
version="$1"; dir="$2"; extra_profiles="$3"
cd "$dir"
trap 'rm -rf .incoming' EXIT
test -f .env.prod || { echo ".env.prod missing on the server (create it in $dir, chmod 600)" >&2; exit 1; }
test -f .incoming/docker-compose.yml && test -f .incoming/docker-compose.prod.yml || { echo "bundle incomplete" >&2; exit 1; }
test "$(cat .incoming/VERSION)" = "$version" || { echo "bundle VERSION does not match $version" >&2; exit 1; }

# Swap only the managed files. Never .env*, certs/, or anything else in the directory.
infra_changed=0
if [ -d infra ] && ! diff -rq .incoming/infra infra >/dev/null 2>&1; then infra_changed=1; fi
put() { # put SRC DEST: copy to a temp name beside DEST, then rename over it (atomic per file)
  mkdir -p "$(dirname "$2")"; cp -p "$1" "$2.new.$$"; mv -f "$2.new.$$" "$2"
}
for entry in $(ls -A .incoming); do
  case "$entry" in
    .env*|certs|.incoming) [ "$entry" = ".env.prod.example" ] || continue ;;
  esac
  if [ -d ".incoming/$entry" ]; then
    # Update in place (keeps the directory inode, so running bind mounts see it); drop files no longer shipped.
    if [ -d "$entry" ]; then
      (cd "$entry" && find . -type f) | while read -r f; do
        [ -e ".incoming/$entry/$f" ] || rm -f "$entry/$f"
      done
    fi
    (cd ".incoming/$entry" && find . -type f) | while read -r f; do put ".incoming/$entry/$f" "$entry/$f"; done
  else
    put ".incoming/$entry" "$entry"
  fi
done

if grep -q '^CYANOTYPE_VERSION=' .env.prod; then
  sed -i "s/^CYANOTYPE_VERSION=.*/CYANOTYPE_VERSION=$version/" .env.prod
else
  printf 'CYANOTYPE_VERSION=%s\n' "$version" >> .env.prod
fi
profiles=""
if grep -q '^DD_API_KEY=.\+' .env.prod; then profiles="--profile datadog"; fi
for p in $extra_profiles; do profiles="$profiles --profile $p"; done
dc() { docker compose --env-file .env.prod -f docker-compose.yml -f docker-compose.prod.yml $profiles "$@"; }
dc pull --quiet
dc run --rm migrate
dc up -d --remove-orphans --wait --wait-timeout 180
# Caddy runs with `admin off` (no reload), so restart the gateway when its config changed.
if [ "$infra_changed" = 1 ]; then dc restart gateway; fi
dc ps
REMOTE
