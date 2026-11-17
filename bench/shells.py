#!/usr/bin/env python3
"""Start-up and idle cost of the raw shells and the apps, measured from outside the process.

For each target and run:
  - start-up: from just before exec to Hyprland's `openwindow` event for the window, which the
    compositor sends when the client maps its surface, i.e. commits its first frame. The same
    signal for every framework, independent of any in-app hook.
  - for apps, time to content: until the app prints `NT_MARK home_loaded` (recorded data, so no
    network time is included).
  - idle: after the window settles, CPU and context switches over a fixed window, then PSS and
    thread count.

Runs are warm (the first, discarded run loads files into the page cache; dropping it needs root).
Targets alternate order between runs. Every window goes to the off-screen output NT-BENCH.

    bench/shells.py [--runs 10] [--idle 5] [--targets shell-iced,shell-gpui,shell-rune,iced,gpui,rune]

Writes bench/results/shells.json and prints medians with min-max.
"""

import argparse
import json
import os
import signal
import socket
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CLK = os.sysconf("SC_CLK_TCK")
APP_ID = "nttest-bench"

TARGETS = {
    "shell-iced": ROOT / "shells/iced/target/release/shell-iced",
    # iced asks wgpu for the high-performance adapter by default (the discrete GPU on a hybrid
    # laptop); this variant sets wgpu's own low-power knobs, as the iced app does.
    "shell-iced-lowpower": ROOT / "shells/iced/target/release/shell-iced",
    "shell-gpui": ROOT / "shells/gpui/target/release/shell-gpui",
    "shell-rune": ROOT / "shells/rune/target/release/shell-rune",
    "iced": ROOT / "apps/iced/target/release/nt-iced",
    "gpui": ROOT / "apps/gpui/target/release/nt-gpui",
    "rune": ROOT / "apps/rune/target/release/nt-rune",
}


def hypr_socket():
    sig = os.environ["HYPRLAND_INSTANCE_SIGNATURE"]
    return f"{os.environ['XDG_RUNTIME_DIR']}/hypr/{sig}/.socket2.sock"


class WindowWatcher:
    """Timestamps Hyprland `openwindow` events for our app_id."""

    def __init__(self):
        self.s = socket.socket(socket.AF_UNIX)
        self.s.connect(hypr_socket())
        self.cv = threading.Condition()
        self.opened = []
        threading.Thread(target=self._run, daemon=True).start()

    def _run(self):
        buf = b""
        while True:
            data = self.s.recv(65536)
            if not data:
                return
            now = time.monotonic()
            buf += data
            *lines, buf = buf.split(b"\n")
            for line in lines:
                if line.startswith(b"openwindow>>"):
                    parts = line.decode(errors="replace").split(">>", 1)[1].split(",", 3)
                    if len(parts) >= 3 and parts[2] == APP_ID:
                        with self.cv:
                            self.opened.append(now)
                            self.cv.notify_all()

    def wait_after(self, t0, timeout):
        end = time.monotonic() + timeout
        with self.cv:
            while True:
                hit = next((t for t in self.opened if t >= t0), None)
                if hit is not None:
                    return hit
                left = end - time.monotonic()
                if left <= 0:
                    return None
                self.cv.wait(left)


def proc_tree(root):
    kids = {}
    for d in os.listdir("/proc"):
        if d.isdigit():
            try:
                with open(f"/proc/{d}/stat") as f:
                    kids.setdefault(int(f.read().rsplit(")", 1)[1].split()[1]), []).append(int(d))
            except (OSError, IndexError, ValueError):
                pass
    out, stack = [], [root]
    while stack:
        p = stack.pop()
        out.append(p)
        stack.extend(kids.get(p, []))
    return out


def cpu_s(pids):
    t = 0
    for p in pids:
        try:
            with open(f"/proc/{p}/stat") as f:
                v = f.read().rsplit(")", 1)[1].split()
            t += int(v[11]) + int(v[12])
        except (OSError, IndexError, ValueError):
            pass
    return t / CLK


def switches(pids):
    """Context switches over all threads: a proxy for wake-ups."""
    n = 0
    for p in pids:
        try:
            for tid in os.listdir(f"/proc/{p}/task"):
                with open(f"/proc/{p}/task/{tid}/status") as f:
                    for line in f:
                        if line.startswith(("voluntary_ctxt_switches", "nonvoluntary_ctxt_switches")):
                            n += int(line.split()[1])
        except OSError:
            pass
    return n


def pss_mb(pids):
    kb = 0
    for p in pids:
        try:
            with open(f"/proc/{p}/smaps_rollup") as f:
                for line in f:
                    if line.startswith("Pss:"):
                        kb += int(line.split()[1])
                        break
        except OSError:
            pass
    return kb / 1024


def threads(pids):
    n = 0
    for p in pids:
        try:
            n += len(os.listdir(f"/proc/{p}/task"))
        except OSError:
            pass
    return n


def one_run(name, path, watcher, idle, data_dir):
    extra = {"WGPU_POWER_PREF": "low", "WGPU_BACKEND": "vulkan"} if name.endswith("-lowpower") else {}
    env = dict(os.environ, **extra, NT_APP_ID=APP_ID, NT_MUTE="1", NT_NO_MPRIS="1", NT_TRACE="1",
               NT_FIXTURES=str(ROOT / ".cache/fixtures"), NT_FIXTURE_MODE="replay",
               NT_DATA_DIR=str(data_dir))
    content = {}

    t0 = time.monotonic()
    proc = subprocess.Popen([str(path)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                            text=True, start_new_session=True)

    def read_marks():
        for line in proc.stderr:
            if line.startswith("NT_MARK home_loaded") and "home" not in content:
                content["home"] = time.monotonic()

    threading.Thread(target=read_marks, daemon=True).start()
    r = {"target": name}
    mapped = watcher.wait_after(t0, 30)
    r["window_ms"] = (mapped - t0) * 1000 if mapped else None
    if not name.startswith("shell"):
        end = time.monotonic() + 30
        while "home" not in content and time.monotonic() < end and proc.poll() is None:
            time.sleep(0.005)
        r["content_ms"] = (content["home"] - t0) * 1000 if "home" in content else None
    time.sleep(3)  # let thumbnails, fonts and caches settle
    pids = proc_tree(proc.pid)
    c0, s0, w0 = cpu_s(pids), switches(pids), time.monotonic()
    time.sleep(idle)
    pids = proc_tree(proc.pid)
    c1, s1, w1 = cpu_s(pids), switches(pids), time.monotonic()
    r["idle_cpu_percent"] = (c1 - c0) / (w1 - w0) * 100
    r["idle_wakeups_per_s"] = (s1 - s0) / (w1 - w0)
    r["pss_mb"] = pss_mb(pids)
    r["threads"] = threads(pids)
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(5)
    except (ProcessLookupError, subprocess.TimeoutExpired):
        os.killpg(proc.pid, signal.SIGKILL)
    time.sleep(0.5)
    return r


def static_facts(name, path):
    stripped = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / f"{name}.stripped"
    subprocess.run(["strip", "-o", str(stripped), str(path)], check=False)
    size = stripped.stat().st_size / 1e6 if stripped.exists() else None
    stripped.unlink(missing_ok=True)
    needed = subprocess.run(["readelf", "-d", str(path)], capture_output=True, text=True).stdout.count("(NEEDED)")
    lock = path.parent.parent.parent / "Cargo.lock"
    crates = lock.read_text().count("[[package]]") if lock.exists() else None
    return {"stripped_mb": size, "needed_libs": needed, "crates": crates}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=10)
    ap.add_argument("--idle", type=float, default=5)
    ap.add_argument("--targets", default="shell-iced,shell-iced-lowpower,shell-gpui,shell-rune")
    a = ap.parse_args()
    names = a.targets.split(",")
    for n in names:
        if not TARGETS[n].exists():
            sys.exit(f"{n}: {TARGETS[n]} is not built")
    # Creates NT-BENCH and its window rule for app_id nttest-bench.
    subprocess.run([str(ROOT / "tools/headless.sh"), "run", "bench", "true"], capture_output=True)
    data = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "nt-bench-data"
    watcher = WindowWatcher()
    out = {"runs": a.runs, "idle_s": a.idle, "static": {}, "results": {n: [] for n in names}}
    for n in names:
        out["static"][n] = static_facts(n, TARGETS[n])
        one_run(n, TARGETS[n], watcher, 0.5, data / n)  # warm-up, discarded
    for i in range(a.runs):
        order = names if i % 2 == 0 else list(reversed(names))
        for n in order:
            out["results"][n].append(one_run(n, TARGETS[n], watcher, a.idle, data / n))
        print(f"run {i + 1}/{a.runs} done", flush=True)

    res = ROOT / "bench/results"
    res.mkdir(parents=True, exist_ok=True)
    (res / "shells.json").write_text(json.dumps(out, indent=2))

    def stat(n, k, fmt="{:.0f}"):
        xs = [r[k] for r in out["results"][n] if isinstance(r.get(k), (int, float))]
        if not xs:
            return "-"
        return f"{fmt.format(statistics.median(xs))} ({fmt.format(min(xs))}-{fmt.format(max(xs))})"

    print(f"\nmedian (min-max) over {a.runs} warm runs")
    cols = ["window_ms", "content_ms", "idle_cpu_percent", "idle_wakeups_per_s", "pss_mb", "threads"]
    print(f"{'target':20}" + "".join(f"{c:>24}" for c in cols) + f"{'stripped MB':>13}{'libs':>6}{'crates':>8}")
    for n in names:
        s = out["static"][n]
        row = f"{n:20}"
        for c in cols:
            row += f"{stat(n, c, '{:.1f}' if c == 'idle_cpu_percent' else '{:.0f}'):>24}"
        row += f"{s['stripped_mb'] or 0:13.1f}{s['needed_libs']:6d}{s['crates'] or 0:8d}"
        print(row)
    print(f"\nwrote {res / 'shells.json'}")


if __name__ == "__main__":
    main()
