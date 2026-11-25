"""CPU and warm-switch labels from the normalized receipts, with placeholders until they exist.

CPU share = CPU-seconds ÷ wall-seconds ÷ logical CPUs; cores busy = CPU-seconds ÷ wall-seconds
(docs/media/normalized.md). Every CPU share and warm time on screen goes through this module. A
missing file or key renders as PENDING, so the final values need only a rerun of render.py. If the
receipt renames a key, edit PATHS only.
"""
import data

NORMALIZED = ".cache/media/out/normalized.json"
NORMALIZED_DOC = "docs/media/normalized.md"
PENDING = "pending normalization"

# label id → key path inside NORMALIZED. "{app}" is replaced by iced, gpui or rune.
PATHS = {
    "fly_wheel_share": ("fly", "{app}", "wheel", "summary", "machine_percent", "median"),
    "fly_wheel_cores": ("fly", "{app}", "wheel", "summary", "cores_busy", "median"),
    "laptop_logical_cpus": ("logical_cpus", "laptop"),
    "fly_logical_cpus": ("logical_cpus", "fly"),
    # The receipt publishes no ad-excluded warm time for the Fly YouTube runs yet; it says the
    # historical windows include ads and cannot establish ad duration.
    "youtube_warm_ads_excluded_s": ("fly", "youtube", "warm", "summary", "content_excluding_ads_s", "median"),
}


def receipt():
    path = data.ROOT / NORMALIZED
    return data.read(NORMALIZED) if path.is_file() else None


def value(key, app=None):
    node = receipt()
    for part in PATHS[key]:
        if not isinstance(node, dict):
            return None
        node = node.get(part.format(app=app))
    return node if isinstance(node, (int, float)) and not isinstance(node, bool) else None


def sources():
    return [path for path in (NORMALIZED, NORMALIZED_DOC) if (data.ROOT / path).is_file()]


def label(cores, share):
    return f"{share:.1f}% • {cores:.2f} cores"


def cpu_text(prefix, app):
    """Return the CPU label for one app, or the pending placeholder."""
    if prefix == "laptop_gate_wheel":
        # The perf gate reports one-core percent; convert with the receipt's logical CPU count.
        logical = value("laptop_logical_cpus")
        if logical is None:
            return PENDING
        cores = data.gate()[app] / 100
        return label(cores, cores * 100 / logical)
    share, cores = value(f"{prefix}_share", app), value(f"{prefix}_cores", app)
    return PENDING if share is None or cores is None else label(cores, share)


def warm_trials():
    """Warm per-trial records from the Fly YouTube rerun (docs/media/normalized.md, "Fly YouTube rerun")."""
    node = receipt()
    if node is None:
        return []
    return [row for row in node.get("web_rerun", []) if row.get("kind") == "result" and row.get("mode") == "warm"]


def warm_lines():
    """Two footnote lines built from the warm rerun trials, or the pending placeholder."""
    trials = warm_trials()
    if not trials or any(row.get("any_frame_ms") is None for row in trials):
        return [f"YouTube warm switch: {PENDING}."]
    words = {1: "one", 2: "two", 3: "three"}
    fastest = min(row["any_frame_ms"] for row in trials) / 1000
    slowest = max(row["any_frame_ms"] for row in trials) / 1000
    ads = [row["ad_mode_ms"] / 1000 for row in trials if row.get("status") == "complete" and row.get("ad_mode_ms")]
    timeouts = sum(row.get("status") == "deadline" for row in trials)
    count = words.get(len(trials), str(len(trials)))
    lines = [f"Warm (Fly rerun, {count} signed-out trials): YouTube shows a first frame in about "
             f"{fastest:.1f}–{slowest:.1f} s."]
    tail = []
    if ads:
        tail.append(f"on {words.get(len(ads), len(ads))} of {count} trials a "
                    f"{' and '.join(f'{s:.1f} s' for s in ads)} pre-roll ad played before the video")
    if timeouts:
        tail.append(f"{words.get(timeouts, timeouts)} trial{'s' if timeouts > 1 else ''} timed out behind an ad")
    if tail:
        text = "; ".join(tail)
        lines.append(f"But {text}.")
    return lines


def across_apps(machine, workload, field, apps=("iced", "gpui", "rune")):
    """(lowest, highest) per-app median of one normalized field, or None."""
    node = receipt()
    if node is None:
        return None
    found = []
    for app in apps:
        try:
            found.append(node[machine][app][workload]["summary"][field]["median"])
        except (KeyError, TypeError):
            return None
    return min(found), max(found)


def wheel_rate(app, field):
    """Laptop wheel instructions per scroll second or per drawn frame, or None."""
    node = receipt()
    try:
        return node["laptop"][app]["wheel"]["summary"][field]["median"]
    except (KeyError, TypeError):
        return None


def cold_trials():
    """Cold per-trial records from the Fly YouTube rerun (trusted click → first video frame)."""
    node = receipt()
    if node is None:
        return []
    return [row for row in node.get("web_rerun", []) if row.get("kind") == "result" and row.get("mode") == "cold"
            and row.get("status") == "complete" and row.get("actual_frame_elapsed_ms") is not None]


def cold_range_s():
    """(fastest, slowest) cold rerun first content frame in seconds, or None."""
    trials = cold_trials()
    if not trials:
        return None
    times = [row["actual_frame_elapsed_ms"] / 1000 for row in trials]
    return min(times), max(times)
