"""Phone-readable stills using the same receipts and visual vocabulary."""
import data
from scenes import (
    APPS, BG, COLOR, GOLD, GRID, MINT, MUTED, NAME, SKY, Page, bar,
    chip, flow, label, line, right_arrow, window_hole,
)


def columns(page, headers, rows, *, x=(-7.25, -2.62, 1.13, 4.7), y=1.7):
    page.add(*[label(title, at, y, 23, MUTED, bold=True) for title, at in zip(headers, x, strict=True)])
    for i, (app, values) in enumerate(rows.items()):
        level = y - .94 - i * .9
        page.add(line(-7.25, level + .45, 7.25, level + .45))
        page.add(label(NAME[app], x[0], level, 35, COLOR[app], bold=True))
        for at, val in zip(x[1:], values, strict=True):
            page.add(label(val, at, level, 33, mono=True, limit=3.4))


class FramePathStill(Page):
    def construct(self):
        self.header("Two paths for the same frame", "Laptop fixture • software target at the Watch player size", "NATIVETUBE  /  FRAME PATH")
        nodes, links, _ = flow(.93, ("Decode", "Copy to RAM", "Upload to UI", "Composite"), GOLD)
        self.add(label("SOFTWARE", -7.25, 1.79, 25, GOLD, mono=True), *nodes, *links)
        self.add(label("848 × 476 RGBA ≈ 1.6 MB/frame  •  ≈48 MB/s at 30 fps", -7.25, .13, 30))
        self.add(line(-7.25, -.45, 7.25, -.45))
        diagram, _ = window_hole(-2.18)
        self.add(label("WAYLAND SUBSURFACE", -7.25, -.76, 24, MINT, mono=True), diagram)


class EGLStill(Page):
    def construct(self):
        self.header("Picking the EGL vendor", "nt-core • Wayland DRM device probe • EGL timing awaits remeasurement", "NATIVETUBE  /  RENDERER INIT")
        self.add(label("A hybrid machine can expose more than one EGL vendor.", -7.25, 1.91, 34, bold=True))
        xs = (-4.77, 0, 4.77)
        names = ("Wayland main device", "DRM driver", "EGL vendor manifest")
        for x, name in zip(xs, names, strict=True):
            self.add(chip(name, x, .61, 3.85, SKY))
        for a, b in zip(xs, xs[1:]):
            self.add(right_arrow(a + 2, .61, b - 2, .61, SKY))
        self.add(
            label("Before EGL loads, nt-core restricts GLVND to the matching vendor.", -7.25, -.62, 31, MINT),
            label("An explicit override or an ambiguous probe keeps the loader’s default.", -7.25, -1.46, 30, MUTED),
            label("This targets unnecessary vendor probing at startup.", -7.25, -2.77, 34, bold=True),
        )


class InstructionsStill(Page):
    def construct(self):
        self.header("How many instructions retire?", "Laptop • AC • userspace perf counters • median of three", "NATIVETUBE  /  CPU")
        counters = data.laptop_counters()
        rows = {}
        for app, result in ((app, counters[app]) for app in APPS):
            rows[app] = (
                f'{result["playback-software"]["cpu"]["instructions"] / 1e9:.2f} B',
                f'{result["playback-gpu"]["cpu"]["instructions"] / 1e6:.0f} M',
                f'{result["wheel"]["normalization"]["instructions_per_scroll_second"] / 1e6:,.0f} M/s',
            )
        columns(self, ("App", "Software playback", "GPU subsurface", "Wheel scroll"), rows)
        self.add(
            label("Both playback columns cover ten seconds of the same clip.", -7.25, -2.64, 28, MUTED),
            label("Wheel values are normalized per second of the wheel phase.", -7.25, -3.23, 28, MUTED),
        )


PMUS = ("cpu_core", "cpu_atom", "cpu_lowpower")


def pmu_split(app):
    """Per-PMU instruction counts of the median run in forced software playback; they sum to its total."""
    cpu = data.laptop_counters()[app]["playback-software"]["cpu"]
    counts = [cpu["per_pmu"]["instructions"][pmu]["count"] for pmu in PMUS]
    if sum(counts) != cpu["instructions"]:
        raise ValueError(f"{app}: per-PMU instructions {sum(counts)} do not sum to {cpu['instructions']}")
    return counts, cpu["instructions"]


def shown_millions(counts, total):
    """Parts in 0.01 M units, rounded by largest remainder so the shown parts add up to the shown total."""
    unit = 10_000
    goal = round(total / unit)
    floors = [count // unit for count in counts]
    order = sorted(range(len(counts)), key=lambda i: counts[i] % unit, reverse=True)
    for i in order[:goal - sum(floors)]:
        floors[i] += 1
    return [f"{part / 100:,.2f}" for part in floors], f"{goal / 100:,.2f}"


class PerCoreStill(Page):
    def construct(self):
        self.header("Which cores ran the instructions?",
                    "Laptop • AC, balance_performance • forced software playback, 10 s • split of each median run",
                    "NATIVETUBE  /  CPU")
        self.add(label("Instruction-count shares per PMU (not scheduled-time shares)", -7.25, 2.02, 29, MINT, bold=True))
        colors = (SKY, GOLD, "#BCA3FF")
        for i, (name, color) in enumerate(zip(("P-core  cpu_core", "E-core  cpu_atom", "LP E-core  cpu_lowpower"),
                                              colors, strict=True)):
            self.add(label(name, -4.8 + i * 4.0, 1.42, 22, color, bold=True))
        for i, app in enumerate((*APPS, "core")):
            y = .72 - i * 1.0
            name, color = ("ntctl play", MINT) if app == "core" else (NAME[app], COLOR[app])
            self.add(label(name, -7.25, y, 30, color, bold=True))
            counts, total = pmu_split(app)
            start = -4.8
            for count, fill in zip(counts, colors, strict=True):
                width = 11.6 * count / total
                self.add(bar(start, y, width - .035, fill, .42),
                         label(f"{count / total * 100:.1f}%", start + width / 2, y, 21, BG, align="center",
                               bold=True, mono=True, limit=width - .09))
                start += width
            parts, shown_total = shown_millions(counts, total)
            self.add(label(f"{' + '.join(parts)} = {shown_total} M instructions", -4.8, y - .39, 18, MUTED, mono=True))
        self.add(label("Each row’s three parts come from one run and sum to that run’s total.", -7.25, -3.3, 24, MUTED))


class GPUStill(Page):
    def construct(self):
        self.header("What the GPU engines were doing", "Laptop • AC • system-wide i915 estimates • medians per counter", "NATIVETUBE  /  GPU")
        self.add(label("GPU subsurface playback", -7.25, 1.97, 31, MINT, bold=True))
        rows = {}
        for app in APPS:
            engines = data.laptop_counters()[app]["playback-gpu"]["gpu"]["engines"]
            rows[app] = tuple(f'{engines[key]["busy_cycles_estimate"] / 1e6:.2f} M' for key in ("rcs0", "vcs0"))
        columns(self, ("App", "Render busy cycles", "Video busy cycles"), rows, x=(-7.25, -2.03, 3.42), y=1.37)
        self.add(
            label("Busy cycles are estimated from engine busy time and frequency.", -7.25, -2.32, 29, MUTED),
            label("The samples also include work from other processes.", -7.25, -2.96, 29, MUTED),
        )


class SizesStill(Page):
    def construct(self):
        self.header("The footprint on disk", "NOW release binaries • strip copy • app Cargo.lock files", "NATIVETUBE  /  BUILD")
        rows = {app: (f"{size:.1f} MiB", str(packages)) for app, (size, packages) in data.sizes().items()}
        columns(self, ("App", "Stripped release binary", "Lockfile packages"), rows, x=(-7.25, -2.76, 3.04))
        self.add(label("Package counts include every entry in each app’s Cargo.lock.", -7.25, -2.67, 28, MUTED))


UPSTREAM = (
    ("iced", "High-performance GPU default", "iced-power-preference.md"),
    ("iced", "Zero-width sibling can reset focus", "iced-focus-sibling-width.md"),
    ("iced", "Basic widget ops omit unfocus", "iced-unfocus-helper.md"),
    ("gpui", "Wayland axis_stop dropped", "gpui-axis-stop.md"),
    ("gpui", "No public cursor-hide operation", "gpui-hide-cursor.md"),
    ("gpui", "Re-entrant key dispatch panics", "gpui-reentrant-keystroke.md"),
    ("rune", "Wrapped LiveText can abort in release", "rune-wrapped-livetext.md"),
)


class UpstreamStill(Page):
    def construct(self):
        self.header("Framework bugs we found (written up, not yet filed)", "Seven local reports in docs/upstream • defects and missing APIs", "NATIVETUBE  /  TOOLKITS")
        for i, (app, title, _) in enumerate(UPSTREAM):
            y = 1.93 - i * .77
            self.add(label(NAME[app], -7.25, y, 29, COLOR[app], bold=True),
                     label(title, -4.55, y, 33))
            if i != len(UPSTREAM) - 1:
                self.add(line(-7.25, y - .38, 7.25, y - .38))


class CostStill(Page):
    def construct(self):
        self.header("What the first agent builds cost", "Project coordinator notes • first full frontend build per framework", "NATIVETUBE  /  AGENT COST")
        columns(self, ("App", "Agent spend", "Elapsed time"), data.cost(), x=(-7.25, -2.33, 2.63))
        self.add(label("Later reviews, fixes and parity work are outside this comparison.", -7.25, -2.61, 30, MUTED))
