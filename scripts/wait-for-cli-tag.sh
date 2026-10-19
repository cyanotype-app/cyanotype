#!/usr/bin/env bash
# Waits for the "CLI release tag" workflow run of a commit on main to finish, so the version of the CLI that the
# images for that commit carry is settled before they are built (docs/releases.md, "Why publish waits").
# Usage: wait-for-cli-tag.sh <sha>   (needs GH_TOKEN and GH_REPO; the token needs actions: read)
set -euo pipefail
sha=${1:?usage: wait-for-cli-tag.sh <sha>}
timeout=${TAG_WAIT_TIMEOUT:-900}   # seconds
interval=${TAG_WAIT_INTERVAL:-10}
deadline=$((SECONDS + timeout))
while :; do
  # Right after the push the run may not be registered yet: "not found" means keep waiting.
  read -r status conclusion url < <(gh run list --workflow cli-tag.yml --branch main --event push --commit "$sha" \
    --limit 1 --json status,conclusion,url --jq '.[0] // empty | [.status, (.conclusion // "-"), .url] | @tsv' \
    || true) || true
  if [[ "${status:-}" == completed ]]; then
    if [[ "$conclusion" == success ]]; then echo "CLI tag job finished for $sha: $url"; exit 0; fi
    echo "::error::CLI tag job for $sha finished with '$conclusion': $url"; exit 1
  fi
  if ((SECONDS >= deadline)); then
    echo "::error::Timed out after ${timeout}s waiting for the CLI tag job on $sha (status: ${status:-not found})"; exit 1
  fi
  echo "CLI tag job for $sha: ${status:-not started}; waiting ${interval}s"
  sleep "$interval"
done
