#!/usr/bin/env bash
# Rebuilds apps/cli/data/places.tsv.gz, the offline reverse-geocoding dataset that `cyanotype migrate` loads into
# Postgres (docs/library.md, "Places"). Maintainers run this occasionally; deploys never download anything.
#
# Source: GeoNames (https://www.geonames.org/), licensed CC BY 4.0: cities1000 (every populated place with more
# than 1000 inhabitants), admin1CodesASCII (region names) and countryInfo (country names). The dumps are
# regenerated daily and are not versioned, so the snapshot is committed instead of fetched at deploy time; this
# script records the snapshot's date and the sha256 of each source file in the header of the output.
#
#   scripts/build-places-dataset.sh            # download, convert, write apps/cli/data/places.tsv.gz
#
# Output format (tab separated, gzip):
#   #version <geonames Last-Modified date of cities1000>
#   #sources <sha256 of each source>
#   C<TAB>cc<TAB>country name
#   A<TAB>cc<TAB>admin1 code<TAB>region name
#   P<TAB>name<TAB>lat<TAB>lon<TAB>cc<TAB>admin1 code<TAB>population
set -euo pipefail

out="$(cd "$(dirname "$0")/.." && pwd)/apps/cli/data/places.tsv.gz"
base=https://download.geonames.org/export/dump
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

for f in cities1000.zip admin1CodesASCII.txt countryInfo.txt; do
  curl -sSfL --retry 3 -o "$tmp/$f" "$base/$f"
done
modified="$(curl -sSfI "$base/cities1000.zip" | tr -d '\r' | awk -F': ' 'tolower($1)=="last-modified"{print $2}')"
version="$(date -u -d "$modified" +%Y-%m-%d)"
unzip -oq "$tmp/cities1000.zip" cities1000.txt -d "$tmp"
sums="$(cd "$tmp" && sha256sum cities1000.zip admin1CodesASCII.txt countryInfo.txt | awk '{printf "%s%s", sep, substr($1,1,16); sep=","}')"

{
  printf '#version\t%s\n#sources\t%s\n' "$version" "$sums"
  # countryInfo: ISO code (col 1) and name (col 5).
  awk -F'\t' '!/^#/ && $1 != "" {printf "C\t%s\t%s\n", $1, $5}' "$tmp/countryInfo.txt"
  # admin1CodesASCII: "CC.code<TAB>name<TAB>asciiname<TAB>id". The ASCII name keeps the output readable everywhere.
  awk -F'\t' '{split($1, k, "."); printf "A\t%s\t%s\t%s\n", k[1], k[2], $2}' "$tmp/admin1CodesASCII.txt"
  # cities1000: name (2), lat (5), lon (6), country (9), admin1 (11), population (15). Coordinates to 3 decimals
  # (~100 m), far finer than the nearest-place match needs.
  awk -F'\t' '{printf "P\t%s\t%.3f\t%.3f\t%s\t%s\t%s\n", $2, $5, $6, $9, $11, $15}' "$tmp/cities1000.txt"
} | LC_ALL=C gzip -9n > "$out"

echo "wrote $out ($(du -h "$out" | cut -f1)), GeoNames snapshot $version"
