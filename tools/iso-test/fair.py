#!/usr/bin/env python3
"""Matched fairness re-measurement of the three apps.

One driver for the isolated laptop container (Intel render node) and the GPU-less Fly VM
(llvmpipe). Every app runs the same scenarios under the same prewarm policy, in round-robin
order (iced, GPUI, Rune, then rotated), with the first round kept apart as the cold run.
A run starts only while the host's 1-minute load average is at most --max-load.

Per run it records Sway's window-map time, the app's own marks (Home, open and switch to
the unified first frame), process-tree PSS from /proc, in-app CPU per phase, libmpv/EGL
mappings during the Home idle window, core's thumbnail cache bytes, and user-space
instructions for bounded windows (perf stat, enabled and disabled at autopilot snapshot
boundaries, so no window includes the harness).
"""
import argparse
from contextlib import ExitStack, contextmanager
import csv
import json
import os
from pathlib import Path
import select
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

HERE = Path(__file__).resolve().parent
for candidate in (HERE / "tools", HERE.parent):
    if (candidate / "performance/procfs.py").is_file():
        sys.path.insert(0, str(candidate))
        break
from performance.procfs import Sampler  # noqa: E402

APPS = ("iced", "gpui", "rune")
SHIPPED = {"iced": "full", "gpui": "full", "rune": "off"}
SOFTWARE = {"iced": "NT_SOFTWARE_VIDEO", "gpui": "NT_SOFTWARE_VIDEO", "rune": "NT_RUNE_SOFTWARE"}
GPU_ATTACH = {"iced": "NT_EVENT gpu_surface_attached",
              "gpui": "attached Wayland GPU surface",
              "rune": "Wayland GL video attached"}
CPU_PMUS = ("cpu_core", "cpu_atom", "cpu_lowpower")
PMU_SYSFS = Path("/sys/bus/event_source/devices")
EGL_LIBS = ("libEGL_mesa", "libgallium", "iris_dri", "swrast_dri", "kms_swrast")


def scenario(suite):
    home = [{"wait_mark": "home_loaded", "timeout": 12}]
    if suite == "latency":
        return home + [
            {"snapshot": "home-end"},
            {"phase": "home_idle"}, {"sleep": 4.0}, {"phase": ""},
            {"snapshot": "open-begin"},
            {"cmd": "open_video", "index": 1}, {"wait_mark": "playback_started", "timeout": 12},
            {"snapshot": "open-end"},
            {"sleep": 0.5},
            {"snapshot": "playback-begin"}, {"phase": "playback"}, {"sleep": 6.0}, {"phase": ""},
            {"snapshot": "playback-end"},
            {"snapshot": "switch-begin"},
            {"cmd": "open_video", "index": 0}, {"wait_mark": "playback_started", "timeout": 12},
            {"snapshot": "switch-end"},
            {"sleep": 1.0},
        ]
    if suite == "software":
        return home + [
            {"snapshot": "home-end"}, {"sleep": 2.0},
            {"cmd": "open_video", "index": 1}, {"wait_mark": "playback_started", "timeout": 12},
            {"sleep": 0.5},
            {"snapshot": "playback-begin"}, {"phase": "playback"}, {"sleep": 6.0}, {"phase": ""},
            {"snapshot": "playback-end"},
            {"sleep": 0.5},
        ]
    if suite == "wheel":
        return home + [
            {"snapshot": "home-end"},
            {"phase": "home_idle"}, {"sleep": 2.0}, {"phase": ""},
            {"snapshot": "wheel-begin"}, {"phase": "wheel"},
            {"repeat": 15, "every": 0.1, "do": {"cmd": "scroll", "ticks": 1}},
            {"repeat": 12, "every": 0.1, "do": {"cmd": "scroll", "ticks": -1}},
            {"phase": ""}, {"snapshot": "wheel-end"},
            {"sleep": 0.5},
        ]
    raise ValueError(suite)


WINDOWS = {"latency": ("home", "open", "playback", "switch"), "software": ("home", "playback"),
           "wheel": ("home", "wheel")}


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, allow_nan=False) + "\n")


def read_sys(path):
    try:
        return Path(path).read_text().strip()
    except OSError:
        return None


def machine():
    supplies = sorted(Path("/sys/class/power_supply").glob("*"))
    ac = None
    for supply in supplies:
        if read_sys(supply / "type") == "Mains":
            ac = read_sys(supply / "online") == "1"
    return {
        "loadavg": [float(x) for x in Path("/proc/loadavg").read_text().split()[:3]],
        "ac": ac,
        "governor": read_sys("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        "energy_performance_preference":
            read_sys("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference"),
        "platform_profile": read_sys("/sys/firmware/acpi/platform_profile"),
    }


def wait_for_load(limit, log, max_wait):
    """Refuse to start while the host 1-minute load is above `limit`; wait up to `max_wait` s."""
    waited = 0.0
    while True:
        load = float(Path("/proc/loadavg").read_text().split()[0])
        if load <= limit:
            return {"loadavg_1m_at_start": load, "waited_s": waited}
        if waited >= max_wait:
            return {"loadavg_1m_at_start": load, "waited_s": waited, "refused": True}
        if waited == 0:
            print(f"  host load {load:.2f} > {limit}; waiting", file=log, flush=True)
        time.sleep(5)
        waited += 5


# ---- Sway window events ---------------------------------------------------------------------------

IPC = struct.Struct("<6sII")


class WindowWatcher:
    def __init__(self):
        self.mapped = {}
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.connect(os.environ["SWAYSOCK"])
        payload = b'["window"]'
        self.sock.sendall(IPC.pack(b"i3-ipc", len(payload), 2) + payload)
        self.stream = self.sock.makefile("rb")
        if not (self.read() or {}).get("success"):
            raise RuntimeError("could not subscribe to Sway window events")
        threading.Thread(target=self.loop, daemon=True).start()

    def read(self):
        header = self.stream.read(IPC.size)
        if len(header) != IPC.size:
            return None
        magic, length, _ = IPC.unpack(header)
        if magic != b"i3-ipc":
            raise RuntimeError("invalid Sway IPC reply")
        return json.loads(self.stream.read(length))

    def loop(self):
        while event := self.read():
            if event.get("change") == "new":
                self.mapped.setdefault(event["container"].get("pid"), time.monotonic())


def window_rect(pid):
    tree = json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"], timeout=5))
    pending = [tree]
    while pending:
        node = pending.pop()
        if node.get("pid") == pid:
            return node["rect"]
        pending.extend(node.get("nodes", []) + node.get("floating_nodes", []))
    return None


# ---- perf stat windows (as tools/media/counters.py) ----------------------------------------------

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
        events[event.rstrip("/")] = {
            "count": count, "status": value if count is None else "counted",
            "runtime_ns": int(runtime) if runtime.isdecimal() else None,
            "running_percent": float(coverage.rstrip("%")) if coverage else None,
        }
    return events


def cpu_totals(events):
    pmus = [pmu for pmu in CPU_PMUS if (PMU_SYSFS / pmu).exists()] or ["cpu"]
    totals = {}
    for metric in ("instructions", "cycles"):
        counts = []
        for pmu in pmus:
            event = events.get(f"{pmu}/{metric}/u" if pmu != "cpu" else f"{metric}:u")
            if event is None:
                counts = None
                break
            if event["count"] is None:
                if event["status"] != "<not counted>" or event["runtime_ns"] != 0:
                    counts = None
                    break
            else:
                counts.append(event["count"])
        totals[metric] = sum(counts) if counts else None
    return totals


class Perf:
    """A perf stat session on one pid, toggled through its control pipe."""

    def __init__(self, resources, pid, run, name, enabled=False):
        self.name = name
        self.log = run / f"perf-{name}.csv"
        self.errors = run / f"perf-{name}.stderr"
        control_read, self.control = os.pipe()
        self.ack, ack_write = os.pipe()
        lifetime_read, self.lifetime = os.pipe()
        resources.callback(os.close, self.control)
        resources.callback(os.close, self.ack)
        self.windows = []
        try:
            stderr = resources.enter_context(self.errors.open("wb"))
            self.process = subprocess.Popen([
                "perf", "stat", "--no-big-num", "--no-scale", "-x,", "-e", "instructions:u,cycles:u",
                *([] if enabled else ["-D", "-1"]),
                "--control", f"fd:{control_read},{ack_write}", "-o", str(self.log), "-p", str(pid),
                "--", "sh", "-c", 'IFS= read -r done < "/proc/self/fd/$1" || :',
                "perf-lifetime", str(lifetime_read),
            ], pass_fds=(control_read, ack_write, lifetime_read), stdout=subprocess.DEVNULL,
                stderr=stderr, env=dict(os.environ, LC_ALL="C"))
        except BaseException:
            os.close(self.lifetime)
            raise
        finally:
            os.close(control_read)
            os.close(ack_write)
            os.close(lifetime_read)
        resources.callback(self.close)
        if enabled:
            self.windows.append([time.monotonic()])

    def command(self, name):
        before = time.monotonic()
        os.write(self.control, (name + "\n").encode())
        if not select.select([self.ack], [], [], 3)[0] or not os.read(self.ack, 64).startswith(b"ack"):
            raise RuntimeError(f"perf {name} acknowledgement missing. See {self.errors}")
        return (before + time.monotonic()) / 2

    def enable(self):
        self.windows.append([self.command("enable")])

    def disable(self):
        if self.windows and len(self.windows[-1]) == 1:
            self.windows[-1].append(self.command("disable"))

    def wait_attached(self):
        """perf reads its control pipe only once its counters are open: a no-op round trip
        (repeat the current state) returns once it has attached."""
        os.write(self.control, (b"enable\n" if self.windows else b"disable\n"))
        if not select.select([self.ack], [], [], 10)[0] or not os.read(self.ack, 64).startswith(b"ack"):
            raise RuntimeError(f"perf {self.name} did not attach. See {self.errors}")

    def close(self):
        if self.lifetime is not None:
            os.close(self.lifetime)
            self.lifetime = None
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()

    def result(self):
        self.close()
        if self.process.returncode:
            return {"error": f"perf exited {self.process.returncode}"}
        events = parse_stats(self.log.read_text()) if self.log.is_file() else {}
        totals = cpu_totals(events)
        return totals | {"window_s": sum(end - start for start, end in self.windows if end),
                         "windows": len(self.windows)}


@contextmanager
def boundaries():
    """A fake `grim` that turns autopilot snapshots into synchronous FIFO handshakes."""
    with tempfile.TemporaryDirectory(prefix="nt-fair-") as directory, ExitStack() as cleanup:
        path = Path(directory)
        shim = path / "grim"
        shim.write_text('#!/bin/sh\nset -eu\nprintf \'%s\\n\' "${3##*/}" > "$NT_FAIR_REQUEST"\n'
                        'IFS= read -r receipt < "$NT_FAIR_ACK"\ntest "$receipt" = ok\n')
        shim.chmod(0o700)
        fifo = {}
        for name in ("request", "ack"):
            os.mkfifo(path / name, 0o600)
            fifo[name] = os.open(path / name, os.O_RDWR)
            cleanup.callback(os.close, fifo[name])
        yield fifo, {
            "PATH": f"{directory}:{os.environ['PATH']}",
            "NT_FAIR_REQUEST": str(path / "request"), "NT_FAIR_ACK": str(path / "ack"),
            "NT_SNAPSHOT_OUTPUT": "fair", "NT_SNAPSHOT_DIR": str(path / "unused"),
        }


# ---- one run --------------------------------------------------------------------------------------

def prefs(profile):
    for name in ("iced", "gpui"):
        (profile / f"{name}-prefs.json").write_text('{"autoplay":false}')
    (profile / "rune-prefs.txt").write_text("autoplay=0\ntheatre=0\nguide=0\n")


def maps(pid):
    try:
        text = Path(f"/proc/{pid}/maps").read_text()
    except OSError:
        return None
    return {"libmpv": "libmpv.so" in text, "egl": any(lib in text for lib in EGL_LIBS)}


def pss(sample):
    """Process-tree PSS of one sample. A short-lived child (the snapshot shim) that exits while
    it is read has no value; the app's own process must have one."""
    rows = sample["processes"]
    if not rows or rows[0]["pss_mib"] is None:
        return None
    return sum(row["pss_mib"] for row in rows if row["pss_mib"] is not None)


def peak_pss(samples):
    values = [v for v in map(pss, samples) if v is not None]
    return max(values) if values else None


def settled_pss(samples):
    values = [v for v in map(pss, samples) if v is not None]
    return sorted(values[-3:])[len(values[-3:]) // 2] if len(values) >= 3 else None


def first(values):
    values = [v for v in values if v is not None]
    return values[0] if values else None


def latencies(report):
    opens = [at for at, cmd in report.get("commands", []) if json.loads(cmd).get("cmd") == "open_video"]
    out = []
    for i, opened in enumerate(opens):
        end = opens[i + 1] if i + 1 < len(opens) else float("inf")
        frame = first(t for t, mark in report.get("all_marks", [])
                      if mark == "playback_started" and opened <= t < end)
        out.append(None if frame is None else frame - opened)
    return out


def prewarm_trace(log_text, home_ms):
    steps = {}
    for line in log_text.splitlines():
        if line.startswith("NT_PREWARM "):
            _, step, *rest = line.split()
            if step == "policy":
                steps["policy"] = rest[0] if rest else None
            elif rest:
                try:
                    steps[f"{step}_after_home_ms"] = float(rest[0]) - home_ms if home_ms is not None else None
                except ValueError:
                    pass
    return steps


def app_run(args, app, suite, condition, run, watcher):
    run.mkdir(parents=True, exist_ok=True)
    scenario_path = run / "scenario.json"
    write_json(scenario_path, scenario(suite))
    binary = args.bin_dir / f"nt-{app}"
    result = {"app": app, "suite": suite, "condition": condition}
    with tempfile.TemporaryDirectory(prefix=f"nt-{app}-") as profile, boundaries() as (fifo, hooks), \
            ExitStack() as stack:
        profile = Path(profile)
        prefs(profile)
        env = {k: v for k, v in os.environ.items() if not k.startswith("NT_")} | hooks | {
            "NT_MUTE": "1", "NT_NO_MPRIS": "1", "NT_LIVE": "0", "NT_TRACE": "1",
            "NT_MPV_OPTS": "ao=null", "NT_FIXTURE_MODE": "replay", "NT_FIXTURES": str(args.fixtures),
            "NT_VIDEO_OVERRIDE": str(args.clip), "NT_DATA_DIR": str(profile),
            "NT_AUTOPILOT": str(scenario_path), "NT_REPORT": str(run / "report.json"),
            "NT_APP_ID": f"nttest-{app}",
        }
        env |= dict(item.split("=", 1) for item in args.env)
        if condition in ("off", "player", "full"):
            env["NT_PREWARM"] = condition
        if suite == "software":
            env[SOFTWARE[app]] = "1"
        log_path = run / "app.log"
        log = stack.enter_context(log_path.open("wb"))
        # Stopped before exec, so the Home counter window starts at the app's first instruction.
        child = subprocess.Popen(["sh", "-c", 'kill -STOP $$; exec "$0"', str(binary)], env=env,
                                 stdout=log, stderr=subprocess.STDOUT)

        def reap():
            for sig in (signal.SIGCONT, signal.SIGTERM):
                if child.poll() is None:
                    child.send_signal(sig)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        stack.callback(reap)
        deadline = time.monotonic() + 5
        while read_sys(f"/proc/{child.pid}/stat").split(") ", 1)[1][0] != "T":
            if time.monotonic() > deadline:
                raise RuntimeError("launcher did not stop before exec")
            time.sleep(0.002)
        perf = {}
        if args.perf:
            for name in WINDOWS[suite]:
                perf[name] = Perf(stack, child.pid, run, name, enabled=name == "home")
            for session in perf.values():
                session.wait_attached()
        started = time.monotonic()
        sampler = stack.enter_context(Sampler(child.pid, started))
        child.send_signal(signal.SIGCONT)
        checks, errors = [], []
        home_seen = rect = None
        deadline = started + 70
        while time.monotonic() < deadline and child.poll() is None:
            if select.select([fifo["request"]], [], [], 0.05)[0]:
                name = os.read(fifo["request"], 128).strip().decode().removesuffix(".png")
                window, _, edge = name.rpartition("-")
                if name == "home-end":
                    home_seen = time.monotonic() - started
                try:
                    if window in perf:
                        perf[window].enable() if edge == "begin" else perf[window].disable()
                except (OSError, RuntimeError) as error:
                    errors.append(str(error))
                os.write(fifo["ack"], b"ok\n")
            if rect is None and child.pid in watcher.mapped:
                rect = window_rect(child.pid)
            elapsed = time.monotonic() - started
            if suite == "latency" and home_seen is not None and home_seen + 2.0 <= elapsed <= home_seen + 3.8:
                checks.append(maps(child.pid))
            if (run / "report.json").is_file():
                break
        for _ in range(100):
            if child.poll() is not None:
                break
            time.sleep(0.05)
        counters = {name: session.result() for name, session in perf.items()}
    samples = [row for row in sampler.samples if row["processes"]]
    write_json(run / "procfs.json", samples)
    text = log_path.read_text(errors="replace")
    report_path = run / "report.json"
    if not report_path.is_file():
        return result | {"error": f"no report (exit {child.returncode})", "perf_errors": errors}
    report = json.loads(report_path.read_text())
    exec_ms = report.get("exec_to_init_ms") or 0.0
    marks = report.get("marks_ms", {})
    home_mark = marks.get("home_loaded")
    mapped = watcher.mapped.get(child.pid)
    result.update({
        "failed_waits": report.get("failed_waits", []),
        "window_ms": None if mapped is None else (mapped - started) * 1000,
        "window_rect": rect,
        "home_ms": None if home_mark is None else exec_ms + home_mark,
        "exec_to_init_ms": exec_ms,
        "frames_total": report.get("frames_total"),
        "peak_pss_mib": peak_pss(samples),
        "perf": counters, "perf_errors": errors,
        "prewarm": prewarm_trace(text, home_mark),
        "gpu_surface_attached": GPU_ATTACH[app] in text,
        "software_fallback": "fell back to software" in text or "software video fallback" in text,
        "comments_loaded": any(m == "comments_loaded" for _, m in report.get("all_marks", [])),
    })
    phases = {p["name"]: p for p in report.get("phases", [])}
    if "home_idle" in phases:
        result["home_cache_mib"] = phases["home_idle"].get("image_cache_mb_end")
    if suite == "latency":
        opens = latencies(report)
        result["open_ms"] = opens[0] if opens else None
        result["switch_ms"] = opens[1] if len(opens) > 1 else None
        if home_seen is not None:
            idle = [r for r in samples if home_seen + 2.0 <= r["t_s"] <= home_seen + 3.8]
            result["home_idle_pss_mib"] = settled_pss(idle)
            result["home_idle_pss_samples"] = len(idle)
        valid = [c for c in checks if c is not None]
        result["idle_maps"] = {
            "checks": len(valid),
            "libmpv": all(c["libmpv"] for c in valid) if valid else None,
            "egl": all(c["egl"] for c in valid) if valid else None,
            "consistent": len({json.dumps(c, sort_keys=True) for c in valid}) == 1 if valid else None,
        }
    if "playback" in phases:
        p = phases["playback"]
        result["playback"] = {"seconds": p["seconds"], "cores": p["cpu_percent"] / 100,
                              "video_frames": p["video_frames_rendered"], "ui_frames": p["frames"]}
    if "wheel" in phases:
        p = phases["wheel"]
        before = phases.get("home_idle", {}).get("image_cache_mb_end")
        result["wheel"] = {"seconds": p["seconds"], "cores": p["cpu_percent"] / 100,
                           "frames": p["frames"], "fps": p["fps"], "long_frames": p["long_frames"],
                           "frame_ms_p95": p["frame_ms_p95"],
                           "cache_mib_end": p.get("image_cache_mb_end"),
                           "cache_mib_added": None if before is None or p.get("image_cache_mb_end") is None
                           else p["image_cache_mb_end"] - before}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=Path)
    parser.add_argument("--bin-dir", type=Path, default=Path("/inputs"))
    parser.add_argument("--fixtures", type=Path, default=Path(os.environ.get("NT_FIXTURES", "/fixtures")))
    parser.add_argument("--clip", type=Path, default=Path(os.environ.get("NT_VIDEO_OVERRIDE", "/clip.mp4")))
    parser.add_argument("--rounds", type=int, default=6)
    parser.add_argument("--apps", nargs="+", choices=APPS, default=list(APPS))
    parser.add_argument("--slots", nargs="+", default=[
        "latency:off", "latency:full", "latency:default", "wheel:off", "wheel:full", "software:default"])
    parser.add_argument("--max-load", type=float, default=3.0)
    parser.add_argument("--max-wait", type=float, default=600.0, help="seconds per slice")
    parser.add_argument("--no-perf", dest="perf", action="store_false")
    parser.add_argument("--label", default="")
    parser.add_argument("--env", action="append", default=[], help="extra KEY=VALUE for the apps")
    args = parser.parse_args()
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    probe = subprocess.run(["perf", "stat", "-e", "instructions:u", "--", "true"],
                           capture_output=True, text=True)
    if args.perf and probe.returncode != 0:
        raise SystemExit(f"perf unavailable: {probe.stderr.strip()}")
    write_json(out / "setup.json", {"label": args.label, "rounds": args.rounds, "apps": args.apps,
                                    "slots": args.slots, "max_load": args.max_load,
                                    "machine": machine(), "shipped_defaults": SHIPPED,
                                    "nproc": os.cpu_count(),
                                    "cpu": next((line.split(":", 1)[1].strip() for line in
                                                 Path("/proc/cpuinfo").read_text().splitlines()
                                                 if line.startswith("model name")), None)})
    watcher = WindowWatcher()
    for rnd in range(1, args.rounds + 1):
        order = [APPS[(i + rnd - 1) % 3] for i in range(3)]
        order = [app for app in order if app in args.apps]
        for slot in args.slots:
            suite, condition = slot.split(":")
            for app in order:
                run = out / suite / condition / app / f"round-{rnd}"
                receipt = run / "sample.json"
                if receipt.is_file():
                    continue
                # While this exists, the host mirrors it as the shared measure lock, so other
                # agents start no heavy work until the slice is over.
                want = out / "slice.want"
                want.write_text(f"{suite}:{condition} {app} round {rnd} {time.time():.0f}\n")
                gate = wait_for_load(args.max_load, sys.stdout, args.max_wait)
                print(f"round {rnd} {suite}:{condition} {app} load {gate['loadavg_1m_at_start']:.2f}",
                      flush=True)
                if gate.get("refused"):
                    want.unlink(missing_ok=True)
                    write_json(receipt, {"app": app, "suite": suite, "condition": condition,
                                         "round": rnd, "refused": gate})
                    continue
                before = machine()
                try:
                    result = app_run(args, app, suite, condition, run, watcher)
                except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
                    result = {"app": app, "suite": suite, "condition": condition, "error": str(error)}
                result |= {"round": rnd, "order": order, "gate": gate, "machine_before": before,
                           "machine_after": machine()}
                want.unlink(missing_ok=True)
                write_json(receipt, result)
                summary = {k: result.get(k) for k in ("window_ms", "home_ms", "open_ms", "switch_ms",
                                                      "home_idle_pss_mib", "error")}
                print("   ", json.dumps(summary), flush=True)
                time.sleep(1.0)


if __name__ == "__main__":
    main()
