"""Quarter-second review sheets for each export, and the QA gates that fail a render.

Gates (a)–(e) run inside the Manim render at every 0.25 s sample (scenes.Page.gate) and leave a
summary in .cache/viz/gates/. gates() checks those summaries and gate (f): background pixels
outside the glass decode to pure #000 at every 0.25 s sample of every export.
"""
import json
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
            sheet = Image.new("RGB", (COLUMNS * cell_width, ROWS * cell_height), "#202020")
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


EXPLAINERS = ("FramePath", "FirstFrame", "ScrollCost", "Memory", "Hiccups")
# Background points along the outer margin. The grid has a narrower border than the cards.
MARGIN_POINTS = ((20, 20), (1900, 20), (20, 1060), (1900, 1060), (960, 12), (960, 1068), (12, 540), (1908, 540))
GRID_POINTS = ((8, 8), (1911, 8), (8, 1071), (1911, 1071), (8, 300), (1911, 820))


def full_frames(video):
    args = ["nice", "-n", "19", "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-xerror",
            "-threads", "2", "-i", str(video), "-filter_threads", "2", "-vf", r"select=not(mod(n\,15))",
            "-fps_mode", "passthrough", "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"]
    size = 1920 * 1080 * 3
    with subprocess.Popen(args, stdout=subprocess.PIPE) as process:
        while raw := process.stdout.read(size):
            yield raw
        if process.wait():
            raise subprocess.CalledProcessError(process.returncode, args)


def gates(names=None):
    """Return a list of failures across the gate summaries and the pure-black background check."""
    failures = []
    report = {}
    gate_dir = data.OUT.parent / "gates"
    for name in EXPLAINERS:
        if names and name not in names:
            continue
        summary = gate_dir / f"{name}.json"
        video = data.OUT / f"{name}.mp4"
        if not summary.is_file():
            failures.append(f"{name}: no gate summary (render with VIZ_AUDIT_TEXT=1)")
            continue
        stats = json.loads(summary.read_text())
        if not stats.get("passed") or not stats.get("samples"):
            failures.append(f"{name}: gate summary did not pass: {stats}")
        if video.is_file() and summary.stat().st_mtime < video.stat().st_mtime - 3600:
            failures.append(f"{name}: gate summary is older than the export")
        report[name] = stats
    for video in sorted(data.OUT.glob("*.mp4")):
        if names and video.stem not in names:
            continue
        points = GRID_POINTS if video.stem.startswith(("SideBySide", "FirstSecond")) else MARGIN_POINTS
        worst, worst_at = 0, 0.0
        for i, raw in enumerate(full_frames(video)):
            for x, y in points:
                offset = (y * 1920 + x) * 3
                value = max(raw[offset:offset + 3])
                if value > worst:
                    worst, worst_at = value, i / 4
        report.setdefault(video.stem, {})["background_max"] = worst
        if worst > 1:
            failures.append(f"{video.stem}: background not pure #000 (max channel {worst} at {worst_at:.2f} s)")
    gate_dir.mkdir(parents=True, exist_ok=True)
    (gate_dir / "audit.json").write_text(json.dumps(
        {"failures": failures, "report": report}, indent=1) + "\n")
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=data.OUT.parent / "quarter-second-audit")
    parser.add_argument("--gates", action="store_true", help="Check the QA gates and fail on any violation")
    parser.add_argument("names", nargs="*", help="Video names to audit, for example SideBySideFly (default: all)")
    args = parser.parse_args()
    if args.gates:
        (data.OUT.parent / "gates").mkdir(parents=True, exist_ok=True)
        failures = gates(set(args.names) or None)
        print("\n".join(failures) or "All QA gates passed.")
        raise SystemExit(1 if failures else 0)
    videos = [data.OUT / f"{name}.mp4" for name in args.names] or sorted(data.OUT.glob("*.mp4"))
    for video in videos:
        create_sheets(video, args.out / video.stem)


if __name__ == "__main__":
    main()
