#!/usr/bin/env python3
"""Extract attributed history or assemble medians from the media collector."""
import argparse
import json
import re
import statistics
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / ".cache/media/out"
APPS = ("iced", "gpui", "rune")
APP_NAMES = {"iced": "iced", "gpui": "GPUI", "rune": "Rune"}
MILESTONES = ("96df4ec", "cb25193", "bf276b1", "2caf0fa", "NOW")
METRICS = {
    "home_ms": ("Time to Home", "ms"),
    "open_ms": ("Open to first frame", "ms"),
    "warm_ms": ("Warm switch to first frame", "ms"),
    "idle_pss_mib": ("Idle PSS", "MiB"),
    "playback_cpu_percent": ("Playback CPU", "% of one core"),
    "window_ms": ("Time to window map", "ms"),
    "stripped_bytes": ("Stripped binary size", "bytes"),
}
PERF = ROOT / ".cache/perf/out/compare-TRpjSyY5/results.json"
NOTES = ["docs/blog/coordinator-notes.md", *(f"apps/{app}/NOTES.md" for app in APPS)]


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                          text=True).stdout


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, allow_nan=False) + "\n")


def report_metrics(report):
    opens = [at for at, cmd in report.get("commands", []) if json.loads(cmd).get("cmd") == "open_video"]
    frames = [at for at, name in report.get("all_marks", []) if name == "playback_started"]
    latencies = []
    for i, started in enumerate(opens):
        end = opens[i + 1] if i + 1 < len(opens) else float("inf")
        frame = next((t for t in frames if started <= t < end), None)
        latencies.append(None if frame is None else frame - started)
    cpu = next((p for p in report.get("phases", []) if p["name"] == "playback"), {})
    return {
        "open_ms": latencies[0] if latencies else None,
        "warm_ms": latencies[1] if len(latencies) > 1 else None,
        "playback_cpu_percent": cpu.get("cpu_percent") if cpu.get("video_frames_rendered", 0) else None,
    }


def git_history(path):
    for rev in git("log", "--follow", "--format=%H", "--", path).splitlines():
        raw = git("show", f"{rev}:{path}")
        if raw:
            yield rev, json.loads(raw)


def note_observations():
    measurements = re.compile(r"playback cpu|idle pss|to home|to first|\| path \| cpu", re.I)
    notes = []
    for name in NOTES:
        path = ROOT / name
        if path.is_file():
            lines = path.read_text().splitlines()
            for i, line in enumerate(lines):
                if measurements.search(line):
                    notes.append({"source": name, "line": i + 1, "excerpt": "\n".join(lines[i:i + 3])})
    return notes


def history():
    """Archive observations under their source commit or candidate binary hash."""
    reports = []
    for app in APPS:
        path = f"bench/results/{app}/report.json"
        for rev, report in git_history(path):
            marks = report.get("marks_ms", {})
            reports.append({
                "revision": rev, "source": path, "app": app, "runs": 1,
                "home_since_init_ms": marks.get("home_loaded"),
                "exec_to_init_ms": report.get("exec_to_init_ms"),
                "metrics": report_metrics(report),
                "failed_waits": report.get("failed_waits", []),
                "playback_seconds": next(
                    (p["seconds"] for p in report.get("phases", []) if p["name"] == "playback"), None),
            })
    other = {}
    for path in ("bench/results/playback.json", "bench/results/shells.json"):
        other[path] = [{"revision": rev, "report": report} for rev, report in git_history(path)]
    perf = json.loads(PERF.read_text())
    return {
        "app_reports": reports,
        "other_reports": other,
        "perf_gate": {"source": str(PERF.relative_to(ROOT)), **perf},
        "notes": note_observations(),
    }


def summarize(samples, offline):
    """Only quote a runtime median for three measured runs from the same build."""
    output = {}
    for rev in MILESTONES:
        output[rev] = {}
        for app in APPS:
            runs = samples.get(rev, {}).get(app, [])
            build = offline.get(rev, {}).get(app, {})
            values = dict.fromkeys(METRICS)
            counts = dict.fromkeys(METRICS, 0)
            for metric in METRICS:
                observed = [run[metric] for run in runs if run.get(metric) is not None]
                counts[metric] = len(observed)
                if len(observed) == 3:
                    values[metric] = statistics.median(observed)
            if "stripped_bytes" in build:
                values["stripped_bytes"] = build["stripped_bytes"]
                counts["stripped_bytes"] = 1
            output[rev][app] = {
                "metrics": values, "counts": counts, "build": build, "runs": runs,
            }
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=OUT / "history.json")
    args = parser.parse_args()
    write_json(args.out, history())


if __name__ == "__main__":
    main()
