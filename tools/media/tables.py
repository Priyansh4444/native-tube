#!/usr/bin/env python3
"""Print Markdown tables for the README from the media JSON outputs."""
import argparse
import json
from pathlib import Path

from metrics import APPS, APP_NAMES, METRICS, MILESTONES, OUT

SHORT = {"window_ms": "Window ms", "home_ms": "Home ms", "open_ms": "Open→frame ms",
         "warm_ms": "Warm switch ms", "idle_pss_mib": "Idle PSS MiB", "peak_pss_mib": "Peak PSS MiB",
         "playback_cpu_percent": "Playback CPU %", "stripped_bytes": "Stripped MiB"}


def fmt(value, metric):
    if value is None:
        return "n/a"
    if metric == "stripped_bytes":
        return f"{value / 2**20:.1f}"
    return f"{value:.0f}" if abs(value) >= 10 else f"{value:.1f}"


def metric_table(data):
    order = [m for m in SHORT if m in METRICS]
    lines = ["| Build | App | " + " | ".join(SHORT[m] for m in order) + " |",
             "|---|---|" + "---:|" * len(order)]
    for rev in MILESTONES:
        for app in APPS:
            values = data[rev][app]["metrics"]
            lines.append(f"| {rev} | {APP_NAMES[app]} | " +
                         " | ".join(fmt(values[m], m) for m in order) + " |")
    return "\n".join(lines)


def stability_table(data):
    lines = ["| App | " + " | ".join(MILESTONES) + " |", "|---|" + "---:|" * len(MILESTONES)]
    for app in APPS:
        cells = []
        for rev in MILESTONES:
            row = data.get(rev, {}).get(app)
            if not row or row.get("score_percent") is None:
                cells.append("n/a")
            else:
                cells.append(f"{row['score_percent']:.0f} ({row['clean']}/{row['assessed']}, w={row['weighted_events']})")
        lines.append(f"| {APP_NAMES[app]} | " + " | ".join(cells) + " |")
    return "\n".join(lines)


def resource_table(data, rev="NOW"):
    build = data.get(rev, {})
    warm = "gpu" if rev == "NOW" else "software"
    rows = (("Cold Home PSS", "cold", "idle", "settled_pss_mib"),
            ("Warm Home PSS", warm, "idle", "settled_pss_mib"),
            ("Software playback PSS", "software", "playback", "settled_pss_mib"),
            ("Software playback peak PSS", "software", "playback", "peak_pss_mib"),
            ("Software playback CPU %", "software", "playback", "cpu_percent"),
            ("Software playback peak CPU %", "software", "playback", "peak_cpu_percent"),
            ("GPU playback PSS", "gpu", "playback", "settled_pss_mib"),
            ("GPU playback peak PSS", "gpu", "playback", "peak_pss_mib"),
            ("GPU playback CPU %", "gpu", "playback", "cpu_percent"),
            ("GPU playback peak CPU %", "gpu", "playback", "peak_cpu_percent"),
            ("Tour peak PSS", "tour", "tour", "peak_pss_mib"))
    names = [*APPS, "core"]
    lines = ["| Measure | " + " | ".join(APP_NAMES.get(a, "ntctl play") for a in names) + " |",
             "|---|" + "---:|" * len(names)]
    for label, mode, section, key in rows:
        cells = [fmt(build.get(app, {}).get(mode, {}).get(section, {}).get(key), key) for app in names]
        lines.append(f"| {label} | " + " | ".join(cells) + " |")
    return "\n".join(lines)


def attribution_table(data, rev="NOW"):
    lines = ["| App | PSS total | core ref | app residual | core share | CPU total | core ref | core share |",
             "|---|---:|---:|---:|---:|---:|---:|---:|"]
    for app in APPS:
        part = data.get(rev, {}).get(app, {})
        pss, cpu = part.get("settled_pss_mib"), part.get("cpu_percent")
        cell = lambda d, k, unit="": "n/a" if not d else f"{d[k]:.0f}{unit}"
        lines.append(f"| {APP_NAMES[app]} | {cell(pss, 'total')} | {cell(pss, 'core_software')} | "
                     f"{cell(pss, 'app_residual')} | {cell(pss, 'core_share_percent', '%')} | "
                     f"{cell(cpu, 'total')} | {cell(cpu, 'core_software')} | {cell(cpu, 'core_share_percent', '%')} |")
    return "\n".join(lines)


def counter_table(data, *, gpu=False):
    from counters import ENGINE_GT, SCENARIOS
    columns = (["RCS Mcycles", "VCS Mcycles", "BCS Mcycles", "CCS Mcycles", "RC6 ms", "Package J", "GPU J"]
               if gpu else ["Instructions M", "Cycles M", "M instructions / s", "Instructions / frame"])
    lines = ["| App | Scenario | " + " | ".join(columns) + " |",
             "|---|---|" + "---:|" * len(columns)]

    def scaled(value, divisor=1):
        return "n/a" if value is None else f"{value / divisor:,.2f}"

    for app in (*APPS, "core"):
        for scenario in SCENARIOS:
            row = data["results"].get(app, {}).get(scenario)
            if row is None:
                continue
            if gpu:
                dev = row.get("gpu", {}) or {}
                engines = dev.get("engines", {})
                values = [scaled(engines.get(e, {}).get("busy_cycles_estimate"), 1e6) for e in ENGINE_GT]
                values += [scaled(dev.get("rc6_ns_gt0"), 1e6)]
                values += [scaled(dev.get("joules", {}).get(domain)) for domain in ("pkg", "gpu")]
            else:
                cpu = row.get("cpu", {}) or {}
                elapsed = row.get("cpu_window_seconds")
                instructions = cpu.get("instructions")
                values = [scaled(instructions, 1e6), scaled(cpu.get("cycles"), 1e6),
                          scaled(instructions / elapsed, 1e6)
                          if instructions is not None and elapsed else "n/a",
                          scaled((row.get("normalization", {}) or {}).get("instructions_per_rendered_video_frame"))]
            lines.append(f"| {APP_NAMES.get(app, 'ntctl')} | {scenario} | " + " | ".join(values) + " |")
    return "\n".join(lines)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metrics", type=Path, default=OUT / "final/metrics.json")
    parser.add_argument("--stability", type=Path, default=OUT / "stability/stability.json")
    parser.add_argument("--resources", type=Path, default=OUT / "resources/resources.json")
    parser.add_argument("--attribution", type=Path, default=OUT / "resources/attribution.json")
    parser.add_argument("--counters", type=Path, default=OUT / "counters/counters.json")
    args = parser.parse_args()
    for title, path, render in (("Metrics", args.metrics, metric_table),
                                ("Stability", args.stability, lambda d: stability_table(d["results"])),
                                ("Resources", args.resources, resource_table),
                                ("Attribution", args.attribution, attribution_table),
                                ("CPU counters", args.counters, counter_table),
                                ("GPU and energy counters", args.counters, lambda d: counter_table(d, gpu=True))):
        if path.is_file():
            print(f"### {title}\n\n{render(json.loads(path.read_text()))}\n")
