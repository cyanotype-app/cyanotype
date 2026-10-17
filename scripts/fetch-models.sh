#!/usr/bin/env bash
# Download the CLIP model files listed in models/clip.lock, verify their sha256, and install them into a directory.
#
#   scripts/fetch-models.sh                              # all files into ./models/clip (what `just dev` and the tests use)
#   scripts/fetch-models.sh DEST [FILE...]               # a directory, and optionally only some of the files
#   LOCK=path/to/clip.lock scripts/fetch-models.sh ...   # another lock file
#
# Files that are already there with the right checksum are left alone, so re-running is cheap. A download that does not
# match its pinned checksum is deleted and fails the script: nothing unverified is ever left in DEST. The Dockerfile runs
# this same script, so the images and a developer checkout get byte-identical models.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
lock="${LOCK:-$here/models/clip.lock}"
dest="${1:-$here/models/clip}"
shift || true
want=("$@")

sha256_of() { sha256sum "$1" | cut -d' ' -f1; }

mkdir -p "$dest"
found=0
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
