#!/usr/bin/env bash
# Download the dog and cat evaluation set (crates/cyanotype-pets/tests/eval/pets.tsv: public-domain and CC0 photos from Wikimedia
# Commons, labelled by eye: dogs, cats, both, and things that are not pets) into a directory and verify them. Needs curl and jq.
#
#   scripts/fetch-pets-eval-set.sh [DEST]      # default ./models/pets-eval
#
# pets.tsv names each file by its Commons page id, not by title or URL (so no name is in the repository): the script asks the
# Commons API for the original file's URL of each id (batches of 50), then downloads it. Each file is the original upload, not a
# thumbnail, so its sha256 is stable and pinned in pets.tsv: a download that does not match is deleted and fails the script,
# and nothing unverified is left in DEST. Files that are already there with the right checksum are left alone, so re-running is
# cheap. Treat the files as untrusted input like any download: they are only ever decoded by the JPEG decoder of the `image` crate.
#
# The bake-off (`PETS_EVAL_DIR=models/pets-eval cargo test --release -p cyanotype-pets --test bakeoff -- --ignored --nocapture`,
# docs/pets.md) reads the files from DEST.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
list="$here/crates/cyanotype-pets/tests/eval/pets.tsv"
dest="${1:-$here/models/pets-eval}"
mkdir -p "$dest"
ua="cyanotype-eval-fetch/1.0 (https://github.com/cyanotype-app/cyanotype; pets evaluation set, ~200 files)"
sha256_of() { sha256sum "$1" | cut -d' ' -f1; }

# Rows that still need a download: "file sha256 pageid".
todo=()
n=0
while IFS=$'\t' read -r file _label _group _note _w _h _bytes sum _licence pageid; do
  case "${file:-}" in ''|'#'*) continue ;; esac
  n=$((n + 1))
  if [ -f "$dest/$file" ] && [ "$(sha256_of "$dest/$file")" = "$sum" ]; then continue; fi
  todo+=("$file $sum $pageid")
done < "$list"

declare -A url_of
resolve() { # page ids (at most 50) -> url_of[pageid]
  local ids
  ids="$(IFS='|'; echo "$*")"
  local json
  json="$(curl --fail --silent --show-error --location --retry 5 --retry-delay 3 --user-agent "$ua" --get \
    --data-urlencode "action=query" --data-urlencode "format=json" --data-urlencode "prop=imageinfo" \
    --data-urlencode "iiprop=url|sha1|size" --data-urlencode "pageids=$ids" \
    "https://commons.wikimedia.org/w/api.php")"
  while IFS=$'\t' read -r id url; do
    url_of[$id]="$url"
  done < <(printf '%s' "$json" | jq -r '.query.pages[] | select(.imageinfo) | [(.pageid|tostring), .imageinfo[0].url] | @tsv')
}

ids=()
for row in "${todo[@]}"; do
  read -r _f _s id <<< "$row"
  ids+=("$id")
done
for ((i = 0; i < ${#ids[@]}; i += 50)); do
  resolve "${ids[@]:i:50}"
  sleep 1
done

for row in "${todo[@]}"; do
  read -r file sum id <<< "$row"
  url="${url_of[$id]:-}"
  if [ -z "$url" ]; then
    echo "error: Commons page id $id ($file) has no file any more" >&2
    exit 1
  fi
  tmp="$dest/.$file.part"
  curl --fail --silent --show-error --location --retry 5 --retry-delay 3 --user-agent "$ua" --output "$tmp" "$url"
  got="$(sha256_of "$tmp")"
  if [ "$got" != "$sum" ]; then
    rm -f "$tmp"
    echo "error: $file (page id $id) has sha256 $got, expected $sum (pinned in pets.tsv)" >&2
    exit 1
  fi
  mv "$tmp" "$dest/$file"
  sleep 0.5
done
ok=0
while IFS=$'\t' read -r file _l _g _n _w _h _b sum _lic _id; do
  case "${file:-}" in ''|'#'*) continue ;; esac
  [ "$(sha256_of "$dest/$file")" = "$sum" ] && ok=$((ok + 1))
done < "$list"
echo "pets evaluation set: $ok of $n photos verified in $dest"
[ "$ok" -eq "$n" ]
