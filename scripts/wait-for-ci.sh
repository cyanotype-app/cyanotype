#!/usr/bin/env bash
# Waits for the CI workflow run of a commit on main and fails unless it succeeded.
# Usage: wait-for-ci.sh <sha>   (needs GH_TOKEN and GH_REPO, or run inside a checkout; token needs actions: read)
set -euo pipefail
sha=${1:?usage: wait-for-ci.sh <sha>}
timeout=${CI_WAIT_TIMEOUT:-2400}   # seconds
interval=${CI_WAIT_INTERVAL:-30}
deadline=$((SECONDS + timeout))
while :; do
  # Latest run first; CI may not be registered yet right after the push, so "none" means keep waiting.
  read -r status conclusion url < <(gh run list --workflow ci.yml --branch main --event push --commit "$sha" \
    --limit 1 --json status,conclusion,url --jq '.[0] // empty | [.status, (.conclusion // "-"), .url] | @tsv' \
    || true) || true
  if [[ "${status:-}" == completed ]]; then
    if [[ "$conclusion" == success ]]; then echo "CI succeeded for $sha: $url"; exit 0; fi
    echo "::error::CI for $sha finished with '$conclusion', refusing to deploy: $url"; exit 1
  fi
  if ((SECONDS >= deadline)); then
    echo "::error::Timed out after ${timeout}s waiting for CI on $sha (status: ${status:-not found})"; exit 1
  fi
  echo "CI for $sha: ${status:-not started}; waiting ${interval}s"
  sleep "$interval"
done
