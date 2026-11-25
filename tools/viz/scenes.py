"""Cairo-only Manim scenes. Coordinates are in a 16 × 9 frame."""
from collections import Counter
from datetime import date, timedelta
from itertools import combinations
import math
import os

import numpy as np
from manim import (
    AnimationGroup, Arrow, Create, Dot, FadeIn, FadeOut,
    GrowFromEdge, LEFT, Line, Rectangle, RoundedRectangle, Scene, Text,
    Transform, UP, ValueTracker, VGroup, VMobject, config, linear,
)

import data
import normalized

config.frame_width = 16
config.frame_height = 9
BG = "#10151F"
INK = "#F4F4EE"
MUTED = "#ACB9CB"
GRID = "#344256"
PANEL = "#1B2535"
MINT = "#82ECC3"
GOLD = "#F6C777"
SKY = "#77DBF7"
ROSE = "#FF939B"
LILAC = "#BCA3FF"
FONT = "DejaVu Sans"
MONO = "DejaVu Sans Mono"
APPS, ALL, COLOR, NAME = data.APPS, data.ALL_APPS, data.APP_COLORS, data.NAMES


def point(x, y):
    return np.array([x, y, 0])


def label(string, x, y, size=32, color=INK, *, align="left", mono=False, limit=14.5, bold=False):
    text = Text(
        string, font=MONO if mono else FONT, font_size=size,
        weight="BOLD" if bold else "NORMAL", color=color,
    )
    if align == "left":
        limit = min(limit, 7.25 - x)
    if align == "right":
        limit = min(limit, x + 7.25)
    if text.width > limit:
        text.scale_to_fit_width(limit)
    text.move_to(point(x, y))
    if align == "left":
        text.shift(point(text.width / 2, 0))
    if align == "right":
        text.shift(point(-text.width / 2, 0))
    return text


def line(x, y, a, b, color=GRID, width=2):
    return Line(point(x, y), point(a, b), color=color, stroke_width=width)


def box(x, y, w, h, color=PANEL, border=GRID):
    return RoundedRectangle(
        corner_radius=.13, width=w, height=h, stroke_color=border,
        stroke_width=2, fill_color=color, fill_opacity=1,
    ).move_to(point(x, y))


def bar(x, y, w, color, height=.32):
    return Rectangle(
        width=max(w, .001), height=height, stroke_width=0,
        fill_color=color, fill_opacity=1,
    ).move_to(point(x + w / 2, y))


def right_arrow(x, y, a, b, color=MUTED):
    return Arrow(point(x, y), point(a, b), buff=0, color=color, stroke_width=3, tip_length=.16)


def chip(name, x, y, w, color):
    return VGroup(box(x, y, w, .68, PANEL, color), label(name, x, y, 25, color, align="center", limit=w - .25))


class Page(Scene):
    def header(self, title, source, kicker="NATIVETUBE  /  PERFORMANCE"):
        self.camera.background_color = BG
        self.camera.frame_width = 16
        self.camera.frame_height = 9
        self.add(
            label(kicker, -7.25, 3.92, 21, MINT, mono=True),
            label(title, -7.25, 3.15, 56, bold=True),
            line(-7.25, 2.59, 7.25, 2.59),
            line(-7.25, -3.73, 7.25, -3.73),
            source_label := label(source, -7.25, -4.09, 20, MUTED, mono=True),
        )
        self.source_label = source_label

    def show(self, *items, hold=4, seconds=.7):
        self.play(*[FadeIn(item, shift=point(0, .12)) for item in items], run_time=seconds)
        self.wait(hold)

    def swap(self, old, new, hold=4):
        self.play(FadeOut(old), run_time=.32)
        self.show(new, hold=hold, seconds=.38)

    def update_to_time(self, t):
        super().update_to_time(t)
        if not os.environ.get("VIZ_AUDIT_TEXT") or self.renderer.skip_animations:
            return
        tick = self.renderer.time * 4
        if t > 0 and abs(tick - round(tick)) > 2.1 / config.frame_rate:
            return
        texts = list(self.visible_text(self.mobjects))
        for a, b in combinations(texts, 2):
            width = min(a.get_right()[0], b.get_right()[0]) - max(a.get_left()[0], b.get_left()[0])
            height = min(a.get_top()[1], b.get_top()[1]) - max(a.get_bottom()[1], b.get_bottom()[1])
            if width > .045 and height > .045:
                raise ValueError(f"{type(self).__name__} @ {tick / 4:.2f}s: "
                                 f"{self.text_value(a)!r} overlaps {self.text_value(b)!r}")

    @classmethod
    def visible_text(cls, mobjects, seen=None):
        if seen is None:
            seen = set()
        for mob in mobjects:
            if id(mob) in seen:
                continue
            seen.add(id(mob))
            if isinstance(mob, (Text, Readout)):
                shapes = mob.family_members_with_points()
                if any(shape.get_fill_opacity() > .045 for shape in shapes):
                    yield mob
            else:
                yield from cls.visible_text(mob.submobjects, seen)

    @staticmethod
    def text_value(mob):
        return mob.text if isinstance(mob, Text) else str(mob.get_value())


def flow(y, names, color):
    xs = (-5.5, -1.83, 1.83, 5.5)
    nodes = [chip(name, x, y, 2.9, color) for name, x in zip(names, xs, strict=True)]
    links = [right_arrow(x + 1.52, y, a - 1.52, y, color) for x, a in zip(xs, xs[1:])]
    return nodes, links, xs


def window_hole(y):
    strips = VGroup(
        box(-3.75, y, 5.2, 2.5),
        bar(-6.05, y + .86, 4.6, GRID, .18),
        bar(-3.1, y + .38, 1.46, GRID, .17),
        bar(-3.1, y + .02, 1.25, GRID, .17),
        bar(-3.1, y - .35, 1.36, GRID, .17),
        bar(-6.05, y - .91, 2.65, GRID, .17),
    )
    hole = box(-4.72, y - .07, 2.69, 1.2, "#16463E", MINT)
    surface = box(-4.36, y - .31, 2.69, 1.2, "#225950", MINT)
    video = VGroup(
        surface,
        label("mpv video", -4.36, y - .31, 25, MINT, align="center"),
    )
    descriptions = VGroup(
        label("Transparent region in the UI surface", -.1, y + .82, 32, SKY),
        label("mpv draws to a subsurface below the UI", -.1, y + .12, 31, MINT),
        label("Wayland composites the surfaces", -.1, y - .58, 28, MUTED),
    )
    return VGroup(strips, hole, video, descriptions), video


class Readout(VGroup):
    def __init__(self, x, y, value, size=42, color=INK, decimals=0):
        self.x, self.y = x, y
        self.size, self.ink, self.decimals = size, color, decimals
        self.previous = f"{value:.{decimals}f}"
        super().__init__(label(self.previous, x, y, size, color, mono=True))

    def get_value(self):
        return self.previous

    def set_value(self, value):
        rendered = f"{value:.{self.decimals}f}"
        if rendered != self.previous:
            self[0].become(label(rendered, self.x, self.y, self.size, self.ink, mono=True))
            self.previous = rendered
        return self


def packet(x, y, color):
    return VGroup(
        box(x, y, .28, .20, PANEL, color),
        line(x - .08, y + .045, x + .08, y + .045, color, 1.6),
        line(x - .08, y - .045, x + .045, y - .045, color, 1.6),
    )


def packet_train(tracker, start, end, n, y_at, color):
    packets = VGroup(*[packet(start, y_at(start), color) for _ in range(n)])

    def update(train):
        progress = tracker.get_value()
        for index, item in enumerate(train):
            along = (progress * (n + 6) - index) / 6
            x = start + float(np.clip(along, 0, 1)) * (end - start)
            item.move_to(point(x, y_at(x)))
            item.set_opacity(1 if 0 <= along < 1 else 0)

    packets.add_updater(update, call_updater=True)
    return packets


class FramePath(Page):
    def construct(self):
        self.header("Where a video frame goes", "Laptop • AC • userspace CPU counters • same clip • coda adds the Fly VM, normalized")
        xs = (-5.5, -1.83, 1.83, 5.5)
        y = .48
        route = VGroup(
            *[line(x, y, a, y, GOLD, 3) for x, a in zip(xs, xs[1:])],
            *[Dot(point(x, y), radius=.09, color=GOLD) for x in xs],
        )
        captions = VGroup(*(chip(name, x, 1.05, 2.9, GOLD) for x, name in zip(
            xs, ("Decode", "Copy to RAM", "Upload to UI", "Composite"), strict=True,
        )))
        title = label("Software path", -7.25, 1.97, 33, GOLD, bold=True)
        recipe = label("848 × 476 pixels × 4 bytes ≈ 1.6 MB per frame", -7.25, -.49, 33, bold=True)
        frames_per_second = 30
        frame_bytes = 848 * 476 * 4
        clock = ValueTracker(0)
        packets = packet_train(clock, xs[0], xs[-1], frames_per_second, lambda _: y, GOLD)
        number = Readout(-7.25, -1.46, 0, size=58, color=GOLD, decimals=1)
        unit = label("MB/s  •  one second of video frames", -4.65, -1.46, 31, GOLD)
        tag = label("30 fps • Each packet is one frame. Animation slowed for clarity.", -7.25, -2.71, 25, MUTED)
        self.show(title, route, captions, recipe, tag, number, unit, hold=.85)
        self.add(packets)

        def count_bytes(counter):
            delivered = max(0, min(frames_per_second, math.floor(clock.get_value() * 36 - 5)))
            counter.set_value(delivered * frame_bytes / 1e6)

        number.add_updater(count_bytes)
        self.play(clock.animate.set_value(1), run_time=5.75, rate_func=linear)
        number.clear_updaters()
        self.remove(packets)
        self.wait(2.6)
        self.play(FadeOut(VGroup(title, captions, recipe, tag, number, unit)), run_time=.4)

        gpu_y = -.24
        gpu_route = VGroup(
            *[line(x, gpu_y, a, gpu_y, MINT, 3) for x, a in zip(xs, xs[1:])],
            *[Dot(point(x, gpu_y), radius=.09, color=MINT) for x in xs],
        )
        self.play(Transform(route, gpu_route), run_time=.75)
        gpu_labels = VGroup(
            label("GPU subsurface path", -7.25, 1.97, 33, MINT, bold=True),
            chip("mpv decodes", xs[0], .31, 2.9, MINT),
            chip("Compositor presents", xs[-1], .31, 3, MINT),
            chip("UI paints around a transparent hole", 0, 1.04, 5.55, SKY),
            right_arrow(0, .68, 0, -.1, SKY),
            label("mpv presents video through a separate Wayland surface.", -7.25, -1.27, 30, MINT),
            label("The video packets bypass the UI’s copy and upload steps.", -7.25, -2.21, 28, MUTED),
        )
        self.show(gpu_labels, hold=.35)
        bypass = ValueTracker(0)
        flowing = packet_train(bypass, xs[0], xs[-1], 12, lambda _: gpu_y, MINT)
        self.add(flowing)
        self.play(bypass.animate.set_value(1), run_time=3.7, rate_func=linear)
        self.remove(flowing)
        self.wait(2.4)
        self.play(FadeOut(VGroup(route, gpu_labels)), run_time=.42)

        mode = label("Software path", -4.6, 1.24, 28, GOLD, bold=True)
        heading = label("User-space instructions per 10 seconds of playback", -7.25, 1.94, 34, bold=True)
        before = VGroup()
        targets = []
        counters = []
        change = ValueTracker(0)
        ctr = data.laptop_counters()
        for i, app in enumerate(APPS):
            y = .5 - i * .88
            self.add(label(NAME[app], -7.25, y, 30, COLOR[app], bold=True))
            soft = ctr[app]["playback-software"]["cpu"]["instructions"] / 1e9
            gpu = ctr[app]["playback-gpu"]["cpu"]["instructions"] / 1e9
            meter = bar(-4.6, y, soft * 1.37, GOLD)
            count = Readout(4.95, y, soft, 34, GOLD, 2)
            self.add(label("B", 6.67, y, 28, MUTED, mono=True))
            before.add(meter, count)
            targets.append(Transform(meter, bar(-4.6, y, gpu * 1.37, MINT)))
            counters.append((count, soft, gpu))
        self.show(heading, mode, before, hold=3.0)
        self.play(FadeOut(mode), run_time=.35)
        for count, soft, gpu in counters:
            count.add_updater(lambda number, soft=soft, gpu=gpu: number.set_value(soft + (gpu - soft) * change.get_value()))
        self.play(*targets, change.animate.set_value(1),
                  FadeIn(label("GPU subsurface path", -4.6, 1.24, 28, MINT, bold=True)), run_time=2.4)
        for count, _, _ in counters:
            count.clear_updaters()
            count.set_color(MINT)
        self.show(label("About 19× fewer instructions per rendered frame.", -7.25, -2.48, 38, MINT, bold=True), hold=5.5)
        rows = [(machine, title, normalized.across_apps(machine, workload, "cores_busy"),
                 normalized.across_apps(machine, workload, "machine_percent"), color)
                for machine, workload, title, color in (
                    ("laptop", "playback-gpu", "GPU subsurface path", MINT),
                    ("laptop", "playback-software", "Software path", GOLD),
                    ("fly", "playback-egl", "GPU/EGL path, emulated", ROSE),
                    ("fly", "playback-software", "Software path", GOLD))]
        if all(cores and share for _, _, cores, share, _ in rows):
            # header() adds five mobjects; trackers in the scene list are not drawable.
            self.play(FadeOut(VGroup(*(m for m in self.mobjects[5:] if isinstance(m, VMobject)))), run_time=.45)
            self.no_gpu_coda(rows)

    def no_gpu_coda(self, rows):
        coda = VGroup(label("What if there’s no GPU?", -7.25, 1.97, 35, bold=True))
        scale = 3.7 / max(cores[1] for _, _, cores, _, _ in rows)
        bars = []
        for i, (machine, title, cores, share, color) in enumerate(rows):
            if i in (0, 2):
                y_group = 1.3 if i == 0 else -.32
                coda.add(label("Laptop • real GPU" if machine == "laptop" else "Fly VM • llvmpipe, no GPU",
                               -7.25, y_group, 25, SKY if machine == "laptop" else GOLD, bold=True))
            y = (.82, .34, -.8, -1.28)[i]
            span = lambda lo, hi, fmt: fmt.format(lo) if fmt.format(lo) == fmt.format(hi) else f"{fmt.format(lo)}–{fmt.format(hi)}"
            coda.add(label(title, -6.9, y, 21, MUTED),
                     label(f"{span(*cores, '{:.3f}')} cores • {span(*share, '{:.2f}')}% of machine",
                           -3.0 + cores[1] * scale + .16, y, 20, color, mono=True))
            bars.append(bar(-3.0, y, cores[1] * scale, color, .26))
        coda.add(label("Without a GPU, the GPU path is emulated on the CPU and becomes the most expensive one.",
                       -7.25, -2.02, 24, MINT, bold=True),
                 label("Cores busy and share of all logical CPUs (16 laptop, 4 Fly) • range across iced, GPUI and Rune "
                       "• medians of three", -7.25, -2.62, 18, MUTED))
        footer = label("Laptop CPU: resource runs on battery, power profile • Fly VM: 4 vCPUs, llvmpipe "
                       "• medians of three", -7.25, -4.09, 20, MUTED, mono=True)
        self.play(FadeOut(self.source_label), run_time=.3)
        self.show(coda, footer, hold=.3)
        self.play(*(GrowFromEdge(item, LEFT) for item in bars), run_time=1.4)
        self.wait(8.5)


class FirstFrame(Page):
    def construct(self):
        fly = data.fly_input()
        source, rows = data.timing(fly)
        if "youtube" in rows:
            self.header("The race to the first frame",
                        "Laptop GPU and Fly VM software rendering • medians • log time axis")
            self.log_race(rows, data.timing({})[1], fly.get("web_warm"))
        else:
            self.header("The race to the first frame", source)
            self.linear_race(rows)
        self.stopwatch_stages()

    def linear_race(self, rows):
        widest = max(row.get(key) or 0 for row in rows.values() for key in ("home_ms", "open_ms"))
        step = 50 if widest <= 350 else 500 if widest <= 2500 else 10 ** math.floor(math.log10(widest))
        limit = max(step, math.ceil(widest * 1.08 / step) * step)
        left, width = -3.82, 8.78
        scale = width / limit
        track = VGroup()
        ys = [1.12 - i * .84 for i in range(len(ALL))]
        for app, y in zip(ALL, ys, strict=True):
            track.add(label(NAME[app], -7.25, y, 28, COLOR[app], bold=True), line(left, y, left + width, y))
        for ms in range(0, limit + 1, step):
            x = left + ms * scale
            track.add(line(x, -1.68, x, 1.39), label(f"{ms}", x, -1.91, 20, MUTED, align="center", mono=True))
        track.add(label("ms", left + width + .46, -1.91, 20, MUTED, mono=True))
        clock_unit = label("ms elapsed", -1.97, -2.65, 24, MUTED)
        self.show(track, hold=.5)

        def race(key, title):
            heading = label(title, -7.25, 1.99, 33, bold=True)
            remaining = [(app, y, rows.get(app, {}).get(key)) for app, y in zip(ALL, ys, strict=True)]
            maximum = max(ms or 0 for _, _, ms in remaining)
            clock = ValueTracker(0)
            number = Readout(-3.82, -2.65, 0, 39, MINT)
            number.add_updater(lambda counter: counter.set_value(clock.get_value()))
            sweep = line(left, -1.61, left, 1.39, MINT, 2)
            sweep.add_updater(lambda cursor: cursor.move_to(point(left + clock.get_value() * scale, -.11)))
            runners = VGroup(number, sweep, heading, clock_unit)
            for app, y, ms in remaining:
                if ms is None:
                    runners.add(label("Unavailable", left + .2, y + .22, 22, MUTED))
                    continue
                dot = Dot(point(left, y), radius=.10, color=COLOR[app])
                dot.add_updater(lambda obj, y=y, ms=ms: obj.move_to(point(left + min(ms, clock.get_value()) * scale, y)))
                finish = label(f"{ms:.0f} ms", left + ms * scale + .16, y + .23, 24, COLOR[app], mono=True)
                finish.add_updater(lambda obj, ms=ms: obj.set_opacity(1 if clock.get_value() >= ms - .001 else 0),
                                   call_updater=True)
                runners.add(dot, finish)
            self.add(*runners)
            self.play(clock.animate.set_value(maximum), run_time=6.5 * maximum / widest, rate_func=linear)
            for mob in runners:
                mob.clear_updaters()
            return runners

        home = race("home_ms", "Launch → Home shown")
        reset = label("A click after Home starts a new clock.", -7.25, -3.23, 26, MUTED)
        self.show(reset, hold=2.6)
        self.play(FadeOut(VGroup(*home, reset)), run_time=.35)
        opened = race("open_ms", "Click → first video frame")
        self.wait(3.2)
        self.play(FadeOut(VGroup(track, *opened)), run_time=.45)

    def log_race(self, fly_rows, laptop_rows, warm):
        # Each app gets a laptop row and a Fly row. YouTube web was measured on Fly only and
        # times a different event, so it sits in its own group below a divider.
        entries, ys, y = [], [], 1.22
        for app in APPS:
            for where, rows in (("laptop, GPU", laptop_rows), ("Fly VM, software rendering", fly_rows)):
                entries.append((app, where, rows[app]["open_ms"]))
                ys.append(y)
                y -= .4
            y -= .08
        divider_y = y + .12
        y -= .46
        entries.append(("youtube", "Fly VM, software rendering", fly_rows["youtube"]["open_ms"]))
        ys.append(y)
        low, high = 50, 3000
        left, width = -1.2, 7.0

        def x_at(ms):
            return left + width * math.log(max(ms, low) / low) / math.log(high / low)

        bottom, top = ys[-1] - .3, ys[0] + .27
        track = VGroup(
            label("Native apps: click → first video frame, fixture replay", -7.25, 1.68, 18, MINT, bold=True),
            line(-7.25, divider_y, 7.25, divider_y, MUTED, 1.5),
            label(f"YouTube web: page load → first content frame • live, signed out, {fly_rows['youtube']['size']}",
                  -7.25, divider_y - .24, 18, ROSE, bold=True),
        )
        for (app, where, _), row_y in zip(entries, ys, strict=True):
            if where.startswith("laptop") or app == "youtube":
                track.add(label(NAME[app], -7.25, row_y, 25, COLOR[app], bold=True))
            track.add(label(where, -4.55, row_y, 17, MUTED), line(left, row_y, left + width, row_y))
        for ms in (50, 100, 200, 500, 1000, 2000):
            x = x_at(ms)
            track.add(line(x, divider_y + .05, x, top), line(x, bottom, x, ys[-1] + .25),
                      label(f"{ms:,}", x, bottom - .2, 19, MUTED, align="center", mono=True))
        track.add(label("ms", left + width + .2, bottom - .2, 19, MUTED, mono=True))
        heading = label("Open → first frame", -7.25, 2.08, 33, bold=True)
        axis_note = label("Log time axis: each gridline step multiplies the time, so 95 ms and 1,857 ms both stay readable.",
                          -7.25, -2.74, 20, MINT)
        self.show(heading, track, axis_note, hold=1.6)

        longest = max(ms for _, _, ms in entries)
        progress = ValueTracker(0)

        def elapsed():
            return low * (longest / low) ** progress.get_value()

        number = Readout(-7.25, bottom - .2, low, 26, MINT)
        number.add_updater(lambda counter: counter.set_value(elapsed()))
        unit = label("ms elapsed", -5.75, bottom - .2, 19, MUTED)
        sweep = line(left, bottom + .05, left, top - .05, MINT, 2)
        sweep.add_updater(lambda cursor: cursor.move_to(point(x_at(elapsed()), (top + bottom) / 2)))
        runners = VGroup(number, unit, sweep)
        for (app, _, ms), row_y in zip(entries, ys, strict=True):
            dot = Dot(point(left, row_y), radius=.085, color=COLOR[app])
            dot.add_updater(lambda obj, row_y=row_y, ms=ms: obj.move_to(point(x_at(min(ms, elapsed())), row_y)))
            finish = label(f"{ms:,.0f} ms", x_at(ms) + .16, row_y + .19, 20, COLOR[app], mono=True)
            finish.add_updater(lambda obj, ms=ms: obj.set_opacity(1 if elapsed() >= ms - .001 else 0),
                               call_updater=True)
            runners.add(dot, finish)
        self.add(*runners)
        self.play(progress.animate.set_value(1), run_time=7, rate_func=linear)
        for mob in runners:
            mob.clear_updaters()
        notes = VGroup(*(label(text, -7.25, y, 18, MUTED)
                         for text, y in zip(normalized.warm_lines(), (-3.1, -3.42))))
        self.show(notes, hold=9.5)
        self.play(FadeOut(VGroup(heading, track, axis_note, notes, *runners)), run_time=.45)

    def stopwatch_stages(self):
        nodes, links, _ = flow(.56, ("Resolve media", "Attach surface", "Decode", "Paint"), SKY)
        self.show(label("What the stopwatch includes", -7.25, 1.98, 35, bold=True), hold=.1)
        for i, node in enumerate(nodes):
            earlier = [FadeIn(links[i - 1])] if i else []
            self.play(*earlier, FadeIn(node), run_time=.38)
        self.show(label("Prewarming can move attach work before the click.", -7.25, -.69, 32, MINT),
                  label("Stage durations and overlap weren’t isolated in these timings.", -7.25, -3.16, 22, MUTED),
                  hold=11.5)


class ScrollCost(Page):
    def construct(self):
        fly = data.fly_input()
        source, rows = data.wheel(fly)
        self.header("What does it cost to scroll?", source + (" • final act adds the Fly VM" if "wheel_cpu" in fly else ""))
        active = [app for app in ALL if app in rows]
        lane_height = .92
        ys = [1.03 - i * lane_height for i in range(len(active))]
        names = VGroup(*(label(NAME[app], -7.25, y, 29, COLOR[app], bold=True)
                         for app, y in zip(active, ys, strict=True)))
        phase = label("One draw at a time", -7.25, 1.99, 35, bold=True)
        estimated = "averaged over the wheel phase at about 60 fps" if "wheel" not in fly else "60 fps illustration"
        scale_note = label(f"Each dot ≈ 1M instructions • {estimated}", -7.25, -2.55, 24, MUTED)
        pulses = 8
        scroll = ValueTracker(0)
        lists, clouds = VGroup(), VGroup()
        for app, y in zip(active, ys, strict=True):
            # Rows move upward inside a small viewport. The changing thumbnails make
            # the steps readable even when the measured instruction ratio is small.
            cards = VGroup()
            for i in range(4):
                card = VGroup(bar(-4.85, 0, .31, COLOR[app], .09),
                              bar(-4.45, 0, .77 if i % 2 else 1.1, MUTED, .06))
                card.add_updater(
                    lambda row, i=i, y=y: row.move_to(point(-4.20, y + ((i * .16 + scroll.get_value() * .16) % .64) - .32)))
                cards.add(card)
            lists.add(VGroup(box(-4.2, y, 1.53, .82), cards))
            throughput = rows[app]["rate"]
            frames_per_second = 60
            dots_per_frame = max(1, round(throughput / frames_per_second / 1e6))
            cloud = VGroup()
            for pulse in range(pulses):
                for dot_id in range(dots_per_frame):
                    origin = point(-2.95 + pulse * 1.25 + (dot_id % 5) * .12, y + ((dot_id // 5) % 5 - 1.5) * .12)
                    dot = Dot(origin, radius=.04, color=COLOR[app])

                    def emit(dot, pulse=pulse, origin=origin):
                        age = scroll.get_value() - pulse
                        dot.move_to(origin + point(0, max(0, 1 - age) * .10))
                        dot.set_opacity(float(np.clip(age * 2, 0, 1)))

                    dot.add_updater(emit, call_updater=True)
                    cloud.add(dot)
            clouds.add(cloud)

        self.show(phase, names, lists, scale_note, hold=.45)
        self.add(clouds)
        self.play(scroll.animate.set_value(pulses), rate_func=linear, run_time=5.6)
        for group in (*lists, *clouds):
            for mob in group.get_family():
                mob.clear_updaters()
        self.wait(1.0)
        self.play(FadeOut(VGroup(phase, lists, scale_note)), run_time=.38)
        heading = label("User-space instructions per second of wheel scrolling", -7.25, 1.99, 34, bold=True)
        longest = max(row["rate"] for row in rows.values())
        targets, values = [], VGroup()
        for app, y, cloud in zip(active, ys, clouds, strict=True):
            rate = rows[app]["rate"]
            width = 8.2 * rate / longest
            targets.append(Transform(cloud, bar(-4.75, y, width, COLOR[app])))
            values.add(label(f"{rate / 1e6:,.0f} M", -4.53 + width, y, 28, mono=True))
        self.play(*targets, FadeIn(heading), run_time=1.8)
        self.show(values, hold=3.9)
        self.play(FadeOut(VGroup(heading, names, clouds, values)), run_time=.42)

        def pipeline(name, y, stages, color, note):
            xs = (-2.9, .71, 4.98)
            return VGroup(
                label(name, -7.25, y, 32, color, bold=True),
                *[chip(stage, x, y, w, color) for stage, x, w in zip(stages, xs, (2.7, 3.15, 4), strict=True)],
                right_arrow(-1.43, y, -.95, y, color),
                right_arrow(2.4, y, 2.88, y, color),
                label(note, -4.25, y - .67, 26, MUTED),
            )

        caption = label("An offset change takes a different route", -7.25, 1.98, 34, bold=True)
        gpui = pipeline("GPUI", .97, ("Scroll tick", "Notify PageView", "Render page tree"), COLOR["gpui"],
                        "GPUI rebuilds the page view as the offset changes.")
        rune = pipeline("Rune", -.55, ("Scroll tick", "Write Live offset", "Translate content"), COLOR["rune"],
                        "Between visible-range changes, the live transform skips the view.")
        caveat = label("Counters also include work such as thumbnail decoding.", -7.25, -2.44, 31, MINT)
        self.show(caption, gpui, hold=2.5)
        self.show(rune, hold=3.9)
        self.show(caveat, hold=3)
        pane = VGroup(
            label("A separate laptop gate measured CPU time", -7.25, 1.98, 36, bold=True),
            *[VGroup(label(NAME[app], -7.25 + i * 4.8, .7, 31, COLOR[app], bold=True),
                     self.cpu_value(normalized.cpu_text("laptop_gate_wheel", app), -7.25 + i * 4.8, -.03,
                                    COLOR[app], 4.5))
              for i, app in enumerate(APPS)],
            label(f"Share of all {normalized.value('laptop_logical_cpus') or '—'} logical CPUs • cores busy "
                  "• distinct perf-gate scenario", -7.25, -1.06, 26, MUTED),
            label("iced and Rune swap places across these two measurements." if "wheel" not in data.fly_input()
                  else "Laptop instruction counts rank Rune below iced.", -7.25, -2.3, 34, MINT, bold=True),
        )
        self.swap(VGroup(caption, gpui, rune, caveat), pane, hold=4.8 if "wheel_cpu" in fly else 6.8)
        if "wheel_cpu" in fly:
            self.play(FadeOut(pane), run_time=.35)
            self.no_gpu_act(rows, fly["wheel_cpu"])

    @staticmethod
    def cpu_value(text, x, y, color, limit, size=30):
        if text == normalized.PENDING:
            return label(text, x, y, min(size, 24), MUTED, limit=limit)
        return label(text, x, y, size, color, mono=True, limit=limit)

    def no_gpu_act(self, laptop_rows, fly_rows):
        heading = label("No GPU: the CPU draws every pixel", -7.25, 2.0, 35, bold=True)
        left_x, right_x, span = -4.75, 1.55, 2.1
        columns = VGroup(
            label("Laptop, GPU • instructions per wheel second", -7.25, 1.36, 19, SKY),
            label("Fly VM, software rendering • wheel CPU", right_x, 1.36, 19, GOLD),
        )
        names, strips, values = VGroup(), VGroup(), VGroup()
        widest_rate = max(laptop_rows[app]["rate"] for app in APPS)
        # Bar lengths compare apps within one machine; dividing by the machine's core count keeps the ratio.
        widest_cpu = max(fly_rows[app]["cpu_percent"] for app in APPS)
        for i, app in enumerate(APPS):
            y = .7 - i * .78
            names.add(label(NAME[app], -7.25, y, 29, COLOR[app], bold=True))
            rate_width = span * laptop_rows[app]["rate"] / widest_rate
            cpu_width = 1.9 * fly_rows[app]["cpu_percent"] / widest_cpu
            strips.add(bar(left_x, y, rate_width, SKY, .24), bar(right_x, y, cpu_width, GOLD, .24))
            per_frame = normalized.wheel_rate(app, "instructions_per_drawn_frame")
            rate_text = f"{laptop_rows[app]['rate'] / 1e6:,.0f} M/s" + (
                f" • {per_frame / 1e6:.1f} M/frame" if per_frame is not None else "")
            values.add(label(rate_text, left_x + rate_width + .14, y, 20, SKY, mono=True,
                             limit=right_x - .25 - (left_x + rate_width + .14)),
                       self.cpu_value(normalized.cpu_text("fly_wheel", app), right_x + cpu_width + .14, y, GOLD,
                                      7.2 - right_x - cpu_width - .14, 22))
        self.show(heading, columns, names, hold=.2)
        self.play(*(GrowFromEdge(item, LEFT) for item in strips), run_time=1.12)
        self.show(values, hold=1.2)
        laptop_order = " > ".join(NAME[app] for app in sorted(APPS, key=lambda a: -laptop_rows[a]["rate"]))
        fly_order = " > ".join(NAME[app] for app in sorted(APPS, key=lambda a: -fly_rows[a]["cpu_percent"]))
        self.show(label(f"With a GPU: {laptop_order}    →    Without one: {fly_order}", -7.25, -1.55, 25, MINT,
                        bold=True), hold=1.4)
        least = min(APPS, key=lambda a: laptop_rows[a]["rate"])
        most = max(APPS, key=lambda a: fly_rows[a]["cpu_percent"])
        flip = (f"The order flips: {NAME[least]} does the least work with a GPU and costs the most without one."
                if least == most else "The order changes once the CPU draws the pixels.")
        self.show(label(flip, -7.25, -2.15, 23),
                  label("When the CPU draws every pixel, how much each framework redraws per frame dominates.",
                        -7.25, -2.7, 23),
                  label(f"Fly CPU: share of all {normalized.value('fly_logical_cpus') or '—'} vCPUs • cores busy. "
                        "Bars compare apps within one machine; the machines use different measures.",
                        -7.25, -3.28, 19, MUTED),
                  hold=7.5)


class Memory(Page):
    def construct(self):
        fly = data.fly_input()
        source, rows = data.memory({})
        compare = "memory_compare" in fly
        self.header("Memory, honestly", source if not compare else
                    "Laptop on battery, power profile • then the Fly VM • process-tree PSS • medians of three")
        columns = APPS if compare else ALL
        stages = (("cold", "Cold Home"), ("warm", "Warm Home"), ("playing", "Playback"), ("peak", "Tour peak"))
        # Render path of each laptop stage, as run by tools/media/resources.py.
        paths = {"cold": "LAPTOP • HOME, PREWARM OFF", "warm": "LAPTOP • HOME, DEFAULT GPU STARTUP",
                 "playing": "LAPTOP • SOFTWARE PLAYBACK", "peak": "LAPTOP • TOUR, DEFAULT GPU PATH"}
        maximum = max((row.get(key) or 0) for row in rows.values() for key, _ in stages)
        scale = 2.68 / maximum
        xs = [-4.2, .3, 4.8] if compare else [-4.95, -1.57, 1.81, 5.19]
        floor = -1.78
        blocks = {}
        chart = VGroup()
        for app, x in zip(columns, xs, strict=True):
            chart.add(label(NAME[app], x, 1.74, 27, COLOR[app], align="center", bold=True),
                      line(x - 1.5, floor, x + 1.5, floor))
            blocks[app] = VGroup(
                self.block(x, floor, 0, MINT), self.block(x, floor, 0, COLOR[app]),
            )
        chart.add(*blocks.values())
        self.show(chart, hold=.55)
        captions = VGroup()
        notes = (
            "Player prewarm disabled. PSS apportions shared memory.",
            "At Home, after the native apps load the player.",
            "Software playback in the native apps.",
            "Highest sampled process-tree memory during the tour.",
        )
        for j, (key, title) in enumerate(stages):
            if j:
                self.play(FadeOut(captions), run_time=.35)
            transitions = []
            values = VGroup()
            for app, x in zip(columns, xs, strict=True):
                val = rows.get(app, {}).get(key)
                old_core, old_app = blocks[app]
                height = (val or 0) * scale
                transitions.extend((Transform(old_app, self.block(x, floor, height, COLOR[app])),
                                    Transform(old_core, self.block(x, floor, 0, MINT))))
                values.add(label(f"{val:.0f} MiB" if val is not None else "Unavailable",
                                 x, floor + height + .28, 25, COLOR[app] if val is not None else MUTED,
                                 align="center", mono=val is not None))
            title = VGroup(label(title, -7.25, 2.16, 28, MINT, bold=True),
                           label(paths[key], 7.25, 2.16, 20, SKY, align="right", mono=True, bold=True))
            self.play(*transitions, FadeIn(title), run_time=1.35)

            note = label(notes[j], -7.25, -2.52, 30, MUTED)
            self.show(values, note, hold=2.4 if key == "playing" else 4.6)
            captions = VGroup(title, values, note)
            if key != "playing":
                continue

            split, split_values = [], VGroup()
            for app, x in zip(columns, xs, strict=True):
                row = rows.get(app, {})
                core = row.get("core")
                if core is None or row.get(key) is None:
                    continue
                remainder = row[key] - core
                base = core * scale
                rem = remainder * scale
                core_block, app_block = blocks[app]
                split.extend((Transform(core_block, self.block(x, floor, base, MINT)),
                              Transform(app_block, self.block(x, floor + base, rem, COLOR[app]))))
                if min(base, rem) >= .28:
                    split_values.add(
                        label(f"{core:.0f}", x, floor + base / 2, 22, BG, align="center", mono=True, bold=True),
                        label(f"{remainder:.0f}", x, floor + base + rem / 2, 22, BG, align="center",
                              mono=True, bold=True),
                    )
            if split:
                self.play(*split, run_time=1.65)
                explanation = label("Mint is the standalone player estimate. Color is the app and integration.",
                                    -7.25, -3.11, 25, MINT)
                self.show(split_values, explanation, hold=4.7)
                captions.add(split_values, explanation)
            else:
                self.wait(5.3)
        self.wait(2)
        if "memory_compare" in fly:
            self.play(FadeOut(VGroup(chart, *captions)), run_time=.38)
            self.fly_memory_act(fly["memory_compare"])
        else:
            self.wait(1)

    def fly_memory_act(self, rows):
        self.add(label("On the Fly VM, next to YouTube", -7.25, 2.04, 30, bold=True),
                 label("FLY VM • SOFTWARE RENDERING", 7.25, 2.04, 20, GOLD, align="right", mono=True, bold=True))
        order = (*APPS, "ntctl", "youtube")
        xs = (-5.91, -2.99, -.07, 2.85, 5.77)
        floor = -1.58
        scale = 2.58 / max(row["mib"] for row in rows.values())
        bars, values = VGroup(), VGroup()
        for app, x in zip(order, xs, strict=True):
            mb = rows[app]["mib"]
            color = MINT if app == "ntctl" else COLOR[app]
            name = "ntctl play" if app == "ntctl" else NAME[app]
            self.add(label(name, x, 1.58, 25, color, align="center", bold=True))
            bars.add(self.block(x, floor, mb * scale, color))
            values.add(label(f"{mb:.0f} MiB", x, floor + mb * scale + .21,
                             24, color, align="center", mono=True))
            detail = "Cold settled" if app == "youtube" else "Player peak" if app == "ntctl" else "App peak"
            self.add(label(detail, x, -1.96, 21, MUTED, align="center"))
        self.play(*(GrowFromEdge(item, -UP) for item in bars), run_time=1.25)
        self.show(values,
                  label(f"YouTube web: signed-out Chromium tree, settled during cold playback; "
                        f"its peak was {rows['youtube']['peak_mib']:,.0f} MiB.", -7.25, -2.71, 24, MUTED),
                  label("App peaks come from the default playback scenario. ntctl play is the core player alone.",
                        -7.25, -3.27, 22, MUTED),
                  hold=7.5)

    @staticmethod
    def block(x, floor, height, color):
        return Rectangle(width=1.8, height=max(height, .001), stroke_width=0, fill_color=color,
                         fill_opacity=float(height > 0)).move_to(point(x, floor + height / 2))


KIND = {
    "wrong behaviour": SKY, "review finding": LILAC, "perf regression": GOLD,
    "crash": ROSE, "freeze/stall": "#F59567",
}


class Hiccups(Page):
    def construct(self):
        self.header("A week of hiccups", "Project incident catalog • dated discovery and fix evidence")
        catalog = data.read(data.HICCUPS)["incidents"]
        areas = ("iced", "GPUI", "Rune", "core", "Other")
        days = sorted({row[key][:10] for row in catalog for key in ("date", "fixed_when") if row[key]})
        start, end = date.fromisoformat(days[0]), date.fromisoformat(days[-1])
        span = (end - start).days
        positions = {(start + timedelta(days=i)).isoformat(): -4.92 + 11.03 * i / span
                     for i in range(span + 1)}
        heading = label("Each thread runs from a recorded discovery to its documented fix.", -7.25, 1.99, 30)
        self.add(heading)
        frame = VGroup()
        for day, x in positions.items():
            frame.add(line(x, 1.34, x, -2.24), label(f"Nov {day[-2:]}", x, 1.63, 22, MUTED, mono=True, align="center"))
        bins = {area: [] for area in areas}
        for row in catalog:
            bins[row["area"] if row["area"] in areas else "Other"].append(row)
        finds = {day: VGroup() for day in days}
        fixes = {day: VGroup() for day in days}
        for i, (area, records) in enumerate(bins.items()):
            y = .91 - .70 * i
            frame.add(label(area, -7.25, y, 30, bold=True), line(-5.35, y - .36, 6.6, y - .36))
            for j, row in enumerate(sorted(records, key=lambda r: (r["date"], r["id"]))):
                found_day = row["date"][:10]
                fixed_day = (row["fixed_when"] or "")[:10]
                a = positions[found_day]
                yp = y + .26 - .52 * j / max(1, len(records) - 1)
                color = KIND.get(row["kind"], MUTED)
                finds[found_day].add(Dot(point(a, yp), radius=.033, color=color))
                if fixed_day:
                    b = positions[fixed_day] if fixed_day != found_day else a + .13
                    r = .039
                    fixes[fixed_day].add(VGroup(
                        line(a, yp, b, yp, color, 2),
                        line(b - r, yp - r, b + r, yp + r, color, 1.5),
                        line(b - r, yp + r, b + r, yp - r, color, 1.5),
                    ))
        legend = VGroup(*[
            label(text, x, -2.78, 21, color)
            for (text, color), x in zip(KIND.items(), (-7.25, -4.5, -1.82, 1.05, 2.85), strict=True)
        ], label("tooling / process", 5.16, -2.78, 21, MUTED))
        key = label("● discovery   × fix", -7.25, -3.33, 23, MUTED)
        cursor = line(positions[days[0]], 1.4, positions[days[0]], -2.22, MINT, 2)
        self.show(frame, legend, key, hold=.65)
        self.add(cursor)
        for day in days:
            if day != days[0]:
                self.play(cursor.animate.set_x(positions[day]), run_time=.34)
            if finds[day]:
                self.play(FadeIn(finds[day], shift=.58 * UP), run_time=.45)
            if fixes[day]:
                self.play(AnimationGroup(*(Create(mark) for mark in fixes[day]), lag_ratio=.025), run_time=.57)
            self.wait(.23)
        self.wait(4.0)
        self.play(FadeOut(VGroup(heading, frame, legend, key, cursor, *finds.values(), *fixes.values())), run_time=.45)
        counts = Counter(row["area"] if row["area"] in areas else "Other" for row in catalog)
        open_count = sum(not row["fixed_when"] for row in catalog)
        fixed = len(catalog) - open_count
        table = []
        for i, area in enumerate(areas):
            name = area if area != "Other" else "Tooling / process"
            y = 1.22 - .64 * i
            table.append(VGroup(label(name, -7.25, y, 28, bold=True), label(str(counts[area]), -3.35, y, 29, MINT, mono=True)))
        overview = VGroup(
            label("Recorded by area", -7.25, 2.0, 33, bold=True),
            *table, line(-2.2, 1.38, -2.2, -2.03),
            label(f"{len(catalog)} found", -.93, 1.21, 46, bold=True),
            label(f"{fixed} with fix evidence", -.93, .35, 40, MINT, bold=True),
            label(f"{open_count} still open in the catalog", -.93, -.45, 33, MUTED),
            label("Core incidents can affect every frontend.", -.93, -1.42, 29, MUTED),
            label("The catalog reflects what the project recorded.", -7.25, -2.82, 29, MUTED),
        )
        self.show(overview, hold=6.5)
        assessed = data.read(data.STABILITY)["results"]["NOW"]
        summary = sum(assessed[app]["clean"] for app in APPS)
        total = sum(assessed[app]["assessed"] for app in APPS)
        panel = VGroup(
            label("A separate stability check", -7.25, 1.91, 38, bold=True),
            label(f"{summary} of {total} NOW runs clean", -7.25, .79, 54, MINT, bold=True),
            label("Each run opened related videos 20 times.", -7.25, -.33, 36),
            label("The test samples frames and can miss brief glitches.", -7.25, -1.24, 31, MUTED),
        )
        self.swap(overview, panel, hold=8.3)

# The runner passes stills.py to Manim separately to keep static and moving output independent.
