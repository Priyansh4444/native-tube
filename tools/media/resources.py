#!/usr/bin/env python3
"""Sample proportional memory and CPU for isolated playback and app phases."""
import argparse
import hashlib
import json
import mmap
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time

from collect import CLIP, FIXTURES, TARGET, alive, headless_slot, maps_mpv, read_report, session
from metrics import APPS, OUT, ROOT, power_state, write_json

sys.path.insert(0, str(ROOT / "tools"))
from performance.analysis import phase_metrics
from performance.procfs import Sampler

SOFTWARE = {"iced": "NT_SOFTWARE_VIDEO", "gpui": "NT_SOFTWARE_VIDEO", "rune": "NT_RUNE_SOFTWARE"}
GPU_ATTACH = {"iced": "NT_EVENT gpu_surface_attached",
              "gpui": "attached Wayland GPU surface",
              "rune": "Wayland GL video attached"}
STEPS = [{"wait_mark": "home_loaded", "timeout": 8}, {"sleep": 6},
         {"cmd": "open_video", "index": 1}, {"wait_mark": "playback_started", "timeout": 8},
         {"phase": "playback"}, {"sleep": 6}, {"phase": ""}]


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def supports_cold_home(binary):
    with binary.open("rb") as stream, mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as image:
        return image.find(b"NT_NO_PREWARM") >= 0


def mapping_state(checks):
    if len(checks) < 3 or any(check is None for check in checks):
        return None
    return checks[0] if all(check == checks[0] for check in checks) else None


def render_path(app, log, *, forced_software):
    if forced_software:
        return "unconfirmed" if GPU_ATTACH[app] in log else "software"
    if GPU_ATTACH[app] in log and "fell back to software" not in log:
        return "gpu"
    return "unconfirmed"


def phase(samples):
    result = phase_metrics(samples)
    rollup = []
    for i, row in enumerate(samples):
        window = [e for e in samples[i:] if e["t_s"] - row["t_s"] <= 1.25]
        if len(window) < 4 or window[-1]["t_s"] - row["t_s"] < 0.75:
            continue
        cpu = phase_metrics(window)["cpu_percent"]
        if cpu is not None:
            rollup.append(cpu)
    return {key: result[key] for key in ("peak_pss_mib", "settled_pss_mib", "cpu_percent")} | {
        "samples": len(samples), "peak_cpu_percent": max(rollup, default=None)}


def read_phase(samples, start, duration):
    return phase([row for row in samples if start + 0.2 <= row["t_s"] <= start + duration - 0.2])


def wait_report(run, pid, timeout, observe_idle=None):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if report := read_report(run):
            return report
        if not alive(pid):
            break
        if observe_idle is not None:
            observe_idle()
        time.sleep(0.05)
    return read_report(run)


def app_run(binary, app, rev, mode, run, fixtures):
    if mode == "cold" and not supports_cold_home(binary):
        return {"mode": mode, "binary_sha256": digest(binary), "cold_home_supported": False,
                "unavailable": "build does not have NT_NO_PREWARM"}
    scenario = run / "scenario.json"
    tour = mode == "tour"
    if tour:
        steps = json.loads(Path(__file__).with_name("tour.json").read_text())
    else:
        steps = STEPS[:2] if mode == "cold" else STEPS
    write_json(scenario, steps)
    hooks = {}
    if mode == "software":
        hooks[SOFTWARE[app]] = "1"
    if mode == "cold":
        hooks["NT_NO_PREWARM"] = "1"
    with session(binary, "bench", run, scenario, fixtures, hooks=hooks) as (pid, started, _, log):
        idle_observed = {}
        home_seen = None

        def observe_home():
            nonlocal home_seen
            if home_seen is None and "NT_MARK home_loaded " in log.read_text(errors="replace"):
                home_seen = time.monotonic() - started
            # Check the mapping throughout the idle window that read_phase measures, not once.
            elapsed = time.monotonic() - started
            if home_seen is not None and home_seen + 2 <= elapsed <= home_seen + 5.5:
                idle_observed.setdefault("checks", []).append(maps_mpv(pid))

        with Sampler(pid, started) as sampler:
            report = wait_report(run, pid, 70 if tour else 28, None if tour else observe_home)
    samples = [row for row in sampler.samples if row["processes"]]
    write_json(run / "procfs.json", samples)
    result = {"binary_sha256": digest(binary), "mode": mode, "clip_sha256": digest(CLIP)}
    if mode == "cold":
        result["cold_home_supported"] = True
    if report is None:
        return result | {"error": "autopilot report unavailable"}
    result["failed_waits"] = report.get("failed_waits", [])
    if tour:
        if not result["failed_waits"]:
            result["tour"] = phase(samples)
        return result
    if home_seen is not None:
        checks = idle_observed.get("checks", [])
        mapped = mapping_state(checks)
        if mapped is (mode != "cold"):
            result["idle"] = read_phase(samples, home_seen + 2, 3.5)
        result.setdefault("idle", {}).update(
            libmpv_mapped=mapped, libmpv_checks=len(checks), libmpv_mapped_checks=checks.count(True),
            libmpv_unreadable_checks=checks.count(None),
        )
    if mode == "cold":
        return result
    first = report.get("marks_ms", {}).get("playback_started")
    init = report.get("exec_to_init_ms")
    if first is not None and init is not None and not result["failed_waits"]:
        result["playback"] = read_phase(samples, (first + init) / 1000, 6)
    log_body = (run / "app.log").read_text(errors="replace")
    result["path"] = render_path(app, log_body, forced_software=mode != "gpu" or rev == "2caf0fa")
    return result


def core_run(binary, run, fixtures):
    out = run / "frame.png"
    log_path = run / "ntctl.log"
    env = {k: v for k, v in os.environ.items() if not k.startswith("NT_")}
    env.update(NT_FIXTURES=str(fixtures), NT_FIXTURE_MODE="replay", NT_VIDEO_OVERRIDE=str(CLIP),
               NT_MUTE="1", NT_NO_MPRIS="1", NT_DATA_DIR=str(run))
    with headless_slot(), log_path.open("wb") as log:
        started = time.monotonic()
        with subprocess.Popen([binary, "play", "PQf9OAI05TQ", out, "6"], env=env,
                              stdout=log, stderr=subprocess.STDOUT) as child:
            first_seen = None
            deadline = started + 38
            try:
                with Sampler(child.pid, started) as sampler:
                    while child.poll() is None and time.monotonic() < deadline:
                        if first_seen is None and "first frame after" in log_path.read_text(errors="replace"):
                            first_seen = time.monotonic() - started
                        time.sleep(0.04)
                if child.poll() is None:
                    raise RuntimeError("ntctl timed out")
            finally:
                if child.poll() is None:
                    child.terminate()
                    child.wait(timeout=5)
    samples = [row for row in sampler.samples if row["processes"]]
    write_json(run / "procfs.json", samples)
    if child.returncode or first_seen is None or not out.is_file():
        raise RuntimeError(f"ntctl playback incomplete, see {log_path}")
    return {"binary_sha256": digest(binary), "clip_sha256": digest(CLIP),
            "path": "software", "playback": read_phase(samples, first_seen, 6)}


def run_matrix(args):
    for rev in args.milestones:
        variants = [(app, mode) for app in APPS for mode in
                    (("cold", "software", "gpu", "tour") if rev == "NOW" else ("cold", "software"))]
        variants.append(("core", "software"))
        for app, mode in variants:
            binary = TARGET / rev / app / "release" / ("ntctl" if app == "core" else f"nt-{app}")
            for i in range(1, 4):
                run = args.out / rev / app / mode / str(i)
                receipt = run / "sample.json"
                if receipt.exists():
                    continue
                run.mkdir(mode=0o700, parents=True, exist_ok=True)
                power = power_state()
                try:
                    if not binary.is_file():
                        raise RuntimeError(f"missing {binary}")
                    result = (core_run(binary, run, args.fixtures) if app == "core" else
                              app_run(binary, app, rev, mode, run, args.tour_fixtures if mode == "tour"
                                      else args.fixtures))
                except (OSError, RuntimeError, ValueError, StopIteration, subprocess.SubprocessError) as error:
                    result = {"error": str(error)}
                result["power"] = power
                write_json(receipt, result)
                print(rev, app, mode, i, result, flush=True)


def median_part(runs, section, key, *, mapped=None, path=None):
    values = []
    for run in runs:
        data = run.get(section, {})
        if (mapped is not None and data.get("libmpv_mapped") != mapped
                or path is not None and run.get("path") != path):
            continue
        if (value := data.get(key)) is not None:
            values.append(value)
    return statistics.median(values) if len(values) == 3 else None


def summarize(directory):
    results = {}
    for rev in ("2caf0fa", "NOW"):
        builds = results[rev] = {}
        for app in (*APPS, "core"):
            variants = builds[app] = {}
            for mode in ("cold", "software", "gpu", "tour"):
                paths = sorted((directory / rev / app / mode).glob("*/sample.json"))
                runs = [json.loads(path.read_text()) for path in paths]
                if not runs:
                    continue
                hashes = {run.get("binary_sha256") for run in runs}
                homogeneous = runs if len(hashes) == 1 and None not in hashes else []
                mapped = mode != "cold"
                sections = {}
                for section in ("idle", "playback", "tour"):
                    eligible = homogeneous
                    if section == "idle" and mode == "cold":
                        eligible = [run for run in eligible if run.get("cold_home_supported") is True]
                    sections[section] = {
                        key: median_part(eligible, section, key,
                                         mapped=mapped if section == "idle" else None,
                                         path=mode if section == "playback" and mode in ("software", "gpu") else None)
                        for key in ("peak_pss_mib", "settled_pss_mib", "cpu_percent", "peak_cpu_percent")
                    }
                variants[mode] = sections | {"runs": runs}
    return results


def attribution(data):
    result = {}
    for rev, builds in data.items():
        core = builds.get("core", {}).get("software", {}).get("playback", {})
        result[rev] = {}
        for app in APPS:
            player = builds[app].get("software", {}).get("playback", {})
            estimates = {}
            for key in ("settled_pss_mib", "cpu_percent"):
                total, reference = player.get(key), core.get(key)
                if total is not None and reference is not None and total > 0:
                    estimates[key] = {"total": total, "core_software": reference,
                                      "app_residual": total - reference,
                                      "core_share_percent": 100 * reference / total}
            result[rev][app] = estimates
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=OUT / "resources")
    parser.add_argument("--fixtures", type=Path, default=FIXTURES)
    parser.add_argument("--tour-fixtures", type=Path, default=OUT.parent / "fixtures")
    parser.add_argument("--milestones", nargs="+", choices=("2caf0fa", "NOW"), default=["2caf0fa"])
    parser.add_argument("--summarize", action="store_true")
    args = parser.parse_args()
    for name in ("out", "fixtures", "tour_fixtures"):
        setattr(args, name, getattr(args, name).resolve())
    if not args.out.is_relative_to(OUT):
        parser.error(f"output must be inside {OUT}")
    if not args.summarize:
        run_matrix(args)
    data = summarize(args.out)
    write_json(args.out / "resources.json", data)
    write_json(args.out / "attribution.json", attribution(data))
