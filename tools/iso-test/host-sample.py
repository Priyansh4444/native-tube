#!/usr/bin/env python3
"""Read-only sampling of host isolation evidence. Omit window titles and app classes."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def query(*args):
    return json.loads(subprocess.check_output(["hyprctl", "-j", *args], timeout=2))


def sample(phase):
    state = {"phase": phase, "epoch_ns": time.time_ns()}
    for field, read in (
        ("cursor", lambda: query("cursorpos")),
        ("focused_monitor", lambda: next((m["name"] for m in query("monitors") if m["focused"]), None)),
        ("active_window", lambda: hashlib.sha256(query("activewindow").get("address", "").encode()).hexdigest()[:12]),
        ("test_windows", lambda: sum(w.get("class", "").startswith(("nttest-", "nt-")) for w in query("clients"))),
        ("test_outputs", lambda: [
            {key: m[key] for key in ("name", "x", "y", "width", "height", "scale")}
            for m in query("monitors", "all") if m["name"].startswith("NT-")
        ]),
    ):
        try:
            state[field] = read()
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            state[field] = type(error).__name__
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    state["runtime_pause"] = (runtime / "nt-input.paused").exists()
    state["state_pause"] = (Path.home() / ".local/state/nt-input.paused").exists()
    print(json.dumps(state), flush=True)


phase = sys.argv[1]
if phase == "during":
    while True:
        sample(phase)
        time.sleep(1)
else:
    sample(phase)
