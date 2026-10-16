#!/usr/bin/env bash
# Vendors the map label glyphs (Noto Sans, SIL OFL 1.1) from protomaps/basemaps-assets into apps/web/public/map/fonts,
# so the map never calls a third-party CDN at runtime. Only the ranges that cover Latin, Greek, Cyrillic and
# Vietnamese labels are kept (about 2 MB in all); the basemap labels places in English where it can.
# Re-run to refresh; the commit is pinned.
set -euo pipefail

commit=028c18f713baecad011301ff7a69acc39bcc2ae7   # protomaps/basemaps-assets, 2025-10-31
ranges=(0-255 256-511 512-767 768-1023 1024-1279 7680-7935 8192-8447)
out="$(cd "$(dirname "$0")/.." && pwd)/apps/web/public/map/fonts"

for font in "Noto Sans Regular" "Noto Sans Medium" "Noto Sans Italic"; do
  mkdir -p "$out/$font"
  for r in "${ranges[@]}"; do
    curl -sSfL --retry 3 -o "$out/$font/$r.pbf" \
      "https://raw.githubusercontent.com/protomaps/basemaps-assets/$commit/fonts/${font// /%20}/$r.pbf"
  done
done
curl -sSfL --retry 3 -o "$out/OFL.txt" "https://raw.githubusercontent.com/protomaps/basemaps-assets/$commit/fonts/OFL.txt"
du -sh "$out"
