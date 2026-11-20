"""Story summaries and baseline-relative budget decisions."""
import json
import math
import statistics

from performance.procfs import COUNTS, MEMORY, TICKS, complete_sum


def percentile(values, fraction):
    """Nearest-rank percentile, with no value for an empty series."""
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)] if values else None


def difference(after, before):
    return None if before is None or after is None else after - before


def median(values):
    return statistics.median(values) if values and None not in values else None


def totals(samples, metric):
    if metric == "processes":
        return [len(sample["processes"]) or None for sample in samples]
    return [complete_sum([row[metric] for row in sample["processes"]]) for sample in samples]


def peak(samples, metric):
    values = totals(samples, metric)
    return max(values) if values and None not in values else None


def settled(samples, metric):
    if len(samples) < 3:
        return None
    return median(totals(samples[-3:], metric))


def counters(processes, metric):
    if not processes:
        return None
    if metric == "cpu_ticks":
        return {row["id"]: row[metric] for row in processes}
    if any(row[metric] is None for row in processes):
        return None
    return {task: value for row in processes for task, value in row[metric].items()}


def activity(samples, metric):
    if len(samples) < 3:
        return None
    values = [counters(sample["processes"], metric) for sample in samples]
    if None in values:
        return None
    # Compare the same task or process identity across adjacent reads. Tasks
    # that come and go between reads can only make these deltas lower bounds.
    deltas = [
        after[key] - before[key]
        for before, after in zip(values, values[1:])
        for key in before.keys() & after.keys()
    ]
    return sum(deltas) if deltas and min(deltas) >= 0 else None


def phase_metrics(samples):
    result = {}
    for metric in (*MEMORY, *COUNTS):
        result[f"peak_{metric}"] = peak(samples, metric)
        result[f"settled_{metric}"] = settled(samples, metric)
    ticks = activity(samples, "cpu_ticks")
    switches = activity(samples, "context_switches_by_task")
    seconds = samples[-1]["t_s"] - samples[0]["t_s"] if len(samples) >= 3 else None
    result["sampled_seconds"] = seconds
    result["cpu_seconds"] = None if ticks is None else ticks / TICKS
    result["cpu_percent"] = None if ticks is None else ticks / TICKS / seconds * 100
    result["context_switches_proxy_per_s"] = None if switches is None else switches / seconds
    return result


def phase_windows(app, expected, home_s, samples):
    phases = app["phases"]
    names = [phase["name"] for phase in phases]
    if names != expected:
        raise ValueError(f"expected phases {expected}, got {names}")
    # The first phase ends at the Home wait. Its boundary is approximated by
    # the externally observed Home mark. Exclude phase edges to allow for
    # logging, wake-up and sampler alignment error.
    start = home_s
    result = {}
    for phase in phases[1:]:
        end = start + phase["seconds"]
        window = [s for s in samples
                  if start + 0.15 <= s["t_s"] - s["span_s"] / 2
                  and s["t_s"] + s["span_s"] / 2 <= end - 0.15]
        # Avoid passing on just a short fragment of a phase when the sampler
        # was starved, the app died, or a sampling timestamp drifted.
        if (len(window) < 3 or window[0]["t_s"] - start > 0.65
                or end - window[-1]["t_s"] > 0.65
                or any(b["t_s"] - a["t_s"] > 0.65 for a, b in zip(window, window[1:]))):
            window = []
        result[phase["name"]] = {"start_s": start, "end_s": end, "samples": len(window),
                                 **phase_metrics(window)}
        start = end
    return result


def playback_latencies(app, expected_opens):
    commands = [(at, json.loads(cmd)["cmd"]) for at, cmd in app["commands"]]
    opens = [i for i, (_, cmd) in enumerate(commands) if cmd == "open_video"]
    if len(opens) != expected_opens:
        raise ValueError(f"expected {expected_opens} video opens, got {len(opens)}")
    latencies = []
    for i in opens:
        start = commands[i][0]
        end = commands[i + 1][0] if i + 1 < len(commands) else math.inf
        watches = [at for at, mark in app["all_marks"] if mark == "watch_loaded" and start <= at < end]
        frames = [at for at, mark in app["all_marks"] if mark == "playback_started" and start <= at < end]
        if not watches or not frames:
            raise ValueError(f"video open at {start:.1f} ms has no Watch/first-frame mark before the next command")
        latencies.append(frames[0] - start)
    return latencies


def draw_metrics(phase):
    has_intervals = phase["frames"] >= 2
    return {
        f"draw_gap_p{p}_ms": phase[f"frame_ms_p{p}"] if has_intervals else None
        for p in (50, 95, 99)
    }


def navigation_growth(recovery):
    # Skip warm-up by comparing two equal, consecutive late windows.
    if len(recovery) < 8:
        return None
    return difference(median(recovery[-3:]), median(recovery[-6:-3]))


def summarize(story, steps, app, samples, home_s, window_ms):
    if app["failed_waits"]:
        raise ValueError(f"autopilot waits failed: {app['failed_waits']}")
    phases = phase_windows(app, [step["phase"] for step in steps if "phase" in step], home_s, samples)
    metrics = {}
    for name, phase in phases.items():
        if name == "settling" or name.startswith(("visit_", "recovered_")):
            continue
        metrics.update({f"{name}_{key}": value for key, value in phase.items()
                        if key not in ("start_s", "end_s", "samples", "sampled_seconds")})

    if story == "load":
        first_frame = app["marks_ms"].get("first_frame")
        home_ms = home_s * 1000
        metrics.update(window_ms=window_ms, home_ms=home_ms,
                       first_frame_ms=None if first_frame is None else
                       home_ms + first_frame - app["marks_ms"]["home_loaded"])
        metrics["idle_pss_mib"] = phases["idle"]["settled_pss_mib"]
        metrics.update({f"startup_peak_{key}": peak(
            [s for s in samples if s["t_s"] <= home_s], key
        ) for key in MEMORY})

    if story in ("browse", "play", "live"):
        for name in ("wheel", "fling", "playing", "switched"):
            phase = next((p for p in app["phases"] if p["name"] == name), None)
            if phase:
                metrics.update({f"{name}_{key}": value for key, value in draw_metrics(phase).items()})
                if name in ("playing", "switched"):
                    metrics[f"{name}_video_rendered_frames"] = phase["video_frames_rendered"] or None
    if story == "live":
        search, = [at for at, cmd in app["commands"] if json.loads(cmd)["cmd"] == "search"]
        ready = next(at for at, mark in app["all_marks"] if mark == "search_loaded" and at >= search)
        metrics["search_content_ms"] = ready - search

    if story in ("play", "live"):
        first, switch = playback_latencies(app, 2)
        metrics.update(click_first_frame_ms=first, warm_switch_ms=switch)
        metrics.update({f"retained_{key}": difference(phases["recovered"][f"settled_{key}"],
                                                      phases["idle"][f"settled_{key}"])
                        for key in (*MEMORY, *COUNTS)})
        for key in MEMORY:
            active = [phases[name][f"peak_{key}"] for name in ("playing", "switched")]
            metrics[f"playback_peak_{key}"] = max(active) if None not in active else None

    if story == "live":
        metrics["home_ms"] = home_s * 1000
        metrics["window_ms"] = window_ms
        for key in MEMORY:
            metrics[f"journey_peak_{key}"] = peak(
                [s for s in samples if home_s <= s["t_s"] <= phases["recovered"]["end_s"] - 0.15],
                key,
            )

    if story == "navigation":
        recovery = [phase for name, phase in phases.items() if name.startswith("recovered_")]
        latencies = playback_latencies(app, len(recovery))
        metrics["visit_first_frame_p95_ms"] = percentile(latencies, 0.95)
        for key in (*MEMORY, *COUNTS):
            values = [phase[f"settled_{key}"] for phase in recovery]
            metrics[f"late_growth_{key}"] = navigation_growth(values)
            metrics[f"warmed_retained_{key}"] = difference(
                median(values[-3:]), phases["idle"][f"settled_{key}"])
    return {"metrics": metrics, "phases": phases}


def compare_metric(before, after, allowed=None, cap=None):
    if before is None or after is None:
        return {"result": "unavailable" if allowed is None and cap is None else "fail",
                "reason": "missing or incomplete samples", "delta": None}
    delta = after - before
    reasons = []
    if allowed is not None and delta > allowed:
        reasons.append("baseline delta exceeds budget")
    if cap is not None and after > cap:
        reasons.append("candidate exceeds absolute cap")
    result = "fail" if reasons else "pass" if allowed is not None or cap is not None else "report"
    return {"result": result, "reason": ", ".join(reasons) or None, "delta": delta}
