#!/usr/bin/env python3
"""Converts the artwork rendered by `FLASHVITA_RENDER_ASSETS` into the 8-bit
palette PNGs the Vita's LiveArea requires, in emulator/static/vita/sce_sys.

    cd emulator && FLASHVITA_RENDER_ASSETS=/tmp/art cargo run
    python3 tools/make_livearea.py /tmp/art
"""
import sys
from pathlib import Path

from PIL import Image

OUT = Path(__file__).resolve().parent.parent / "emulator/static/vita/sce_sys"
TARGETS = {
    "icon0.png": OUT / "icon0.png",
    "pic0.png": OUT / "pic0.png",
    "bg0.png": OUT / "livearea/contents/bg0.png",
    "startup.png": OUT / "livearea/contents/startup.png",
}

src = Path(sys.argv[1])
for name, dst in TARGETS.items():
    img = Image.open(src / name).convert("RGB")
    pal = img.quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.FLOYDSTEINBERG)
    pal.save(dst, optimize=True)
    print(f"{dst.relative_to(OUT.parent.parent.parent)}  {pal.size[0]}x{pal.size[1]}  {dst.stat().st_size} bytes")
