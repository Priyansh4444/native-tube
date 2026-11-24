#!/usr/bin/env python3
"""Plot the measured series with gaps where a milestone has no sample."""
import argparse
import math
import json
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

from metrics import APPS, APP_NAMES, METRICS, MILESTONES, ROOT

COLORS = ("#005baa", "#94355c", "#187557")
MARKERS = ("o", "s", "^")
LABELS = ("96df4ec\nFirst delivery", "cb25193\nReview fixes", "bf276b1\nSpec v2",
          "2caf0fa\nBaseline", "NOW\nWorking tree")


def charts(data, output):
    output.mkdir(parents=True, exist_ok=True)
    for metric, (title, unit) in METRICS.items():
        fig, ax = plt.subplots(figsize=(8.6, 4.6), layout="constrained")
        fig.patch.set_facecolor("#f7f9fc")
        ax.set_facecolor("#ffffff")
        if metric == "stripped_bytes":
            unit = "MiB"
        for app, color, marker in zip(APPS, COLORS, MARKERS):
            values = [data[rev][app]["metrics"][metric] for rev in MILESTONES]
            if metric == "stripped_bytes":
                values = [None if value is None else value / 2**20 for value in values]
            # Default idle footprints span three different libmpv startup regimes.
            spans = ((0, 3), (3, 4), (4, 5)) if metric == "idle_pss_mib" else ((0, len(values)),)
            for start, end in spans:
                ax.plot(range(start, end), values[start:end],
                        label=APP_NAMES[app] if start == 0 else "_nolegend_",
                        marker=marker, color=color, linewidth=2, markersize=7)
            if metric != "stripped_bytes":
                # Faint band: lowest to highest of the three runs behind each median.
                for i, rev in enumerate(MILESTONES):
                    runs = [r.get(metric) for r in data[rev][app].get("runs", []) if r]
                    runs = [v for v in runs if v is not None]
                    if values[i] is not None and len(runs) == 3:
                        ax.vlines(i + (APPS.index(app) - 1) * .07, min(runs), max(runs), color=color, alpha=.35, linewidth=4)
        ax.set_title(title, loc="left", weight="bold", fontsize=17, pad=15)
        method = ("One stripped release binary per build" if metric == "stripped_bytes"
                  else "Median of three muted fixture-replay runs (bar: lowest to highest run, clipped at the top)")
        subtitle = f"{method}  •  Gaps indicate unavailable measurements"
        if metric == "idle_pss_mib":
            subtitle = "Default Home state. Left of divider: linked libmpv. See README for prewarm by build."
            ax.axvline(2.5, color="#526277", linestyle=":", linewidth=1)
        ax.text(0.0, 1.015, subtitle,
                transform=ax.transAxes, fontsize=8, color="#414c60")
        ax.set_ylabel(unit)
        ax.set_xticks(range(len(LABELS)), LABELS)
        ax.tick_params(axis="x", length=0, labelsize=9)
        ax.set_xlim(-0.2, len(LABELS) - 0.8)
        finite = [float(v) for rev in MILESTONES for app in APPS
                  if (v := data[rev][app]["metrics"][metric]) is not None
                  if math.isfinite(float(v))]
        if metric == "stripped_bytes":
            finite = [v / 2**20 for v in finite]
        peak = max(finite, default=1)
        ax.set_ylim(0, peak * 1.15)
        ax.grid(axis="y", color="#dfe5ed")
        ax.set_axisbelow(True)
        ax.spines[["top", "right"]].set_visible(False)
        ax.legend(loc="upper left", bbox_to_anchor=(1.01, 1.0), frameon=False)
        fig.savefig(output / f"{metric}.png", dpi=150)
        plt.close(fig)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("metrics", type=Path)
    parser.add_argument("--out", type=Path, default=ROOT / "docs/media")
    args = parser.parse_args()
    charts(json.loads(args.metrics.read_text()), args.out)
