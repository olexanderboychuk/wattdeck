#!/bin/sh
# The renderer's contract, with a committed fixture: it needs no terminal, so CI
# runs it. The image must be the size of the frame and must not be one colour.
set -eu
repo=$(cd "$(dirname "$0")/.." && pwd)
cap="$repo/tests/fixtures/frame_capture.txt"
out=$(mktemp -d); trap 'rm -rf "$out"' EXIT
python3 tools/render_frame.py "$cap" 'start' "$out/x.png"
[ -s "$out/x.png" ] || { echo "no PNG written"; exit 1; }
python3 - "$out/x.png" <<'EOF'
import sys
from PIL import Image
im = Image.open(sys.argv[1])
w, h = im.size
if w < 600 or h < 150:
    raise SystemExit(f"image too small: {w}x{h}")
# getcolors with a max equal to the pixel count is the non-deprecated way to
# enumerate the colours (Image.getdata is deprecated in Pillow 12+).
colours = im.convert("RGB").getcolors(w * h)
if colours is None or len(colours) < 2:
    raise SystemExit("the image is one flat colour")
EOF
echo "render_frame tests pass"
