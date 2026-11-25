"""Extract every quarter-second frame from each export for a visual review."""
import argparse
from pathlib import Path
import subprocess

from PIL import Image, ImageDraw

import data

SIZE = (384, 216)
COLUMNS, ROWS, CAPTION = 4, 6, 17
CAPACITY = COLUMNS * ROWS


def frames(video):
    # The render verifier requires 60 fps. Select frame numbers directly so the
    # samples are from 0.00 s, 0.25 s, 0.50 s, … on the exported video timeline.
    args = ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-xerror",
            "-threads", "2", "-i", str(video), "-filter_threads", "2", "-vf",
            r"select=not(mod(n\,15)),scale=384:216", "-fps_mode", "passthrough",
            "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"]
    with subprocess.Popen(args, stdout=subprocess.PIPE) as process:
        while raw := process.stdout.read(SIZE[0] * SIZE[1] * 3):
            yield Image.frombytes("RGB", SIZE, raw)
        if process.wait():
            raise subprocess.CalledProcessError(process.returncode, args)


def create_sheets(video, destination):
    destination.mkdir(parents=True, exist_ok=True)
    cell_width, cell_height = SIZE[0], SIZE[1] + CAPTION
    count = 0
    for i, image in enumerate(frames(video)):
        index = i % CAPACITY
        if not index:
            sheet = Image.new("RGB", (COLUMNS * cell_width, ROWS * cell_height), "#10151F")
            draw = ImageDraw.Draw(sheet)
        x, y = index % COLUMNS * cell_width, index // COLUMNS * cell_height
        draw.text((x + 8, y + 1), f"{video.stem}  {i / 4:.2f} s", fill="white")
        sheet.paste(image, (x, y + CAPTION))
        if index == CAPACITY - 1:
            sheet.save(destination / f"{i // CAPACITY + 1:02d}.jpg", quality=87)
        count = i + 1
    if not count:
        raise ValueError(f"No frames in {video}")
    if count % CAPACITY:
        sheet.save(destination / f"{(count - 1) // CAPACITY + 1:02d}.jpg", quality=87)
    print(f"{video.stem}: {count} sampled frames", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=data.OUT.parent / "quarter-second-audit")
    parser.add_argument("names", nargs="*", help="Video names to audit, for example SideBySideFly (default: all)")
    args = parser.parse_args()
    videos = [data.OUT / f"{name}.mp4" for name in args.names] or sorted(data.OUT.glob("*.mp4"))
    for video in videos:
        create_sheets(video, args.out / video.stem)


if __name__ == "__main__":
    main()
