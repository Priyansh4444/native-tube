#!/usr/bin/env python3
"""Playback benchmark: NativeTube's core player vs mpv + yt-dlp vs YouTube in a browser.

For each player and run: the first video is a cold start (the player has just been
started), the following ones are warm (the player is running and the user picks the
next video). For each video we record time to the first frame of the video itself
(not an ad), then let it play muted, wait 2 s, and measure CPU and memory over a 20 s
window from /proc (the whole process tree: browser GPU/renderer processes included).

All runs are muted. Everything runs on the off-screen output NT-BENCH.

    bench/playback.py [--runs 2] [--window 20] [--players core,mpv,web] [ids...]

Writes bench/results/playback.json and prints a summary table.
"""

import argparse
import json
import os
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRATCH = Path(os.environ.get("NT_BENCH_SCRATCH", "/tmp/nt-bench"))
CLK = os.sysconf("SC_CLK_TCK")
DEFAULT_IDS = ["aqz-KE-bpKQ", "5C_HPTJg5ek", "kJQP7kiw5Fk"]
YTDL_FORMAT = "bestvideo[height<=?720][vcodec^=avc1]+bestaudio[ext=m4a]/best[height<=?720]"


# ---------------------------------------------------------------- /proc sampling

def proc_tree(root_pids):
    """Every live pid in the trees under root_pids."""
    children = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            with open(f"/proc/{d}/stat") as f:
                ppid = int(f.read().rsplit(")", 1)[1].split()[1])
            children.setdefault(ppid, []).append(int(d))
        except (OSError, IndexError, ValueError):
            pass
    out, stack = set(), list(root_pids)
    while stack:
        p = stack.pop()
        if p in out:
            continue
        out.add(p)
        stack.extend(children.get(p, []))
    return out


def cpu_seconds(pids):
    total = 0
    for p in pids:
        try:
            with open(f"/proc/{p}/stat") as f:
                fields = f.read().rsplit(")", 1)[1].split()
            total += int(fields[11]) + int(fields[12])  # utime + stime
        except (OSError, IndexError, ValueError):
            pass
    return total / CLK


def pss_mb(pids):
    """Proportional set size: shared libraries are split between the processes using them."""
    total = 0
    for p in pids:
        try:
            with open(f"/proc/{p}/smaps_rollup") as f:
                for line in f:
                    if line.startswith("Pss:"):
                        total += int(line.split()[1])
                        break
        except OSError:
            pass
    return total / 1024


# ---------------------------------------------------------------- players

class LineProc:
    """A child process whose stdout lines are JSON events (or raw log lines)."""

    def __init__(self, args, env=None, json_lines=True):
        self.t_launch = time.monotonic()
        self.proc = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.STDOUT, text=True, bufsize=1, env=env,
                                     start_new_session=True)
        self.json_lines = json_lines
        self.events = []
        self.cv = threading.Condition()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.proc.stdout:
            ev = None
            if self.json_lines:
                try:
                    ev = json.loads(line)
                except ValueError:
                    pass
            else:
                ev = {"line": line.rstrip()}
            if ev is not None:
                with self.cv:
                    self.events.append((time.monotonic(), ev))
                    self.cv.notify_all()

    def wait_for(self, pred, after_index, timeout):
        end = time.monotonic() + timeout
        with self.cv:
            while True:
                for i in range(after_index, len(self.events)):
                    t, ev = self.events[i]
                    if pred(ev):
                        return t, ev
                left = end - time.monotonic()
                if left <= 0:
                    return None, None
                self.cv.wait(left)

    def send(self, line):
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()

    def stop(self):
        try:
            os.killpg(self.proc.pid, signal.SIGTERM)
            self.proc.wait(5)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


class Core:
    name = "NativeTube core"

    def start(self):
        env = dict(os.environ, NT_MUTE="1", NT_DATA_DIR=str(SCRATCH / "ntdata"))
        env.pop("NT_VIDEO_OVERRIDE", None)
        env.pop("NT_FIXTURES", None)
        self.p = LineProc([str(ROOT / "core/target/release/ntctl"), "serve"], env=env)
        self.p.wait_for(lambda e: e.get("event") == "ready", 0, 10)

    def pids(self):
        return proc_tree([self.p.proc.pid])

    def open(self, vid, cold):
        n = len(self.p.events)
        self.p.send(f"open {vid}")
        _, ev = self.p.wait_for(lambda e: e.get("event") == "first_frame", n, 60)
        if not ev:
            return {"ok": False}
        return {"ok": ev["ok"], "ms": ev["ms"], "size": ev.get("size"), "error": ev.get("error") or None}

    def position(self):
        n = len(self.p.events)
        self.p.send("stats")
        _, ev = self.p.wait_for(lambda e: e.get("event") == "stats", n, 5)
        return ev and ev["position"]

    def stop(self):
        self.p.stop()

    note = "decodes and renders every frame to RAM; no window (the apps add presentation)"


class Mpv:
    name = "mpv + yt-dlp"

    def start(self):
        # AF_UNIX paths are limited to 107 bytes, so keep it short.
        self.sock = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "nt-bench-mpv.sock"
        self.sock.unlink(missing_ok=True)
        self.args = ["mpv", "--no-config", "--idle=yes", "--force-window=yes", "--mute=yes",
                     "--hwdec=vaapi", "--vo=gpu-next", "--wayland-app-id=nt-bench", "--keep-open=yes",
                     f"--ytdl-format={YTDL_FORMAT}", f"--input-ipc-server={self.sock}",
                     "--msg-level=all=v", "--msg-time", "--terminal=yes", "--input-terminal=no", "--really-quiet=no"]
        self.t_launch = None

    def _ipc(self, cmd):
        for _ in range(100):
            try:
                s = socket.socket(socket.AF_UNIX)
                s.connect(str(self.sock))
                s.sendall((json.dumps({"command": cmd}) + "\n").encode())
                data = s.recv(65536).decode()
                s.close()
                for line in data.splitlines():
                    msg = json.loads(line)
                    if "error" in msg:
                        return msg
                return None
            except OSError:
                time.sleep(0.05)
        raise RuntimeError("mpv IPC unavailable")

    def pids(self):
        return proc_tree([self.p.proc.pid])

    def open(self, vid, cold):
        url = f"https://www.youtube.com/watch?v={vid}"
        t0 = time.monotonic()
        if cold:
            # A cold start includes launching the player, as it does for the others.
            self.p = LineProc(self.args, json_lines=False)
            t0 = self.p.t_launch
        n = len(self.p.events)
        self._ipc(["loadfile", url, "replace"])
        t, ev = self.p.wait_for(lambda e: "first video frame after restart shown" in e["line"], n, 60)
        if not ev:
            return {"ok": False}
        size = None
        r = self._ipc(["get_property", "video-params"])
        if r and isinstance(r.get("data"), dict):
            size = f'{r["data"].get("w")}x{r["data"].get("h")}'
        return {"ok": True, "ms": (t - t0) * 1000, "size": size}

    def position(self):
        r = self._ipc(["get_property", "time-pos"])
        return r and r.get("data")

    def stop(self):
        try:
            self._ipc(["quit"])
        except RuntimeError:
            pass
        self.p.stop()

    note = "mpv with --hwdec=vaapi --vo=gpu-next, window on NT-BENCH; yt-dlp runs per video"


class MpvDirect(Mpv):
    """mpv as a player only: it gets the same stream URLs NativeTube's resolver finds
    (the resolve time is counted), so this compares player against player."""
    name = "mpv (same stream URLs)"
    note = "mpv --hwdec=vaapi --vo=gpu-next fed by NativeTube's resolver; window on NT-BENCH"

    def start(self):
        super().start()
        self.args = [x for x in self.args if not x.startswith("--ytdl-format")] + [
            "--ytdl=no", "--stream-lavf-o=request_size=10485760,initial_request_size=1048576", "--cache-secs=60"]

    def open(self, vid, cold):
        t0 = time.monotonic()
        if cold:
            self.p = LineProc(self.args, json_lines=False)
            t0 = self.p.t_launch
        env = dict(os.environ, NT_DATA_DIR=str(SCRATCH / "ntdata"))
        out = subprocess.run([str(ROOT / "core/target/release/ntctl"), "resolve", vid, "--json"],
                             capture_output=True, text=True, env=env).stdout
        try:
            urls = json.loads([l for l in out.splitlines() if l.startswith("{")][-1])
        except (IndexError, ValueError):
            return {"ok": False, "error": "resolve failed"}
        n = len(self.p.events)
        self._ipc(["set_property", "user-agent", urls["user_agent"]])
        opts = {"audio-file": urls["audio"]} if urls.get("audio") else {}
        self._ipc(["loadfile", urls["video"], "replace", -1, opts])
        t, ev = self.p.wait_for(lambda e: "first video frame after restart shown" in e["line"], n, 60)
        if not ev:
            return {"ok": False}
        return {"ok": True, "ms": (t - t0) * 1000}


class Web:
    name = "YouTube in Helium (Chromium 157)"

    def start(self):
        self.profile = SCRATCH / "helium-profile"
        self.port = "9333"
        if not self.profile.exists():
            self.profile.mkdir(parents=True)
        subprocess.run([str(ROOT / "tools/headless.sh"), "setup"], capture_output=True)
        self.browser = subprocess.Popen(
            ["helium-browser", "--class=nt-bench", f"--user-data-dir={self.profile}", "--no-first-run",
             "--no-default-browser-check", "--mute-audio", "--autoplay-policy=no-user-gesture-required",
             f"--remote-debugging-port={self.port}", "--window-size=1280,800", "--ozone-platform=wayland",
             "about:blank"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        self.cdp = LineProc(["node", str(ROOT / "bench/cdp.mjs"), self.port])
        self.cdp.wait_for(lambda e: e.get("event") == "ready", 0, 30)

    def pids(self):
        return proc_tree([self.browser.pid])

    def open(self, vid, cold):
        n = len(self.cdp.events)
        self.cdp.send(f"open {vid} {'cold' if cold else 'warm'}")
        _, ev = self.cdp.wait_for(lambda e: e.get("event") == "first_frame", n, 70)
        if not ev:
            return {"ok": False}
        return {"ok": ev["ok"], "ms": ev["ms"], "size": ev.get("size"), "ad_seen": ev.get("ad"),
                "first_any_frame_ms": ev.get("any_ms"), "full_reload": ev.get("reloaded")}

    def position(self):
        n = len(self.cdp.events)
        self.cdp.send("stats")
        _, ev = self.cdp.wait_for(lambda e: e.get("event") == "stats", n, 5)
        return ev and ev.get("t")

    def stop(self):
        try:
            self.cdp.send("quit")
        except (BrokenPipeError, OSError):
            pass
        self.cdp.stop()
        try:
            os.killpg(self.browser.pid, signal.SIGTERM)
            self.browser.wait(10)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            pass

    note = "Helium ships uBlock Origin, so no pre-roll ads: a best case for YouTube"


PLAYERS = {"core": Core, "mpvdirect": MpvDirect, "mpv": Mpv, "web": Web}


def run_player(player, ids, window, settle):
    player.start()
    results = []
    try:
        for i, vid in enumerate(ids):
            cold = i == 0
            r = player.open(vid, cold)
            r.update({"id": vid, "cold": cold})
            if r.get("ok"):
                time.sleep(settle)
                pids = player.pids()
                c0, p0, w0 = cpu_seconds(pids), player.position(), time.monotonic()
                time.sleep(window)
                pids = player.pids()
                c1, p1, w1 = cpu_seconds(pids), player.position(), time.monotonic()
                r["cpu_percent"] = (c1 - c0) / (w1 - w0) * 100
                r["pss_mb"] = pss_mb(pids)
                r["processes"] = len(pids)
                if isinstance(p0, (int, float)) and isinstance(p1, (int, float)):
                    r["played_s"] = p1 - p0
            print(f"  {player.name:34} {vid} {'cold' if cold else 'warm'}: "
                  + (f"first frame {r['ms']:7.0f} ms, CPU {r.get('cpu_percent', 0):5.1f}%, "
                     f"PSS {r.get('pss_mb', 0):6.0f} MB, played {r.get('played_s', 0):4.1f}/{window} s"
                     if r.get("ok") else f"FAILED {r}"), flush=True)
            results.append(r)
    finally:
        player.stop()
    return results


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=2)
    ap.add_argument("--window", type=float, default=20)
    ap.add_argument("--settle", type=float, default=2)
    ap.add_argument("--players", default="core,mpvdirect,mpv,web")
    ap.add_argument("ids", nargs="*", default=DEFAULT_IDS)
    a = ap.parse_args()
    SCRATCH.mkdir(parents=True, exist_ok=True)
    shutil.rmtree(SCRATCH / "helium-profile", ignore_errors=True)  # cold browser cache for run 1
    subprocess.run([str(ROOT / "tools/headless.sh"), "setup"], capture_output=True)

    names = a.players.split(",")
    out = {"ids": a.ids, "window_s": a.window, "runs": a.runs, "notes": {}, "results": {n: [] for n in names}}
    for run in range(a.runs):
        print(f"run {run + 1}/{a.runs}", flush=True)
        # Alternate the order between runs so no player always goes first.
        order = names if run % 2 == 0 else list(reversed(names))
        for n in order:
            p = PLAYERS[n]()
            out["notes"][n] = p.note
            for r in run_player(p, a.ids, a.window, a.settle):
                r["run"] = run + 1
                out["results"][n].append(r)
            time.sleep(2)

    res_dir = ROOT / "bench/results"
    res_dir.mkdir(parents=True, exist_ok=True)
    (res_dir / "playback.json").write_text(json.dumps(out, indent=2))

    def med(xs):
        xs = [x for x in xs if isinstance(x, (int, float))]
        return statistics.median(xs) if xs else float("nan")

    print("\nmedians (ms to first frame of the video; CPU % of one core; PSS MB)")
    print(f"{'player':34} {'cold':>8} {'warm':>8} {'CPU%':>7} {'PSS':>7} {'fails':>6}")
    for n in names:
        rs = out["results"][n]
        ok = [r for r in rs if r.get("ok")]
        print(f"{PLAYERS[n].name:34} {med([r['ms'] for r in ok if r['cold']]):8.0f} "
              f"{med([r['ms'] for r in ok if not r['cold']]):8.0f} {med([r.get('cpu_percent') for r in ok]):7.1f} "
              f"{med([r.get('pss_mb') for r in ok]):7.0f} {len(rs) - len(ok):6d}")
    print(f"\nwrote {res_dir / 'playback.json'}")


if __name__ == "__main__":
    sys.exit(main())
