#!/usr/bin/env python3
"""Screen every kept frame for the owner's desktop notification colors."""
import argparse
from pathlib import Path
import subprocess

import numpy as np
from PIL import Image

REGION = (860, 20, 1270, 160)
REGIONS = {"upper-right": REGION, "lower-right": (860, 650, 1280, 800)}
PANEL, BUTTON = (10, 10, 10), (27, 27, 27)


def score(pixels, tolerance):
    pixels = pixels.astype(np.int16)
    return tuple(np.mean(np.all(np.abs(pixels - color) <= tolerance, axis=-1))
                 for color in (PANEL, BUTTON))


def scan(path, region=REGION):
    if path.suffix.lower() == ".png":
        with Image.open(path) as image:
            yield "still", score(np.asarray(image.convert("RGB").crop(region)), 0)
        return
    left, top, right, bottom = region
    width, height = right - left, bottom - top
    size = width * height * 3
    with subprocess.Popen([
        "ffmpeg", "-nostdin", "-loglevel", "error", "-threads", "2", "-filter_threads", "1",
        "-i", path, "-map", "0:v:0", "-vf", f"crop={width}:{height}:{left}:{top}:exact=1",
        "-pix_fmt", "rgb24", "-fps_mode", "passthrough", "-f", "rawvideo", "pipe:1",
    ], stdout=subprocess.PIPE) as decoder:
        n = 0
        while data := decoder.stdout.read(size):
            if len(data) != size:
                raise RuntimeError(f"truncated decoded frame in {path}")
            yield f"frame={n}", score(np.frombuffer(data, dtype=np.uint8).reshape(-1, 3), 4)
            n += 1
        if decoder.wait() or not n:
            raise RuntimeError(f"could not inspect decoded frames in {path}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inputs", nargs="+", type=Path)
    parser.add_argument("--comparison", action="store_true", help="screen each of the three 2×2 video tiles")
    args = parser.parse_args()
    flagged = checked = 0
    regions = list(REGIONS.items())
    if args.comparison:
        from compose import W, H

        regions = []
        for app, x, y in (("iced", 0, 0), ("gpui", W, 0), ("rune", 0, H)):
            for name, region in REGIONS.items():
                left, top, right, bottom = (v // 2 for v in region)
                regions.append((f"{app}/{name}", (x + left, y + top, x + right, y + bottom)))
    for path in args.inputs:
        for name, region in regions:
            frames = 0
            for label, (panel, button) in scan(path, region):
                if panel > 0.25 and button > 0.02:
                    flagged += 1
                    print(f"FLAG {path} {name} {label} panel={panel:.2f} button={button:.2f}")
                frames += 1
            if path.suffix.lower() != ".png":
                print(f"{path} ({name}): {frames} decoded frames checked")
        checked += frames
    print(f"{flagged} flagged across {checked} frames")
