#!/usr/bin/env python3
"""Exercise repeated related opens with short, sampled visual checks."""
import argparse
from collections import Counter
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

from PIL import Image, ImageStat

from collect import FIXTURES, TARGET, alive, hypr, read_report, session
from metrics import APPS, MILESTONES, OUT, power_state, write_json

WEIGHTS = {"crash": 5, "failed_wait": 2, "freeze": 3, "black": 2, "stale": 2, "probe_error": 1,
           "report_timeout": 2}
DOMAINS = ("page", "video", "error", "targets")
UNSUPPORTED_REPLAY = {("96df4ec", "gpui"), ("cb25193", "gpui")}
PLAYER_REGION = (255, 210, 525, 338)


def steps(check_stale=False):
    first = [{"wait_mark": "home_loaded", "timeout": 6}, {"sleep": 0.5},
             {"cmd": "scroll", "ticks": 3}, {"cmd": "fling", "velocity": 900},
             {"sleep": 0.6}, {"cmd": "scroll_to_top"}, {"sleep": 0.45},
             {"cmd": "open_video", "index": 1},
             {"wait_mark": "watch_loaded", "timeout": 4},
             {"wait_mark": "playback_started", "timeout": 4}]
    for i in range(1, 21):
        visual = i in (1, 10, 20)
        if check_stale and visual:
            first += [{"snapshot": f"{i}-prev"}]
        first += [{"cmd": "open_video", "index": 0}]
        watch = {"wait_mark": "watch_loaded", "timeout": 1.6}
        if not check_stale:
            first += [watch]
        first += [{"wait_mark": "playback_started", "timeout": 1.6}]
        if visual:
            first += [{"sleep": 0.25}, {"snapshot": f"{i}-a"},
                      {"sleep": 2.05}, {"snapshot": f"{i}-b"}]
        if check_stale:
            first += [watch]
        first += [{"cmd": "back"}, {"sleep": 0.55}]
    return first


def probe_errors(log):
    malformed = 0
    episodes = 0
    last_error = None
    n = 0
    player_rect = None
    for line in log.splitlines():
        if not line.startswith("NT_STATE "):
            continue
        try:
            payload = json.loads(line[len("NT_STATE "):])
            if not all(key in payload for key in DOMAINS):
                raise ValueError("incomplete probe")
            n += 1
        except (TypeError, ValueError):
            malformed += 1
            continue
        video = payload["video"] or {}
        error = payload["error"] or ("failed video" if video.get("status") == "failed" else None)
        if error and error != last_error:
            episodes += 1
        last_error = error
        for target in payload["targets"]:
            if target["kind"] == "player":
                player_rect = target["rect"]
    return {"frames": n, "malformed": malformed, "error_episodes": episodes,
            "player_rect": player_rect}


def image_patch(path):
    # The center region stays inside the watch player on the common 1280×800 layout.
    with Image.open(path) as image:
        rgb = image.convert("RGB")
        return rgb if rgb.size == (270, 128) else rgb.crop(PLAYER_REGION)


@contextmanager
def snapshot_region(player_only):
    if not player_only:
        yield {}
        return
    bench = next(m for m in hypr("monitors", "all") if m["name"] == "NT-BENCH")
    if (bench["width"], bench["height"], bench["scale"]) != (1280, 800, 1):
        raise RuntimeError("player crop requires NT-BENCH at 1280×800 and scale 1")
    left, top, right, bottom = PLAYER_REGION
    rectangle = f"{bench['x'] + left},{bench['y'] + top} {right-left}x{bottom-top}"
    grim = shutil.which("grim")
    if not grim:
        raise RuntimeError("grim is unavailable")
    with tempfile.TemporaryDirectory(prefix="nt-player-shot-") as directory:
        wrapper = Path(directory) / "grim"
        wrapper.write_text('''#!/bin/sh
set -eu
[ "$#" -eq 3 ] && [ "$1" = -o ] && [ "$2" = NT-BENCH ] || exit 2
exec "$NT_MEDIA_GRIM" -s 1 -g "$NT_MEDIA_PLAYER_REGION" "$3"
''')
        wrapper.chmod(0o700)
        yield {"PATH": f"{directory}:{os.environ['PATH']}", "NT_MEDIA_GRIM": grim,
               "NT_MEDIA_PLAYER_REGION": rectangle}


def completed_switches(report, failed_waits):
    if report is None:
        return set()
    opens = [i for i, command in report.get("commands", [])
             if json.loads(command).get("cmd") == "open_video"]
    if not opens:
        return set(range(1, 21)) if not failed_waits else set()
    marks = report.get("all_marks", [])
    stops = [t for t, command in report.get("commands", []) if json.loads(command).get("cmd") == "back"]
    return {i for i, (opened, closed) in enumerate(zip(opens[1:], stops), 1)
            if {"watch_loaded", "playback_started"} <= {name for t, name in marks if opened <= t < closed}}


def analyze(run, report, unexpected_exit):
    log = (run / "app.log").read_text(errors="replace")
    probe = probe_errors(log)
    events = Counter()
    events["crash"] = int(unexpected_exit or bool(re.search(r"\bpanicked at\b|\bSIG(SEGV|ABRT)\b", log)))
    waits = re.findall(r"NT_AUTOPILOT: timed out waiting for (\w+)", log)
    if report is not None:
        waits = report.get("failed_waits", waits)
    events["failed_wait"] = len(waits)
    if report is None and not events["crash"]:
        events["report_timeout"] = 1
    if probe["frames"] or probe["malformed"]:
        events["probe_error"] = probe["error_episodes"] + probe["malformed"]
    checks = []
    eligible = completed_switches(report, waits)
    for i in (1, 10, 20):
        a, b = (run / "frames" / f"{i}-{n}.png" for n in ("a", "b"))
        entry = {"switch": i, "black": None, "freeze": None}
        if i in eligible and a.exists() and b.exists():
            first, second = image_patch(a), image_patch(b)
            entry["black"] = all(v <= 8 for v in ImageStat.Stat(first).mean)
            entry["freeze"] = first.tobytes() == second.tobytes()
            events["black"] += int(entry["black"])
            events["freeze"] += int(entry["freeze"])
            previous = run / "frames" / f"{i}-prev.png"
            if previous.exists():
                entry["stale"] = first.tobytes() == image_patch(previous).tobytes()
                events["stale"] += int(entry["stale"])
        checks.append(entry)
    return {"counts": dict(events), "failed_waits": waits, "checks": checks, "probe": probe,
            "weight": sum(events[k] * weight for k, weight in WEIGHTS.items()),
            "finished": report is not None}


def one(binary, run, fixtures, *, check_stale=False, player_only=False):
    scenario = run / "scenario.json"
    snapshots = run / "frames"
    write_json(scenario, steps(check_stale))
    hooks = {"NT_PROBE": "1", "NT_SNAPSHOT_OUTPUT": "NT-BENCH", "NT_SNAPSHOT_DIR": str(snapshots)}
    report = None
    with snapshot_region(player_only) as region, session(
            binary, "bench", run, scenario, fixtures, full_output=True, hooks=hooks | region) as (pid, *_):
        deadline = time.monotonic() + 85
        while alive(pid) and time.monotonic() < deadline:
            report = read_report(run)
            if report is not None:
                break
            time.sleep(0.07)
        report = report or read_report(run)
        unexpected_exit = report is None and not alive(pid)
    return analyze(run, report, unexpected_exit)


def summary(rows):
    eligible = [row for row in rows if "infrastructure_error" not in row]
    assessed = [row for row in eligible
                if row["weight"] > 0 or row["finished"]
                and all(check["black"] is not None for check in row["checks"])]
    clean = sum(row["weight"] == 0 for row in assessed)
    by_kind = Counter()
    for row in assessed:
        by_kind.update(row["counts"])
    return {"attempts": len(rows), "assessed": len(assessed), "clean": clean,
            "score_percent": 100 * clean / len(assessed) if assessed else None,
            "weighted_events": sum(row["weight"] for row in assessed) if assessed else None,
            "counts": dict(by_kind), "runs": rows}


def main(args):
    result = {"weights": WEIGHTS, "results": {}}
    for rev in args.milestones:
        entry = result["results"][rev] = {}
        for app in args.apps:
            if (rev, app) in UNSUPPORTED_REPLAY:
                entry[app] = {"score_percent": None, "reason": "Home fixture keys predate the canonical corpus"}
                continue
            build = "96df4ec-routed" if rev == "96df4ec" else rev
            binary = TARGET / build / app / "release" / f"nt-{app}"
            rows = []
            for i in range(1, 4):
                run = args.out / rev / app / f"run-{i}"
                receipt = run / "sample.json"
                if args.cached_only and not receipt.is_file():
                    continue
                run.mkdir(mode=0o700, parents=True, exist_ok=True)
                if receipt.is_file():
                    row = json.loads(receipt.read_text())
                else:
                    power = power_state()
                    try:
                        row = one(binary, run, args.fixtures, check_stale=rev == "NOW",
                                  player_only=args.player_only)
                        row["power"] = power
                        with binary.open("rb") as stream:
                            row["binary_sha256"] = hashlib.file_digest(stream, "sha256").hexdigest()
                    except (OSError, RuntimeError, ValueError, StopIteration, subprocess.SubprocessError) as error:
                        row = {"infrastructure_error": str(error)}
                    write_json(receipt, row)
                rows.append(row)
                print(rev, app, i, row.get("counts", row.get("infrastructure_error")), flush=True)
            if rows:
                entry[app] = summary(rows)
            write_json(args.out / "stability.json", result)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=OUT / "stability")
    parser.add_argument("--fixtures", type=Path, default=FIXTURES)
    parser.add_argument("--milestones", nargs="+", choices=MILESTONES, default=MILESTONES)
    parser.add_argument("--apps", nargs="+", choices=APPS, default=APPS)
    parser.add_argument("--player-only", action="store_true")
    parser.add_argument("--cached-only", action="store_true",
                        help="summarize existing runs without launching anything")
    args = parser.parse_args()
    args.out = args.out.resolve()
    args.fixtures = args.fixtures.resolve()
    if not args.out.is_relative_to(OUT):
        parser.error(f"output must be inside {OUT}")
    main(args)
