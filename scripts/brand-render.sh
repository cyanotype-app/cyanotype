#!/usr/bin/env bash
# Regenerate every raster brand asset (PNGs and favicon.ico) from the SVGs in docs/brand.
# Needs only Chromium's headless shell (the Playwright one is fine) and python3; no network.
#
#   CHROMIUM_PATH=/path/to/chrome-headless-shell scripts/brand-render.sh
#
# The wordmark SVGs are outlines of Geist; scripts/brand-outline.py regenerates those.
# The tagline of social-preview.png is real text, set in the Geist file apps/web already ships
# (run `pnpm install` in apps/web first).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
brand="$root/docs/brand"
pub="$root/apps/web/public"
chrome=${CHROMIUM_PATH:-$(ls -d "$HOME"/.cache/ms-playwright/chromium_headless_shell-*/*/chrome-headless-shell 2>/dev/null | head -1)}
[ -x "${chrome:-}" ] || { echo "set CHROMIUM_PATH to a chrome-headless-shell binary" >&2; exit 1; }
font="$root/apps/web/node_modules/@fontsource-variable/geist/files/geist-latin-wght-normal.woff2"
[ -f "$font" ] || { echo "missing $font (run pnpm install in apps/web)" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp "$brand"/*.svg "$tmp"/
cp "$font" "$tmp/geist.woff2"

# shoot <html-file> <out.png> <width> <height> [transparent]
shoot() {
  local bg=FFFFFFFF
  [ "${5:-}" = transparent ] && bg=00000000
  "$chrome" --no-sandbox --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
    --allow-file-access-from-files --default-background-color=$bg \
    --window-size="$3,$4" --screenshot="$2" "file://$1" >/dev/null 2>&1
}

# svg_png <svg> <out.png> <width> <height> [transparent]: the SVG scaled to fill the image
svg_png() {
  local page="$tmp/page-$(basename "$2").html"
  printf '<!doctype html><body style="margin:0"><img src="%s" width="%s" height="%s" style="display:block">' \
    "$1" "$3" "$4" >"$page"
  shoot "$page" "$2" "$3" "$4" "${5:-}"
}

# Web app icons: the rounded tile for "any", the full-bleed tile where the OS draws the shape.
svg_png icon.svg "$pub/icon-192.png" 192 192 transparent
svg_png icon.svg "$pub/icon-512.png" 512 512 transparent
svg_png tile.svg "$pub/icon-maskable-512.png" 512 512
svg_png tile.svg "$pub/apple-touch-icon.png" 180 180
for s in 16 32 48; do svg_png icon.svg "$tmp/ico-$s.png" $s $s transparent; done
python3 -I - "$tmp" "$pub/favicon.ico" <<'PY'
import struct, sys
from pathlib import Path
tmp, out = Path(sys.argv[1]), sys.argv[2]
sizes = (16, 32, 48)
pngs = [(s, (tmp / f"ico-{s}.png").read_bytes()) for s in sizes]
data = struct.pack("<HHH", 0, 1, len(pngs))
offset = 6 + 16 * len(pngs)
for s, png in pngs:
    data += struct.pack("<BBBBHHII", s, s, 0, 0, 1, 32, len(png), offset)
    offset += len(png)
Path(out).write_bytes(data + b"".join(png for _, png in pngs))
PY

# GitHub: org avatar (the mark stays well inside the centred circle GitHub crops to).
svg_png tile.svg "$brand/github-avatar.png" 1024 1024

# README header, transparent so it sits on either GitHub theme.
for v in light dark; do svg_png "readme-$v.svg" "$brand/readme-$v.png" 1900 400 transparent; done

# Social preview, 1280x640: wordmark and one line on paper, 40 px safe margin and then some.
cat >"$tmp/social.html" <<'HTML'
<!doctype html>
<style>
  @font-face { font-family: Geist; src: url(geist.woff2); font-weight: 100 900; }
  body { margin: 0; width: 1280px; height: 640px; background: #f4efe4; color: #003153;
         display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 44px; }
  img { width: 880px; display: block; }
  p { margin: 0; font: 500 40px/1 Geist; letter-spacing: -0.01em; opacity: 0.78; }
</style>
<img src="wordmark-ink.svg" alt="Cyanotype">
<p>Self-hosted photo library</p>
HTML
shoot "$tmp/social.html" "$brand/social-preview.png" 1280 640

echo "brand assets regenerated"
