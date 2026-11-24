#!/usr/bin/env python3
"""Contact sheets for privacy review: every still, and video frames sampled once per second.

Desktop notifications appear near the top-right corner of a headless output, so each tile shows the
whole downscaled frame plus an enlarged top-right corner next to it.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

from PIL import Image, ImageDraw

TILE = (320, 200)
CORNER = (480, 0, 1280, 250)  # x0, y0, x1, y1 on a 1280×800 frame
COLUMNS = 4
PER_SHEET = 24


def frames(paths, workdir):
    for path in paths:
        if path.suffix.lower() == ".png":
            yield path.name, path
            continue
        out = workdir / path.stem
        out.mkdir()
        subprocess.run(["ffmpeg", "-nostdin", "-loglevel", "error", "-i", str(path), "-vf", "fps=1",
                        str(out / "%04d.png")], check=True)
        for frame in sorted(out.glob("*.png")):
            yield f"{path.stem} t={int(frame.stem) - 1}s", frame


def tile(label, path):
    with Image.open(path) as image:
        image = image.convert("RGB")
        whole = image.resize(TILE)
        corner = image.crop(CORNER).resize((TILE[0], TILE[0] * 250 // 800))
    cell = Image.new("RGB", (TILE[0] * 2 + 6, TILE[1] + 18), "white")
    cell.paste(whole, (0, 18))
    cell.paste(corner, (TILE[0] + 6, 18))
    ImageDraw.Draw(cell).text((2, 2), label, fill="black")
    return cell


def main(args):
    args.out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as directory:
        cells = [tile(label, path) for label, path in frames(args.inputs, Path(directory))]
    for n, start in enumerate(range(0, len(cells), PER_SHEET)):
        chunk = cells[start:start + PER_SHEET]
        w, h = chunk[0].size
        rows = (len(chunk) + COLUMNS - 1) // COLUMNS
        sheet = Image.new("RGB", (w * COLUMNS, h * rows), "#888888")
        for i, cell in enumerate(chunk):
            sheet.paste(cell, ((i % COLUMNS) * w, (i // COLUMNS) * h))
        sheet.save(args.out / f"sheet-{n:02d}.png")
    print(f"{len(cells)} frames on {n + 1} sheets in {args.out}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inputs", nargs="+", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    main(parser.parse_args())
