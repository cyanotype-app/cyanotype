#!/usr/bin/env bash
# Download the search model files listed in models/<name>.lock (clip, siglip2), verify their sha256, and install them into a
# directory.
#
#   scripts/fetch-models.sh                              # every model into ./models/<name> (what `just models` and the tests use)
#   MODELS=siglip2 scripts/fetch-models.sh               # only some of them (space separated names)
#   LOCK=models/siglip2.lock scripts/fetch-models.sh DEST [FILE...]    # one lock file into a directory, optionally only some files
#
# Files that are already there with the right checksum are left alone, so re-running is cheap. A download that does not
# match its pinned checksum is deleted and fails the script: nothing unverified is ever left in DEST. The Dockerfile runs
# this same script, so the images and a developer checkout get byte-identical models.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"

sha256_of() { sha256sum "$1" | cut -d' ' -f1; }

fetch_lock() {
  local lock="$1" dest="$2"
  shift 2
  local want=("$@") found=0 sum file url tmp got
  mkdir -p "$dest"
  while read -r sum file url; do
    case "${sum:-}" in ''|'#'*) continue ;; esac
    if [ ${#want[@]} -gt 0 ] && ! printf '%s\n' "${want[@]}" | grep -qx "$file"; then continue; fi
    found=$((found + 1))
    if [ -f "$dest/$file" ] && [ "$(sha256_of "$dest/$file")" = "$sum" ]; then
      echo "ok       $file"
      continue
    fi
    echo "fetching $file"
    tmp="$dest/.$file.part"
    curl --fail --silent --show-error --location --retry 3 --retry-delay 2 --output "$tmp" "$url"
    got="$(sha256_of "$tmp")"
    if [ "$got" != "$sum" ]; then
      rm -f "$tmp"
      echo "error: $file has sha256 $got, expected $sum (pinned in $lock)" >&2
      exit 1
    fi
    mv "$tmp" "$dest/$file"
  done < "$lock"
  if [ "$found" -eq 0 ]; then
    echo "error: none of the requested files (${want[*]:-all}) are in $lock" >&2
    exit 1
  fi
}

if [ -n "${LOCK:-}" ]; then
  dest="${1:?usage: LOCK=models/<name>.lock fetch-models.sh DEST [FILE...]}"
  shift
  fetch_lock "$LOCK" "$dest" "$@"
else
  for name in ${MODELS:-clip siglip2}; do
    echo "== $name"
    fetch_lock "$here/models/$name.lock" "${1:-$here/models}/$name"
  done
fi
