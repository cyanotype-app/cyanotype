#!/usr/bin/env bash
# Download the OCR bake-off images (crates/cyanotype-ocr/tests/bakeoff/images.tsv: 38 text photos, and qr.tsv: 6 photos with QR
# codes; all CC0 / public domain from Wikimedia Commons, 1024 px or 1280 px thumbnails) into a directory and check them.
#
#   scripts/fetch-ocr-set.sh [DEST]      # default ./models/ocr-set
#
# The bake-off (`OCR_SET=<DEST> cargo run --release -p cyanotype-ocr --example bakeoff`, docs/build.md) reads the files from there.
# Files already present are left alone; a download that is not a JPEG or PNG is deleted and fails the script. The thumbnail
# server does not serve byte-identical files twice (metadata differs, pixels do not), so files are not pinned by hash. Commons
# asks for a polite pace: a pause between files, and a longer wait after a 429.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
dir="$here/crates/cyanotype-ocr/tests/bakeoff"
dest="${1:-$here/models/ocr-set}"
mkdir -p "$dest"
ua="cyanotype-ocr-eval-fetch/1.0 (research; contact via the project repository)"
n=0
fetch() { # file url
  local file="$1" url="$2" tmp="$dest/.$1.part" try
  n=$((n + 1))
  [ -s "$dest/$file" ] && return 0
  for try in 1 2 3 4 5 6; do
    if curl --fail --silent --show-error --location --user-agent "$ua" --output "$tmp" "$url"; then
      break
    fi
    rm -f "$tmp"
    [ "$try" = 6 ] && { echo "error: could not fetch $file from $url" >&2; exit 1; }
    sleep $((8 * try))
  done
  case "$(head -c 4 "$tmp" | od -An -tx1 | tr -d ' ')" in
    ffd8ff*|89504e47) ;;
    *) rm -f "$tmp"; echo "error: $file is not a JPEG or PNG (from $url)" >&2; exit 1 ;;
  esac
  mv "$tmp" "$dest/$file"
  sleep 2.5
}
while IFS=$'\t' read -r file _cat _title _lic url _words; do
  case "${file:-}" in ''|'#'*) continue ;; esac
  fetch "$file" "$url"
done < "$dir/images.tsv"
while IFS=$'\t' read -r file _title _lic url _present _payloads; do
  case "${file:-}" in ''|'#'*) continue ;; esac
  fetch "$file" "$url"
done < "$dir/qr.tsv"
echo "OCR bake-off set: $n images in $dest"
