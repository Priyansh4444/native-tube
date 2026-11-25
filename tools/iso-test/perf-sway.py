"""Run the existing fixture performance gate on the private Sway renderer."""
from collections import deque
import importlib
import json
import os
import socket
from pathlib import Path
import struct
import subprocess
import sys
from threading import Condition, Thread
import time


def windows():
    tree = json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"], timeout=5))
    pending = [tree]
    found = []
    while pending:
        node = pending.pop()
        if (node.get("app_id") or "").startswith("nttest-"):
            found.append(node)
        pending.extend(node.get("nodes", []) + node.get("floating_nodes", []))
    return found


def require_window(pid):
    clients = windows()
    if len(clients) != 1 or clients[0]["pid"] != pid or clients[0]["app_id"] != "nttest-bench":
        raise RuntimeError("expected one owned benchmark window in Sway")
    if clients[0]["rect"] != dict(x=0, y=0, width=1280, height=800):
        raise RuntimeError(f"unexpected benchmark window geometry: {clients[0]['rect']}")


class SwayWindowWatcher:
    """Timestamp Sway's compositor window:new event as it arrives."""

    def __init__(self):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.connect(os.environ["SWAYSOCK"])
        self.stream = self.socket.makefile("rb")
        payload = b'["window"]'
        self.socket.sendall(b"i3-ipc" + struct.pack("=II", len(payload), 2) + payload)
        if not self._message()["success"]:
            raise RuntimeError("unable to subscribe to Sway window events")
        self.opened = deque(maxlen=128)
        self.cv = Condition()
        Thread(target=self._read, daemon=True).start()

    def _message(self):
        header = self.stream.read(14)
        if not header:
            return None
        if len(header) != 14 or not header.startswith(b"i3-ipc"):
            raise RuntimeError("invalid Sway IPC header")
        size, _ = struct.unpack("=II", header[6:])
        return json.loads(self.stream.read(size))

    def _read(self):
        while event := self._message():
            if event["change"] == "new" and event["container"]["app_id"] == "nttest-bench":
                with self.cv:
                    self.opened.append(time.monotonic())
                    self.cv.notify_all()

    def wait_after(self, started, timeout):
        with self.cv:
            if self.cv.wait_for(lambda: self.opened and self.opened[-1] >= started, timeout):
                return next(t for t in self.opened if t >= started)
        return None


def main():
    out, *args = sys.argv[1:]
    apps = args or ["iced", "gpui", "rune"]
    smoke = "--smoke" in apps
    apps = [app for app in apps if app != "--smoke"] or ["iced", "gpui", "rune"]
    binaries = Path(os.environ["XDG_RUNTIME_DIR"]) / "benchmark-binaries"
    binaries.mkdir()
    for app in apps:
        assert app in {"iced", "gpui", "rune"}, app
        (binaries / f"baseline-{app}").symlink_to(f"/inputs/baseline-{app}")
        (binaries / f"candidate-{app}").symlink_to(f"/inputs/nt-{app}")
    sys.path.insert(0, "/iso/tools")
    gate = importlib.import_module("check-performance")
    gate.WindowWatcher = SwayWindowWatcher
    gate.test_windows = windows
    gate.window_ready = require_window
    gate.GUARD = Path("/usr/local/bin/nt-explore")
    gate.CLIP = Path(os.environ["NT_VIDEO_OVERRIDE"])
    gate.UNINSTRUMENTED["background_throttling"] = (
        "test window stays focused on the private seat; no foreground-to-background transition"
    )
    sys.argv = [sys.argv[0], "2caf0fa25b4c1973f23f5f5928fdc064eac945cb", str(binaries), out, *apps]
    if smoke:
        sys.argv.append("--smoke")
    return gate.main()


if __name__ == "__main__":
    sys.exit(main())
