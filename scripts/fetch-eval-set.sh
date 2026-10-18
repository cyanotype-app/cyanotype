#!/usr/bin/env bash
# Download the search evaluation set (crates/cyanotype-clip/tests/eval/photos.tsv: 209 CC0 / public-domain photos from
# Wikimedia Commons, 330 px thumbnails) into a directory and check them.
#
#   scripts/fetch-eval-set.sh [DEST]      # default ./models/eval
#
# The evaluation (`SEARCH_EVAL_DIR=models/eval cargo test --release -p cyanotype-clip --test bakeoff -- --ignored --nocapture`)
# reads the files from there and checks each one's size and mean colour against photos.tsv (the thumbnail server's files
# differ byte for byte from one download to the next, in metadata only, so a hash of the bytes cannot pin them). Files that
# are already present are left alone; a download that is not a JPEG is deleted and fails the script.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
list="$here/crates/cyanotype-clip/tests/eval/photos.tsv"
dest="${1:-$here/models/eval}"
mkdir -p "$dest"
ua="cyanotype-eval-fetch/1.0 (https://github.com/cyanotype-app/cyanotype)"
n=0
while IFS=$'\t' read -r file _category _w _h _r _g _b _title url; do
  case "${file:-}" in ''|'#'*) continue ;; esac
  n=$((n + 1))
  if [ -s "$dest/$file" ]; then continue; fi
  tmp="$dest/.$file.part"
  curl --fail --silent --show-error --location --retry 5 --retry-delay 3 --user-agent "$ua" --output "$tmp" "$url"
  if [ "$(head -c 3 "$tmp" | od -An -tx1 | tr -d ' ')" != "ffd8ff" ]; then
    rm -f "$tmp"
    echo "error: $file is not a JPEG (from $url)" >&2
    exit 1
  fi
  mv "$tmp" "$dest/$file"
  sleep 0.3
done < "$list"
echo "evaluation set: $n photos in $dest"
