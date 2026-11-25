#!/usr/bin/env python3
"""Summarize the matched fairness runs (tools/iso-test/fair.py) into JSON and Markdown tables.

Each environment directory holds <suite>/<condition>/<app>/round-<n>/sample.json. Round 1 is the
cold run and is reported apart; the warm statistics are the median and min–max of the other
rounds. A difference is a win only when the ranges don't overlap.
"""
import argparse
import json
from pathlib import Path
import statistics

APPS = ("iced", "gpui", "rune")
NAMES = {"iced": "iced", "gpui": "GPUI", "rune": "Rune"}
SHIPPED = {"iced": "full", "gpui": "full", "rune": "off"}


def instr(sample, window):
    return ((sample.get("perf") or {}).get(window) or {}).get("instructions")


def per(value, count):
    return None if value is None or not count else value / count


# (key, label, unit, suite, extractor)
METRICS = [
    ("window_ms", "Window map", "ms", "latency", lambda s: s.get("window_ms")),
    ("home_ms", "Home loaded", "ms", "latency", lambda s: s.get("home_ms")),
    ("open_ms", "Open → first frame", "ms", "latency", lambda s: s.get("open_ms")),
    ("switch_ms", "Warm switch → first frame", "ms", "latency", lambda s: s.get("switch_ms")),
    ("home_idle_pss_mib", "Home idle PSS", "MiB", "latency", lambda s: s.get("home_idle_pss_mib")),
    ("peak_pss_mib", "Peak PSS (Home, open, 6 s playback, switch)", "MiB", "latency",
     lambda s: s.get("peak_pss_mib")),
    ("home_cache_mib", "Home thumbnail cache", "MiB", "latency", lambda s: s.get("home_cache_mib")),
    ("playback_cores", "Default-path playback", "cores", "latency",
     lambda s: (s.get("playback") or {}).get("cores")),
    ("playback_ui_frames", "UI frames during 6 s playback", "frames", "latency",
     lambda s: (s.get("playback") or {}).get("ui_frames")),
    ("playback_minstr_per_frame", "Default-path playback instructions", "M/video frame", "latency",
     lambda s: per(instr(s, "playback"), (s.get("playback") or {}).get("video_frames")) and
     per(instr(s, "playback"), (s.get("playback") or {}).get("video_frames")) / 1e6),
    ("home_minstr", "Instructions exec → Home", "M", "latency",
     lambda s: instr(s, "home") and instr(s, "home") / 1e6),
    ("open_minstr", "Instructions per open", "M", "latency",
     lambda s: instr(s, "open") and instr(s, "open") / 1e6),
    ("switch_minstr", "Instructions per switch", "M", "latency",
     lambda s: instr(s, "switch") and instr(s, "switch") / 1e6),
    ("software_cores", "Forced-software playback", "cores", "software",
     lambda s: (s.get("playback") or {}).get("cores")),
    ("software_minstr_per_frame", "Forced-software playback instructions", "M/video frame", "software",
     lambda s: per(instr(s, "playback"), (s.get("playback") or {}).get("video_frames")) and
     per(instr(s, "playback"), (s.get("playback") or {}).get("video_frames")) / 1e6),
    ("wheel_cores", "Wheel scroll (27 notches, 2.7 s)", "cores", "wheel",
     lambda s: (s.get("wheel") or {}).get("cores")),
    ("wheel_frames", "Frames drawn during wheel", "frames", "wheel",
     lambda s: (s.get("wheel") or {}).get("frames")),
    ("wheel_minstr_per_frame", "Wheel instructions", "M/drawn frame", "wheel",
     lambda s: per(instr(s, "wheel"), (s.get("wheel") or {}).get("frames")) and
     per(instr(s, "wheel"), (s.get("wheel") or {}).get("frames")) / 1e6),
    ("wheel_cache_added_mib", "Thumbnail bytes decoded during wheel", "MiB", "wheel",
     lambda s: (s.get("wheel") or {}).get("cache_mib_added")),
]
# Counts and cache bytes describe work, not a better/worse outcome.
DESCRIPTIVE = {"home_cache_mib", "playback_ui_frames", "wheel_frames", "wheel_cache_added_mib"}


def load(env_dir):
    rows = []
    for path in sorted(env_dir.glob("*/*/*/round-*/sample.json")):
        sample = json.loads(path.read_text())
        sample.setdefault("round", int(path.parent.name.split("-")[1]))
        rows.append(sample)
    return rows


def valid(sample):
    return not sample.get("error") and not sample.get("refused") and not sample.get("failed_waits")


def stats(values):
    values = [v for v in values if v is not None]
    if not values:
        return None
    return {"n": len(values), "median": statistics.median(values), "min": min(values), "max": max(values)}


def summarize(rows):
    out = {}
    for key, _, _, suite, extract in METRICS:
        out[key] = {}
        for condition in ("off", "full", "default"):
            per_app = {}
            for app in APPS:
                runs = [r for r in rows if r["app"] == app and r["suite"] == suite
                        and r["condition"] == condition and valid(r)]
                if not runs:
                    continue
                cold = [extract(r) for r in runs if r["round"] == 1]
                warm = stats([extract(r) for r in runs if r["round"] > 1])
                if warm:
                    per_app[app] = {"cold": cold[0] if cold else None, "warm": warm}
            if per_app:
                out[key][condition] = per_app | {"verdict": verdict(key, per_app)}
    return out


def verdict(key, per_app):
    if key in DESCRIPTIVE:
        return None
    if any(a not in per_app or per_app[a]["warm"]["n"] < 5 for a in APPS):
        return {"kind": "incomplete", "text": "Fewer than five valid warm samples for at least one app"}

    def beats(a, b):
        return per_app[a]["warm"]["max"] < per_app[b]["warm"]["min"]

    tied = [app for app in APPS if not any(beats(other, app) for other in APPS)]
    if len(tied) == 1:
        best, = tied
        return {"kind": "win", "text": f"{NAMES[best]} wins", "best": best}
    rest = [app for app in APPS if all(beats(leader, app) for leader in tied)]
    text = " = ".join(NAMES[a] for a in tied) + " tied within noise"
    if rest:
        text += "; ahead of " + ", ".join(NAMES[a] for a in rest)
    return {"kind": "tie", "text": text, "tied": tied}


def fmt(value, unit):
    if value is None:
        return "—"
    if unit == "cores":
        return f"{value:.3f}"
    if abs(value) >= 100:
        return f"{value:,.0f}"
    return f"{value:.1f}" if abs(value) >= 10 else f"{value:.2f}"


def table(summary, env_name, conditions):
    lines = []
    for condition in conditions:
        label = {"off": "NT_PREWARM=off", "full": "NT_PREWARM=full",
                 "default": "shipped default (iced full, GPUI full, Rune off)"}[condition]
        rows = [(key, lab, unit) for key, lab, unit, _, _ in METRICS if condition in summary[key]]
        if not rows:
            continue
        lines += [f"### {env_name}: {label}", "",
                  "Warm median (min–max) over valid rounds 2–6. Cold values come from round 1.", "",
                  "| Metric | Unit | iced | GPUI | Rune | Verdict |", "|---|---|---|---|---|---|"]
        for key, lab, unit in rows:
            entry = summary[key][condition]
            cells = []
            for app in APPS:
                if app not in entry:
                    cells.append("—")
                    continue
                w = entry[app]["warm"]
                cells.append(f"{fmt(w['median'], unit)} ({fmt(w['min'], unit)}–{fmt(w['max'], unit)}); "
                             f"cold {fmt(entry[app]['cold'], unit)}"
                             + (f"; warm n={w['n']}" if w["n"] != 5 else ""))
            v = entry.get("verdict")
            lines.append(f"| {lab} | {unit} | " + " | ".join(cells) + f" | {v['text'] if v else 'descriptive'} |")
        lines.append("")
    return lines


def checks(rows):
    """Policy and path evidence per environment."""
    out = {}
    for app in APPS:
        for condition in ("off", "full", "default"):
            runs = [r for r in rows if r["app"] == app and r["suite"] == "latency" and r["condition"] == condition]
            if not runs:
                continue
            maps = [r.get("idle_maps") or {} for r in runs if valid(r)]
            pre = [r.get("prewarm") or {} for r in runs if valid(r)]
            out[f"{app}/{condition}"] = {
                "runs": len(runs), "valid": sum(map(valid, runs)),
                "policy": sorted({p.get("policy") for p in pre}, key=str),
                "libmpv_mapped_at_home": sorted({m.get("libmpv") for m in maps}, key=str),
                "egl_mapped_at_home": sorted({m.get("egl") for m in maps}, key=str),
                "player_prewarm_after_home_ms": stats([p.get("player_after_home_ms") for p in pre]),
                "surface_prewarm_after_home_ms": stats([p.get("surface_after_home_ms") for p in pre]),
                "gpu_surface_attached": sorted({r.get("gpu_surface_attached") for r in runs if valid(r)}, key=str),
                "comments_loaded_on_open": sorted({r.get("comments_loaded") for r in runs if valid(r)}, key=str),
                "loadavg_1m_at_start": stats([(r.get("gate") or {}).get("loadavg_1m_at_start") for r in runs]),
                "epp": sorted({(r.get("machine_before") or {}).get("energy_performance_preference")
                               for r in runs}, key=str),
                "governor": sorted({(r.get("machine_before") or {}).get("governor") for r in runs}, key=str),
                "ac": sorted({(r.get("machine_before") or {}).get("ac") for r in runs}, key=str),
            }
    out["invalid"] = [
        {k: r.get(k) for k in ("app", "suite", "condition", "round", "error", "failed_waits")}
        for r in rows if not valid(r)
    ]
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env", action="append", required=True, help="NAME=DIR")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--markdown", type=Path)
    args = parser.parse_args()
    result = {}
    md = []
    for item in args.env:
        name, path = item.split("=", 1)
        rows = load(Path(path))
        summary = summarize(rows)
        result[name] = {"source": path, "samples": len(rows), "metrics": summary, "checks": checks(rows),
                        "setup": json.loads((Path(path) / "setup.json").read_text())
                        if (Path(path) / "setup.json").is_file() else None}
        md += table(summary, name, ("off", "full", "default"))
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    if args.markdown:
        args.markdown.write_text("\n".join(md) + "\n")


if __name__ == "__main__":
    main()
