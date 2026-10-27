#!/usr/bin/env -S python3 -I
"""Turn the word "Cyanotype" (Geist 600, letter-spacing -0.02em) into SVG outlines and
write docs/brand/wordmark*.svg and readme-*.svg. Stdlib only: Chromium prints the text to a
PDF (Skia embeds a variable font as Type 3 glyph paths at the exact weight), and the glyph
outlines are read back out of the PDF. Run it only when the wordmark layout changes:

    scripts/brand-outline.py <chrome-headless-shell> <geist-latin-wght-normal.woff2> <out-dir>

Geist is SIL OFL; outlines of it may be redistributed.
"""
import re
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

chrome, font, out = sys.argv[1], Path(sys.argv[2]).resolve(), Path(sys.argv[3])
WORD = "Cyanotype"

with tempfile.TemporaryDirectory() as tmp:
    tmp = Path(tmp)
    (tmp / "g.woff2").write_bytes(font.read_bytes())
    (tmp / "t.html").write_text(
        "<style>@font-face{font-family:G;src:url(g.woff2);font-weight:100 900}"
        "@page{size:7000px 1500px;margin:0}body{margin:0}"
        f"p{{margin:0;font:600 1000px/1500px G;letter-spacing:-0.02em;white-space:nowrap}}</style><p>{WORD}</p>"
    )
    subprocess.run(
        [chrome, "--no-sandbox", "--allow-file-access-from-files", "--no-pdf-header-footer",
         f"--print-to-pdf={tmp / 't.pdf'}", f"file://{tmp / 't.html'}"],
        check=True, capture_output=True,
    )
    pdf = (tmp / "t.pdf").read_bytes()

objs = {}
for m in re.finditer(rb"(\d+) 0 obj\s*(<<.*?>>)?\s*(?:stream\r?\n(.*?)endstream)?", pdf, re.S):
    body = m.group(3)
    if body is not None:
        try:
            body = zlib.decompress(body)
        except zlib.error:
            pass
    objs[int(m.group(1))] = body

text = next(b for b in objs.values() if b and b"BT" in b).decode("latin1")
procs = dict(re.findall(r"/(g[0-9A-F]*) (\d+) 0 R", re.search(r"/CharProcs <<(.*?)>>", pdf.decode("latin1"), re.S).group(1)))
shown = re.findall(r"(?:(-?[\d.]+) 0 Td )?<([0-9A-F]{2})> Tj", text)


def outline(code, dx):
    """Glyph path (font units, y down, baseline 0) shifted right by dx."""
    name = "g" + (code.lstrip("0") or "0")
    src = objs[int(procs[name])].decode("latin1")
    d = []
    for line in src.split("\n")[1:]:
        t = line.split()
        if not t or t[-1] in ("f", "d1"):
            continue
        op, v = t[-1], [float(n) for n in t[:-1]]
        if op == "h":
            d.append("Z")
            continue
        v = [round(c + (dx if i % 2 == 0 else 0), 1) for i, c in enumerate(v)]
        d.append({"m": "M", "l": "L", "c": "C"}[op] + " ".join(f"{c:g}" for c in v))
    return "".join(d)


# The mark is scaled so its visible height is 1.3 x the cap height (710) and centred on the cap
# height. Its visible right edge (the tip of the upper cap, x = 80 in mark units) sits
# 0.35 x cap height from the ink of the "C".
S = 1230 / 100
mark_tx, mark_ty = -12.5 * S, -355 - 50 * S
CAP, C_BEARING = 710, 39
x = 80 * S + mark_tx + 0.35 * CAP - C_BEARING
paths = []
for td, code in shown:
    x += float(td) if td else 0
    paths.append(outline(code, x))
right = x + 552  # right edge of the final "e"
pad = 10
top = mark_ty + 12.5 * S - pad
bottom = 150 + pad  # descender of y and p
view = (-pad, round(top), round(right + 2 * pad), round(bottom - top))

MARK = ('<path d="M73.5 28.5 A31 31 0 1 0 73.5 71.5" fill="none" stroke="{c}" stroke-width="13" stroke-linecap="round"/>'
        '<circle cx="50" cy="50" r="10.5" fill="{c}"/>')


def wordmark(mark_c, text_c, extra=0):
    vx, vy, vw, vh = view
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vx - extra} {vy - extra} {vw + 2 * extra} {vh + 2 * extra}" role="img" aria-label="Cyanotype">\n'
            f'  <g transform="translate({mark_tx:g} {mark_ty:g}) scale({S:g})">{MARK.format(c=mark_c)}</g>\n'
            f'  <path fill="{text_c}" d="{"".join(paths)}"/>\n</svg>\n')


INK, PAPER, SKY = "#003153", "#f4efe4", "#86b4ea"
files = {
    "wordmark.svg": wordmark("currentColor", "currentColor"),
    "wordmark-ink.svg": wordmark(INK, INK),
    "wordmark-dark.svg": wordmark(SKY, PAPER),
    "readme-light.svg": wordmark(INK, INK, 180),
    "readme-dark.svg": wordmark(SKY, PAPER, 180),
}
for name, svg in files.items():
    (out / name).write_text(svg)
print("viewBox", view)
