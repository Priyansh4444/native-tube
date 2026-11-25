#!/usr/bin/env python3
"""Bound perf stat to NativeTube autopilot workloads and a local-clip ntctl control."""
import argparse
from contextlib import ExitStack, contextmanager
import csv
import hashlib
import json
import os
from pathlib import Path
import re
import select
import statistics
import subprocess
import sys
import tempfile
import time

from collect import CLIP, FIXTURES, TARGET, alive, headless_slot, read_report, session
from metrics import APPS, OUT, ROOT, power_state, write_json
from resources import SOFTWARE, render_path

sys.path.insert(0, str(ROOT / "tools"))
from performance.stories import open_video, steps_for

CPU_PMUS = ("cpu_core", "cpu_atom", "cpu_lowpower")
CPU_EVENTS = ("instructions", "cycles")
ENGINE_GT = {"rcs0": 0, "vcs0": 1, "bcs0": 0, "ccs0": 0}
I915_EVENTS = [*(f"{e}-busy" for e in ENGINE_GT), "actual-frequency-gt0",
               "actual-frequency-gt1", "rc6-residency-gt0"]
POWER_EVENTS = ["energy-pkg", "energy-gpu"]
PMU_SYSFS = Path("/sys/bus/event_source/devices")
PARANOID = Path("/proc/sys/kernel/perf_event_paranoid")
SCENARIOS = ("home", "idle", "wheel", "playback-gpu", "playback-software", "warm-switch")


def parse_stats(text):
    events = {}
    for fields in csv.reader(text.splitlines()):
        if len(fields) < 5 or not fields[2].strip():
            continue
        value, unit, event, runtime, coverage = map(str.strip, fields[:5])
        if not value or not (value[0].isdigit() or value.startswith("<not ")):
            continue
        try:
            count = int(value) if value.isdecimal() else float(value)
        except ValueError:
            count = None
        key = event.rstrip("/")
        if key in events:
            raise ValueError(f"perf returned duplicate event {key}")
        events[key] = {
            "count": count, "unit": unit, "status": value if count is None else "counted",
            "runtime_ns": int(runtime) if runtime.isdecimal() else None,
            "running_percent": float(coverage.rstrip("%")) if coverage else None,
        }
    return events


def cpu_totals(events):
    active_pmus = [pmu for pmu in CPU_PMUS if (PMU_SYSFS / pmu).exists()]
    expected = active_pmus or ["cpu"]
    per_pmu = {}
    totals = {}
    for metric in CPU_EVENTS:
        by_pmu = per_pmu[metric] = {
            pmu: events.get(f"{pmu}/{metric}/u")
            if pmu != "cpu" else events.get(f"{metric}:u") for pmu in expected
        }
        counts = []
        for event in by_pmu.values():
            if event is None:
                break
            if event["count"] is None:
                if event["status"] != "<not counted>" or event["runtime_ns"] != 0:
                    break
            else:
                counts.append(event["count"])
        else:
            totals[metric] = sum(counts) if counts else None
            continue
        totals[metric] = None
    return totals | {"per_pmu": per_pmu}


def gpu_events():
    try:
        policy = int(PARANOID.read_text())
    except (OSError, ValueError):
        return [], "perf_event_paranoid unavailable"
    if policy > 0:
        return [], f"perf_event_paranoid={policy}"
    planned = [("i915", event) for event in I915_EVENTS] + [("power", event) for event in POWER_EVENTS]
    present = [f"{pmu}/{event}/" for pmu, event in planned
               if (PMU_SYSFS / pmu / "events" / event).is_file()]
    return present, None if present else "no supported system-wide events"


def gpu_totals(events, enabled_seconds, engine_gt):
    def count(key, unit):
        row = events.get(key, {})
        return row.get("count") if row.get("unit") == unit else None

    # The i915 frequency event accumulates MHz·s. Divide by measured enabled time.
    freq = {}
    for gt in (0, 1):
        value = count(f"i915/actual-frequency-gt{gt}", "M")
        freq[str(gt)] = value / enabled_seconds if value is not None and enabled_seconds > 0 else None
    engines = {}
    for engine, gt in engine_gt.items():
        busy = count(f"i915/{engine}-busy", "ns")
        mhz = freq.get(str(gt))
        engines[engine] = {"gt": gt, "busy_ns": busy,
                           "busy_cycles_estimate": busy * mhz / 1000
                           if busy is not None and mhz is not None else None}
    return {"average_mhz": freq, "engines": engines,
            "rc6_ns_gt0": count("i915/rc6-residency-gt0", "ns"),
            "joules": {name: count(f"power/energy-{name}", "Joules") for name in ("pkg", "gpu")}}


class Perf:
    """A disabled perf stat session. Command ACKs bound the sampled windows."""

    def __init__(self, resources, options, events, run, name):
        self.log = run / f"{name}.csv"
        self.errors = run / f"{name}.stderr"
        control_read, self.control = os.pipe()
        self.ack, ack_write = os.pipe()
        lifetime_read, self.lifetime = os.pipe()
        resources.callback(os.close, self.control)
        resources.callback(os.close, self.ack)
        self.windows = []
        try:
            stderr = resources.enter_context(self.errors.open("wb"))
            self.process = subprocess.Popen([
                "perf", "stat", "--no-big-num", "-x,", "-e", ",".join(events),
                "-D", "-1", "--control", f"fd:{control_read},{ack_write}",
                "--timeout", "85000", "-o", self.log, *options,
                "--", "sh", "-c", 'IFS= read -r done < "/proc/self/fd/$1" || :',
                "perf-lifetime", str(lifetime_read),
            ], pass_fds=(control_read, ack_write, lifetime_read), stdout=subprocess.DEVNULL, stderr=stderr,
                env=dict(os.environ, LC_ALL="C"))
        except BaseException:
            os.close(self.lifetime)
            raise
        finally:
            os.close(control_read)
            os.close(ack_write)
            os.close(lifetime_read)
        resources.callback(self.close)

    def command(self, name):
        before = time.monotonic()
        os.write(self.control, (name + "\n").encode())
        if not select.select([self.ack], [], [], 2)[0] or not os.read(self.ack, 64).startswith(b"ack"):
            raise RuntimeError(f"perf {name} acknowledgement missing. See {self.errors}")
        return (before + time.monotonic()) / 2

    def enable(self):
        self.windows.append([self.command("enable")])

    def disable(self):
        self.windows[-1].append(self.command("disable"))

    def close(self):
        # EOF ends the dummy workload, so perf flushes while the app can still run.
        os.close(self.lifetime)
        self.process.wait(timeout=3)

    def result(self):
        if self.process.returncode:
            raise RuntimeError(f"perf exited {self.process.returncode}. See {self.errors}")
        seconds = sum(end - start for start, end in self.windows)
        return parse_stats(self.log.read_text()), seconds


def snap(name):
    return {"snapshot": name}


def scenario_steps(name):
    steps = [{"wait_mark": "home_loaded", "timeout": 6}, {"sleep": 2}]
    if name == "home":
        workload = [{"cmd": "home"}, {"wait_mark": "home_loaded", "timeout": 6}]
    elif name == "idle":
        workload = [{"sleep": 10}]
    elif name == "wheel":
        workload = [step for step in steps_for("quick")["browse"] if "repeat" in step]
    elif name.startswith("playback-"):
        steps += [*open_video(1, timeout=4), {"sleep": 0.5}]
        workload = [{"sleep": 10}]
    else:
        steps += [*open_video(1, timeout=4), {"sleep": 0.7}]
        for i in range(5):
            steps += [snap(f"begin-{i}"), {"cmd": "open_video", "index": 0},
                      {"wait_mark": "playback_started", "timeout": 4}, snap(f"end-{i}"),
                      {"wait_mark": "watch_loaded", "timeout": 4},
                      {"cmd": "back"}, {"sleep": 0.8}]
        return steps
    return steps + [snap("begin-0"), {"phase": name}, *workload, {"phase": ""}, snap("end-0")]


@contextmanager
def boundaries():
    """Convert counter-only snapshot requests into synchronous timing markers."""
    with tempfile.TemporaryDirectory(prefix="nt-count-") as directory, ExitStack() as cleanup:
        path = Path(directory)
        executable = path / "grim"
        executable.write_text("""#!/bin/sh
set -eu
printf '%s\n' "${3##*/}" > "$NT_COUNTER_REQUEST"
IFS= read -r receipt < "$NT_COUNTER_ACK"
test "$receipt" = ok
""")
        executable.chmod(0o700)
        fifo = {}
        for name in ("request", "ack"):
            target = path / name
            os.mkfifo(target, 0o600)
            fifo[name] = os.open(target, os.O_RDWR)
            cleanup.callback(os.close, fifo[name])
        yield fifo, {
            "PATH": f"{directory}:{os.environ['PATH']}",
            "NT_COUNTER_REQUEST": str(path / "request"), "NT_COUNTER_ACK": str(path / "ack"),
            "NT_SNAPSHOT_OUTPUT": "NT-BENCH", "NT_SNAPSHOT_DIR": str(path / "markers"),
        }


def await_marker(fd, pid, expected):
    deadline = time.monotonic() + 43
    while alive(pid) and time.monotonic() < deadline:
        if select.select([fd], [], [], 0.15)[0]:
            if os.read(fd, 128).strip().decode() != f"{expected}.png":
                raise RuntimeError(f"expected autopilot boundary {expected}")
            return
    raise RuntimeError(f"app did not reach autopilot boundary {expected}")


def collect_windows(pid, run, count, engine_gt, fifo=None, *, allow_gpu=True):
    allowed, policy = gpu_events() if allow_gpu else ([], "synthetic test")
    with ExitStack() as cleanup:
        # On hybrid CPUs, an event's running fraction also reflects time scheduled
        # on that PMU class. Scaling each PMU to the whole window overcounts migrations.
        cpu = Perf(cleanup, ["--no-scale", "-p", str(pid)], ["instructions:u", "cycles:u"], run, "cpu")
        gpu = Perf(cleanup, ["-a"], allowed, run, "gpu") if allowed else None
        gpu_error = None
        for i in range(count):
            if fifo:
                await_marker(fifo["request"], pid, f"begin-{i}")
            if gpu and not gpu_error:
                try:
                    gpu.enable()
                except (OSError, RuntimeError) as error:
                    gpu_error = str(error)
            cpu.enable()
            if fifo:
                os.write(fifo["ack"], b"ok\n")
                await_marker(fifo["request"], pid, f"end-{i}")
            else:
                time.sleep(10)
            cpu.disable()
            if gpu and not gpu_error:
                try:
                    gpu.disable()
                except (OSError, RuntimeError) as error:
                    gpu_error = str(error)
            if fifo:
                os.write(fifo["ack"], b"ok\n")
        # Finish both collectors before parsing their native CSV output.
    cpu_raw, cpu_secs = cpu.result()
    payload = {"cpu": cpu_totals(cpu_raw), "cpu_window_seconds": cpu_secs,
               "cpu_slices": [end - start for start, end in cpu.windows],
               "gpu": {"status": "not measured", "reason": gpu_error or policy}}
    if gpu and not gpu_error:
        try:
            raw, secs = gpu.result()
            payload["gpu"] = {"status": "measured" if any(e["count"] is not None for e in raw.values())
                              else "unavailable", "window_seconds": secs, "raw": raw,
                              **gpu_totals(raw, secs, engine_gt)}
        except (OSError, RuntimeError, ValueError) as error:
            payload["gpu"] = {"status": "unavailable", "reason": str(error)}
    return payload


def wait_report(run, pid):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        report = read_report(run)
        if report is not None or not alive(pid):
            return report
        time.sleep(0.025)
    return None


def app_run(binary, app, name, run, fixtures, engine_gt):
    scenario = run / "scenario.json"
    write_json(scenario, scenario_steps(name))
    with boundaries() as (fifo, hooks):
        if name == "playback-software":
            hooks[SOFTWARE[app]] = "1"
        with session(binary, "bench", run, scenario, fixtures, hooks=hooks) as (pid, *_):
            result = collect_windows(pid, run, 5 if name == "warm-switch" else 1, engine_gt, fifo)
            report = wait_report(run, pid)
    if report is None or report.get("failed_waits"):
        raise RuntimeError(f"missing report or failed autopilot waits. See {run / 'app.log'}")
    if name.startswith("playback-"):
        log = (run / "app.log").read_text(errors="replace")
        path = render_path(app, log, forced_software=name == "playback-software")
        if path != name.removeprefix("playback-"):
            raise RuntimeError(f"playback path {path} could not substantiate {name}")
    phase = next((p for p in report["phases"] if p["name"] == name), {})
    if phase and abs(result["cpu_window_seconds"] - phase["seconds"]) > 0.25:
        raise RuntimeError("CPU counter window did not match the in-app phase")
    if name == "home" and not any(json.loads(c)["cmd"] == "home" for _, c in report["commands"]):
        raise RuntimeError("Home command missing in report")
    if name == "warm-switch":
        opens = [at for at, command in report["commands"] if json.loads(command)["cmd"] == "open_video"]
        backs = [at for at, command in report["commands"] if json.loads(command)["cmd"] == "back"]
        if len(opens) != 6 or len(backs) != 5 or not all(
                any(opened <= t < closed and mark == "playback_started" for t, mark in report["all_marks"])
                for opened, closed in zip(opens[1:], backs)):
            raise RuntimeError("report did not confirm five related opens and first frames")
    normalize(result, name, phase)
    return result


def normalize(result, name, phase):
    total = result["cpu"]["instructions"]
    seconds = phase.get("seconds")
    frames = phase.get("video_frames_rendered", 0)
    result["normalization"] = {"scenario_seconds": seconds, "video_frames_rendered": frames or None,
                               "instructions_per_rendered_video_frame": total / frames
                               if total is not None and frames else None,
                               "instructions_per_scroll_second": total / seconds
                               if name == "wheel" and total is not None and seconds else None,
                               "instructions_per_home_load": total if name == "home" else None,
                               "instructions_per_switch": total / 5
                               if name == "warm-switch" and total is not None else None}
    if name.startswith("playback-") and not frames:
        raise RuntimeError("playback phase has no rendered pictures")


def core_run(binary, run, fixtures, engine_gt):
    env = {k: v for k, v in os.environ.items() if not k.startswith("NT_")}
    env.update(NT_MUTE="1", NT_NO_MPRIS="1", NT_DATA_DIR=str(run), NT_FIXTURE_MODE="replay",
               NT_FIXTURES=str(fixtures), NT_VIDEO_OVERRIDE=str(CLIP))
    outfile = run / "ntctl.log"
    with headless_slot(), outfile.open("wb") as log, subprocess.Popen(
            [binary, "play", "PQf9OAI05TQ", run / "ntctl-final-frame.png", "13"],
            env=env, stdout=log, stderr=subprocess.STDOUT) as process:
        deadline = time.monotonic() + 30
        while "first frame after" not in outfile.read_text(errors="replace"):
            if process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError("ntctl did not publish first frame")
            time.sleep(0.01)
        result = collect_windows(process.pid, run, 1, engine_gt)
        if process.wait(timeout=20):
            raise RuntimeError("ntctl did not finish successfully")
    return result


def file_hash(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def engine_map():
    # On this Arrow Lake-P iGPU, VCS is in the separate media GT. Keep busy
    # ns and frequency on other devices, with cycles unavailable until mapped.
    cards = Path("/sys/class/drm").glob("card[0-9]*")
    for card in cards:
        device = card / "device/device"
        if device.is_file() and device.read_text().strip() == "0x7d51":
            return ENGINE_GT
    return {engine: None for engine in ENGINE_GT}


def add_attribution(builds):
    results = builds["results"]
    baseline = results.get("core", {}).get("playback-software", {})
    reference = (baseline.get("cpu") or {}).get("instructions")
    reference_s = baseline.get("cpu_window_seconds")
    for app in APPS:
        row = results.get(app, {}).get("playback-software")
        if not row:
            continue
        row.pop("attribution", None)
        total = (row.get("cpu") or {}).get("instructions")
        seconds = row.get("cpu_window_seconds")
        same_clip = row.get("clip_sha256") == baseline.get("clip_sha256")
        same_snapshot = row.get("source_manifest_sha256") == baseline.get("source_manifest_sha256")
        if total is not None and reference is not None and seconds and reference_s and same_clip and same_snapshot:
            core = reference * seconds / reference_s
            row["attribution"] = {"core_reference_instructions": core,
                                  "app_minus_reference_instructions": total - core}


def median_tree(rows):
    if all(isinstance(row, dict) for row in rows):
        return {key: median_tree([row.get(key) for row in rows])
                for key in dict.fromkeys(key for row in rows for key in row)}
    if all(type(row) in (int, float) for row in rows):
        return statistics.median(rows)
    return rows[0] if all(row == rows[0] for row in rows) else None


def summarize_samples(samples):
    result = median_tree(samples)
    result["runs"] = samples
    if any("error" in sample for sample in samples):
        result["error"] = "one or more of the three counter runs failed"
        return result
    # The median of each PMU independently need not add up to the median total.
    # Keep the per-PMU breakdown from the observed run that supplied each total.
    cpu = result["cpu"]
    for metric in CPU_EVENTS:
        total = cpu[metric]
        representative = next((sample["cpu"] for sample in samples
                               if total is not None and sample["cpu"][metric] == total), None)
        cpu["per_pmu"][metric] = representative["per_pmu"][metric] if representative else None
    return result


def measure(args):
    engine_gt = engine_map()
    destination = args.out / "counters.json"
    builds = {"cpu_events": CPU_EVENTS, "engine_gt": engine_gt, "results": {}}
    if destination.exists():
        builds = json.loads(destination.read_text())
        if builds["engine_gt"] != engine_gt:
            raise RuntimeError("saved counter results use a different GPU engine map")
    matrix = [(app, name) for app in args.apps for name in args.scenarios]
    if "playback-software" in args.scenarios and args.core:
        matrix += [("core", "playback-software")]
    for app, name in matrix:
        binary = TARGET / "NOW" / app / "release" / ("ntctl" if app == "core" else f"nt-{app}")
        samples = []
        for repeat in range(1, 4):
            samples.append(measure_sample(args, app, name, binary, repeat, engine_gt))
        result = summarize_samples(samples)
        builds["results"].setdefault(app, {})[name] = result
        add_attribution(builds)
        write_json(destination, builds)


def measure_sample(args, app, name, binary, repeat, engine_gt):
    run = args.out / app / name / str(repeat)
    run.mkdir(mode=0o700, parents=True, exist_ok=True)
    receipt = run / "sample.json"
    identity = {"binary_sha256": file_hash(binary), "clip_sha256": file_hash(CLIP),
                "fixture_index_sha256": file_hash(args.fixtures / "index.txt"),
                "source_manifest_sha256": file_hash(OUT / "build/NOW-source.sha256")}
    if receipt.exists():
        result = json.loads(receipt.read_text())
        if any(result.get(key) != value for key, value in identity.items()):
            raise RuntimeError(f"cached sample has different inputs: {receipt}")
    if not receipt.exists() or (args.retry_errors and "error" in result):
        try:
            result = (core_run(binary, run, args.fixtures, engine_gt) if app == "core"
                      else app_run(binary, app, name, run, args.fixtures, engine_gt))
        except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
            result = {"error": str(error)}
        result.update(identity)
        result["machine"] = power_state()
        write_json(receipt, result)
    print(app, name, repeat, result.get("error", result.get("cpu", {}).get("instructions")), flush=True)
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=OUT / "counters")
    parser.add_argument("--fixtures", type=Path, default=FIXTURES)
    parser.add_argument("--apps", nargs="+", choices=APPS, default=APPS)
    parser.add_argument("--scenarios", nargs="+", choices=SCENARIOS, default=SCENARIOS)
    parser.add_argument("--core", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--retry-errors", action="store_true")
    opts = parser.parse_args()
    opts.out = opts.out.resolve()
    opts.fixtures = opts.fixtures.resolve()
    if not opts.out.is_relative_to(OUT):
        parser.error(f"output must be inside {OUT}")
    measure(opts)
