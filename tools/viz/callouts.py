"""Difference callouts and end-card numbers, derived from the receipts at render time.

Every badge compares values on one machine only. Differences use the rounded values shown on
screen, so the badge always equals the visible subtraction. render.py writes each label into
sources.json with the receipts it came from; a re-measurement only needs a rerender.
"""

import data

# Prewarm policy of the measured NOW snapshot. Replace once the matched re-measurement lands.
PREWARM_NOTE = "In these runs, iced and GPUI prewarm the video surface; Rune attaches it on the first open."
STAGES = (("cold", "Cold Home"), ("warm", "Warm Home"), ("playing", "Playback"), ("peak", "Tour peak"))


def spread(rows, field, apps=data.APPS):
    """(highest app, lowest app, rounded gap) for one field on one machine."""
    shown = {app: round(rows[app][field]) for app in apps if rows.get(app, {}).get(field) is not None}
    high, low = max(shown, key=shown.get), min(shown, key=shown.get)
    return high, low, shown[high] - shown[low]


def first_frame(fly):
    _, laptop = data.timing({})
    result = {"laptop": spread(laptop, "open_ms")}
    if "timing" in fly:
        result["fly"] = spread(fly["timing"], "open_ms")
    return result


def scroll():
    _, rows = data.wheel({})
    most = max(data.APPS, key=lambda app: rows[app]["rate"])
    least = min(data.APPS, key=lambda app: rows[app]["rate"])
    return most, least, rows[most]["rate"] / rows[least]["rate"]


def memory():
    _, rows = data.memory({})
    return {key: spread(rows, key) for key, _ in STAGES}


def fly_memory(fly):
    rows = fly.get("memory_compare")
    return spread(rows, "mib") if rows else None


def endings(fly):
    """Scene → (value, decimals, unit, takeaway, color app or key, badge, caveat)."""
    _, laptop = data.timing({})
    fast = min(data.APPS, key=lambda app: laptop[app]["open_ms"])
    slow, _, gap = first_frame(fly)["laptop"]
    most, least, ratio = scroll()
    _, mem = data.memory({})
    heavy, lean, mem_gap = memory()["peak"]
    catalog = data.read(data.HICCUPS)["incidents"]
    fixed = sum(bool(row["fixed_when"]) for row in catalog)
    fly_mem = fly_memory(fly)
    fly_note = (f"Laptop tour, local fixtures. On the Fly VM, {data.NAMES[fly_mem[1]]} peaked lowest."
                if fly_mem else "Laptop tour, local fixtures.")
    return {
        "FramePath": (19, 0, "×", "About 19× fewer instructions per rendered frame.", "mint",
                      "Software → GPU subsurface • laptop", "Laptop • AC counters • same clip in all three apps"),
        "FirstFrame": (round(laptop[fast]["open_ms"]), 0, "ms",
                       f"{data.NAMES[fast]} • laptop click → first video frame", fast,
                       f"+{gap} ms • {data.NAMES[slow]} vs {data.NAMES[fast]}, laptop",
                       "Native apps replay local fixtures; YouTube web loads live over the network."),
        "ScrollCost": (round(data.wheel({})[1][most]["rate"] / 1e6), 0, "M/s",
                       f"{data.NAMES[most]} • laptop wheel instructions per second", most,
                       f"≈{ratio:.1f}× • {data.NAMES[most]} vs {data.NAMES[least]}, laptop",
                       "Laptop • AC • user-space instruction counters"),
        "Memory": (round(mem[lean]["peak"]), 0, "MiB", f"{data.NAMES[lean]} • laptop tour peak, process-tree PSS",
                   lean, f"+{mem_gap} MiB • {data.NAMES[heavy]} vs {data.NAMES[lean]}, laptop", fly_note),
        "Hiccups": (len(catalog), 0, "", f"{fixed} with fix evidence", "mint",
                    f"{len(catalog) - fixed} still open in the catalog", "Project incident catalog"),
    }


def labels(fly):
    """sources.json entries for every derived on-screen difference."""
    timing_sources = sorted({path for row in data.timing({})[1].values() for path in row["sources"]})
    result = {"FirstFrame": {}, "ScrollCost": {}, "Memory": {}}
    for machine, (high, low, gap) in first_frame(fly).items():
        rows = data.timing({})[1] if machine == "laptop" else fly["timing"]
        result["FirstFrame"][f"+{gap} ms badge, {machine}: {data.NAMES[high]} minus {data.NAMES[low]} open_ms"] = sorted(
            {path for app in (high, low) for path in rows[app]["sources"]})
    most, least, ratio = scroll()
    wheel_sources = sorted({path for row in data.wheel({})[1].values() for path in row["sources"]})
    result["ScrollCost"][f"≈{ratio:.1f}× badge, laptop: {data.NAMES[most]} ÷ {data.NAMES[least]} wheel rate"] = (
        wheel_sources)
    mem_sources = sorted({path for row in data.memory({})[1].values() for path in row["sources"]})
    for key, (high, low, gap) in memory().items():
        result["Memory"][f"+{gap} MiB badge, laptop {key}: {data.NAMES[high]} minus {data.NAMES[low]}"] = mem_sources
    if fly_mem := fly_memory(fly):
        high, low, gap = fly_mem
        rows = fly["memory_compare"]
        result["Memory"][f"+{gap} MiB badge, Fly native peaks: {data.NAMES[high]} minus {data.NAMES[low]}"] = sorted(
            {path for app in (high, low) for path in rows[app]["sources"]})
    for scene, (value, decimals, unit, takeaway, _, badge, caveat) in endings(fly).items():
        result.setdefault(scene, {})[f"end card: {value:.{decimals}f} {unit} • {takeaway} • {badge} • {caveat}"] = {
            "FramePath": [data.METHOD, data.COUNTERS, data.REVIEW],
            "FirstFrame": timing_sources + ([data.FLY_METHOD] if fly else []),
            "ScrollCost": wheel_sources,
            "Memory": mem_sources + ([data.FLY_METHOD] if fly else []),
            "Hiccups": [data.HICCUPS],
        }[scene]
    return result
