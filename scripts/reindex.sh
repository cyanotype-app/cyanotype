#!/usr/bin/env bash
# Run `cyanotype reindex` on the production server over SSH, as a one-shot container (the Reindex workflow
# uses this; you can too). Prints the container's output; the last line of stdout is the JSON report.
#
# Usage: scripts/reindex.sh [reindex flags...]     e.g.  scripts/reindex.sh --mark-missing
#        scripts/reindex.sh --apply --rethumb
# With no --apply it is a dry run: nothing is written. --json is added for you.
# Env:
#   DEPLOY_SSH   ssh destination (user@host or a Host alias). Required.
#   DEPLOY_DIR   directory on the server (default cyanotype, relative to the login home)
# Uses the image version already in the server's .env.prod, so deploy a release that has `reindex` first.
# Never prints .env.prod; the container reads it on the server.
set -euo pipefail

: "${DEPLOY_SSH:?set DEPLOY_SSH (user@host or ssh alias)}"
dir="${DEPLOY_DIR:-cyanotype}"
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=scripts/lib-remote.sh
. "$here/lib-remote.sh"
dir="$(deploy_dir_normalize "$dir")" || exit 2

# Only known flags (and the --assign-to email) may reach the server's shell line.
args=(--json)
while [ $# -gt 0 ]; do
  case "$1" in
    --apply|--dry-run|--mark-missing|--rethumb|--backfill-metadata|--no-manifest) args+=("$1") ;;
    --assign-to)
      shift
      [[ "${1:-}" =~ ^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+$ ]] || { echo "--assign-to needs an email address" >&2; exit 2; }
      args+=(--assign-to "$1") ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
  shift
done

ssh_opts=(-o BatchMode=yes)
# shellcheck disable=SC2016  # the single-quoted script is expanded by the remote shell
remote_ssh bash -c '
set -euo pipefail
cd "$1"; shift
test -f .env.prod || { echo ".env.prod missing on the server" >&2; exit 1; }
profiles=""
if grep -q "^DD_API_KEY=.\+" .env.prod; then profiles="--profile datadog"; fi
# shellcheck disable=SC2086
exec docker compose --env-file .env.prod -f docker-compose.yml -f docker-compose.prod.yml $profiles run --rm -T reindex "$@"
' _ "$dir" "${args[@]}" </dev/null
