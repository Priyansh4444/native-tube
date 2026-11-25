"""Map the published Fly summary and reviewed tour captures to visualization inputs."""
from pathlib import Path

import data

BASE = Path(".cache/fly-bench/out")
SUMMARY = BASE / "summary.json"
VM = BASE / "vm"
NATIVE = VM / "native-windowed"
BROWSER = VM / "web-tour-click/tours/web"


def run_paths(directory):
    return [str(directory / f"run-{repeat}/sample.json") for repeat in range(1, 4)]


def tours():
    review = BASE / "privacy-final/videos.txt"
    if not (data.ROOT / review).is_file():
        return {}
    reviewed = set((data.ROOT / review).read_text().splitlines())
    output = {}
    for app in data.APPS:
        directory = NATIVE / "tours" / app
        video, report, scenario = (str(directory / name) for name in (f"{app}.mp4", "report.json", "scenario.json"))
        if video in reviewed and all((data.ROOT / path).is_file() for path in (video, report, scenario)):
            output[app] = {"video": video, "report": report, "scenario": scenario,
                           "sources": [video, report, scenario, str(review), data.FLY_METHOD]}
    video = str(BROWSER / "youtube.mp4")
    cues = Path("tools/viz/fly-web-cues.json")
    if video in reviewed and (data.ROOT / video).is_file():
        output["youtube"] = data.read(cues) | {
            "video": video,
            "sources": [video, str(BROWSER / "navigation.json"), str(cues), str(review), data.FLY_METHOD],
        }
    return output if set(data.APPS) <= output.keys() else {}


def benchmark():
    if not (data.ROOT / data.FLY_METHOD).is_file() or not (data.ROOT / SUMMARY).is_file():
        return {}
    measured = data.read(SUMMARY)
    reference = [data.FLY_METHOD, str(SUMMARY)]
    timing, peaks, wheel = {}, {}, {}
    for app in data.APPS:
        row = measured[app]
        defaults = reference + run_paths(NATIVE / app / "default")
        timing[app] = {"home_ms": row["home_ms"], "open_ms": row["open_ms"], "sources": defaults}
        peaks[app] = {"mib": row["peak_pss_mib"], "sources": defaults}
        wheel[app] = {"cpu_percent": row["wheel_cpu_percent"],
                      "sources": reference + run_paths(NATIVE / app / "wheel")}
    cold = measured["web"]["cold"]
    web_sources = reference + run_paths(VM / "web")
    timing["youtube"] = {"open_ms": int(cold["ms"] + .5), "size": cold["runs"][0]["size"].replace("x", "×"),
                         "sources": web_sources}
    peaks["youtube"] = {"mib": cold["settled_pss_mib"], "peak_mib": cold["peak_pss_mib"], "sources": web_sources}
    peaks["ntctl"] = {"mib": measured["ntctl"]["peak_pss_mib"], "sources": reference + run_paths(NATIVE / "ntctl")}
    extended = [row["warm"] for row in measured["web_extended"]]
    warm = {"min_s": min(row["ms"] for row in extended if row["ok"]) / 1000,
            "max_s": max(row["wall_ms"] for row in extended) / 1000,
            "sources": reference + run_paths(VM / "web-extended/web")}
    result = {"timing": timing, "web_warm": warm, "memory_compare": peaks, "wheel_cpu": wheel}
    if captures := tours():
        result["tours"] = captures
    return result
