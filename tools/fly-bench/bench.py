#!/usr/bin/env python3
"""Run the NativeTube media workloads on a dedicated Sway output."""
import argparse
from contextlib import contextmanager, suppress
import json
import os
from pathlib import Path
import selectors
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT / "tools"), str(ROOT / "tools/media")]
from collect import inspect_binary, maps_mpv, read_report
from metrics import APPS, report_metrics, write_json
from performance.analysis import phase_metrics
from performance.procfs import Sampler
from performance.stories import steps_for
from recorder import record
from resources import SOFTWARE, mapping_state, read_phase, render_path

BIN = ROOT / "bin"
FIXTURES = ROOT / ".cache/media/fixtures"
CLIP = ROOT / ".cache/test-clip-720p30.mp4"
MEDIA = ROOT / "tools/media"
RESOLUTION = (1280, 800)
COLD_ID, WARM_ID = "aqz-KE-bpKQ", "5C_HPTJg5ek"  # Same pair as the saved live ntctl run.
PERF = ["perf", "stat", "--no-big-num", "-x,", "-e", "instructions:u,cycles:u"]


@contextmanager
def process(args, log, *, env=None, stdin=None, stdout=None, group=False):
    with log.open("wb") as stream:
        child = subprocess.Popen(args, env=env, stdin=stdin, stdout=stdout or stream,
                                 stderr=stream, start_new_session=group)
        try:
            yield child
        finally:
            for sig in (signal.SIGTERM, signal.SIGKILL):
                with suppress(ProcessLookupError):
                    if group:
                        os.killpg(child.pid, sig)
                    elif child.poll() is None:
                        child.send_signal(sig)
                try:
                    child.wait(timeout=4)
                    break
                except subprocess.TimeoutExpired:
                    continue
            child.wait()


def grab(destination=None):
    cmd = ["grim", "-o", "HEADLESS-1"]
    if destination is None:
        return subprocess.check_output([*cmd, "-t", "ppm", "-"], timeout=3)
    subprocess.run([*cmd, str(destination)], check=True, timeout=3)


def covers_output(pid, *, fullscreen):
    tree = json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"], timeout=3))

    def windows(node):
        if node.get("pid") == pid:
            yield node
        for child in node["nodes"] + node["floating_nodes"]:
            yield from windows(child)

    (window,) = list(windows(tree)) or ({},)
    rect = window.get("rect", {})
    return (bool(window.get("fullscreen_mode")) is fullscreen
            and (rect.get("x"), rect.get("y"), rect.get("width"), rect.get("height")) == (0, 0, *RESOLUTION))


def app_env(profile):
    for name in ("iced", "gpui"):
        (profile / f"{name}-prefs.json").write_text('{"autoplay":false}')
    (profile / "rune-prefs.txt").write_text("autoplay=0\ntheatre=0\nguide=0\n")
    return {k: v for k, v in os.environ.items() if not k.startswith("NT_")} | {
        "NT_MUTE": "1", "NT_NO_MPRIS": "1", "NT_LIVE": "0", "NT_TRACE": "1",
        "NT_FIXTURE_MODE": "replay", "NT_FIXTURES": str(FIXTURES),
        "NT_VIDEO_OVERRIDE": str(CLIP), "NT_DATA_DIR": str(profile),
    }


IPC_HEADER = struct.Struct("<6sII")


def read_ipc(stream):
    header = stream.read(IPC_HEADER.size)
    if not header:
        return None
    magic, length, _ = IPC_HEADER.unpack(header)
    if magic != b"i3-ipc":
        raise RuntimeError("invalid Sway IPC reply")
    return json.loads(stream.read(length))


@contextmanager
def window_events():
    mapped = {}
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(3)
        connection.connect(os.environ["SWAYSOCK"])
        request = b'["window"]'
        connection.sendall(IPC_HEADER.pack(b"i3-ipc", len(request), 2) + request)
        with connection.makefile("rb") as stream:
            if not (read_ipc(stream) or {}).get("success"):
                raise RuntimeError("could not subscribe to Sway window events")
            connection.settimeout(None)

            def observe():
                while event := read_ipc(stream):
                    received = time.monotonic()
                    if event.get("change") == "new":
                        mapped[event["container"].get("pid")] = received

            watcher = threading.Thread(target=observe, daemon=True)
            watcher.start()
            try:
                yield mapped
            finally:
                connection.shutdown(socket.SHUT_RDWR)
                watcher.join(timeout=3)


def scenario(mode):
    idle = [{"wait_mark": "home_loaded", "timeout": 8}, {"sleep": 6}]
    match mode:
        case "cold":
            return idle
        case "wheel":
            return idle + [{"phase": "wheel"}] + [
                step for step in steps_for("quick")["browse"] if "repeat" in step
            ] + [{"phase": ""}]
        case "tour":
            return json.loads((MEDIA / "tour.json").read_text())
        case _:
            return json.loads((MEDIA / "measure.json").read_text())


def playback_steps(values, report):
    values.update(report_metrics(report))
    if report.get("failed_waits"):
        for key in ("open_ms", "warm_ms", "playback_cpu_percent"):
            values[key] = None


def app_run(app, mode, run, perf):
    log = run / "app.log"
    commands = run / "scenario.json"
    write_json(commands, scenario(mode))
    checks, home = [], None
    capture = None
    with tempfile.TemporaryDirectory() as profile, window_events() as mapped:
        env = app_env(Path(profile)) | {"NT_AUTOPILOT": str(commands), "NT_REPORT": str(run / "report.json")}
        if mode == "tour":
            env["NT_FIXTURES"] = str(FIXTURES.with_name("tour-fixtures-now"))
        if mode == "cold":
            env["NT_NO_PREWARM"] = "1"
        if mode == "software":
            env[SOFTWARE[app]] = "1"
        started = time.monotonic()
        with process([BIN / f"nt-{app}"], log, env=env) as child, Sampler(child.pid, started) as sampler, perf(child.pid):
            deadline = started + (65 if mode == "tour" else 35)
            geometry_checked = False
            report = None
            with log.open(errors="replace") as stream:
                while time.monotonic() < deadline and child.poll() is None:
                    if not geometry_checked and child.pid in mapped:
                        geometry_checked = covers_output(child.pid, fullscreen=False)
                    for line in stream:
                        if home is None and line.startswith("NT_MARK home_loaded "):
                            home = time.monotonic() - started
                    elapsed = time.monotonic() - started
                    if home is not None and home + 2 <= elapsed <= home + 3.8:
                        checks.append(maps_mpv(child.pid))
                    if mode == "tour" and home is not None and geometry_checked:
                        capture = record(run / f"{app}.mp4", grab,
                                         lambda: child.poll() is None and read_report(run) is None)
                        report = read_report(run)
                        break
                    if report := read_report(run):
                        break
                    time.sleep(0.01)
            pid = child.pid
            exit_code = child.poll()
    samples = [row for row in sampler.samples if row["processes"]]
    write_json(run / "procfs.json", samples)
    body = log.read_text(errors="replace")
    values = {
        "window_ms": (mapped[pid] - started) * 1000 if geometry_checked else None,
        "home_ms": home * 1000 if home is not None else None,
        "libmpv_mapped": mapping_state(checks), "libmpv_checks": len(checks),
    }
    if report is None or not geometry_checked:
        return values | {"error": f"no usable window/report (exit={exit_code})"}
    values["failed_waits"] = report.get("failed_waits", [])
    if values["failed_waits"]:
        return values
    if mode in ("cold", "default") and home is not None:
        if mapping_state(checks) is (mode == "default"):
            values["idle_pss_mib"] = read_phase(samples, home + 2, 1.8)["settled_pss_mib"]
    if mode in ("default", "software"):
        playback_steps(values, report)
        values["path"] = render_path(app, body, forced_software=mode == "software")
        values["peak_pss_mib"] = phase_metrics(samples)["peak_pss_mib"]
    if mode == "wheel":
        values["wheel_cpu_percent"] = next(p["cpu_percent"] for p in report["phases"] if p["name"] == "wheel")
    if capture:
        values["capture"] = capture
    return values


def core_run(run, perf):
    out = run / "frame.png"
    log = run / "ntctl.log"
    first = None
    with tempfile.TemporaryDirectory() as profile:
        env = app_env(Path(profile))
        started = time.monotonic()
        with process([BIN / "ntctl", "play", "PQf9OAI05TQ", out, "6"], log, env=env) as child, \
                Sampler(child.pid, started) as sampler, perf(child.pid):
            deadline = started + 35
            while child.poll() is None and time.monotonic() < deadline:
                if first is None and "first frame after" in log.read_text(errors="replace"):
                    first = time.monotonic() - started
                time.sleep(0.04)
            exit_code = child.poll()
    samples = [row for row in sampler.samples if row["processes"]]
    write_json(run / "procfs.json", samples)
    if exit_code != 0 or first is None or not out.is_file():
        return {"error": f"ntctl playback incomplete (exit={exit_code})"}
    phase = read_phase(samples, first, 6)
    return {"playback_cpu_percent": phase["cpu_percent"], "peak_pss_mib": phase["peak_pss_mib"],
            "playback_pss_mib": phase["settled_pss_mib"]}


@contextmanager
def counters(run, pid, enabled):
    if not enabled:
        yield
        return
    with process([*PERF, "-p", str(pid)], run / "perf.csv") as counter:
        try:
            yield
        finally:
            if counter.poll() is None:
                counter.send_signal(signal.SIGINT)
                with suppress(subprocess.TimeoutExpired):
                    counter.wait(timeout=2)


def browser_eval(port, expression, *args):
    script = ROOT / "tools/fly-bench/eval.mjs"
    return json.loads(subprocess.check_output(["node", script, str(port), expression, *args],
                                              timeout=3, stderr=subprocess.DEVNULL))


def cdp_read(child, timeout):
    with selectors.DefaultSelector() as selector:
        selector.register(child.stdout, selectors.EVENT_READ)
        if not selector.select(timeout):
            return None
        line = child.stdout.readline()
    if not line:
        raise RuntimeError("CDP driver exited")
    return json.loads(line)


def cdp_send(child, command):
    child.stdin.write((command + "\n").encode())
    child.stdin.flush()


BOT_PROBE = """(() => {
    const text = document.body?.innerText ?? '';
    if (/sign in to confirm you.re not a bot/i.test(text)) return 'bot check';
    if (/video unavailable|this video is not available/i.test(text)) return 'video unavailable';
    if (location.hostname.includes('consent.')) return 'consent interstitial';
    return null;
})()"""


VISIBLE_AD_SKIP = """(() => {
    const player = document.querySelector('#movie_player.ad-showing');
    if (!player) return null;
    for (const button of player.querySelectorAll('button')) {
        const label = (button.getAttribute('aria-label') || button.innerText).trim();
        if (!button.matches('.ytp-skip-ad-button, .ytp-ad-skip-button, .ytp-ad-skip-button-modern')
            && !/^skip(?: ad)?$/i.test(label)) continue;
        if (button.disabled || button.getAttribute('aria-disabled') === 'true') continue;
        const rect = button.getBoundingClientRect();
        const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2;
        if (rect.width > 0 && rect.height > 0 && button.contains(document.elementFromPoint(x, y))) {
            return {x, y};
        }
    }
    return null;
})()"""


def web_open(cdp, port, run, vid, mode, *, skip_visible_ads=False):
    cdp_send(cdp, f"open {vid} {mode}")
    deadline = time.monotonic() + 65
    ad_skips = 0
    while time.monotonic() < deadline:
        if event := cdp_read(cdp, 2):
            if event["event"] == "first_frame" and event["id"] == vid:
                if skip_visible_ads:
                    event["visible_ad_skips"] = ad_skips
                return event
        try:
            outcome = browser_eval(port, BOT_PROBE)
            if skip_visible_ads and not outcome and browser_eval(port, VISIBLE_AD_SKIP, "click"):
                ad_skips += 1
        except (OSError, ValueError, subprocess.SubprocessError):
            continue
        if outcome:
            grab(run / f"{mode}-interstitial.png")
            return {"ok": False, "id": vid, "status": outcome}
    return {"ok": False, "id": vid, "status": "CDP deadline"}


def web_tour(cdp, port, run):
    # Navigate the same public steps as the app tour while capturing the separate VM display.
    def navigate():
        browser_eval(port, "location.href='https://www.youtube.com/'")
        time.sleep(4)
        for _ in range(10):
            browser_eval(port, "window.scrollBy(0,120)")
            time.sleep(0.1)
        browser_eval(port, "location.href='https://www.youtube.com/results?search_query=rust'")
        time.sleep(3)
        cold = web_open(cdp, port, run, COLD_ID, "cold", skip_visible_ads=True)
        time.sleep(6 if cold["ok"] else 2)
        warm = web_open(cdp, port, run, WARM_ID, "warm", skip_visible_ads=True) if cold["ok"] else None
        if warm and warm["ok"]:
            time.sleep(3)
            for _ in range(2):
                browser_eval(port, "history.back()")
                time.sleep(3)
        write_json(run / "navigation.json", {"cold": cold, "warm": warm})

    failures = []

    def drive():
        try:
            navigate()
        except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
            failures.append(str(error))

    driver = threading.Thread(target=drive)
    driver.start()
    try:
        capture = record(run / "youtube.mp4", grab, driver.is_alive)
    finally:
        driver.join()
    return {"capture": capture, "navigation_errors": failures}


def web_run(run, perf, *, tour=False):
    port = 9333
    with tempfile.TemporaryDirectory() as profile:
        args = ["chromium", f"--user-data-dir={profile}", "--no-first-run", "--no-default-browser-check",
                "--ozone-platform=wayland", "--window-size=1280,800", "--start-fullscreen", "--mute-audio",
                "--autoplay-policy=no-user-gesture-required", "--disable-sync",
                f"--remote-debugging-port={port}", "about:blank"]
        env = dict(os.environ)
        with process(args, run / "chromium.log", env=env, group=True) as browser, process(
            ["node", ROOT / "bench/cdp.mjs", str(port)], run / "cdp.log",
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        ) as cdp:
            ready = cdp_read(cdp, 15)
            if not ready or ready["event"] != "ready":
                raise RuntimeError("Chromium CDP unavailable")
            deadline = time.monotonic() + 5
            while not covers_output(browser.pid, fullscreen=True):
                if time.monotonic() >= deadline:
                    raise RuntimeError("Chromium did not map a full-output window")
                time.sleep(0.05)
            if tour:
                return web_tour(cdp, port, run)
            started = time.monotonic()
            results = {}
            with Sampler(browser.pid, started) as sampler, perf(browser.pid):
                for mode, vid in (("cold", COLD_ID), ("warm", WARM_ID)):
                    results[mode] = opened = web_open(cdp, port, run, vid, mode)
                    if not opened["ok"]:
                        break
                    first = time.monotonic() - started
                    time.sleep(6)
                    cdp_send(cdp, "stats")
                    stats = cdp_read(cdp, 5)
                    opened["stats"] = stats and {key: stats.get(key) for key in ("t", "w", "h", "total", "dropped")}
                    opened["resources"] = read_phase(sampler.samples, first, 6)
            write_json(run / "procfs.json", sampler.samples)
    return results


def sample(out, name, perf_enabled, run_case):
    run = out / name
    run.mkdir(parents=True, exist_ok=True)
    receipt = run / "sample.json"
    if receipt.exists():
        return json.loads(receipt.read_text())
    print(name, flush=True)
    try:
        result = run_case(run, lambda pid: counters(run, pid, perf_enabled))
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        result = {"error": str(error)}
    write_json(receipt, result)
    print(result, flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path)
    parser.add_argument("--runs", type=int, default=3, choices=(1, 3))
    parser.add_argument("--only", choices=(*APPS, "core", "web"))
    parser.add_argument("--no-tours", action="store_true")
    parser.add_argument("--tours-only", action="store_true")
    parser.add_argument("--no-web", action="store_true")
    args = parser.parse_args()
    out = args.out.resolve()
    outputs = json.loads((out / "outputs.json").read_text())
    expected = [o for o in outputs if o["active"]]
    if len(expected) != 1 or expected[0]["name"] != "HEADLESS-1" or expected[0]["scale"] != 1 or (
        expected[0]["rect"]["width"], expected[0]["rect"]["height"]
    ) != RESOLUTION:
        raise RuntimeError("expected one active 1280×800 headless output at scale 1")
    with (out / "perf-probe.log").open("wb") as stream:
        probe = subprocess.run([*PERF, "--", "true"], stdout=stream, stderr=subprocess.STDOUT)
    perf = probe.returncode == 0
    sizes = {name: inspect_binary(BIN / binary) for name, binary in
             [(app, f"nt-{app}") for app in APPS] + [("ntctl", "ntctl")]}
    write_json(out / "binaries.json", sizes)

    blocked = None
    repeats = 0 if args.tours_only else args.runs
    for repeat in range(1, repeats + 1):
        for app in APPS:
            if args.only and args.only != app:
                continue
            for mode in ("default", "software", "cold", "wheel"):
                sample(out, f"{app}/{mode}/run-{repeat}", perf,
                       lambda run, count: app_run(app, mode, run, count))
        if args.only in (None, "core"):
            sample(out, f"ntctl/run-{repeat}", perf, core_run)
        if args.only in (None, "web") and not args.no_web and blocked is None:
            result = sample(out, f"web/run-{repeat}", perf, web_run)
            cold = result.get("cold", {})
            if cold.get("status") in ("bot check", "consent interstitial", "video unavailable"):
                blocked = cold["status"]
                write_json(out / "web/status.json", {"status": blocked, "attempts": repeat})
    if not args.no_tours:
        for app in APPS:
            if args.only in (None, app):
                sample(out, f"tours/{app}", False, lambda run, count: app_run(app, "tour", run, count))
        if args.only in (None, "web") and not args.no_web and blocked is None:
            sample(out, "tours/web", False, lambda run, count: web_run(run, count, tour=True))


if __name__ == "__main__":
    main()
