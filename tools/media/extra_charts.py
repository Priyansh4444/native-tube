#!/usr/bin/env python3
"""Render the incident chronology, stability scores and resource attribution."""
import argparse
from collections import Counter
from datetime import datetime
import json
from pathlib import Path

from charts import COLORS, LABELS, plt
from hiccups import KINDS
from metrics import APPS, APP_NAMES, MILESTONES, OUT, ROOT

PALETTE = ("#005baa", "#94355c", "#187557", "#ab5700", "#5946ad", "#2f707d", "#5f6674", "#8a6d00")
POS = range(len(MILESTONES))


def canvas(title, subtitle="", *, xlabel=True):
    fig, ax = plt.subplots(figsize=(9, 4.8), layout="constrained")
    fig.patch.set_facecolor("#f7f9fc")
    ax.set_title(title, loc="left", weight="bold", fontsize=17, pad=15)
    ax.text(0, 1.015, subtitle, transform=ax.transAxes, fontsize=8.5, color="#414c60")
    ax.grid(axis="y", color="#dfe5ed")
    ax.set_axisbelow(True)
    ax.spines[["top", "right"]].set_visible(False)
    if xlabel:
        ax.set_xticks(list(POS), LABELS, fontsize=9)
        ax.tick_params(axis="x", length=0)
    return fig, ax


def save(fig, out, stem):
    fig.savefig(out / f"{stem}.png", dpi=150)
    plt.close(fig)


def dates(data):
    return [datetime.fromisoformat(data["milestones"][rev]) for rev in MILESTONES]


def event_histogram(incidents, edges, field):
    result = Counter()
    for row in incidents:
        if value := row[field]:
            stamp = datetime.fromisoformat(value)
            i = next((i for i, cutoff in enumerate(edges) if stamp <= cutoff), None)
            if i is not None:
                result[i] += 1
    return [result[i] for i in POS]


def incident_charts(data, out):
    records, edges = data["incidents"], dates(data)
    fig, ax = canvas("Incidents found and fixed", "Events grouped by the first milestone at or after the dated record")
    for dx, field, label, color in ((-.18, "date", "Found", COLORS[0]), (.18, "fixed_when", "Fixed", COLORS[2])):
        bars = ax.bar([i + dx for i in POS], event_histogram(records, edges, field), .34, label=label, color=color)
        ax.bar_label(bars, fmt=lambda v: str(int(v)) if v else "", padding=2)
    ax.legend(loc="upper left")
    ax.set_ylabel("Incidents")
    ax.set_ylim(bottom=0, top=max(ax.get_ylim()[1], 4) * 1.09)
    save(fig, out, "incidents-found-fixed")

    fig, ax = canvas("Open incidents by area", "Known by the checkpoint; same-bucket discoveries and fixes can leave no open count")
    areas = ("iced", "GPUI", "Rune", "core", "test tooling", "orchestration")
    for area, color in zip(areas, PALETTE):
        values = [sum(row["area"] == area and datetime.fromisoformat(row["date"]) <= cutoff
                      and (row["fixed_when"] is None or datetime.fromisoformat(row["fixed_when"]) > cutoff)
                      for row in records) for cutoff in edges]
        ax.plot(list(POS), values, marker="o", color=color, label=area)
    ax.legend(loc="upper left", ncols=3)
    ax.set_ylabel("Open incidents")
    ax.set_ylim(bottom=-.08, top=max(ax.get_ylim()[1], 1.5))
    save(fig, out, "incidents-open")

    # Per app: incidents found in each interval (bars) and still open at each checkpoint (lines).
    fig, (found, still) = plt.subplots(1, 2, figsize=(12, 4.8), layout="constrained", sharey=False)
    fig.patch.set_facecolor("#f7f9fc")
    fig.suptitle("Incidents per app", x=0.01, ha="left", weight="bold", fontsize=17)
    owners = (("iced", COLORS[0]), ("GPUI", COLORS[1]), ("Rune", COLORS[2]), ("core", PALETTE[3]))
    width = .8 / len(owners)
    for j, (area, color) in enumerate(owners):
        rows = [row for row in records if row["area"] == area]
        heights = event_histogram(rows, edges, "date")
        bars = found.bar([i + (j - 1.5) * width for i in POS], heights, width * .92, color=color, label=area)
        found.bar_label(bars, fmt=lambda v: str(int(v)) if v else "", padding=1, fontsize=7)
        values = [sum(datetime.fromisoformat(row["date"]) <= cutoff
                      and (row["fixed_when"] is None or datetime.fromisoformat(row["fixed_when"]) > cutoff)
                      for row in rows) for cutoff in edges]
        still.plot(list(POS), values, marker="os^D"[j], color=color, label=area, linewidth=2)
    for ax, title in ((found, "Found in the interval ending at each milestone"),
                      (still, "Open at each milestone (found, not yet fixed)")):
        ax.set_title(title, loc="left", fontsize=10, color="#414c60")
        ax.set_xticks(list(POS), LABELS, fontsize=8)
        ax.tick_params(axis="x", length=0)
        ax.grid(axis="y", color="#dfe5ed")
        ax.set_axisbelow(True)
        ax.spines[["top", "right"]].set_visible(False)
        ax.set_ylabel("Incidents")
        ax.set_ylim(bottom=0)
    found.set_ylim(top=found.get_ylim()[1] * 1.12)
    still.set_ylim(top=max(still.get_ylim()[1], 3) * 1.12)
    found.legend(loc="upper left", ncols=4, fontsize=8)
    still.yaxis.get_major_locator().set_params(integer=True)
    found.yaxis.get_major_locator().set_params(integer=True)
    save(fig, out, "incidents-by-app")

    fig, ax = canvas("Incidents found by kind","Classification follows the reported symptom or workflow interruption")
    lower = [0] * len(edges)
    for kind, color in zip(KINDS, PALETTE):
        heights = event_histogram([row for row in records if row["kind"] == kind], edges, "date")
        ax.bar(list(POS), heights, .6, bottom=lower, label=kind, color=color)
        lower = [a + b for a, b in zip(lower, heights)]
    ax.legend(loc="upper left", ncols=2, fontsize=8)
    ax.set_ylabel("Incidents")
    save(fig, out, "incidents-by-kind")


def stability_charts(data, out, notes=None):
    """One panel per app: clean-run score per milestone, each point labelled with its counts."""
    notes = notes or {}
    fig, axes = plt.subplots(1, 3, figsize=(13, 4.4), layout="constrained", sharey=True)
    fig.patch.set_facecolor("#f7f9fc")
    fig.suptitle("Playback stress stability per app", x=0.01, ha="left", weight="bold", fontsize=17)
    short = [label.split("\n")[0] for label in LABELS]
    for ax, app, color, marker in zip(axes, APPS, COLORS, ("o", "s", "^")):
        rows = [data.get(rev, {}).get(app, {}) for rev in MILESTONES]
        values = [row.get("score_percent") for row in rows]
        ax.plot(list(POS), values, marker=marker, color=color, linewidth=2.2, markersize=8)
        for i, (rev, row) in enumerate(zip(MILESTONES, rows)):
            if row.get("score_percent") is not None:
                text = f"{row['clean']}/{row['assessed']} clean\nweight {row['weighted_events']}"
                ax.annotate(text, (i, row["score_percent"]), textcoords="offset points", xytext=(0, -30),
                            ha="center", fontsize=7.5, color="#414c60")
            else:
                ax.text(i, 50, notes.get((rev, app), "not measured"), rotation=90, ha="center",
                        va="center", fontsize=8, color="#7a8394")
        ax.set_title(APP_NAMES[app], loc="left", fontsize=12, weight="bold", color=color)
        ax.set_xticks(list(POS), short, fontsize=8.5)
        ax.set_xlim(-0.4, len(MILESTONES) - 0.6)
        ax.set_ylim(0, 108)
        ax.tick_params(axis="x", length=0)
        ax.grid(axis="y", color="#dfe5ed")
        ax.set_axisbelow(True)
        ax.spines[["top", "right"]].set_visible(False)
    axes[0].set_ylabel("% of assessed runs with zero events")
    fig.text(0.01, -0.02, "Three 20-switch stress runs per build. Events: crash ×5, failed wait ×2, freeze ×3, "
             "black frame ×2, probe error ×1, report timeout ×2. Sampled checks can miss artifacts between samples.",
             fontsize=8, color="#414c60")
    fig.savefig(out / "stability-by-app.png", dpi=150, bbox_inches="tight", pad_inches=0.15)
    plt.close(fig)


def get_variant(build, app, mode, section, key):
    return build.get(app, {}).get(mode, {}).get(section, {}).get(key)


def attribution_charts(data, out):
    for rev, build in data.items():
        for key, title, unit in (("settled_pss_mib", "Steady playback memory", "PSS (MiB)"),
                                 ("cpu_percent", "Playback CPU", "% of one core")):
            core = get_variant(build, "core", "software", "playback", key)
            if core is None:
                continue
            fig, ax = canvas(f"{title} · {rev}", "Software totals use the same local clip; GPU totals are shown separately", xlabel=False)
            for i, app in enumerate(APPS):
                total = get_variant(build, app, "software", "playback", key)
                if total is not None:
                    ax.bar(i - .18, core, .33, color=COLORS[0], label="Core + libmpv reference" if i == 0 else None)
                    ax.bar(i - .18, total - core, .33, bottom=core, color=COLORS[2], hatch="//",
                           label="App total minus core reference" if i == 0 else None)
                    ax.plot(i - .18, total, "_", color="#111111")
                gpu = get_variant(build, app, "gpu", "playback", key)
                if gpu is not None:
                    ax.bar(i + .18, gpu, .33, color=COLORS[1], label="GPU app total" if i == 0 else None)
            ax.set_xticks(range(len(APPS)), [APP_NAMES[app] for app in APPS])
            ax.set_ylabel(unit)
            ax.legend(loc="upper left", fontsize=8)
            ax.set_ylim(bottom=0, top=ax.get_ylim()[1] * 1.21)
            save(fig, out, f"attribution-{rev}-{key}")


def resource_charts(data, out):
    series = {
        "memory": ("Proportional memory", "PSS (MiB)", (
            ("Cold Home", "cold", "idle", "settled_pss_mib"),
            ("Warm Home", "software", "idle", "settled_pss_mib"),
            ("Software steady", "software", "playback", "settled_pss_mib"),
            ("Software peak", "software", "playback", "peak_pss_mib"),
            ("GPU steady", "gpu", "playback", "settled_pss_mib"),
            ("GPU peak", "gpu", "playback", "peak_pss_mib"),
            ("Tour peak", "tour", "tour", "peak_pss_mib"),
        )),
        "cpu": ("Playback CPU by render path", "% of one core", (
            ("Software average", "software", "playback", "cpu_percent"),
            ("Software peak", "software", "playback", "peak_cpu_percent"),
            ("GPU average", "gpu", "playback", "cpu_percent"),
            ("GPU peak", "gpu", "playback", "peak_cpu_percent"),
        )),
    }
    for rev, build in data.items():
        for stem, (title, unit, rows) in series.items():
            columns = [(label, [get_variant(build, app, "gpu" if label == "Warm Home" and rev == "NOW"
                                           else mode, section, key) for app in APPS])
                       for label, mode, section, key in rows]
            columns = [(label, values) for label, values in columns if any(v is not None for v in values)]
            if not columns:
                continue
            fig, ax = canvas(f"{title} · {rev}", "Median of three runs • missing bars indicate unavailable measurements", xlabel=False)
            width = .82 / len(columns)
            for j, ((label, values), color) in enumerate(zip(columns, PALETTE)):
                positions = [i + (j - (len(columns) - 1) / 2) * width for i in range(len(APPS))]
                bars = [(pos, value) for pos, value in zip(positions, values) if value is not None]
                ax.bar([pos for pos, _ in bars], [value for _, value in bars], width * .93,
                       color=color, label=label)
            ax.set_xticks(range(len(APPS)), [APP_NAMES[app] for app in APPS])
            ax.set_ylabel(unit)
            ax.legend(loc="upper left", fontsize=8, ncols=3)
            ax.set_ylim(bottom=0, top=ax.get_ylim()[1] * 1.23)
            save(fig, out, f"resources-{rev}-{stem}")


def main(args):
    args.out.mkdir(parents=True, exist_ok=True)
    incident_charts(json.loads(args.hiccups.read_text()), args.out)
    if args.stability:
        notes = {("96df4ec", "gpui"): "n/a: Home replay key missing",
                 ("cb25193", "gpui"): "n/a: Home replay key missing"}
        if not json.loads(args.stability.read_text())["results"].get("NOW"):
            notes |= {("NOW", app): "pending: core still changing" for app in APPS}
        stability_charts(json.loads(args.stability.read_text())["results"], args.out, notes)
    if args.resources:
        data = json.loads(args.resources.read_text())
        attribution_charts(data, args.out)
        resource_charts(data, args.out)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hiccups", type=Path, default=OUT / "hiccups.json")
    parser.add_argument("--stability", type=Path)
    parser.add_argument("--resources", type=Path)
    parser.add_argument("--out", type=Path, default=ROOT / "docs/media")
    main(parser.parse_args())
