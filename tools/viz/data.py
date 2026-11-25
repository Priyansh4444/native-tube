"""Read the measurement receipts once for both animated and static outputs."""
import json
import os
import math
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / ".cache/viz/out"
MEDIA = ".cache/media/out"
REVIEW = "docs/blog/takes/review.md"
COUNTERS = f"{MEDIA}/counters/counters.json"
METRICS = f"{MEDIA}/final/metrics.json"
RESOURCES = f"{MEDIA}/resources/resources.json"
ATTRIBUTION = f"{MEDIA}/resources/attribution.json"
HICCUPS = f"{MEDIA}/hiccups.json"
STABILITY = f"{MEDIA}/stability/stability.json"
COST = "docs/blog/coordinator-notes.md"
METHOD = "docs/media/README.md"
FLY_METHOD = "docs/media/fly-bench.md"
FLY = ".cache/fly-bench/out/viz-input.json"
GATE = ".cache/perf/out/compare-isDkA304/summary.md"
APPS = ("iced", "gpui", "rune")
ALL_APPS = (*APPS, "youtube")
NAMES = dict(zip(ALL_APPS, ("iced", "GPUI", "Rune", "YouTube web"), strict=True))
APP_COLORS = dict(zip(ALL_APPS, ("#77DBF7", "#F6C777", "#BCA3FF", "#FF939B"), strict=True))


def read(path):
    return json.loads((ROOT / path).read_text())


def laptop_counters():
    return read(COUNTERS)["results"]


def fly_input():
    if os.environ.get("VIZ_LAPTOP_ONLY"):
        return {}
    if snapshot := os.environ.get("VIZ_RENDER_FLY_SNAPSHOT"):
        return json.loads(snapshot)
    from fly import benchmark
    if not (ROOT / FLY).exists():
        return benchmark()
    if not (ROOT / FLY_METHOD).is_file():
        raise ValueError(f"{FLY} requires {FLY_METHOD}")
    data = benchmark() | read(FLY)
    for section in ("timing", "memory", "wheel", "tours"):
        group = data.get(section)
        if group is None:
            continue
        if not set(APPS) <= group.keys() or group.keys() - set(ALL_APPS):
            raise ValueError(f"{section} must represent all three native apps on the same VM")
        for row in group.values():
            if section == "wheel" and (not math.isfinite(row["rate"]) or row["rate"] <= 0):
                raise ValueError("Fly wheel inputs must have positive, measured instruction rates")
            for path in row["sources"]:
                if not (ROOT / path).is_file():
                    raise ValueError(f"Missing Fly receipt: {path}")
    return data


def timing(fly):
    if "timing" in fly:
        return "Fly VM • see benchmark methods", fly["timing"]
    rows = read(METRICS)["NOW"]
    return "Laptop • AC • fixture replay • median of three", {
        app: {
            "home_ms": rows[app]["metrics"]["home_ms"],
            "open_ms": rows[app]["metrics"]["open_ms"],
            "sources": [METRICS, METHOD, REVIEW],
        }
        for app in APPS
    }


def memory(fly):
    if "memory" in fly:
        return "Fly VM • process-tree PSS • see benchmark methods", fly["memory"]
    rows, ref = read(RESOURCES)["NOW"], read(ATTRIBUTION)["NOW"]
    return "Laptop • battery • process-tree PSS • median of three", {
        app: {
            "cold": rows[app]["cold"]["idle"]["settled_pss_mib"],
            "warm": rows[app]["gpu"]["idle"]["settled_pss_mib"],
            "playing": ref[app]["settled_pss_mib"]["total"],
            "peak": rows[app]["tour"]["tour"]["peak_pss_mib"],
            "core": ref[app]["settled_pss_mib"]["core_software"],
            "sources": [RESOURCES, ATTRIBUTION, METHOD, REVIEW],
        }
        for app in APPS
    }


def gate():
    result = {}
    for line in (ROOT / GATE).read_text().splitlines():
        columns = [x.strip() for x in line.strip("|").split("|")]
        if len(columns) > 5 and columns[2] == "wheel_cpu_percent":
            result[columns[0]] = float(columns[5])
    return result


def wheel(fly):
    if "wheel" in fly:
        return "Fly VM • see benchmark methods", fly["wheel"]
    return "Laptop • AC • 27-tick wheel story • median of three", {
        app: {
            "rate": laptop_counters()[app]["wheel"]["normalization"]["instructions_per_scroll_second"],
            "sources": [COUNTERS, METHOD, REVIEW],
        }
        for app in APPS
    }


def sizes():
    metrics = read(METRICS)["NOW"]
    return {
        app: (
            metrics[app]["metrics"]["stripped_bytes"] / (2**20),
            (ROOT / f"apps/{app}/Cargo.lock").read_text().count("[[package]]"),
        )
        for app in APPS
    }


def cost():
    md = (ROOT / COST).read_text()
    amounts = re.search(r"^\| Cost \| (.*?) \| (.*?) \| (.*?) \|$", md, re.M).groups()
    elapsed = re.search(r"^\| .*? \| (49 min) \| (59 min) \| (55 min) \|$", md, re.M).groups()
    return dict(zip(APPS, zip(amounts, elapsed, strict=True), strict=True))
