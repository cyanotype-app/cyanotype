# Cyanotype brand

The mark is a **C open to the right with a sun in the middle**. These files are the source of
truth; everything raster is generated from them.

## Colours

| Name | Hex | Use |
| --- | --- | --- |
| Ink (Prussian blue) | `#003153` | the mark and wordmark on light, tile background |
| Paper | `#f4efe4` | the mark on an ink tile, light backgrounds |
| Sky | `#86b4ea` | the mark on dark (the app's dark accent) |
| Night | `#06090d` | dark tile background (the app's dark page colour) |

In the web app the mark is drawn in `currentColor` and takes the accent: `#1d4f91` in light,
`#86b4ea` in dark. The wordmark text is set in Geist 600, letter-spacing -0.02em.

## Files

| File | For |
| --- | --- |
| `mark.svg` | the mark alone in `currentColor`: inline it, or colour it with CSS |
| `mark-ink.svg`, `mark-dark.svg` | the mark in a fixed colour (on light, on dark) |
| `tile.svg`, `tile-dark.svg` | paper mark on an ink tile, sky mark on a night tile; full bleed, square |
| `icon.svg` | the tile with rounded corners; the favicon (also copied to `apps/web/public/icon.svg`) |
| `wordmark.svg` | mark and "Cyanotype" in `currentColor` |
| `wordmark-ink.svg`, `wordmark-dark.svg` | the wordmark in fixed colours (on light, on dark) |
| `readme-light.svg/png`, `readme-dark.svg/png` | the README header, with extra margin, transparent |
| `github-avatar.png` | 1024 x 1024 organisation avatar; the mark sits well inside the circle GitHub crops to |
| `social-preview.png` | 1280 x 640 repository social preview |

The web app's `favicon.ico` (16, 32, 48), `icon-192.png`, `icon-512.png`, `icon-maskable-512.png`
(full bleed, mark inside the 80% safe zone) and `apple-touch-icon.png` (180, opaque) are in
`apps/web/public`.

All SVG text is converted to outlines, so the files render the same everywhere without Geist.

## Regenerating

- `scripts/brand-render.sh` rebuilds every PNG and the ICO from the SVGs (Chromium's headless
  shell and python3 only).
- `scripts/brand-outline.py` rebuilds the wordmark SVGs (the outlined text) when the layout changes.

Change the SVGs, run the scripts, commit both.

## Using it

- Clear space: keep at least the height of the C's centre dot (about 20% of the mark's height)
  free on every side; for the wordmark, the height of the "C" in the text.
- Minimum size: the mark 16 px high, the wordmark 96 px wide. Below that use the tile icon.
- Do use the mark in `currentColor`, ink on light and sky on dark, and the supplied tile for app icons.
- Don't recolour outside the palette, add gradients, shadows or outlines, rotate or flip the mark,
  close the C, stretch it, or set "Cyanotype" in another font next to it.
