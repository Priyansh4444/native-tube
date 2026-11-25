#!/usr/bin/env python3
"""Normalize saved media receipts. This program does not launch apps or a browser."""
import csv
import json
from pathlib import Path
from statistics import median

from counters import add_attribution, cpu_totals, parse_stats, summarize_samples
from metrics import APPS, OUT, ROOT, write_json
from resources import read_phase

FLY = ROOT / ".cache/fly-bench/out"
NATIVE = FLY / "vm/native-windowed"
FPS = 30
REPEATS = (1, 2, 3)


def read(path):
    return json.loads(path.read_text())


def cpu_fields(percent, logical_cpus):
    return {"cores_busy": percent / 100, "machine_percent": percent / logical_cpus}


def marks(report):
    commands = [(ms, json.loads(command)) for ms, command in report["commands"]]
    frames = [ms for ms, name in report["all_marks"] if name == "playback_started"]
    return commands, frames


def app_switches(report):
    commands, frames = marks(report)
    opens = [(ms, command["index"]) for ms, command in commands if command["cmd"] == "open_video"]
    redirects = [ms for ms, command in commands if command["cmd"] in ("open_video", "back")]
    return [{
        "index": index, "command_ms": start,
        "playback_mark_ms": next((end for end in frames if start <= end <
                                  next((t for t in redirects if t > start), float("inf"))), None),
    } for start, index in opens]


def audit_pair(report, run):
    transitions = app_switches(report)
    first, related = transitions
    assert first["index"] == 1 and related["index"] == 0 and not report["failed_waits"], run
    assert any(name == "watch_loaded" and first["command_ms"] < t < related["command_ms"]
               for t, name in report["all_marks"]), run
    return transitions


def pmu_receipt(path):
    with path.open() as stream:
        rows = {row[2]: row for row in csv.reader(stream) if len(row) >= 5}
    return {key: int(rows[f"{key}:u"][0]) for key in ("instructions", "cycles")}


def playback_work(instructions, seconds, pixels, rendered_frames=None):
    return {
        "instructions_per_video_second": instructions / seconds,
        "instructions_per_expected_frame": instructions / (seconds * FPS),
        "instructions_per_expected_frame_pixel": instructions / (seconds * FPS * pixels) if pixels else None,
        "instructions_per_renderer_frame": instructions / rendered_frames if rendered_frames else None,
    }


def summarize(samples):
    summary = {}
    for key in samples[0]:
        values = [sample[key] for sample in samples]
        if all(type(value) in (int, float) for value in values):
            summary[key] = {"median": median(values), "min": min(values), "max": max(values)}
    return summary


def relative_to_core(group):
    control = group["ntctl"]["playback-software"]["summary"]
    for app in APPS:
        for mode, entry in group[app].items():
            entry["delta_vs_ntctl_software"] = {
                key: item["median"] - control[key]["median"]
                for key, item in entry["summary"].items()
                if mode.startswith("playback-") and key in control and
                key in ("cores_busy", "machine_percent", "instructions_per_video_second",
                        "instructions_per_expected_frame", "instructions_per_expected_frame_pixel")
            }


def laptop_rows(counters, resources, logical_cpus):
    result = {}
    pmu_checks = []
    counter_switches = []
    for app, scenarios in counters["results"].items():
        result["ntctl" if app == "core" else app] = app_rows = {}
        for mode, aggregate in scenarios.items():
            samples = []
            for repeat, counter in zip(REPEATS, aggregate["runs"], strict=True):
                run = OUT / "counters" / app / mode / str(repeat)
                count = counter["cpu"]["instructions"]
                seconds = counter["cpu_window_seconds"]
                sample = {"instructions": count, "counter_seconds": seconds}
                report = read(run / "report.json") if app != "core" else None
                phase = next((p for p in report["phases"] if p["name"] == mode), {}) if report else {}
                if mode.startswith("playback-"):
                    path = mode.removeprefix("playback-")
                    pixels = (1280 * 720 if app == "core" else 848 * 476) if path == "software" else None
                    sample.update(playback_work(count, phase.get("seconds", seconds), pixels,
                                                phase.get("video_frames_rendered")))
                if phase and not mode.startswith("playback-"):
                    sample.update(cpu_fields(phase["cpu_percent"], logical_cpus))
                if mode == "home":
                    sample["instructions_per_home_reload"] = count
                if mode == "wheel":
                    sample["drawn_frames"] = phase["frames"]
                    sample["instructions_per_scroll_second"] = count / phase["seconds"]
                    sample["instructions_per_drawn_frame"] = count / phase["frames"]
                if mode == "warm-switch":
                    transitions = app_switches(report)[1:]
                    counter_switches.append({"app": app, "run": repeat, "transitions": transitions})
                    assert len(transitions) == 5 and all(t["index"] == 0 for t in transitions), run
                    sample["instructions_per_related_open_to_frame_marker"] = count / len(transitions)
                samples.append(sample)
                for event in ("instructions", "cycles"):
                    cpu = counter["cpu"]
                    raw_sum = sum(value["count"] or 0 for value in cpu["per_pmu"][event].values())
                    assert raw_sum == cpu[event], (run, event)
                    pmu_checks.append(raw_sum)
            entry = app_rows[mode] = {"runs": samples, "summary": summarize(samples)}
            if mode.startswith("playback-"):
                path = mode.removeprefix("playback-")
                entry["resource_cpu_runs"] = [
                    cpu_fields(run["playback"]["cpu_percent"], logical_cpus)
                    for run in resources[app][path]["runs"]
                ]
                entry["summary"].update(summarize(entry["resource_cpu_runs"]))
            for event in ("instructions", "cycles"):
                cpu = aggregate["cpu"]
                raw_sum = sum(value["count"] or 0 for value in cpu["per_pmu"][event].values())
                assert raw_sum == cpu[event], (app, mode, event)
                pmu_checks.append(raw_sum)
    relative_to_core(result)
    return result, counter_switches, len(pmu_checks)


def fly_rows():
    result = {}
    mode_names = {"default": "playback-egl", "software": "playback-software", "cold": "cold", "wheel": "wheel"}
    for app in (*APPS, "ntctl"):
        result[app] = app_rows = {}
        for mode, name in (mode_names.items() if app != "ntctl" else (("", "playback-software"),)):
            samples = []
            for repeat in REPEATS:
                run = NATIVE / app / mode / f"run-{repeat}"
                receipt = read(run / "sample.json")
                pmu = pmu_receipt(run / "perf.csv")
                sample = {"whole_scenario_instructions": pmu["instructions"], "whole_scenario_cycles": pmu["cycles"],
                          "ipc": pmu["instructions"] / pmu["cycles"]}
                if mode == "cold":
                    idle = read_phase(read(run / "procfs.json"), receipt["home_ms"] / 1000 + 2, 1.8)
                    sample.update(cpu_fields(idle["cpu_percent"], 4))
                else:
                    key = "wheel_cpu_percent" if mode == "wheel" else "playback_cpu_percent"
                    sample.update(cpu_fields(receipt[key], 4))
                if mode in ("default", "software", "wheel"):
                    phase, = read(run / "report.json")["phases"]
                    sample["phase_seconds"] = phase["seconds"]
                    sample["drawn_frames"] = phase["frames"]
                    sample["video_frames_rendered"] = phase["video_frames_rendered"]
                samples.append(sample)
            app_rows[name] = {"runs": samples, "summary": summarize(samples)}
    relative_to_core(result)
    return result


def audit_warm(resources):
    audits = {"laptop": {}, "fly": {}}
    for app in APPS:
        for machine, base in (("laptop", OUT / "metrics-now/NOW"), ("fly", NATIVE)):
            for repeat in REPEATS:
                run = base / app / ("default" if machine == "fly" else "") / f"run-{repeat}"
                report = read(run / "report.json")
                transitions = audit_pair(report, run)
                checks = (resources[app]["gpu"]["runs"][repeat - 1]["idle"] if machine == "laptop"
                          else read(run / "sample.json"))
                assert checks["libmpv_mapped"] and checks["libmpv_checks"] > 0, run
                if machine == "laptop":
                    assert checks["libmpv_checks"] == checks["libmpv_mapped_checks"], run
                audits[machine].setdefault(app, []).append({
                    "run": repeat, "transitions": transitions, "libmpv_checks": checks["libmpv_checks"],
                    "warm_home_pss_mib": checks.get("settled_pss_mib", checks.get("idle_pss_mib")),
                })
    return audits


def audit_other_switches(milestones):
    runs = []
    for app in APPS:
        for build in ("bf276b1", "2caf0fa"):
            for repeat in REPEATS:
                run = OUT / "metrics-final" / build / app / f"run-{repeat}"
                transitions = audit_pair(read(run / "report.json"), run)
                related = transitions[1]
                assert abs(related["playback_mark_ms"] - related["command_ms"] -
                           milestones[build][app]["runs"][repeat - 1]["warm_ms"]) < 0.1, run
                runs.append({"app": app, "build": build, "run": repeat, "transitions": transitions})
        for repeat in REPEATS:
            run = NATIVE / app / "software" / f"run-{repeat}"
            transitions = audit_pair(read(run / "report.json"), run)
            runs.append({"app": app, "machine": "fly", "path": "software", "run": repeat,
                         "transitions": transitions})
    return runs


def check_pss():
    measurements = 0
    for root in (OUT / "resources/NOW", NATIVE, FLY / "vm/web", FLY / "vm/web-extended"):
        for path in root.rglob("procfs.json"):
            for sample in read(path):
                for process in sample["processes"]:
                    if process["pss_mib"] is None or process["rss_mib"] is None:
                        continue
                    assert 0 <= process["pss_mib"] <= process["rss_mib"], path
                    measurements += 1
    return measurements


def web_receipts():
    path = FLY / "normalize-web/frames.log"
    if not path.is_file():
        return []
    return [event for line in path.read_text().splitlines() if line.startswith("{")
            and (event := json.loads(line))["kind"] in ("result", "error")]


def historical_web_cpu(fly, extended):
    control = fly["ntctl"]["playback-software"]["summary"]
    cold = [read(FLY / "vm/web" / f"run-{n}/sample.json")["cold"] for n in REPEATS]
    groups = {"cold": cold, "warm_extended": [run["warm"] for run in extended if run["warm"]["ok"]]}
    result = {}
    for mode, runs in groups.items():
        summary = summarize([cpu_fields(run["resources"]["cpu_percent"], 4) for run in runs])
        result[mode] = {"summary": summary, "delta_vs_ntctl_software": {
            key: row["median"] - control[key]["median"] for key, row in summary.items()
        }}
    return result


def instruction_scaling():
    check = OUT / "counters/normalization-check"
    counts = [cpu_totals(parse_stats((check / f"{seconds}.csv").read_text()))["instructions"]
              for seconds in (1, 2)]
    ratio = counts[1] / counts[0]
    assert 1.9 <= ratio <= 2.1, counts
    return {"cpu_seconds": [1, 2], "instructions": counts, "ratio": ratio}


def check_spread(group, metric):
    counts = [entry["summary"][metric] for modes in group.values() for entry in modes.values()
              if metric in entry["summary"]]
    return max((row["max"] - row["min"]) / row["median"] * 100 for row in counts)


def main():
    laptop_cpus = sum(line.startswith("processor") for line in Path("/proc/cpuinfo").read_text().splitlines())
    assert laptop_cpus > 0
    counters_path = OUT / "counters/counters.json"
    counters = read(counters_path)
    # Rebuild the existing aggregate from its untouched raw run receipts.
    for scenarios in counters["results"].values():
        for mode, row in list(scenarios.items()):
            scenarios[mode] = summarize_samples(row["runs"])
    add_attribution(counters)
    resources = read(OUT / "resources/resources.json")["NOW"]
    laptop, counter_switches, pmu_sums = laptop_rows(counters, resources, laptop_cpus)
    fly = fly_rows()
    audit = audit_warm(resources)
    milestones = read(OUT / "final/metrics.json")
    other_switches = audit_other_switches(milestones)
    all_switches = [transition for runs in audit.values() for entries in runs.values() for entry in entries
                    for transition in entry["transitions"]]
    all_switches += [transition for entry in counter_switches + other_switches
                     for transition in entry["transitions"]]
    for transition in all_switches:
        assert transition["playback_mark_ms"] > transition["command_ms"], transition
    ipc = [run["ipc"] for modes in fly.values() for entry in modes.values() for run in entry["runs"]]
    assert len(ipc) == 39 and all(0.1 <= value <= 6 for value in ipc)
    idle = {"laptop": max(laptop[app]["idle"]["summary"]["machine_percent"]["max"] for app in APPS),
            "fly": max(fly[app]["cold"]["summary"]["machine_percent"]["max"] for app in APPS)}
    assert all(percent < 1 for percent in idle.values())
    spread = {"laptop_playback_instructions_per_second": check_spread(laptop, "instructions_per_video_second"),
              "laptop_scroll_instructions_per_second": check_spread(laptop, "instructions_per_scroll_second"),
              "fly_whole_scenario_instructions": check_spread(fly, "whole_scenario_instructions")}
    assert all(percent < 10 for percent in spread.values())
    rerun = web_receipts()
    for event in rerun:
        if event.get("any_frame_ms") is not None:
            assert event["any_frame_ms"] > 0, event
        if (elapsed := event.get("actual_frame_elapsed_ms")) is not None:
            assert elapsed >= event["any_frame_ms"] > 0, event
            assert abs(elapsed - event["ad_mode_ms"] - event["actual_frame_excluding_ad_ms"]) < 0.1, event
    historical_web = [read(FLY / "vm/web-extended/web" / f"run-{n}/sample.json") for n in REPEATS]
    home_outliers = [{"app": app, "run": repeat, "home_ms": entry["home_ms"]}
                     for app, record in milestones["NOW"].items()
                     for repeat, entry in enumerate(record["runs"], 1)
                     if entry["home_ms"] > 2 * record["metrics"]["home_ms"]]
    write_json(counters_path, counters)
    write_json(OUT / "normalized.json", {
        "logical_cpus": {"laptop": laptop_cpus, "fly": 4}, "clip_fps": FPS,
        "software_dimensions": {"laptop_apps": [848, 476], "ntctl": [1280, 720]},
        "laptop": laptop, "fly": fly, "warm_audit": audit, "counter_switch_audit": counter_switches,
        "other_switch_audit": other_switches,
        "fly_raw_pmu_scope": "whole scenario; no work-unit rate can be recovered",
        "web_rerun": rerun, "historical_web": historical_web, "home_outliers": home_outliers,
        "historical_fly_web_playback_cpu": historical_web_cpu(fly, historical_web),
        "sanity": {
            "pss_le_rss_process_samples": check_pss(),
            "frame_mark_after_open": len(all_switches),
            "hybrid_pmu_sums_checked": pmu_sums,
            "fly_user_pmu_ipc": {"count": len(ipc), "min": min(ipc), "max": max(ipc)},
            "idle_max_machine_percent": idle,
            "synthetic_instruction_scaling": instruction_scaling(),
            "max_peak_to_peak_percent_of_median": spread,
        },
    })


if __name__ == "__main__":
    main()
