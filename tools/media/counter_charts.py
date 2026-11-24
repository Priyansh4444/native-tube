#!/usr/bin/env python3
"""Make phone-readable PNGs from counter receipts."""
import argparse
import json
from pathlib import Path

from charts import plt
from counters import ENGINE_GT, SCENARIOS
from metrics import APPS, APP_NAMES, OUT, ROOT

TITLES = {
    "home": "Home reload", "idle": "Idle Home · 10 s", "wheel": "Wheel scroll",
    "playback-gpu": "GPU playback · 10 s", "playback-software": "Software playback · 10 s",
    "warm-switch": "Warm related switch ×5",
}
FOREGROUND = "#173151"
REFERENCE = "#12676a"
RESIDUAL = "#bb5511"
ENGINE_COLORS = ("#15538b", "#94426a", "#167265", "#b75614")


def grid(results, title, subtitle, synthetic):
    fig, axes = plt.subplots(3, 2, figsize=(10, 11.75), layout="constrained")
    fig.patch.set_facecolor("#f4f7fb")
    prefix = "Synthetic check · " if synthetic else ""
    fig.suptitle(f"{prefix}{title}\n{subtitle}", weight="bold", fontsize=17,
                 color="#13283f", x=.02, ha="left")
    for name, ax in zip(SCENARIOS, axes.flat):
        ax.set_title(TITLES[name], loc="left", fontsize=14, weight="bold", color="#13283f")
        ax.set_facecolor("#ffffff")
        ax.set_yticks(range(len(APPS)), [APP_NAMES[app] for app in APPS], fontsize=13)
        ax.set_ylim(len(APPS) - .5, -.5)
        ax.tick_params(axis="x", labelsize=10)
        ax.tick_params(axis="y", length=0)
        ax.spines[["top", "right", "left"]].set_visible(False)
        ax.grid(axis="x", color="#dce5f1")
        ax.set_axisbelow(True)
        yield fig, ax, name, [results.get(app, {}).get(name, {}) for app in APPS]


def absent(ax, y):
    ax.text(.02, y, "n/a", transform=ax.get_yaxis_transform(), va="center",
            fontsize=12, color="#425670")


def save(fig, path):
    fig.savefig(path, dpi=150)
    plt.close(fig)


def cpu_charts(results, output, synthetic):
    for fig, ax, _, rows in grid(
            results, "CPU instructions", "Billions of user-space instructions · scales vary by panel", synthetic):
        for i, row in enumerate(rows):
            total = (row.get("cpu") or {}).get("instructions")
            if total is None:
                absent(ax, i)
                continue
            parts = row.get("attribution")
            if parts:
                core = parts["core_reference_instructions"] / 1e9
                residual = parts["app_minus_reference_instructions"] / 1e9
                ax.barh(i, core, height=.5, color=REFERENCE)
                if residual >= 0:
                    ax.barh(i, residual, left=core, height=.5, color=RESIDUAL)
                else:
                    ax.barh(i, -residual, left=total / 1e9, height=.5, color="#fff7ed",
                            edgecolor=RESIDUAL, hatch="//", linewidth=1)
            else:
                ax.barh(i, total / 1e9, height=.5, color=FOREGROUND)
            ax.plot(total / 1e9, i, "|", color="#101923", markersize=11)
        ax.set_xlim(left=0)
        ax.margins(x=.12)
    from matplotlib.patches import Patch
    fig.legend(handles=[
        Patch(color=FOREGROUND, label="App total"),
        Patch(color=REFERENCE, label="ntctl player reference"),
        Patch(color=RESIDUAL, label="App minus reference"),
        Patch(facecolor="#fff7ed", edgecolor=RESIDUAL, hatch="//", label="Below reference"),
    ], loc="outside lower center", ncols=2, frameon=False, fontsize=12)
    save(fig, output / "cpu-instructions.png")


def gpu_charts(results, output, synthetic):
    from matplotlib.patches import Patch

    for fig, ax, _, rows in grid(
            results, "GPU engine work", "Estimated millions of busy cycles · system-wide i915", synthetic):
        for i, row in enumerate(rows):
            engines = (row.get("gpu") or {}).get("engines", {})
            values = [engines.get(engine, {}).get("busy_cycles_estimate") for engine in ENGINE_GT]
            if any(value is None for value in values):
                absent(ax, i)
                continue
            left = 0
            for value, color in zip(values, ENGINE_COLORS):
                ax.barh(i, value / 1e6, left=left, height=.5, color=color)
                left += value / 1e6
        ax.set_xlim(left=0)
    fig.legend(handles=[Patch(color=color, label=name) for color, name in zip(ENGINE_COLORS, ENGINE_GT)],
               loc="outside lower center", ncols=4, frameon=False, fontsize=12)
    save(fig, output / "gpu-busy-cycles.png")

    for fig, ax, _, rows in grid(
            results, "Energy during each scenario", "Joules · system-wide package and GPU domains overlap", synthetic):
        for i, row in enumerate(rows):
            energy = (row.get("gpu") or {}).get("joules", {})
            if all(energy.get(name) is None for name in ("pkg", "gpu")):
                absent(ax, i)
                continue
            for delta, name, color in ((-.15, "pkg", FOREGROUND), (.15, "gpu", RESIDUAL)):
                if energy.get(name) is not None:
                    ax.barh(i + delta, energy[name], height=.25, color=color)
        ax.set_xlim(left=0)
    fig.legend(handles=[Patch(color=FOREGROUND, label="Package"), Patch(color=RESIDUAL, label="GPU")],
               loc="outside lower center", ncols=2, frameon=False, fontsize=12)
    save(fig, output / "gpu-energy.png")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("receipt", type=Path)
    parser.add_argument("--out", type=Path, default=ROOT / "docs/media")
    parser.add_argument("--synthetic", action="store_true")
    args = parser.parse_args()
    args.out = args.out.resolve()
    if args.synthetic and not args.out.is_relative_to(OUT):
        parser.error("synthetic charts must stay in the media cache")
    args.out.mkdir(parents=True, exist_ok=True)
    data = json.loads(args.receipt.read_text())["results"]
    cpu_charts(data, args.out, args.synthetic)
    gpu_charts(data, args.out, args.synthetic)
