#!/usr/bin/env python3
"""Assemble three reviewed tours and a metrics key into a silent 2×2 MP4."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

from metrics import APPS, APP_NAMES, MILESTONES, OUT, ROOT

W, H = 640, 600
VISIBLE_METRICS = (
    ("Time to Home", "home_ms", " ms"),
    ("Open to first frame", "open_ms", " ms"),
    ("Warm switch", "warm_ms", " ms"),
    ("Idle PSS", "idle_pss_mib", " MiB"),
    ("Playback CPU", "playback_cpu_percent", "%"),
)


def shell(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout


def checked_duration(path):
    probe = json.loads(shell(
        "ffprobe", "-v", "error", "-show_entries", "stream=codec_type,width,height:format=duration",
        "-of", "json", str(path),
    ))
    streams = probe["streams"]
    if streams != [{"codec_type": "video", "width": 1280, "height": 800}]:
        raise ValueError(f"{path} must be a silent 1280×800 video")
    return float(probe["format"]["duration"])


def earliest(data, app):
    candidates = [(rev, data[rev][app]["metrics"]) for rev in MILESTONES[:-1]]
    for rev, values in candidates:
        if all(values[key] is not None for _, key, _ in VISIBLE_METRICS):
            return rev
    return max(candidates, key=lambda item: sum(item[1][key] is not None
                                              for _, key, _ in VISIBLE_METRICS))[0]


def tile_columns(data, app):
    baseline = earliest(data, app)
    labels = [APP_NAMES[app]]
    start, now = [f"START {baseline}"], ["NOW"]
    for title, key, unit in VISIBLE_METRICS:
        labels.append(title)
        for rev, column in ((baseline, start), (MILESTONES[-1], now)):
            value = data[rev][app]["metrics"][key]
            column.append("n/a" if value is None else f"{value:.1f}{unit}")
    return labels, start, now


def draw(text, x, y, size):
    return (f"drawtext=fontfile=font.ttf:textfile={text}:expansion=none:"
            f"x={x}:y={y}:fontsize={size}:fontcolor=0xf1f6ff:line_spacing=6:y_align=font")


def compose(data, inputs, output):
    duration = max(map(checked_duration, inputs))
    output.parent.mkdir(parents=True, exist_ok=True)
    pending = output.with_suffix(".pending.mp4")
    with tempfile.TemporaryDirectory(prefix="compose-", dir=output.parent) as directory:
        working = Path(directory)
        (working / "font.ttf").symlink_to(ROOT / "assets/fonts/Roboto-Regular.ttf")
        filters = []
        for index, app in enumerate(APPS):
            draws = []
            for x, column in zip((24, 310, 480), tile_columns(data, app)):
                for row, value in enumerate(column):
                    label = f"{app}-{x}-{row}.txt"
                    (working / label).write_text(value)
                    draws.append(draw(label, x, 408 + 30 * row, 20))
            filters.append(
                f"[{index}:v]setpts=PTS-STARTPTS,fps=30,scale={W}:400,setsar=1,"
                f"tpad=stop_mode=clone:stop_duration={duration},trim=duration={duration},"
                f"pad={W}:{H}:0:0:color=0x101827,"
                f"{','.join(draws)}[tile{index}]"
            )
        (working / "heading.txt").write_text("NativeTube")
        starts = sorted({earliest(data, app) for app in APPS})
        key = [draw("heading.txt", 52, 58, 34)]
        for row, line in enumerate((
            f"START  {', '.join(starts)}",
            "Earliest build with all five metrics",
            "NOW  Working-tree build",
            "Median of three fixture runs",
            "Warm switch opens the related video",
            "Playback CPU is percent of one core",
            "Muted local test clip",
        )):
            label = f"key-{row}.txt"
            (working / label).write_text(line)
            key.append(draw(label, 52, 132 + 33 * row, 23))
        filters.append(
            f"color=c=0x101827:s={W}x{H}:r=30:d={duration},"
            f"{','.join(key)}[key]")
        filters.append(
            f"[tile0][tile1][tile2][key]xstack=inputs=4:layout=0_0|{W}_0|0_{H}|{W}_{H}[tiles]")
        args = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-y"]
        for path in inputs:
            args += ["-threads", "2", "-i", str(path.resolve())]
        try:
            subprocess.run([
                *args, "-filter_complex_threads", "2", "-filter_complex", ";".join(filters),
                "-map", "[tiles]", "-an", "-c:v", "libx264", "-threads", "2", "-preset", "fast",
                "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
                "-t", str(duration), str(pending),
            ], cwd=working, check=True)
            pending.replace(output)
        finally:
            pending.unlink(missing_ok=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("metrics", type=Path)
    parser.add_argument("--tours", type=Path, default=OUT / "tours")
    parser.add_argument("--out", type=Path, default=OUT / "side-by-side.mp4")
    args = parser.parse_args()
    output = args.out.resolve()
    if not output.is_relative_to(OUT):
        parser.error(f"output must be inside {OUT}")
    compose(json.loads(args.metrics.read_text()), [args.tours / app / f"{app}.mp4" for app in APPS], output)
