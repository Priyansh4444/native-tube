#!/usr/bin/env python3
"""Render, verify, and index the NativeTube explainer pack."""
import argparse
from fractions import Fraction
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys

from PIL import Image, ImageDraw

import audit
import callouts
import data
import normalized
import obsidian
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
PUBLICATION_VIDEOS = ("Demo", "FrameworkUpdates", "FirstFrameRealTime", "ScrollRealTime")


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
        "stream=codec_type,codec_name,profile,width,height,avg_frame_rate,pix_fmt:format=duration", "-of", "json", str(path),
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
    # Manim concatenates partials with sub-frame timestamp rounding. Assign
    # every encoded picture its exact 60 fps timestamp before delivery.
    subprocess.run([
        "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-xerror", "-y",
        "-threads", "2", "-i", str(source), "-filter_threads", "2",
        "-vf", "setpts=N/(60*TB)", "-r", "60", "-fps_mode", "cfr",
        "-an", "-c:v", "libx264", "-threads", "2", "-preset", "veryfast",
        "-profile:v", "high", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(output),
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


def verify(recordings=None, *, wanted=None):
    previews = HERE / "previews"
    previews.mkdir(exist_ok=True)
    qa = CACHE / "qa"
    qa.mkdir(exist_ok=True)
    results = {}
    names = [*EXPLAINERS, "SideBySide", "FirstSecondReplay"]
    if recordings and "SideBySideFly" in recordings:
        names.append("SideBySideFly")
    for name in PUBLICATION_VIDEOS:
        if (data.OUT / f"{name}.mp4").is_file() or wanted is not None and name in wanted:
            names.append(name)
    for name in names:
        if wanted is not None and name not in wanted:
            continue
        path = data.OUT / f"{name}.mp4"
        info = probe(path)
        expected = [{"codec_type": "video", "codec_name": "h264", "width": 1920,
                     "height": 1080, "profile": "High", "avg_frame_rate": "60/1", "pix_fmt": "yuv420p"}]
        seconds = float(info["format"]["duration"])
        duration_ok = 0 < seconds < 140
        if info["streams"] != expected or not constant_frame_clock(path) or not duration_ok:
            raise ValueError(f"Video format check failed: {path}: {info}")
        times = [seconds * share for share in (.12, .3, .5, .7, .88)]
        for i, at in enumerate(times, 1):
            extract(path, at, qa / f"{name}-{i}.png", 960)
        extract(path, times[-2], previews / f"{name}.jpg", 560)
        publication_images(path, seconds)
        results[name] = info
    for name in STILLS:
        if wanted is not None and name not in wanted:
            continue
        still_preview(data.OUT / f"{name}.png", previews / f"{name}.jpg")
        results[name] = {"width": 1600, "height": 900}
    receipt = data.OUT / "verification.json"
    previous = json.loads(receipt.read_text()) if wanted is not None and receipt.is_file() else {}
    receipt.write_text(json.dumps(previous | results, indent=2) + "\n")
    return results



def publication_images(path, seconds):
    poster_dir, contact_dir = (data.OUT / name for name in ("posters", "contact"))
    poster_dir.mkdir(exist_ok=True)
    contact_dir.mkdir(exist_ok=True)
    poster = poster_dir / f"{path.stem}.png"
    extract(path, seconds * (.24 if path.stem == "Demo" else .60), poster, 1920)
    montage = Image.new("RGB", (1440, 598), "black")
    draw = ImageDraw.Draw(montage)
    temp = CACHE / "publication-frame.png"
    for i, share in enumerate((.04, .22, .38, .56, .75, .96)):
        at = seconds * share
        extract(path, at, temp, 480)
        x, y = (i % 3) * 480, (i // 3) * 299
        with Image.open(temp) as frame:
            montage.paste(frame, (x, y + 22))
        draw.text((x + 8, y + 3), f"{path.stem}   {at:.2f} s", fill="white")
    montage.save(contact_dir / f"{path.stem}.png")
    # One frame per second, six across, for the frame-by-frame review.
    for old in contact_dir.glob(f"{path.stem}-1fps-*.jpg"):
        old.unlink()
    raw = subprocess.check_output([
        "nice", "-n", "19", "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-threads", "2",
        "-i", str(path), "-vf", "fps=1,scale=320:180", "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"])
    frames = [Image.frombytes("RGB", (320, 180), raw[i:i + 320 * 180 * 3]) for i in range(0, len(raw), 320 * 180 * 3)]
    per_sheet = 6 * 6
    for sheet_index in range(0, len(frames), per_sheet):
        chunk = frames[sheet_index:sheet_index + per_sheet]
        rows = math.ceil(len(chunk) / 6)
        sheet = Image.new("RGB", (6 * 320, rows * 198), "#202020")
        draw = ImageDraw.Draw(sheet)
        for i, frame in enumerate(chunk):
            x, y = (i % 6) * 320, (i // 6) * 198
            sheet.paste(frame, (x, y + 18))
            draw.text((x + 6, y + 3), f"{path.stem}  {sheet_index + i:d} s", fill="white")
        sheet.save(contact_dir / f"{path.stem}-1fps-{sheet_index // per_sheet + 1:02d}.jpg", quality=88)

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
        "ScrollCost": wheel_rates | {
            "≈1 M instructions per dot; illustrative 60 fps frame scale": [
                "tools/viz/scenes.py", data.COUNTERS, data.REVIEW,
                *[path for row in wheel.values() for path in row["sources"]],
            ],
            "GPUI PageView and Rune Live offset": ["apps/gpui/NOTES.md", "apps/rune/NOTES.md", data.REVIEW],
            **({"27 wheel ticks; medians of three": [data.METHOD, data.REVIEW]} if "wheel" not in fly else {}),
        },
        "Memory": rows_by_metric(memory, ("cold", "warm", "playing", "peak", "core"), lambda n: f"{n:.0f} MiB"),
        "Hiccups": {
            "dates and area/kind for every incident": [data.HICCUPS],
            f"{len(catalog)} found; {sum(bool(row['fixed_when']) for row in catalog)} fixes; "
            f"{sum(not row['fixed_when'] for row in catalog)} open": [data.HICCUPS, data.REVIEW],
            "7 iced; 6 GPUI; 7 Rune; 14 core; 9 tooling (orchestration and test tooling)": [data.HICCUPS, data.REVIEW],
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
        result["Memory"]["stage titles: cold Home, player prewarm off; warm Home, player loaded; playback (mint = "
                          "player alone); peak during the tour; laptop on battery; Fly VM next to YouTube web"] = [
            data.RESOURCES, data.METHOD, "tools/media/resources.py", data.FLY_METHOD]
    if "memory" not in fly:
        result["Memory"]["median of three"] = [data.METHOD, data.RESOURCES]
    for name, receipt in recordings.items():
        tiles = receipt["tiles"]
        result[name] = {"footage per tile, cut per step and aligned on each tile's own command": {
            app: tile["sources"] for app, tile in tiles.items()}}
        for step, values in receipt["steps"].items():
            for app, seconds in values.items():
                result[name][f"{step} timer {app} {seconds * 1000:,.0f} ms (own command → mark; open stops at "
                             "the first visible frame)"] = tile_sources(tiles[app])
        if name == "FirstSecondReplay":
            result[name]["4× slow motion of the click → first frame step"] = ["tools/viz/tours.py"]
        if name == "SideBySideFly":
            result[name]["YouTube web: live network, not raced; stage cues"] = [
                data.FLY_METHOD, "tools/viz/fly-web-cues.json", "tools/viz/tours.py"]
        if status := receipt["web_status"]:
            result[name]["browser availability"] = [status["source"]]
    return result



def tile_sources(tile):
    return [path for path in tile["sources"] if path.endswith((".json", ".mp4"))][:3]


def add_obsidian_sources(references, fly, recordings):
    for scene, labels in callouts.labels(fly).items():
        references.setdefault(scene, {}).update(labels)
    first = references["FirstFrame"]
    if rerun := normalized.cold_range_s():
        first[f"YouTube cold rerun, trusted click: {rerun[0]:.2f}–{rerun[1]:.2f} s "
              f"({len(normalized.cold_trials())} trials)"] = [*normalized_sources(), data.FLY_METHOD]
    first["row labels: Fly VM, llvmpipe EGL (native); Fly VM, software Chromium (YouTube)"] = [
        data.FLY_METHOD, "docs/blog/fairness/neon.md"]
    first["prewarm note: " + callouts.PREWARM_NOTE] = ["docs/blog/fairness/carbon.md", "docs/blog/fairness/neon.md"]
    references["Memory"]["Rune warm Home: libmpv only, no EGL surface"] = [
        "docs/blog/fairness/neon.md", "docs/blog/fairness/carbon.md"]
    for name in recordings:
        if name in references:
            references[name]["intro and end cards; end-card values repeat the stopped timers"] = [
                "tools/viz/obsidian.py", "tools/viz/tours.py"]


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
    wanted = set(args.only or (*STILLS, "Cover", *PUBLICATION_VIDEOS))
    if explainers := [name for name in EXPLAINERS if name in wanted]:
        scene_set("scenes", explainers, "1920,1080", still=False, fly=fly)
    if stills := [name for name in STILLS if name in wanted]:
        scene_set("stills", stills, "1600,900", still=True, fly=fly)
    tour_receipts = tours.main(fly, wanted)
    references = source_map(fly, tour_receipts)
    if "Cover" in wanted:
        obsidian.cover()
    if (stale := data.OUT / "Cover.png").is_file():
        stale.unlink()
    existing = data.read(".cache/viz/out/sources.json") if (data.OUT / "sources.json").is_file() else {}
    for name, build in {
        "Demo": obsidian.demo,
        "FrameworkUpdates": obsidian.framework_updates,
        "FirstFrameRealTime": tours.first_frame_clip,
        "ScrollRealTime": obsidian.scroll_clip,
    }.items():
        if name in wanted:
            references[name] = build()
        elif name in existing:
            references[name] = existing[name]
    references["Cover"] = {"article title": [obsidian.ARTICLE]}
    add_obsidian_sources(references, fly, tour_receipts)
    for scene, numbers in references.items():
        for label, paths in numbers.items():
            if isinstance(paths, dict):
                paths = [path for tile in paths.values() for path in tile]
            for path in paths:
                if not (data.ROOT / path).is_file():
                    raise FileNotFoundError(f"{scene} / {label}: {path}")
    (data.OUT / "sources.json").write_text(json.dumps(references, indent=2, ensure_ascii=False) + "\n")
    for name, info in verify(tour_receipts, wanted=wanted).items():
        duration = info.get("format", {}).get("duration")
        print(f"{name}: {duration}s" if duration else f"{name}: 1600×900")
    failures = audit.gates(wanted)
    if failures:
        raise SystemExit("QA gates failed:\n  " + "\n  ".join(failures))



if __name__ == "__main__":
    main()
