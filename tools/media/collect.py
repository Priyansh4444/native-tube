#!/usr/bin/env python3
"""Run one muted fixture-replay job at a time on the reserved headless outputs."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing, contextmanager, suppress
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

from metrics import APPS, MILESTONES, OUT, ROOT, derived_open, power_state, report_metrics, summarize, write_json
from recorder import HEIGHT, WIDTH, grim, record

sys.path.insert(0, str(ROOT / "bench"))
from shells import hypr_socket, proc_tree, pss_mb
sys.path.insert(0, str(ROOT / "tools"))
from performance.procfs import Sampler

TARGET = OUT.parent / "target"
HEADLESS = ROOT / "tools/headless.sh"
GUARD = ROOT / "tools/nt-explore/target/debug/nt-explore"
RUNTIME = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "native-tube-headless"
CLIP = ROOT / ".cache/test-clip-720p30.mp4"
FIXTURES = ROOT / ".cache/fixtures"
MEDIA_FIXTURES = OUT.parent / "fixtures"


def command(*args, **kwargs):
    return subprocess.run(args, check=True, capture_output=True, text=True, timeout=10,
                          **kwargs).stdout


def hypr(*query):
    return json.loads(command("hyprctl", "-j", *query))


def test_windows():
    return [w for w in hypr("clients") if w["class"].startswith("nttest-")]


def alive(pid):
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
        return stat.rsplit(") ", 1)[1][0] not in "ZX"
    except (FileNotFoundError, ProcessLookupError):
        return False


def live_pidfiles():
    for path in RUNTIME.glob("*.pid"):
        try:
            if alive(int(path.read_text())):
                return True
        except FileNotFoundError:
            continue
    return False


def slice_expired(_signal, _frame):
    raise TimeoutError("media slice reached its 110-second runtime limit")


@contextmanager
def headless_slot():
    RUNTIME.mkdir(exist_ok=True)
    with (RUNTIME / "bench.explore.lock").open("a") as lock:
        announced = False
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                pass
            else:
                if not live_pidfiles() and not test_windows():
                    previous = signal.signal(signal.SIGALRM, slice_expired)
                    signal.setitimer(signal.ITIMER_REAL, 110)
                    try:
                        yield
                    finally:
                        signal.setitimer(signal.ITIMER_REAL, 0)
                        signal.signal(signal.SIGALRM, previous)
                    return
                fcntl.flock(lock, fcntl.LOCK_UN)
            if not announced:
                print("Waiting for the active headless test to finish.", flush=True)
                announced = True
            time.sleep(2)


def window_geometry(pid, output, full_output):
    windows = test_windows()
    monitor = next(m for m in hypr("monitors", "all") if m["name"] == f"NT-{output.upper()}")
    if ((monitor["width"], monitor["height"], monitor["scale"]) != (WIDTH, HEIGHT, 1)
            or len(windows) != 1 or windows[0]["pid"] != pid
            or windows[0]["class"] != f"nttest-{output}" or windows[0]["monitor"] != monitor["id"]):
        raise RuntimeError(f"expected only the owned test window on NT-{output.upper()}")
    window = windows[0]
    if full_output and (window["at"] != [monitor["x"], monitor["y"]]
                        or window["size"] != [WIDTH, HEIGHT]):
        raise RuntimeError("test window does not cover its output")


def watch_window(events, output):
    # Match the dedicated test class, then independently verify PID and monitor.
    with events.makefile() as stream:
        for line in stream:
            seen = time.monotonic()
            prefix, _, payload = line.partition(">>")
            if prefix == "openwindow" and payload.split(",", 3)[2:3] == [f"nttest-{output}"]:
                return seen
    raise RuntimeError("no matching openwindow event")


@contextmanager
def session(binary, output, run, scenario, fixtures, *, full_output=False, fixture_mode="replay", hooks=None):
    for required in (binary, GUARD, CLIP, fixtures / "index.txt"):
        if not required.is_file():
            raise RuntimeError(f"missing headless input: {required}")
    log = RUNTIME / f"{output}.log"
    pid_file = RUNTIME / f"{output}.pid"
    with headless_slot(), tempfile.TemporaryDirectory(prefix="nt-media-") as directory, \
            socket.socket(socket.AF_UNIX) as listener, socket.socket(socket.AF_UNIX) as events:
        profile = Path(directory) / "profile"
        profile.mkdir()
        for app in ("iced", "gpui"):
            (profile / f"{app}-prefs.json").write_text('{"autoplay":false}')
        (profile / "rune-prefs.txt").write_text("autoplay=0\ntheatre=0\nguide=0\n")
        activation = {"XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"}
        env = {key: value for key, value in os.environ.items()
               if not key.startswith("NT_") and key not in activation}
        env.update(
            NT_MUTE="1", NT_NO_MPRIS="1", NT_LIVE="0", NT_TRACE="1",
            NT_FIXTURE_MODE=fixture_mode, NT_FIXTURES=str(fixtures), NT_DATA_DIR=str(profile),
            NT_VIDEO_OVERRIDE=str(CLIP), NT_AUTOPILOT=str(scenario), NT_REPORT=str(run / "report.json"),
        )
        env.update(hooks or {})
        path = Path(directory) / "lease"
        listener.bind(str(path))
        listener.listen(1)
        listener.settimeout(4)
        events.settimeout(12)
        events.connect(hypr_socket())
        executor = ThreadPoolExecutor(max_workers=1)
        window = executor.submit(watch_window, events, output)
        guardian = None
        try:
            # Same guard mode as tools/check-performance.py: without the flag nt-explore
            # would treat the lease path as an app and start an exploration session.
            guardian = int(command(HEADLESS, "run", output, GUARD, "--guarded-app",
                                   path, pid_file, binary, env=env))
            lease, _ = listener.accept()
            with closing(lease), lease.makefile() as ready:
                lease.settimeout(4)
                pid, started = ready.readline().split()
                pid, started = int(pid), float(started)
                mapped_ms = (window.result(timeout=12) - started) * 1000
                if mapped_ms < 0:
                    raise RuntimeError("matching test window mapped before this app was launched")
                window_geometry(pid, output, full_output)
                yield pid, started, mapped_ms, log
        finally:
            with suppress(OSError):
                events.shutdown(socket.SHUT_RDWR)
            executor.shutdown()
            until = time.monotonic() + 3
            while guardian is not None and alive(guardian) and time.monotonic() < until:
                time.sleep(0.02)
            if guardian is not None and alive(guardian):
                raise RuntimeError("media guardian remained alive after its lease closed")
            if guardian is not None:
                shutil.copyfile(log, run / "app.log")


def maps_mpv(pid):
    try:
        return "libmpv.so" in Path(f"/proc/{pid}/maps").read_text()
    except OSError:
        return None


def wait_for(check, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result is not None:
            return result
        time.sleep(0.01)
    return None


def measure_one(binary, run, fixtures):
    with session(binary, "bench", run, Path(__file__).with_name("measure.json"), fixtures) as (
            pid, started, mapped_ms, log), Sampler(pid, started) as sampler:
        values = {"window_ms": mapped_ms}

        def home():
            with log.open() as stream:
                if any(line.startswith("NT_MARK home_loaded ") for line in stream):
                    return time.monotonic()
            return None

        seen = wait_for(home, 8)
        if seen is not None:
            values["home_ms"] = (seen - started) * 1000
        time.sleep(2)
        if seen is not None and alive(pid):
            pss = pss_mb(proc_tree(pid))
            if pss > 0:
                values["idle_pss_mib"] = pss
                # Whether the player library was already loaded (prewarm) when idle PSS was taken.
                values["idle_libmpv_mapped"] = maps_mpv(pid)
        report = wait_for(lambda: read_report(run), 35)
        if report is None:
            values["error"] = "app did not write an autopilot report"
        else:
            values.update(report_metrics(report))
            values["failed_waits"] = report.get("failed_waits", [])
            add_derived_open(values, report)
            # Peak over the whole scripted run: Home, six seconds of playback and the warm switch.
            pss = [sum(proc["pss_mib"] for proc in row["processes"]) for row in sampler.samples
                   if row["processes"] and all(proc["pss_mib"] is not None for proc in row["processes"])]
            if pss and not values["failed_waits"]:
                values["peak_pss_mib"] = max(pss)
                values["pss_samples"] = len(pss)
        return values


def add_derived_open(values, report):
    sleep_ms = 1000 * json.loads(Path(__file__).with_name("measure.json").read_text())[1]["sleep"]
    if values.get("open_ms") is None and not values.get("failed_waits"):
        if (derived := derived_open(report, sleep_ms)) is not None:
            values["open_ms"] = derived
            values["open_basis"] = "home_loaded + scenario sleep (no command timestamps)"


def read_report(run):
    try:
        return json.loads((run / "report.json").read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return None


def record_one(app, binary, run, fixtures):
    destination = run / f"{app}.mp4"
    report_path = run / "report.json"
    with session(binary, app, run, Path(__file__).with_name("tour.json"),
                 fixtures, full_output=True) as (pid, _, _, log):
        if not wait_for(lambda: True if "NT_MARK home_loaded " in log.read_text(errors="replace") else None, 8):
            raise RuntimeError(f"{app}: Home did not load before capture")
        time.sleep(0.2)
        capture = record(destination, lambda: grim(f"NT-{app.upper()}"),
                         lambda: alive(pid) and not report_path.exists())
    report = read_report(run)
    if report is None or report.get("failed_waits"):
        destination.rename(run / f"{app}.failed.mp4")
        raise RuntimeError(f"{app}: incomplete tour, see {run / 'app.log'}")
    write_json(run / "capture.json", capture)


def prime(app, binary, run, fixtures):
    # Use a private copy for a single logged-out, local-clip run through the app's polite gate.
    with session(binary, app, run, Path(__file__).with_name("tour.json"),
                 fixtures, fixture_mode="auto"):
        report = wait_for(lambda: read_report(run), 90)
    if report is None or report.get("failed_waits"):
        raise RuntimeError(f"{app}: fixture warm-up did not finish, see {run / 'app.log'}")


def inspect_binary(binary):
    with binary.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    with tempfile.TemporaryDirectory() as directory:
        stripped = Path(directory) / "stripped"
        command("strip", "-o", stripped, binary)
        return {"stripped_bytes": stripped.stat().st_size, "sha256": digest}


def collect(args):
    samples, offline, binaries = {}, {}, {}
    for rev in args.milestones:
        samples[rev], offline[rev] = {}, {}
        for app in args.apps:
            binary = TARGET / rev / app / "release" / f"nt-{app}"
            if not binary.is_file():
                offline[rev][app] = {"reason": "release binary unavailable"}
                continue
            offline[rev][app] = inspect_binary(binary)
            if args.offline:
                continue
            if rev == "96df4ec":
                binary = TARGET / "96df4ec-routed" / app / "release" / f"nt-{app}"
                if not binary.is_file():
                    offline[rev][app]["reason"] = "test-class routing build unavailable"
                    continue
                offline[rev][app]["runtime_routing_patch"] = "66f442f (app ID only)"
                with binary.open("rb") as stream:
                    offline[rev][app]["runtime_sha256"] = hashlib.file_digest(stream, "sha256").hexdigest()
            binaries[rev, app] = binary
            samples[rev][app] = [None, None, None]
    # Round-robin: run 1 of every build, then run 2, then run 3, so a change in background
    # load during the pass spreads over all builds instead of biasing one milestone.
    for i in range(1, args.runs + 1):
        for (rev, app), binary in binaries.items():
            run = args.out / rev / app / f"run-{i}"
            receipt = run / "sample.json"
            if receipt.is_file():
                values = json.loads(receipt.read_text())
            else:
                run.mkdir(mode=0o700, parents=True, exist_ok=True)
                power = power_state()
                try:
                    values = measure_one(binary, run, args.fixtures)
                except (RuntimeError, OSError, ValueError, StopIteration, subprocess.SubprocessError) as e:
                    values = {"error": str(e)}
                values["power"] = power
                write_json(receipt, values)
            samples[rev][app][i - 1] = values
            print(rev, app, i, values, flush=True)
            write_json(args.out / "metrics.json", summarize(samples, offline))
    write_json(args.out / "metrics.json", summarize(samples, offline))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures", type=Path, default=FIXTURES)
    modes = parser.add_subparsers(dest="mode", required=True)
    measure = modes.add_parser("metrics")
    measure.add_argument("--milestones", nargs="+", choices=MILESTONES, default=MILESTONES)
    measure.add_argument("--apps", nargs="+", choices=APPS, default=APPS)
    measure.add_argument("--out", type=Path, default=OUT / "measurements")
    measure.add_argument("--offline", action="store_true", help="measure release sizes only")
    measure.add_argument("--runs", type=int, default=3, choices=(1, 2, 3),
                         help="fewer than three only for probes; medians need three")
    capture = modes.add_parser("record")
    capture.add_argument("app", choices=APPS)
    capture.add_argument("--out", type=Path, default=OUT / "tours")
    warmup = modes.add_parser("prime", help="fill missing tour fixtures through one app run")
    warmup.add_argument("app", choices=APPS)
    warmup.add_argument("--out", type=Path, default=OUT / "fixture-warmup")
    args = parser.parse_args()
    args.out = args.out.resolve()
    args.fixtures = args.fixtures.resolve()
    if not args.out.is_relative_to(OUT):
        parser.error(f"output must be inside {OUT}")
    if args.mode == "metrics":
        collect(args)
        return
    run = args.out / args.app
    binary = TARGET / "NOW" / args.app / "release" / f"nt-{args.app}"
    if args.mode == "prime":
        if args.fixtures.parent != MEDIA_FIXTURES.parent or args.fixtures == OUT:
            parser.error(f"prime requires a private fixture directory under {MEDIA_FIXTURES.parent}")
        if not binary.is_file():
            parser.error(f"missing build: {binary}")
        if not args.fixtures.exists():
            shutil.copytree(FIXTURES, args.fixtures)
        run.mkdir(mode=0o700, parents=True)
        prime(args.app, binary, run, args.fixtures)
        return
    run.mkdir(mode=0o700, parents=True)
    record_one(args.app, binary, run, args.fixtures)


if __name__ == "__main__":
    main()
