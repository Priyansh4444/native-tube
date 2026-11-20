"""External Linux process-tree sampling. Missing fields remain missing."""
import os
from pathlib import Path
import threading
import time

MEMORY = {
    "pss_mib": ("Pss",),
    "rss_mib": ("Rss",),
    "private_mib": ("Private_Clean", "Private_Dirty"),
    "swap_mib": ("Swap",),
}
COUNTS = ("threads", "fds", "processes")
SWITCHES = ("voluntary_ctxt_switches", "nonvoluntary_ctxt_switches")
TICKS = os.sysconf("SC_CLK_TCK")
INTERVAL_S = 0.25


def stat(path):
    fields = path.read_text().rsplit(") ", 1)[1].split()
    return fields[0], int(fields[1]), int(fields[19]), int(fields[11]) + int(fields[12])


def fields(path):
    try:
        return dict(line.split(":", 1) for line in path.read_text().splitlines() if ":" in line)
    except OSError:
        return {}


def count(values, key):
    try:
        return int(values[key].split()[0])
    except (KeyError, IndexError, ValueError):
        return None


def complete_sum(values):
    return sum(values) if values and None not in values else None


def snapshot(root, proc=Path("/proc")):
    processes = {}
    children = {}
    for path in proc.iterdir():
        if not path.name.isdecimal():
            continue
        try:
            state, parent, started, cpu = stat(path / "stat")
        except (OSError, ValueError, IndexError):
            continue
        if state not in ("Z", "X"):
            pid = int(path.name)
            processes[pid] = (started, cpu)
            children.setdefault(parent, []).append(pid)

    rows = []
    pending = [root] if root in processes else []
    while pending:
        pid = pending.pop()
        pending.extend(children.get(pid, []))
        path = proc / str(pid)
        started, cpu = processes[pid]
        rollup = fields(path / "smaps_rollup")
        memory = {}
        for metric, keys in MEMORY.items():
            kib = complete_sum([count(rollup, key) for key in keys])
            memory[metric] = None if kib is None else kib / 1024
        try:
            tasks = list((path / "task").iterdir())
        except OSError:
            tasks = None
        try:
            fds = len(list((path / "fd").iterdir()))
        except OSError:
            fds = None
        switches = None
        if tasks:
            switches = {}
            for task in tasks:
                try:
                    _, _, task_started, _ = stat(task / "stat")
                except (OSError, IndexError, ValueError):
                    continue
                status = fields(task / "status")
                value = complete_sum([count(status, key) for key in SWITCHES])
                if value is not None:
                    switches[f"{task.name}:{task_started}"] = value
            if len(switches) != len(tasks):
                switches = None
        try:
            state, _, still_started, _ = stat(path / "stat")
            if still_started != started or state in ("Z", "X"):
                memory = dict.fromkeys(MEMORY)
                tasks = fds = switches = None
        except (OSError, IndexError, ValueError):
            memory = dict.fromkeys(MEMORY)
            tasks = fds = switches = None
        rows.append({
            "id": f"{pid}:{started}", "cpu_ticks": cpu,
            "threads": None if tasks is None else len(tasks),
            "fds": fds, "context_switches_by_task": switches, **memory,
        })
    return rows


class Sampler:
    def __init__(self, pid, started):
        self.pid = pid
        self.started = started
        self.samples = []
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.collect)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.stop.set()
        self.thread.join()

    def collect(self):
        while not self.stop.is_set():
            before = time.monotonic()
            processes = snapshot(self.pid)
            after = time.monotonic()
            self.samples.append({
                "t_s": (before + after) / 2 - self.started,
                "span_s": after - before, "processes": processes,
            })
            self.stop.wait(max(0, INTERVAL_S - (after - before)))
