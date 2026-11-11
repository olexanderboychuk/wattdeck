#!/usr/bin/env python3
"""Paint one PTY frame as a PNG: dark grid, one glyph per cell.

    render_frame.py <capture> <frame-label> <out.png> [--font <path>]

The capture is the probe's stdout: `===== label =====` blocks of the text
ratatui drew. This paints that text in a monospace font, so the image is the
screen the terminal showed. The font must cover the box-drawing and block
glyphs ratatui uses; if no font is found, the renderer refuses.
"""
import argparse, os, sys, unicodedata

from PIL import Image, ImageDraw, ImageFont

BACKGROUND = (16, 18, 20)
FOREGROUND = (214, 218, 224)
BORDER = 12

CANDIDATES = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/google-noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "/Library/Fonts/Menlo.ttc",
    "C:/Windows/Fonts/consola.ttf",
]


def pick_font(path):
    if path:
        if not os.path.isfile(path):
            sys.exit(f"font not found: {path}")
        return path
    for c in CANDIDATES:
        if os.path.isfile(c):
            return c
    sys.exit(
        "no monospace font found; pass --font <path>. Looked for: "
        + ", ".join(CANDIDATES)
    )


def frames(path):
    label, rows = None, []
    out = {}
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if line.startswith("===== ") and line.endswith(" ====="):
            if label:
                out[label] = rows
            label, rows = line[6:-6], []
        elif label is not None:
            rows.append(line)
    if label:
        out[label] = rows
    return out


def cell_width(ch):
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("capture")
    ap.add_argument("label")
    ap.add_argument("out")
    ap.add_argument("--font", default="")
    ap.add_argument("--size", type=int, default=14)
    a = ap.parse_args()

    all_frames = frames(a.capture)
    if a.label not in all_frames:
        sys.exit(f"no frame labelled {a.label}; labels: {', '.join(all_frames)}")
    rows = [r for r in all_frames[a.label] if r.strip()]
    if not rows:
        sys.exit(f"frame {a.label} is empty")

    font = ImageFont.truetype(pick_font(a.font), a.size)
    cells = [font.getlength(ch) for ch in "wm"]
    cw = max(cells)
    asc, desc = font.getmetrics()
    lh = asc + desc

    width = max(sum(cell_width(c) * cw for c in r) for r in rows)
    img = Image.new("RGB", (int(width + 2 * BORDER), int(len(rows) * lh + 2 * BORDER)), BACKGROUND)
    draw = ImageDraw.Draw(img)
    for y, row in enumerate(rows):
        x = BORDER
        for ch in row:
            if ch != " ":
                draw.text((x, BORDER + y * lh), ch, fill=FOREGROUND, font=font)
            x += cell_width(ch) * cw
    img.save(a.out)
    print(f"{a.out} {img.size[0]}x{img.size[1]} from frame {a.label} ({len(rows)} rows, font {a.font or pick_font('')})")


main()
