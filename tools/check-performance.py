"""Measure live YouTube RAM or compare fixture-backed NativeTube stories on NT-BENCH."""
import argparse
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import socket
import subprocess
import sys
import tempfile
import time

from performance.analysis import compare_metric, median, summarize
from performance.live import Cooldown, DEFAULT_HEIGHT, RunLog, benchmark_env, comparable_key, verify_live
from performance.procfs import INTERVAL_S, Sampler
from performance.stories import live_steps, steps_for

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bench"))
from shells import WindowWatcher

RUNTIME = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "native-tube-headless"
HEADLESS = ROOT / "tools/headless.sh"
FIXTURES = Path(os.environ.get("NT_FIXTURES", ROOT / ".cache/fixtures")).resolve()
CLIP = ROOT / ".cache/test-clip-720p30.mp4"
LIVE_COOLDOWN = RUNTIME / "perf-live-cooldown"
GUARD = ROOT / "tools/nt-explore/target/debug/nt-explore"
BUDGET = ROOT / "tools/perf-budget.json"
APP_ID = "nttest-bench"
APPS = ("iced", "gpui", "rune")
BUILDS = ("baseline", "candidate")
STORY_DEADLINE_S = 95


def command(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, capture_output=True, timeout=10, **kwargs).stdout


def hypr(*query):
    return json.loads(command("hyprctl", "-j", *query))


def test_windows():
    return [client for client in hypr("clients") if client["class"].startswith("nttest-")]


def window_ready(pid):
    monitor = next(monitor for monitor in hypr("monitors", "all") if monitor["name"] == "NT-BENCH")
    windows = test_windows()
    if (len(windows) != 1 or windows[0]["class"] != APP_ID
            or windows[0]["pid"] != pid or windows[0]["monitor"] != monitor["id"]):
        raise RuntimeError("expected exactly one owned window on NT-BENCH")
    window = windows[0]
    if window["at"] != [monitor["x"], monitor["y"]] or window["size"] != [1280, 800]:
        raise RuntimeError(f"unexpected NT-BENCH window geometry: {window['at']} {window['size']}")


def alive(pid):
    try:
        kind = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1][0]
        return kind not in ("Z", "X")
    except (FileNotFoundError, ProcessLookupError):
        return False


def wait_for(check, deadline, description, pid):
    while time.monotonic() < deadline:
        result = check()
        if result is not None:
            return result
        if not alive(pid):
            # The report is written just before Quit, so it can arrive between checks.
            result = check()
            if result is not None:
                return result
            raise RuntimeError(f"app exited while waiting for {description}")
        time.sleep(0.005)
    raise RuntimeError(f"timed out waiting for {description}")


def app_report(path):
    try:
        return json.loads(path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return None


class DesktopBusy(RuntimeError):
    pass


def check_desktop(pid_file):
    if test_windows():
        raise DesktopBusy("another native test window is active")
    if pid_file.is_file() and alive(int(pid_file.read_text())):
        raise DesktopBusy("NT-BENCH already has a live process")


def finish(guardian, pid, log_path, out):
    if log_path.exists():
        (out / "app.log").write_bytes(log_path.read_bytes())
    until = time.monotonic() + 2
    while alive(guardian) and time.monotonic() < until:
        time.sleep(0.02)
    if alive(guardian):
        raise DesktopBusy("headless guardian did not exit when its lease closed")
    if pid is not None:
        until = time.monotonic() + 2
        while any(window["pid"] == pid for window in test_windows()):
            if time.monotonic() >= until:
                raise DesktopBusy("owned window remained after its guardian exited")
            time.sleep(0.02)


def measure(binary, watcher, out, scenario, story, steps, *, height_override=None):
    pid_file = RUNTIME / "bench.pid"
    check_desktop(pid_file)
    out.mkdir()
    log_path = RUNTIME / "bench.log"
    report_path = out / "app-report.json"
    sample = {"artifact_dir": str(out)}
    guardian = pid = sampler = None

    with tempfile.TemporaryDirectory(dir=out, prefix="profile-") as data, \
            tempfile.TemporaryDirectory(prefix="nt-perf-lease-") as lease_dir, \
            socket.socket(socket.AF_UNIX) as listener:
        for app in ("iced", "gpui"):
            (Path(data) / f"{app}-prefs.json").write_text('{"autoplay":false}')
        (Path(data) / "rune-prefs.txt").write_text("autoplay=0\ntheatre=0\nguide=0\n")
        ipc = Path(lease_dir) / "mpv" if story == "live" else None
        env = benchmark_env(os.environ, data, scenario, report_path, FIXTURES, CLIP, ipc,
                            height_override=height_override)
        lease_path = Path(lease_dir) / "guard"
        listener.bind(str(lease_path))
        listener.listen(1)
        listener.settimeout(4)
        try:
            guardian = int(command(HEADLESS, "run", "bench", GUARD, "--guarded-app",
                                   lease_path, pid_file, binary, env=env))
            lease, _ = listener.accept()
            with lease:
                lease.settimeout(2)
                with lease.makefile("r") as ready:
                    app_pid, started_at = ready.readline().split()
                pid, started = int(app_pid), float(started_at)
                sampler = Sampler(pid, started)
                # Read the mark before compositor queries can delay observing it.
                with sampler, log_path.open("rb") as log:
                    events = RunLog(log, started, ipc)

                    def ready_home():
                        events.poll()
                        return events.home

                    def ready_report():
                        events.poll()
                        return app_report(report_path)

                    home = wait_for(ready_home, started + 12, "Home content", pid)
                    mapped = watcher.wait_after(started, max(0, started + 15 - time.monotonic()))
                    if mapped is None:
                        raise RuntimeError("app did not map a window")
                    window_ready(pid)
                    app = wait_for(ready_report, started + (70 if ipc else STORY_DEADLINE_S),
                                   "complete autopilot report", pid)
                    events.poll()
                if ipc:
                    write_json(out / "media-samples.json", events.player)
                sample.update(summarize(story, steps, app, sampler.samples, home - started,
                                        (mapped - started) * 1000))
                if ipc:
                    sample["media"] = verify_live(events, sample["phases"], Path(data) / "library.json",
                                                  height_override or DEFAULT_HEIGHT)
        except Cooldown as error:
            LIVE_COOLDOWN.write_text(str(time.time() + error.seconds))
            sample["error"] = str(error)
            sample["cooldown"] = True
        except (RuntimeError, ValueError, StopIteration, KeyError, TypeError, OSError,
                subprocess.SubprocessError) as error:
            sample["error"] = str(error) or type(error).__name__
        finally:
            if sampler is not None:
                write_json(out / "proc-samples.json", sampler.samples)
                sample["proc_samples"] = len(sampler.samples)
            if guardian is not None:
                finish(guardian, pid, log_path, out)
    return sample


def stripped_size(binary, out):
    destination = out / (binary.name + ".stripped")
    command("strip", "-o", destination, binary)
    size = destination.stat().st_size
    destination.unlink()
    return size


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2, allow_nan=False) + "\n")


def budgets(config, app, story):
    limits = dict(config["stories"].get(story, {}))
    app_limits = config["apps"][app]
    for metric in config["pss_budgets"].get(story, []):
        limits[metric] = app_limits["idle_pss_mib"]
    if story == "binary":
        limits["stripped_bytes"] = app_limits["stripped_bytes"]
    return limits


def compare_story(samples, limits, caps):
    names = set(limits) | set(caps)
    for runs in samples.values():
        for run in runs:
            names.update(run.get("metrics", {}))
    result = {}
    for metric in sorted(names):
        before, after = (median([sample.get("metrics", {}).get(metric) for sample in samples[build]])
                         for build in BUILDS)
        allowed, cap = limits.get(metric), caps.get(metric)
        result[metric] = {"baseline": before, "candidate": after,
                          "max_increase": allowed, "cap": cap,
                          **compare_metric(before, after, allowed, cap)}
    return result


def unit(metric):
    for suffix, label in (("_ms", "ms"), ("_mib", "MiB"), ("_seconds", "s"),
                          ("_percent", "% of one core"), ("_per_s", "count/s"), ("_bytes", "bytes")):
        if metric.endswith(suffix):
            return label
    return "count"


UNINSTRUMENTED = {
    "compositor_presentation": "application draw timestamps cannot verify compositor presentation",
    "input_to_present_scroll_latency": "autopilot command and draw hooks lack per-input presentation correlation",
    "missed_vsyncs_and_dropped_frames": "draw gaps do not identify intended presentation deadlines or dropped pictures",
    "gpu_memory": "no per-process GPU allocation telemetry",
    "energy": "no per-process energy telemetry",
    "wakeups": "task context switches are a scheduling proxy",
    "background_throttling": "headless output is unfocused throughout; no foreground-to-background transition",
    "back_restored_state": "baseline has no state probe; the story sends Back and samples after its settling period",
}


def compare(args):
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    for required in (CLIP, FIXTURES / "index.txt", GUARD):
        if not required.is_file():
            raise RuntimeError(f"missing benchmark input: {required}")
    config = json.loads(args.budget.read_text())
    stories = steps_for(args.mode)
    report = {
        "baseline": args.baseline, "mode": args.mode, "smoke": args.smoke,
        "provenance": "fixture_replay_local_clip",
        "requested_repeats": 1 if args.smoke else 3, "sample_interval_s": INTERVAL_S,
        "kernel": platform.release(), "arch": platform.machine(), "input": "autopilot",
        "clip_sha256": sha256(CLIP), "fixture_index_sha256": sha256(FIXTURES / "index.txt"),
        "fixtures": str(FIXTURES), "budgets": config,
        "uninstrumented": UNINSTRUMENTED, "apps": {}, "passed": False,
    }
    for story, steps in stories.items():
        write_json(out / f"{story}.json", steps)

    lines = [
        "| App | Story | Metric | Unit | Baseline | Candidate | Delta | Budget + | Cap | Result |",
        "| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |",
    ]
    failures = []

    def record(app, story, app_result, samples):
        caps = {"stripped_bytes": config["apps"][app]["max_stripped_bytes"]} if story == "binary" else {}
        app_result["runs"][story] = samples
        decisions = compare_story(samples, budgets(config, app, story), caps)
        app_result["results"][story] = decisions
        for metric, entry in decisions.items():
            entry["unit"] = unit(metric)
            if entry["result"] == "fail":
                failures.append(f"{app}/{story}/{metric}: {entry['reason']}")
            if entry["result"] not in ("report", "unavailable"):
                values = [entry[k] for k in ("baseline", "candidate", "delta", "max_increase", "cap")]
                columns = " | ".join("—" if v is None else f"{v:,.2f}" for v in values)
                lines.append(f"| {app} | {story} | {metric} | {entry['unit']} | {columns} | {entry['result']} |")

    try:
        watcher = WindowWatcher()
        for app in args.apps:
            paths = {build: args.binaries.resolve() / f"{build}-{app}" for build in BUILDS}
            result = {"binary_sha256": {build: sha256(path) for build, path in paths.items()},
                      "runs": {}, "results": {}}
            report["apps"][app] = result
            record(app, "binary", result, {
                build: [{"metrics": {"stripped_bytes": stripped_size(path, out)}}]
                for build, path in paths.items()
            })
            for story, steps in stories.items():
                samples = {build: [] for build in BUILDS}
                for i in range(report["requested_repeats"]):
                    order = BUILDS if i % 2 == 0 else BUILDS[::-1]
                    for build in order:
                        run_dir = out / f"{app}-{story}-{i + 1}-{build}"
                        print(f"Measuring {run_dir.name}", flush=True)
                        sample = measure(paths[build], watcher, run_dir, out / f"{story}.json", story, steps)
                        if "error" in sample:
                            failures.append(f"{run_dir.name}: {sample['error']}")
                        samples[build].append(sample)
                record(app, story, result, samples)
    except (DesktopBusy, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        failures.append(str(error))
    finally:
        report["failures"] = failures
        report["passed"] = not failures and not args.smoke
        report["status"] = "failed" if failures else "smoke" if args.smoke else "passed"
        write_json(out / "results.json", report)
        table = "\n".join(lines) + "\n"
        details = "\n".join(f"- {failure}" for failure in failures)
        summary = f"Status: {report['status']}\n\n{table}\n{details}\n"
        (out / "summary.md").write_text(summary)
        print(summary)
    return int(not report["passed"] and report["status"] != "smoke")


LIVE_RAM_METRICS = (
    ("idle_settled", "Home before browsing"), ("wheel_peak", "Home scrolling peak"),
    ("playing_settled", "First stream settled"), ("switched_settled", "Second stream settled"),
    ("playback_peak", "Playback peak"), ("journey_peak", "Peak from first Home content"),
    ("recovered_settled", "Back on Home"), ("retained", "Retained vs. first Home"),
)


def live_usage(args):
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    if LIVE_COOLDOWN.is_file():
        remaining = float(LIVE_COOLDOWN.read_text()) - time.time()
        if remaining > 0:
            raise RuntimeError(f"Live performance runs paused for another {math.ceil(remaining / 60)} min")
    if not GUARD.is_file():
        raise RuntimeError(f"missing benchmark guardian: {GUARD}")
    paths = {app: ROOT / f"apps/{app}/target/release/nt-{app}" for app in args.apps}
    for binary in paths.values():
        if not binary.is_file():
            raise RuntimeError(f"missing release binary: {binary}")
    steps = live_steps()
    scenario = out / "live-youtube.json"
    write_json(scenario, steps)
    results = {}
    height_cap = args.height or DEFAULT_HEIGHT
    height_source = "core::resolve::default_max_height" if args.height is None else "--height"
    report = {
        "provenance": "live_youtube", "quality_cap_p": height_cap, "quality_cap_source": height_source,
        "sample_interval_s": INTERVAL_S,
        "kernel": platform.release(), "arch": platform.machine(), "input": "autopilot",
        "units": {key: unit(key) for key in ("pss_mib", "rss_mib", "private_mib", "swap_mib")},
        "apps": results,
    }
    lines = ["Live YouTube RAM (MiB, one journey per app)", "",
             f"Auto quality cap {height_cap}p ({'app default' if args.height is None else '--height'}).", "",
             "| App | Workload | PSS | RSS | Private |",
             "| --- | --- | ---: | ---: | ---: |"]
    failure = None
    try:
        watcher = WindowWatcher()
        for app, binary in paths.items():
            print(f"Measuring {app} on live YouTube", flush=True)
            binary_hash = sha256(binary)
            run = measure(binary, watcher, out / f"live-{app}", scenario, "live", steps,
                          height_override=args.height)
            results[app] = {"binary_sha256": binary_hash, **run}
            if sha256(binary) != binary_hash:
                failure = f"{app}: release binary changed during the live journey"
                break
            if "error" in run:
                failure = f"{app}: {run['error']}"
                break
            for metric, label in LIVE_RAM_METRICS:
                values = [run["metrics"][f"{metric}_{kind}_mib"] for kind in ("pss", "rss", "private")]
                if None in values:
                    failure = f"{app}: missing RAM samples for {label}"
                row = " | ".join("—" if v is None else f"{v:,.1f}" for v in values)
                lines.append(f"| {app} | {label} | {row} |")
            if failure:
                break
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        failure = str(error)
    comparable = (not failure and len(results) > 1
                  and len({comparable_key(run["media"]) for run in results.values()}) == 1)
    report.update(comparable=comparable, failure=failure)
    report["status"] = "incomplete" if failure else "observed" if len(results) == 1 else (
        "comparable" if comparable else "different_streams")
    if failure:
        lines.extend(["", f"Live run incomplete. {failure}"])
    elif len(results) > 1:
        lines.extend(["", "Video IDs, dimensions, codecs and FPS match across the measured apps."
                      if comparable else "Apps received different video IDs or stream formats. Compare as separate observations."])
    write_json(out / "results.json", report)
    (out / "summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    return int(bool(failure) or report["status"] == "different_streams")


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("quick", "soak", "live"), default="quick")
    parser.add_argument("--smoke", action="store_true", help="exercise the harness with one run per build")
    parser.add_argument("--budget", type=Path, default=BUDGET)
    parser.add_argument("--height", type=int, metavar="PIXELS",
                        help=f"live Auto quality cap (app default {DEFAULT_HEIGHT}p)")
    parser.add_argument("baseline")
    parser.add_argument("binaries", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("apps", nargs="*", metavar="APP", help=", ".join(APPS))
    args = parser.parse_intermixed_args(argv)
    if args.height is not None:
        if args.mode != "live":
            parser.error("--height requires --mode live")
        if not 0 < args.height <= 2**32 - 1:
            parser.error("--height must be a positive 32-bit unsigned integer")
    if not args.apps:
        if args.mode != "live":
            parser.error("at least one app is required for fixture comparisons")
        args.apps = list(APPS)
    for app in args.apps:
        if app not in APPS:
            parser.error(f"unknown app {app!r} (choose from {', '.join(APPS)})")
    return args


def main():
    args = parse_args()
    RUNTIME.mkdir(parents=True, exist_ok=True)
    try:
        with (RUNTIME / "bench.explore.lock").open("a") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise DesktopBusy("NT-BENCH is in use") from None
            return live_usage(args) if args.mode == "live" else compare(args)
    except (DesktopBusy, ValueError, OSError, RuntimeError) as error:
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
