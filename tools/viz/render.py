#!/usr/bin/env python3
"""Render, verify, and index the NativeTube explainer pack."""
import argparse
from fractions import Fraction
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from PIL import Image

import data
import normalized
import stills
from stills import UPSTREAM
import tours

HERE = Path(__file__).resolve().parent
CACHE = data.OUT.parent
MANIM = CACHE / "manim"
EXPLAINERS = {
    "FramePath": (5, 15, 32),
    "FirstFrame": (5, 10, 15, 25),
    "ScrollCost": (6, 9, 26, 37),
    "Memory": (5, 13, 23, 41),
    "Hiccups": (6, 10, 28),
}
STILLS = (
    "FramePathStill", "EGLStill", "InstructionsStill", "PerCoreStill",
    "GPUStill", "SizesStill", "UpstreamStill", "CostStill",
)


def scene_set(file, names, resolution, *, still, fly):
    args = ["--renderer", "cairo", "--progress_bar", "none", "--resolution", resolution,
            "--media_dir", str(MANIM), "--fps", "60"]
    if still:
        args.append("-s")
    log = CACHE / f"{file}.log"
    with log.open("w") as output:
        subprocess.run(
            [sys.executable, "-m", "manim", *args, str(HERE / f"{file}.py"), *names],
            stdout=output, stderr=subprocess.STDOUT, check=True,
            env=os.environ | {"VIZ_RENDER_FLY_SNAPSHOT": json.dumps(fly)},
        )
    for name in names:
        if still:
            source = MANIM / "images" / file / f"{name}_ManimCE_v0.19.2.png"
            if not source.is_file():
                candidates = list((MANIM / "images" / file).glob(f"{name}*.png"))
                if len(candidates) != 1:
                    raise FileNotFoundError(f"Expected one rendered image for {name}: {candidates}")
                source = candidates[0]
            shutil.copyfile(source, data.OUT / f"{name}.png")
        else:
            source = MANIM / "videos" / file / "1080p60" / f"{name}.mp4"
            write_explainer(source, data.OUT / f"{name}.mp4")


def probe(path):
    result = subprocess.check_output([
        "ffprobe", "-v", "error", "-show_entries",
        "stream=codec_type,codec_name,width,height,avg_frame_rate,pix_fmt:format=duration", "-of", "json", str(path),
    ], text=True)
    return json.loads(result)



def constant_frame_clock(path):
    response = subprocess.check_output([
        "ffprobe", "-v", "error", "-select_streams", "v:0", "-show_packets",
        "-show_entries", "stream=time_base:packet=pts", "-of", "json", str(path),
    ], text=True)
    info = json.loads(response)
    step = 1 / (60 * Fraction(info["streams"][0]["time_base"]))
    stamps = sorted(packet["pts"] for packet in info["packets"])
    return bool(stamps) and step.denominator == 1 and all(
        stamp == frame * step.numerator for frame, stamp in enumerate(stamps)
    )


def write_explainer(source, output):
    if constant_frame_clock(source):
        shutil.copyfile(source, output)
        return
    # Manim concatenates partials with sub-frame timestamp rounding. Assign
    # every encoded picture its exact 60 fps timestamp before delivery.
    subprocess.run([
        "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-xerror", "-y",
        "-threads", "2", "-i", str(source), "-filter_threads", "2",
        "-vf", "setpts=N/(60*TB)", "-r", "60", "-fps_mode", "cfr",
        "-an", "-c:v", "libx264", "-threads", "2", "-preset", "veryfast",
        "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(output),
    ], check=True)

def still_preview(path, output):
    with Image.open(path) as source:
        if source.size != (1600, 900):
            raise ValueError(f"Wrong size for {path}: {source.size}")
        source.thumbnail((560, 315), Image.Resampling.LANCZOS)
        source.convert("RGB").save(output, quality=86, optimize=True)


def extract(path, at, output, width):
    subprocess.run([
        "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-threads", "2",
        "-ss", str(at), "-i", str(path), "-filter_threads", "2", "-vf", f"scale={width}:-2",
        "-frames:v", "1", str(output),
    ], check=True)


def verify(recordings=None):
    previews = HERE / "previews"
    previews.mkdir(exist_ok=True)
    qa = CACHE / "qa"
    qa.mkdir(exist_ok=True)
    results = {}
    comparison_times = {"SideBySide": (1, 7, 12, 19), "FirstSecondReplay": (.2, 1.35, 2.3)}
    if recordings and "SideBySideFly" in recordings:
        comparison_times["SideBySideFly"] = (1, 7.5, 9.1, 17, 32)
    for name, times in (EXPLAINERS | comparison_times).items():
        path = data.OUT / f"{name}.mp4"
        info = probe(path)
        expected = [{"codec_type": "video", "codec_name": "h264", "width": 1920,
                     "height": 1080, "avg_frame_rate": "60/1", "pix_fmt": "yuv420p"}]
        seconds = float(info["format"]["duration"])
        if (info["streams"] != expected or not constant_frame_clock(path)
                or (name in EXPLAINERS and not 30 <= seconds <= 75)):
            raise ValueError(f"Video format check failed: {path}: {info}")
        times = [at for at in times if at < seconds]
        for i, at in enumerate(times, 1):
            extract(path, at, qa / f"{name}-{i}.png", 960)
        extract(path, times[-2], previews / f"{name}.jpg", 560)
        results[name] = info
    for name in STILLS:
        still_preview(data.OUT / f"{name}.png", previews / f"{name}.jpg")
        results[name] = {"width": 1600, "height": 900}
    (data.OUT / "verification.json").write_text(json.dumps(results, indent=2) + "\n")
    return results


def rows_by_metric(rows, fields, fmt):
    return {f"{app} {field} {fmt(row[field])}": row["sources"] for app, row in rows.items()
            for field in fields if row.get(field) is not None}


def normalized_sources():
    return normalized.sources() or ["tools/viz/normalized.py"]


def source_map(fly, recordings):
    _, timing = data.timing(fly)
    _, memory = data.memory(fly)
    _, wheel = data.wheel(fly)
    ctr = data.laptop_counters()
    frame = {"848 × 476 RGBA; 4 bytes per pixel; ≈1.6 MB per frame; 30 fps; 48.4 MB/s at 30 fps": [
        data.REVIEW, data.METHOD, "core/src/player.rs", "apps/iced/src/player_view.rs",
        "apps/gpui/src/player.rs", "apps/rune/src/video.rs",
    ]}
    playback = {f"{app} {mode} {ctr[app][mode]['cpu']['instructions']} instructions": [data.COUNTERS, data.REVIEW]
                for app in data.APPS for mode in ("playback-software", "playback-gpu")}
    wheel_rates = rows_by_metric(wheel, ("rate",), lambda n: f"{n / 1e6:,.0f} M/s")
    gate_rates = {f"laptop gate {app}: {normalized.cpu_text('laptop_gate_wheel', app)}":
                  [data.GATE, data.REVIEW, *normalized_sources()] for app in data.APPS}
    still_wheel = {f"{app} {ctr[app]['wheel']['normalization']['instructions_per_scroll_second'] / 1e6:,.0f} M/s":
                   [data.COUNTERS, data.REVIEW] for app in data.APPS}
    catalog = data.read(data.HICCUPS)["incidents"]
    raced = ("open_ms",) if "youtube" in timing else ("home_ms", "open_ms")
    result = {
        "FramePath": frame | playback | {"10 seconds; ≈19× fewer per frame": [data.METHOD, data.COUNTERS, data.REVIEW]},
        "FirstFrame": rows_by_metric(timing, raced, lambda n: f"{n:,.0f} ms") | {
            "ms axis and elapsed clock (visual scale fitted to measured maxima)": sorted({
                path for row in timing.values() for path in row["sources"]
            }),
        },
        "ScrollCost": wheel_rates | gate_rates | {
            "≈1 M instructions per dot; illustrative 60 fps frame scale": [
                "tools/viz/scenes.py", data.COUNTERS, data.REVIEW,
                *[path for row in wheel.values() for path in row["sources"]],
            ],
            "GPUI PageView and Rune Live offset": ["apps/gpui/NOTES.md", "apps/rune/NOTES.md", data.REVIEW],
            **({"27 wheel ticks; medians of three": [data.METHOD, data.REVIEW]} if "wheel" not in fly else {}),
        },
        "Memory": rows_by_metric(memory, ("cold", "warm", "playing", "peak", "core"), lambda n: f"{n:.0f} MiB") | {
            f"{app} app and integration {row['playing'] - row['core']:.0f} MiB": row["sources"]
            for app, row in memory.items() if row.get("playing") is not None and row.get("core") is not None
        },
        "Hiccups": {
            "dates and area/kind for every incident": [data.HICCUPS],
            f"{len(catalog)} found; {sum(bool(row['fixed_when']) for row in catalog)} fixes; "
            f"{sum(not row['fixed_when'] for row in catalog)} open": [data.HICCUPS, data.REVIEW],
            "7 iced; 6 GPUI; 7 Rune; 14 core; 9 tooling and process": [data.HICCUPS, data.REVIEW],
            "9 of 9 NOW runs clean; 20 related opens per run": [data.STABILITY, data.METHOD, data.REVIEW],
        },
        "FramePathStill": frame,
        "EGLStill": {"vendor selection mechanism and unpublished timing": ["core/src/gpu.rs", "core/src/gpu/linux.rs", data.REVIEW]},
        "InstructionsStill": playback | still_wheel | {"ten seconds; medians of three": [data.METHOD, data.COUNTERS]},
        "PerCoreStill": {
            f"{'ntctl play' if app == 'core' else app} software playback: "
            + " + ".join(f"{pmu} {part} M ({count / total * 100:.1f}%)" for pmu, part, count in
                         zip(stills.PMUS, *stills.shown_millions(counts, total)[:1], counts, strict=True))
            + f" = {stills.shown_millions(counts, total)[1]} M; instruction-count shares, AC": [data.COUNTERS, data.METHOD]
            for app in (*data.APPS, "core") for counts, total in [stills.pmu_split(app)]},
        "GPUStill": {f"{app} {engine} {row['busy_cycles_estimate']/1e6:.2f} M": [data.COUNTERS, data.METHOD]
                     for app in data.APPS for engine, row in
                     ctr[app]["playback-gpu"]["gpu"]["engines"].items() if engine in ("rcs0", "vcs0")},
        "SizesStill": {f"{app} {size:.1f} MiB; {packages} packages": [data.METRICS, f"apps/{app}/Cargo.lock", data.REVIEW]
                       for app, (size, packages) in data.sizes().items()},
        "UpstreamStill": {"seven reports, written up locally and not yet filed": [f"docs/upstream/{file}" for _, _, file in UPSTREAM]},
        "CostStill": {f"{app} {cost}; {elapsed}": [data.COST, data.REVIEW] for app, (cost, elapsed) in data.cost().items()},
    }
    if "timing" not in fly:
        result["FirstFrame"]["median of three"] = [data.METHOD, data.METRICS]
    else:
        first = result["FirstFrame"]
        first.update(rows_by_metric(data.timing({})[1], ("open_ms",), lambda n: f"laptop, GPU {n:.0f} ms"))
        first["row labels: laptop, GPU vs Fly VM, software rendering"] = [data.FLY_METHOD, data.METHOD]
        first["log time axis, 50 to 3,000 ms, gridlines 50/100/200/500/1,000/2,000"] = [
            "tools/viz/scenes.py", data.FLY_METHOD]
        if browser := timing.get("youtube"):
            first[f"Chromium signed-out live network at {browser['size']}"] = browser["sources"]
    if "wheel_cpu" in fly:
        scroll = result["ScrollCost"]
        scroll.update(rows_by_metric(
            fly["wheel_cpu"], ("cpu_percent",), lambda n: f"Fly bar length from {n:.1f}% of one core (not shown)"))
        scroll.update({f"Fly {app}: {normalized.cpu_text('fly_wheel', app)}": [data.FLY_METHOD, *normalized_sources()]
                       for app in data.APPS})
        scroll["No-GPU act: laptop instructions per wheel second vs Fly wheel CPU; order flip; 4 vCPUs"] = [
            *normalized_sources(),
            data.COUNTERS, data.FLY_METHOD, ".cache/fly-bench/out/summary.json"]
    coda = {}
    for machine, workload, title in (("laptop", "playback-gpu", "laptop GPU subsurface path"),
                                     ("laptop", "playback-software", "laptop software path"),
                                     ("fly", "playback-egl", "Fly GPU/EGL path, emulated by llvmpipe"),
                                     ("fly", "playback-software", "Fly software path")):
        cores = normalized.across_apps(machine, workload, "cores_busy")
        share = normalized.across_apps(machine, workload, "machine_percent")
        if cores and share:
            coda[f"No-GPU coda: {title} {cores[0]:.3f}–{cores[1]:.3f} cores; "
                 f"{share[0]:.2f}–{share[1]:.2f}% of machine"] = normalized_sources()
    if coda:
        result["FramePath"].update(coda)
        result["FramePath"]["16 laptop and 4 Fly logical CPUs; medians of three; range across apps"] = normalized_sources()
        result["FramePath"]["coda footer: laptop CPU from resource runs on battery, power profile"] = [
            data.RESOURCES, data.METHOD, *normalized_sources()]
    for app in data.APPS:
        if (per_frame := normalized.wheel_rate(app, "instructions_per_drawn_frame")) is not None:
            result["ScrollCost"][f"{app} {per_frame / 1e6:.1f} M instructions per drawn frame"] = normalized_sources()
    warm_trials = normalized.warm_trials()
    for line in normalized.warm_lines():
        result["FirstFrame"]["YouTube warm footnote: " + line] = normalized_sources()
    for row in warm_trials:
        values = ", ".join(f"{key} {row[key] / 1000:.3f} s" for key in
                           ("any_frame_ms", "actual_frame_elapsed_ms", "actual_frame_excluding_ad_ms", "ad_mode_ms")
                           if row.get(key) is not None)
        result["FirstFrame"][f"Fly YouTube rerun warm trial {row['repeat']} ({row['status']}): {values}"] = (
            normalized_sources())
    if "memory_compare" in fly:
        result["Memory"].update(rows_by_metric(fly["memory_compare"], ("mib",), lambda n: f"Fly {n:.0f} MiB"))
        result["Memory"].update(rows_by_metric(fly["memory_compare"], ("peak_mib",), lambda n: f"Fly peak {n:,.0f} MiB"))
        result["Memory"]["stage tags: cold Home prewarm off; warm Home default GPU startup; software playback; "
                          "tour default GPU path; laptop on battery, power profile; FLY VM • SOFTWARE RENDERING"] = [
            data.RESOURCES, data.METHOD, "tools/media/resources.py", data.FLY_METHOD]
    if "memory" not in fly:
        result["Memory"]["median of three"] = [data.METHOD, data.RESOURCES]
    for name, receipt in recordings.items():
        result[name] = {"stopwatches, badges, first-frame flash, sampled CPU and memory per tile": {
            app: tile["sources"] for app, tile in receipt["tiles"].items()
        }, "first-second duration and 4× replay speed" if name == "FirstSecondReplay" else "aligned tour duration": [
            "tools/viz/tours.py", *[tile["sources"][0] for tile in receipt["tiles"].values()]
        ]}
        result[name]["CPU strip: sampled cores busy of the machine's logical CPUs (16 laptop, 4 Fly)"] = [
            *normalized_sources(), "tools/viz/tours.py"]
        if name == "SideBySideFly":
            result[name]["one Skip click on a browser ad; YouTube stage cues"] = [
                data.FLY_METHOD, "tools/viz/fly-web-cues.json", "tools/viz/tours.py"]
        if status := receipt["web_status"]:
            result[name]["browser availability"] = [status["source"]]
    return result


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--laptop", action="store_true", help="Force the saved laptop baseline")
    cli.add_argument("--only", nargs="+", metavar="NAME",
                     help="Re-encode only these outputs, for example FirstFrame ScrollCost SideBySideFly")
    args = cli.parse_args()
    os.chdir(data.ROOT)
    os.environ["OPENBLAS_NUM_THREADS"] = "1"
    os.environ["OMP_NUM_THREADS"] = "1"
    os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
    os.environ["VIZ_AUDIT_TEXT"] = "1"
    if args.laptop:
        os.environ["VIZ_LAPTOP_ONLY"] = "1"
    data.OUT.mkdir(parents=True, exist_ok=True)
    fly = data.fly_input()
    print("Fly inputs:", ", ".join(fly) if fly else "none, using laptop", flush=True)
    wanted = set(args.only or (*EXPLAINERS, *STILLS, "SideBySide", "FirstSecondReplay", "SideBySideFly"))
    if explainers := [name for name in EXPLAINERS if name in wanted]:
        scene_set("scenes", explainers, "1920,1080", still=False, fly=fly)
    if stills := [name for name in STILLS if name in wanted]:
        scene_set("stills", stills, "1600,900", still=True, fly=fly)
    tour_receipts = tours.main(fly, wanted)
    references = source_map(fly, tour_receipts)
    for scene, numbers in references.items():
        for label, paths in numbers.items():
            if isinstance(paths, dict):
                paths = [path for tile in paths.values() for path in tile]
            for path in paths:
                if not (data.ROOT / path).is_file():
                    raise FileNotFoundError(f"{scene} / {label}: {path}")
    (data.OUT / "sources.json").write_text(json.dumps(references, indent=2, ensure_ascii=False) + "\n")
    for name, info in verify(tour_receipts).items():
        duration = info.get("format", {}).get("duration")
        print(f"{name}: {duration}s" if duration else f"{name}: 1600×900")



if __name__ == "__main__":
    main()
