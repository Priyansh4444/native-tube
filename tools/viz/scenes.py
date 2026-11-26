"""Cairo-only Manim scenes in the Obsidian OLED look. Coordinates are in a 16 × 9 frame (120 px per unit).

Each explainer is a run of one-idea beats on a pure black canvas. Glass (#121212, sheen, 8 % rim) is
reserved for small elements. While rendering with VIZ_AUDIT_TEXT=1, every 0.25 s sample runs the
QA gates in Page.gate(); a failure aborts the render, and a passing summary lands in .cache/viz/gates/.
"""
from collections import Counter
from datetime import date, timedelta
from itertools import combinations
import json
import math
import os
import random
import re

import numpy as np
from manim import (
    AnimationGroup, Arrow, Create, Dot, FadeIn, FadeOut, GrowFromEdge, LEFT, Line, MoveAlongPath,
    MovingCameraScene, ORIGIN, Rectangle, ReplacementTransform, RoundedRectangle, Text, Transform, UP,
    ValueTracker, VGroup, VMobject, config, linear, rate_functions,
)
from manimpango import register_font

import callouts
import data
import normalized

for font in ("Roboto-Regular.ttf", "Roboto-Medium.ttf", "Roboto-Bold.ttf"):
    register_font(str(data.ROOT / "assets/fonts" / font))

config.frame_width = 16
config.frame_height = 9
BG = "#000000"
INK = "#FFFFFF"
MUTED = "#999999"      # white at 60 % on black: the only secondary text colour
GRID = "#292929"       # 8 % white rim over the #121212 surface
TRACK = "#0F0F0F"      # 6 % white track
PANEL = "#121212"
SURFACE_HI = "#1A1A1A"
RING = "#3D3D3D"       # 16 % white highlight ring
MINT = "#4FE3B0"
GOLD = "#FFB547"
SKY = "#4FD1FF"
ROSE = "#FF4B4B"
LILAC = "#B98CFF"
FONT = "Roboto"
FRAMEWORK_FONT = "Google Sans"
MONO = "Roboto"
APPS, ALL, COLOR, NAME = data.APPS, data.ALL_APPS, data.APP_COLORS, data.NAMES
SOFT, GPU = GOLD, MINT  # software-path and GPU-path series

# Hero numbers, badges and caveats come from the receipts (callouts.py), one machine per card.
ENDINGS = callouts.endings(data.fly_input())
QUESTIONS = {
    "FramePath": "Where does a video frame go?",
    "FirstFrame": "How fast does the first frame show up?",
    "ScrollCost": "What does it cost to scroll?",
    "Memory": "How much memory does each app hold?",
    "Hiccups": "What broke this week, and what got fixed?",
}

# Type scale. Manim font_size N renders Roboto at about 1.66 N px per em at 1080p.
EM_PX_PER_SIZE = 1.66
FOOT, LABEL, VALUE, TITLE, HERO = 14.5, 22, 30, 40, 96
SAFE_X, SAFE_Y = 7.2, 3.7     # 96 px safe margin
GLASS_LIMIT = .35 * 16 * 9    # no glass container above 35 % of the frame
WORD_LIMIT = 14
NOT_PROSE = {"iced", "GPUI", "Rune", "YouTube", "web", "ntctl", "NativeTube", "vs", "vs.", "ms", "s", "MiB",
             "M/s", "B", "MB", "MB/s", "cores", "×", "px", "M", "•", "→", "+", "–", "−", "/", "·", "Nov", "●", "○"}


def point(x, y):
    return np.array([x, y, 0])


def label(string, x, y, size=LABEL, color=INK, *, align="left", mono=False, limit=14.4, bold=False, role="label", font=FONT):
    weight = "BOLD" if bold else "NORMAL"
    text = Text(string, font=MONO if mono else font, font_size=size, weight=weight, color=color)
    natural = text.width
    if align == "left":
        limit = min(limit, SAFE_X - x)
    elif align == "right":
        limit = min(limit, x + SAFE_X)
    else:
        limit = min(limit, 2 * (SAFE_X - abs(x)))
    if text.width > limit:
        text.scale_to_fit_width(limit)
    text.base_em = size * EM_PX_PER_SIZE * text.width / max(natural, 1e-6)
    text.base_height = max(text.height, 1e-6)
    text.role, text.words = role, string
    text.move_to(point(x, y))
    if align == "left":
        text.shift(point(text.width / 2, 0))
    if align == "right":
        text.shift(point(-text.width / 2, 0))
    return text


def line(x, y, a, b, color=GRID, width=2):
    return Line(point(x, y), point(a, b), color=color, stroke_width=width)


def box(x, y, w, h, color=PANEL, border=GRID, radius=.1):
    """Obsidian glass: #121212 surface, 175° sheen, 1 px white rim at 8 %."""
    shell = RoundedRectangle(
        corner_radius=min(radius, h * .32), width=w, height=h, stroke_color=border, stroke_width=1.2,
        fill_color=color, fill_opacity=1,
    ).set_sheen(.055, point(-.08, -1)).move_to(point(x, y))
    shell.glass = True
    return shell


def badge(text, x, y, color=INK, size=VALUE):
    """A glass difference badge with the neutral 16 % highlight ring."""
    words = label(text, x, y, size, color, align="center", bold=True, role="value")
    shell = box(x, y, words.width + .5, words.height + .36, SURFACE_HI, RING)
    shell.set_stroke(width=2)
    return VGroup(shell, words)


def chip(name, x, y, w, color=GRID, size=LABEL, h=.78):
    shell = box(x, y, w, h, PANEL, color)
    shell.set_stroke(width=2)
    return VGroup(shell, label(name, x, y, size, INK, align="center", limit=w - .3, bold=True))


def pulse(mob, factor=1.07):
    """One soft pulse that returns the value to its resting size."""
    return mob.animate(rate_func=rate_functions.there_and_back).scale(factor)


def bracket(points, color=INK, width=3):
    path = VMobject(stroke_color=color, stroke_width=width)
    path.set_points_as_corners([point(x, y) for x, y in points])
    return path


def bar(x, y, w, color, height=.5, *, vertical=False):
    """Thick bar with rounded ends and a soft glow; (x, y) is the start of the bar's axis."""
    w = max(w, .001)
    width, tall = (height, w) if vertical else (w, height)
    shape = RoundedRectangle(corner_radius=max(.0005, min(height / 2, w / 2, .25)), width=width, height=tall,
                             stroke_width=10 if color not in (GRID, TRACK) else 0, stroke_color=color,
                             stroke_opacity=.14, fill_color=color, fill_opacity=1)
    return shape.move_to(point(x, y + w / 2) if vertical else point(x + w / 2, y))


def ghost(x, y, w, height=.5, *, vertical=False):
    shape = bar(x, y, w, TRACK, height, vertical=vertical)
    shape.set_fill(INK, .06).set_stroke(RING, 1.5, 1)
    return shape


def right_arrow(x, y, a, b, color=MUTED):
    return Arrow(point(x, y), point(a, b), buff=0, color=color, stroke_width=4, tip_length=.2)


class Readout(VGroup):
    def __init__(self, x, y, value, size=VALUE, color=INK, decimals=0, *, align="left", role="value"):
        self.x, self.y, self.align, self.role = x, y, align, role
        self.size, self.ink, self.fmt = size, color, "{:,.%df}" % decimals
        self.previous = self.fmt.format(value)
        super().__init__(label(self.previous, x, y, size, color, mono=True, bold=True, align=align, role=role))

    def get_value(self):
        return self.previous

    def set_value(self, value):
        rendered = self.fmt.format(value)
        if rendered != self.previous:
            self[0].become(label(rendered, self.x, self.y, self.size, self.ink, mono=True, bold=True,
                                 align=self.align, role=self.role))
            self.previous = rendered
        return self


class Page(MovingCameraScene):
    title_font = FONT
    # ---------------------------------------------------------------- page furniture
    def setup(self):
        self.camera.background_color = BG
        self.heading = None
        self.footer = None
        self.gate_stats = {"samples": 0, "max_words": 0, "min_label_em": 999, "min_foot_em": 999,
                           "max_glass_pct": 0}

    def header(self, title, source, kicker=None):
        """Static still layout: title on black, the source as a small glass tag."""
        self.add(label(title, -SAFE_X, 3.3, TITLE, bold=True, role="title"))
        self.add(*self.source_tag(source))

    @staticmethod
    def source_tag(text, font=FONT):
        words = label(text, -SAFE_X + .22, -3.35, FOOT, MUTED, role="foot", limit=14.0, font=font)
        shell = box(-SAFE_X + words.width / 2 + .22, -3.35, words.width + .44, .44)
        return shell, words

    def open(self, footnote):
        if not config.save_last_frame:
            self.intro(QUESTIONS[type(self).__name__])
        self.foot(footnote)

    def title(self, text, run_time=.55):
        new = label(text, -SAFE_X, 3.3, TITLE, bold=True, role="title", font=self.title_font)
        if self.heading is None:
            self.play(FadeIn(new, shift=point(0, -.12)), run_time=run_time, rate_func=rate_functions.ease_out_cubic)
        else:
            self.play(ReplacementTransform(self.heading, new), run_time=run_time,
                      rate_func=rate_functions.ease_in_out_cubic)
        self.heading = new

    def foot(self, text):
        new = VGroup(*self.source_tag(text, font=self.title_font))
        if self.footer is None:
            self.play(FadeIn(new), run_time=.3)
        else:
            self.play(FadeOut(self.footer), run_time=.2)
            self.play(FadeIn(new), run_time=.25)
        self.footer = new

    def clear_stage(self, *keep, run_time=.4):
        """Fade everything except the heading, footer and anything kept."""
        keep = {id(m) for m in (self.heading, self.footer, self.camera.frame, *keep) if m is not None}
        gone = [m for m in self.mobjects if id(m) not in keep and isinstance(m, VMobject)]
        if gone:
            self.play(*(FadeOut(m, shift=point(0, .1)) for m in gone), run_time=run_time)

    def intro(self, question):
        """Two-second bookend on pure black: wordmark, the video's question and the three app rails."""
        mark = label("NativeTube · iced vs GPUI vs Rune", -SAFE_X, 1.55, LABEL, MUTED, bold=True)
        ask = label(question, -SAFE_X, .35, 62, bold=True, role="title")
        rails = VGroup(*(bar(-SAFE_X + i * 2.5, -1.0, 2.25, COLOR[app], .14) for i, app in enumerate(APPS)))
        self.play(AnimationGroup(FadeIn(mark, shift=point(0, .12)), FadeIn(ask, shift=point(0, .2)),
                                 *(GrowFromEdge(item, LEFT) for item in rails), lag_ratio=.15),
                  run_time=1.0, rate_func=rate_functions.ease_out_cubic)
        self.wait(.6)
        self.play(FadeOut(VGroup(mark, ask, rails), shift=point(0, .2)), run_time=.4)

    def wait(self, duration=1, **kwargs):
        # No idle hold longer than 1.5 s.
        return super().wait(min(duration, 1.5), **kwargs)

    # ---------------------------------------------------------------- motion helpers
    @staticmethod
    def bounds(mob):
        pts = [m.points for m in mob.family_members_with_points() if isinstance(m, VMobject)
               and max(m.get_fill_opacity(), m.get_stroke_opacity() if m.get_stroke_width() > 0 else 0) > .045]
        if not pts:
            return None
        allpts = np.concatenate(pts)
        return allpts[:, 0].min(), allpts[:, 1].min(), allpts[:, 0].max(), allpts[:, 1].max()

    def visible_bounds(self, extra=()):
        boxes = [self.bounds(m) for m in (*self.mobjects, *extra) if m is not self.camera.frame]
        boxes = [b for b in boxes if b is not None]
        if not boxes:
            return None
        arr = np.array(boxes)
        return arr[:, 0].min(), arr[:, 1].min(), arr[:, 2].max(), arr[:, 3].max()

    def push(self, x, y, zoom=.9, extra=()):
        """Camera push-in toward a point, clamped so all visible content stays inside the safe frame."""
        w = 16 * zoom
        bounds = self.visible_bounds(extra)
        if bounds is not None:
            x0, y0, x1, y1 = bounds
            w = min(16, max(w, 16 * (x1 - x0) / (2 * SAFE_X), 16 * (y1 - y0) / (2 * SAFE_Y)))
            k = w / 16
            lo_x, hi_x = x1 - SAFE_X * k, x0 + SAFE_X * k
            lo_y, hi_y = y1 - SAFE_Y * k, y0 + SAFE_Y * k
            x = float(np.clip(x, lo_x, hi_x)) if lo_x <= hi_x else 0
            y = float(np.clip(y, lo_y, hi_y)) if lo_y <= hi_y else 0
        return self.camera.frame.animate.set(width=w).move_to(point(x, y))

    def release(self):
        return self.camera.frame.animate.set(width=16).move_to(ORIGIN)

    def difference(self, marks, badges, focus, *, worse=(), hold=1.1, zoom=.9):
        """End a comparison on an explicit difference: brackets draw, badges spring in, the larger value pulses."""
        self.play(AnimationGroup(*(Create(mark) for mark in marks), lag_ratio=.2),
                  self.push(*focus, zoom=zoom, extra=[*marks, *badges]), run_time=.6,
                  rate_func=rate_functions.ease_in_out_cubic)
        self.play(AnimationGroup(*(FadeIn(item, scale=.6) for item in badges), lag_ratio=.18),
                  run_time=.5, rate_func=rate_functions.ease_out_back)
        if worse:
            self.play(*(pulse(item) for item in worse), run_time=.55)
        self.wait(hold)
        self.play(self.release(), run_time=.45, rate_func=rate_functions.ease_in_out_cubic)
        return VGroup(*marks, *badges)

    # ---------------------------------------------------------------- end card
    def tear_down(self):
        if not config.save_last_frame and type(self).__name__ in ENDINGS:
            self.outro()
        if os.environ.get("VIZ_AUDIT_TEXT") and not config.save_last_frame:
            gates = data.OUT.parent / "gates"
            gates.mkdir(parents=True, exist_ok=True)
            (gates / f"{type(self).__name__}.json").write_text(
                json.dumps(self.gate_stats | {"passed": True}, indent=1) + "\n")
        super().tear_down()

    def outro(self):
        """2.5 s end card on black: the takeaway counts up and its single-machine badge springs in."""
        value, digits, unit, takeaway, color, badge_text, caveat = ENDINGS[type(self).__name__]
        color = COLOR.get(color, MINT)
        self.camera.frame.set(width=16).move_to(ORIGIN)
        old = [m for m in self.mobjects if isinstance(m, VMobject) and m is not self.camera.frame]
        self.play(*(FadeOut(m, scale=.97) for m in old), run_time=.3)
        hero_x = -SAFE_X + .36
        counter = Readout(hero_x, 1.05, 0, 120, color, digits, role="hero")
        final = label(f"{value:,.{digits}f}", hero_x, 1.05, 120, color, mono=True, bold=True)
        units = VGroup()
        if unit:
            units = label(unit, 0, 0, 44, color, bold=True, role="value")
            units.next_to(final, point(1, 0), buff=.25).align_to(final, point(0, -1))
        copy = label(takeaway, -SAFE_X, -.55, 34, INK, bold=True)
        tag = badge(badge_text, 0, 0, INK, 26)
        tag.move_to(point(hero_x + tag.width / 2, -1.6))
        tail = VGroup(*self.source_tag(caveat))
        self.add(counter)
        target = ValueTracker(0)
        counter.add_updater(lambda n: n.set_value(target.get_value()))
        self.play(target.animate.set_value(value), FadeIn(copy, shift=point(0, .12)), FadeIn(units),
                  FadeIn(tail), run_time=.85, rate_func=rate_functions.ease_out_cubic)
        counter.clear_updaters()
        self.play(FadeIn(tag, scale=.6, rate_func=rate_functions.ease_out_back), pulse(counter, 1.05), run_time=.55)
        MovingCameraScene.wait(self, .8)

    # ---------------------------------------------------------------- QA gates
    def update_to_time(self, t):
        super().update_to_time(t)
        if not os.environ.get("VIZ_AUDIT_TEXT") or self.renderer.skip_animations or config.save_last_frame:
            return
        tick = self.renderer.time * 4
        if t > 0 and abs(tick - round(tick)) > 2.1 / config.frame_rate:
            return
        self.gate(tick / 4)

    def gate(self, at):
        scene = type(self).__name__
        frame = self.camera.frame
        scale = 16 / frame.width
        cx, cy = frame.get_center()[:2]
        stats = self.gate_stats
        stats["samples"] += 1

        def fail(message):
            raise ValueError(f"QA gate {scene} @ {at:.2f}s: {message}")

        # (a) everything visible stays inside the 96 px safe frame, after the camera transform.
        for mob in self.mobjects:
            if mob is frame:
                continue
            bounds = self.bounds(mob)
            if bounds is None:
                continue
            x0, y0, x1, y1 = ((bounds[0] - cx) * scale, (bounds[1] - cy) * scale,
                              (bounds[2] - cx) * scale, (bounds[3] - cy) * scale)
            if x0 < -SAFE_X - .02 or x1 > SAFE_X + .02 or y0 < -SAFE_Y - .02 or y1 > SAFE_Y + .02:
                fail(f"outside the safe frame: {self.describe(mob)} at x {x0:.2f}..{x1:.2f}, y {y0:.2f}..{y1:.2f}")
        texts = list(self.visible_text(self.mobjects))
        words = 0
        for text in texts:
            opacity = max(m.get_fill_opacity() for m in text.family_members_with_points())
            role = getattr(text, "role", "label")
            if opacity >= .98 and hasattr(text, "base_em"):
                # (b) rendered size: footnote ≥ 22 px, every other label ≥ 32 px.
                em = text.base_em * text.height / text.base_height * scale
                floor = 22 if role == "foot" else 32
                key = "min_foot_em" if role == "foot" else "min_label_em"
                stats[key] = min(stats[key], round(em, 1))
                if em < floor - .5:
                    fail(f"text {text.words!r} renders at {em:.1f} px em (< {floor})")
            if opacity > .5 and role != "foot":
                words += sum(1 for token in re.split(r"[\s,:;()?]+", getattr(text, "words", ""))
                             if token and token not in NOT_PROSE and not re.search(r"\d", token)
                             and re.search(r"[A-Za-z]", token))
        # (c) at most 14 prose words on screen.
        stats["max_words"] = max(stats["max_words"], words)
        if words > WORD_LIMIT:
            fail(f"{words} prose words on screen: " + " | ".join(
                t.words for t in texts if getattr(t, "role", "") != "foot" and hasattr(t, "words")))
        # (d) no glass container above 35 % of the frame.
        for mob in self.mobjects:
            for part in mob.get_family():
                if getattr(part, "glass", False) and isinstance(part, VMobject) and part.get_fill_opacity() > .5:
                    area = part.width * part.height * scale * scale
                    stats["max_glass_pct"] = max(stats["max_glass_pct"], round(area / 144 * 100, 1))
                    if area > GLASS_LIMIT:
                        fail(f"glass container covers {area / 144:.0%} of the frame")
        # (e) no text-on-text overlap.
        for a, b in combinations(texts, 2):
            width = min(a.get_right()[0], b.get_right()[0]) - max(a.get_left()[0], b.get_left()[0])
            height = min(a.get_top()[1], b.get_top()[1]) - max(a.get_bottom()[1], b.get_bottom()[1])
            if width > .045 and height > .045:
                fail(f"{getattr(a, 'words', '?')!r} overlaps {getattr(b, 'words', '?')!r}")

    @staticmethod
    def describe(mob):
        texts = [getattr(m, "words", None) for m in mob.get_family() if isinstance(m, Text)]
        return f"{type(mob).__name__}({', '.join(t for t in texts if t)[:60]})"

    @classmethod
    def visible_text(cls, mobjects, seen=None):
        if seen is None:
            seen = set()
        for mob in mobjects:
            if id(mob) in seen:
                continue
            seen.add(id(mob))
            if isinstance(mob, Text):
                if any(shape.get_fill_opacity() > .045 for shape in mob.family_members_with_points()):
                    yield mob
            else:
                yield from cls.visible_text(mob.submobjects, seen)


# ------------------------------------------------------------------ shared drawing for stills
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
    video = VGroup(surface, label("mpv video", -4.36, y - .31, 25, MINT, align="center"))
    descriptions = VGroup(
        label("Transparent region in the UI surface", -.1, y + .82, 32, INK),
        label("mpv draws to a subsurface below the UI", -.1, y + .12, 31, MINT),
        label("Wayland composites the surfaces", -.1, y - .58, 28, MUTED),
    )
    return VGroup(strips, hole, video, descriptions), video


# ------------------------------------------------------------------ FramePath
FIXTURE_BARS = ("#FF1700", "#00D700", "#FFEE00", "#000DFF", "#FF26FF", "#00E6FF")


def frame_tile(x, y, w):
    h = w * 476 / 848
    stripes = VGroup(*(Rectangle(width=w / 6, height=h, stroke_width=0, fill_color=c, fill_opacity=1)
                       .move_to(point(x - w / 2 + w / 12 + i * w / 6, y)) for i, c in enumerate(FIXTURE_BARS)))
    edge = RoundedRectangle(corner_radius=.08, width=w, height=h, stroke_color=RING, stroke_width=3,
                            fill_opacity=0).move_to(point(x, y))
    return VGroup(stripes, edge)


def packet(x, y, color):
    return RoundedRectangle(corner_radius=.05, width=.42, height=.26, stroke_width=0, fill_color=color,
                            fill_opacity=1).move_to(point(x, y))


class FramePath(Page):
    def construct(self):
        self.open("Laptop fixture clip • 30 fps • one packet = one frame • animation slowed for clarity")
        self.software_beat()
        self.hole_beat()
        self.instructions_beat()
        rows = [(machine, title, normalized.across_apps(machine, workload, "cores_busy"), color)
                for machine, workload, title, color in (
                    ("laptop", "playback-software", "Software", SOFT), ("laptop", "playback-gpu", "GPU", GPU),
                    ("fly", "playback-software", "Software", SOFT), ("fly", "playback-egl", "GPU", GPU))]
        if all(cores for _, _, cores, _ in rows):
            self.no_gpu_beat(rows)

    def software_beat(self):
        self.title("Every video frame is 1.6 MB")
        tile = frame_tile(-4.55, 1.1, 5.2)
        size = label("848 × 476 px", -SAFE_X + .05, -.72, LABEL, MUTED)
        xs = (-5.55, -1.85, 1.85, 5.55)
        y = -1.75
        self.chips = [chip(name, x, y, 3.1, SOFT, 24, .9) for name, x in
                      zip(("Decode", "Copy to RAM", "Upload to UI", "Composite"), xs, strict=True)]
        self.links = [right_arrow(a + 1.6, y, b - 1.6, y, MUTED) for a, b in zip(xs, xs[1:])]
        rate = Readout(-.6, 1.4, 0, HERO, SOFT, 1, role="hero")
        unit = label("MB/s", 3.0, 1.1, 40, SOFT, bold=True, role="value")
        self.play(FadeIn(tile, scale=.94), FadeIn(size), run_time=.6, rate_func=rate_functions.ease_out_cubic)
        self.play(AnimationGroup(*(FadeIn(c, shift=point(0, .15)) for c in self.chips),
                                 *(Create(a) for a in self.links), lag_ratio=.12),
                  FadeIn(rate), FadeIn(unit), run_time=.9)
        clock = ValueTracker(0)
        count, frame_mb = 30, 848 * 476 * 4 / 1e6
        lane, start, end = -2.75, xs[0] - 1.38, xs[-1] + 1.38
        packets = VGroup(*(packet(start, lane, SOFT) for _ in range(count)))

        def flow_packets(train):
            progress = clock.get_value()
            for index, item in enumerate(train):
                along = float(np.clip((progress * (count + 8) - index) / 8, 0, 1))
                item.move_to(point(start + along * (end - start), lane))
                item.set_opacity(1 if 0 < along < 1 else 0)

        packets.add_updater(flow_packets, call_updater=True)
        rate.add_updater(lambda n: n.set_value(
            min(count, max(0, math.floor(clock.get_value() * (count + 8) - 8 + 1e-9))) * frame_mb))
        self.add(packets)
        self.play(clock.animate.set_value(1), run_time=5.2, rate_func=linear)
        rate.clear_updaters()
        rate.set_value(count * frame_mb)
        packets.clear_updaters()
        self.remove(packets)
        self.play(pulse(rate, 1.05), run_time=.5)
        self.wait(1.0)
        self.play(FadeOut(VGroup(tile, size, rate, unit)), run_time=.4)

    def hole_beat(self):
        self.title("Leave a hole for video")
        _, copy, upload, composite = self.chips
        window = VGroup(box(2.1, .95, 6.6, 3.5), bar(-.95, 2.4, 6.1, GRID, .2))
        for i in range(3):
            window.add(bar(2.3, 1.55 - .5 * i, 2.6 - .4 * i, GRID, .2))
        hole_at = point(.45, .7)
        surface = frame_tile(hole_at[0], hole_at[1], 2.9)
        hole = RoundedRectangle(corner_radius=.06, width=2.9, height=2.9 * 476 / 848, stroke_color=GPU,
                                stroke_width=4, fill_opacity=0).move_to(hole_at)
        caption = label("UI chrome", 2.3, -.32, LABEL, MUTED)
        self.play(copy.animate.set_opacity(.18), upload.animate.set_opacity(.18),
                  *(link.animate.set_opacity(.18) for link in self.links), run_time=.5)
        self.play(FadeIn(window, scale=.96), run_time=.55, rate_func=rate_functions.ease_out_cubic)
        self.play(FadeIn(hole), FadeIn(caption), run_time=.4)
        track = ValueTracker(0)
        path = VMobject().set_points_smoothly([point(-5.55, -1.25), point(-4.6, 1.0), point(-2.0, 1.4), hole_at])
        packets = VGroup(*(packet(-5.55, -1.25, GPU) for _ in range(12)))

        def drop(train):
            for index, item in enumerate(train):
                along = float(np.clip((track.get_value() * 18 - index) / 6, 0, 1))
                item.move_to(path.point_from_proportion(along))
                item.set_opacity(1 if 0 < along < 1 else 0)

        packets.add_updater(drop, call_updater=True)
        self.add(packets)
        self.play(track.animate.set_value(.4), run_time=1.4, rate_func=linear)
        self.add(surface)
        self.bring_to_front(hole, packets)
        self.play(track.animate.set_value(1), FadeIn(surface), composite[0].animate.set_stroke(GPU, 3),
                  run_time=2.2, rate_func=linear)
        packets.clear_updaters()
        self.remove(packets)
        self.foot("mpv draws into a Wayland subsurface below the UI • the compositor stacks the two")
        self.wait(1.2)
        self.clear_stage()

    def instructions_beat(self):
        self.title("Instructions per 10 s of playback")
        self.foot("Laptop • AC • user-space instruction counters • same clip • medians of three")
        ctr = data.laptop_counters()
        soft = {app: ctr[app]["playback-software"]["cpu"]["instructions"] / 1e9 for app in APPS}
        gpu = {app: ctr[app]["playback-gpu"]["cpu"]["instructions"] / 1e9 for app in APPS}
        span = lambda values: f"{min(values):.2f}–{max(values):.2f} B"
        hero = label(span([round(v, 2) for v in soft.values()]), -SAFE_X, 1.85, 88, SOFT, mono=True, bold=True,
                     role="hero")
        path_name = label("Software path", -SAFE_X, .85, 26, MUTED)
        left, scale = -4.7, 9.6 / max(soft.values())
        ys = (0.0, -.85, -1.7)
        names, bars, values, ghosts = VGroup(), VGroup(), VGroup(), VGroup()
        for app, y in zip(APPS, ys, strict=True):
            names.add(label(NAME[app], -SAFE_X, y, 28, COLOR[app], bold=True))
            bars.add(bar(left, y, soft[app] * scale, SOFT))
            ghosts.add(ghost(left, y, soft[app] * scale))
            values.add(label(f"{soft[app]:.2f} B", left + soft[app] * scale + .18, y, VALUE, SOFT, bold=True,
                             role="value"))
        self.play(FadeIn(hero, shift=point(0, .15)), FadeIn(path_name), FadeIn(names), run_time=.5)
        self.play(AnimationGroup(*(GrowFromEdge(b, LEFT) for b in bars), lag_ratio=.12), run_time=.9,
                  rate_func=rate_functions.ease_out_cubic)
        self.play(FadeIn(values), run_time=.3)
        self.wait(1.0)
        self.add(ghosts)
        self.bring_to_front(bars)
        new_values = VGroup(*(label(f"{gpu[app]:.2f} B", left + gpu[app] * scale + .18, y, VALUE, GPU, bold=True,
                                    role="value") for app, y in zip(APPS, ys, strict=True)))
        self.play(*(Transform(b, bar(left, y, gpu[app] * scale, GPU)) for b, app, y in zip(bars, APPS, ys)),
                  *(ReplacementTransform(a, b) for a, b in zip(values, new_values)),
                  ReplacementTransform(hero, label(span([round(v, 2) for v in gpu.values()]), -SAFE_X, 1.85, 88, GPU,
                                                   mono=True, bold=True, role="hero")),
                  ReplacementTransform(path_name, label("GPU subsurface path", -SAFE_X, .85, 26, MUTED)),
                  run_time=1.6, rate_func=rate_functions.ease_in_out_cubic)
        y0 = ys[-1]
        gpu_end, soft_end = left + gpu["rune"] * scale, left + soft["rune"] * scale
        mark = bracket([(gpu_end + 1.45, y0 - .34), (gpu_end + 1.45, y0 - .45), (soft_end, y0 - .45),
                        (soft_end, y0 - .34)])
        tag = badge("≈19× fewer per frame", (gpu_end + 1.45 + soft_end) / 2, y0 - .98, INK, VALUE)
        self.difference([mark], [tag], (0, -1.2), worse=[ghosts], hold=1.2)
        self.clear_stage()

    def no_gpu_beat(self, rows):
        self.title("And without a GPU?")
        self.foot("Cores busy • medians of three • laptop on battery • Fly VM 4 vCPUs, llvmpipe • scaled per machine")
        span = lambda lo, hi: f"{lo:.2f}" if f"{lo:.2f}" == f"{hi:.2f}" else f"{lo:.2f}–{hi:.2f}"
        left, width = -3.9, 6.0
        groups = (("Laptop", 2.2, rows[:2]), ("Fly VM", -.35, rows[2:]))
        slots, items, heads = {}, {}, VGroup()
        for name, top, pair in groups:
            heads.add(label(name, -SAFE_X, top, 30, INK, bold=True))
            peak = max(cores[1] for _, _, cores, _ in pair)
            for i, (machine, title, cores, color) in enumerate(pair):
                y = top - .8 - .85 * i
                slots[(machine, i)] = y
                w = width * cores[1] / peak
                items[(machine, title)] = VGroup(
                    label(title, -SAFE_X, y, 26, MUTED),
                    bar(left, y, w, color, .55),
                    label(f"{span(*cores)} cores", left + w + .18, y, 28, color, bold=True, role="value"))
        self.play(FadeIn(heads), run_time=.4)
        for machine in ("laptop", "fly"):
            group = [items[key] for key in items if key[0] == machine]
            self.play(AnimationGroup(*(AnimationGroup(FadeIn(g[0]), GrowFromEdge(g[1], LEFT), FadeIn(g[2]))
                                       for g in group), lag_ratio=.25), run_time=1.1,
                      rate_func=rate_functions.ease_out_cubic)
            self.wait(.6)
        # The order flips: the emulated GPU path's bar rises above the software bar.
        egl, soft = items[("fly", "GPU")], items[("fly", "Software")]
        top_y, low_y = slots[("fly", 0)], slots[("fly", 1)]
        self.play(*(FadeOut(m) for m in (egl[0], egl[2], soft[0], soft[2])), run_time=.25)
        for m in (egl[0], egl[2]):
            m.shift(point(0, top_y - low_y))
        for m in (soft[0], soft[2]):
            m.shift(point(0, low_y - top_y))
        self.play(egl[1].animate(path_arc=-1.4).shift(point(0, top_y - low_y)),
                  soft[1].animate(path_arc=-1.4).shift(point(0, low_y - top_y)),
                  run_time=1.1, rate_func=rate_functions.ease_in_out_cubic)
        self.play(*(FadeIn(m, shift=point(.1, 0)) for m in (egl[0], egl[2], soft[0], soft[2])), run_time=.3)
        mark = bracket([(left - .12, top_y + .2), (left - .28, top_y + .2), (left - .28, low_y - .2),
                        (left - .12, low_y - .2)])
        tag = badge("Order flips", 2.6, -2.75, INK, 34)
        self.difference([mark], [tag], (0, -1.0), worse=[egl[1]], hold=1.4)


# ------------------------------------------------------------------ FirstFrame
class FirstFrame(Page):
    LEFT, WIDTH, YS = -4.5, 7.0, (1.3, .05, -1.2)

    def construct(self):
        fly = data.fly_input()
        _, laptop = data.timing({})
        self.open("Laptop • real GPU • local fixture replay • medians of three")
        self.lanes = None
        self.race("Laptop: click → first frame", laptop, 320)
        if "timing" in fly:
            self.race("Fly VM: the same click", fly["timing"], 320,
                      footnote="Fly VM • llvmpipe EGL, no GPU • local fixture replay • medians of three")
            if "youtube" in fly["timing"]:
                self.browser(fly["timing"])
        self.stopwatch()

    def x_at(self, ms, limit):
        return self.LEFT + self.WIDTH * ms / limit

    def race(self, title, rows, limit, footnote=None):
        self.title(title)
        if footnote:
            self.foot(footnote)
        if self.lanes is None:
            self.lane_names = VGroup(*(label(NAME[app], -SAFE_X, y, 34, COLOR[app], bold=True)
                                       for app, y in zip(APPS, self.YS)))
            self.tracks = VGroup(*(ghost(self.LEFT, y, self.WIDTH, .62).set_fill(INK, .03).set_stroke(width=0)
                                   for y in self.YS))
            self.play(FadeIn(self.lane_names), FadeIn(self.tracks), run_time=.4)
            self.lanes = True
        else:
            self.clear_stage(self.lane_names, self.tracks, run_time=.35)
        clock = ValueTracker(0)
        longest = max(rows[app]["open_ms"] for app in APPS)
        stopwatch = Readout(SAFE_X - 1.0, 2.35, 0, 90, INK, role="hero", align="right")
        unit = label("ms", SAFE_X, 2.2, 32, MUTED, align="right", role="value")
        bars, values = VGroup(), VGroup()
        for app, y in zip(APPS, self.YS):
            ms = rows[app]["open_ms"]
            runner = bar(self.LEFT, y, .001, COLOR[app], .62)
            runner.add_updater(lambda b, y=y, ms=ms, app=app: b.become(
                bar(self.LEFT, y, max(.001, self.x_at(min(ms, clock.get_value()), limit) - self.LEFT), COLOR[app], .62)))
            value = label(f"{ms:,.0f} ms", self.x_at(ms, limit) + .22, y, 40, COLOR[app], bold=True, role="value")
            value.add_updater(lambda v, ms=ms: v.set_opacity(1 if clock.get_value() >= ms - .001 else 0),
                              call_updater=True)
            bars.add(runner)
            values.add(value)
        stopwatch.add_updater(lambda n: n.set_value(clock.get_value()))
        self.add(bars, values, stopwatch, unit)
        # Real proportional speed: 1 ms of measured time takes 12.5 ms of animation.
        self.play(clock.animate.set_value(longest), run_time=longest * .0125, rate_func=linear)
        for mob in (*bars, *values, stopwatch):
            mob.clear_updaters()
        self.wait(.4)
        fast = min(APPS, key=lambda app: round(rows[app]["open_ms"]))
        x_fast = self.x_at(rows[fast]["open_ms"], limit)
        marks = [line(x_fast, self.YS[0] + .5, x_fast, self.YS[-1] - .5, INK, 2).set_stroke(opacity=.6)]
        tags = []
        for i, (app, y) in enumerate(zip(APPS, self.YS)):
            if app == fast:
                continue
            gap = round(rows[app]["open_ms"]) - round(rows[fast]["open_ms"])
            end = self.x_at(rows[app]["open_ms"], limit)
            if end - x_fast > .05:
                marks.append(bracket([(x_fast, y + .44), (end, y + .44)], INK, 3))
            tag = badge(f"+{gap} ms", 0, 0, COLOR[app], 28)
            tag.move_to(point(values[i].get_right()[0] + .3 + tag.width / 2, y))
            tags.append(tag)
        worst = max(APPS, key=lambda app: rows[app]["open_ms"])
        self.difference(marks, tags, (1.5, .1), worse=[bars[APPS.index(worst)]], hold=1.2)
        self.lane_bars, self.lane_values = bars, values

    def browser(self, rows):
        self.title("YouTube in a browser?")
        self.foot(f"YouTube web: Fly VM, Chromium, signed out, {rows['youtube']['size']}, live network "
                  "• native apps: local fixtures")
        limit = 3400
        rerun = normalized.cold_range_s()
        ys = (1.85, 1.05, .25)
        self.clear_stage(self.lane_names, self.lane_bars, self.lane_values, self.tracks, run_time=.3)
        moves = []
        for i, app in enumerate(APPS):
            ms, y = rows[app]["open_ms"], ys[i]
            moved = label(f"{ms:,.0f} ms", self.x_at(ms, limit) + .22, y, 30, COLOR[app], bold=True, role="value")
            moves += [self.lane_names[i].animate.move_to(point(self.lane_names[i].get_center()[0], y)),
                      Transform(self.lane_bars[i], bar(self.LEFT, y, self.x_at(ms, limit) - self.LEFT, COLOR[app], .45)),
                      ReplacementTransform(self.lane_values[i], moved),
                      Transform(self.tracks[i], ghost(self.LEFT, y, self.WIDTH, .45).set_fill(INK, .03).set_stroke(width=0))]
        self.play(*moves, run_time=1.0, rate_func=rate_functions.ease_in_out_cubic)
        y = -1.0
        name = label("YouTube web", -SAFE_X, y, 32, COLOR["youtube"], bold=True)
        tag = badge("live network, page load", 0, 0, INK, 24)
        tag.move_to(point(-SAFE_X + tag.width / 2, -1.95))
        clock = ValueTracker(0)
        web_ms = rows["youtube"]["open_ms"]
        runner = bar(self.LEFT, y, .001, ROSE, .62)
        runner.add_updater(lambda b: b.become(bar(self.LEFT, y, max(.001, self.x_at(min(web_ms, clock.get_value()),
                                                                                    limit) - self.LEFT), ROSE, .62)))
        value = label(f"{web_ms:,.0f} ms", self.x_at(web_ms, limit) + .22, y, 40, ROSE, bold=True, role="value")
        self.play(FadeIn(name), FadeIn(tag), run_time=.4)
        self.add(runner)
        self.play(clock.animate.set_value(web_ms), run_time=2.2, rate_func=rate_functions.ease_in_quad)
        runner.clear_updaters()
        self.play(FadeIn(value, scale=.8), pulse(runner, 1.03), run_time=.5)
        words = VGroup()
        if rerun:
            lo, hi = (v * 1000 for v in rerun)
            band = ghost(self.x_at(lo, limit), y - .62, self.x_at(hi, limit) - self.x_at(lo, limit), .3)
            band.set_stroke(ROSE, 2, 1).set_fill(ROSE, .25)
            words = label(f"cold rerun {rerun[0]:.2f}–{rerun[1]:.2f} s", self.x_at(hi, limit) + .2, y - .62, 26, ROSE,
                          bold=True, role="value")
            self.play(GrowFromEdge(band, LEFT), FadeIn(words), run_time=.6)
        self.wait(1.3)
        trials = normalized.warm_trials()
        if trials:
            fastest = min(row["any_frame_ms"] for row in trials) / 1000
            slowest = max(row["any_frame_ms"] for row in trials) / 1000
            ads = [row["ad_mode_ms"] / 1000 for row in trials if row.get("status") == "complete" and row.get("ad_mode_ms")]
            timeouts = sum(row.get("status") == "deadline" for row in trials)
            text = f"Warm switch: {fastest:.1f}–{slowest:.1f} s"
            if ads:
                text += f" • one trial sat behind a {ads[0]:.1f} s ad"
            if timeouts:
                text += " • one timed out"
            warm = label(text, -SAFE_X, -2.75, 28, INK, bold=True)
            self.play(FadeOut(tag), FadeOut(words), run_time=.3)
            self.play(FadeIn(warm, shift=point(0, .12)), run_time=.5)
            self.wait(1.5)
        self.clear_stage(run_time=.4)

    def stopwatch(self):
        self.title("Inside the stopwatch")
        self.foot("Stage times not isolated • " + callouts.PREWARM_NOTE.removeprefix("In these runs, "))
        xs = (-5.4, -1.8, 1.8, 5.4)
        chips = [chip(name, x, .7, 3.0, SKY, 30, 1.1) for name, x in zip(("Resolve", "Attach", "Decode", "Paint"), xs)]
        links = [right_arrow(a + 1.55, .7, b - 1.55, .7, MUTED) for a, b in zip(xs, xs[1:])]
        for i, node in enumerate(chips):
            self.play(*([Create(links[i - 1])] if i else []), FadeIn(node, shift=point(.15, 0)), run_time=.32)
        note = label("Prewarm moves attach before the click", -SAFE_X, -1.2, 36, INK, bold=True)
        self.play(FadeIn(note, shift=point(0, .12)), chips[1].animate.shift(point(0, .35)), run_time=.6)
        self.play(chips[1].animate.shift(point(0, -.35)), run_time=.4)
        self.wait(1.4)


# ------------------------------------------------------------------ ScrollCost
class ScrollCost(Page):
    XS = (-4.8, 0.0, 4.8)

    def construct(self):
        fly = data.fly_input()
        _, rows = data.wheel({})
        self.open("Laptop • AC • user-space instructions • 27-tick wheel • medians of three • 1 dot ≈ 1 M")
        self.rain(rows)
        self.routes()
        if "wheel_cpu" in fly:
            self.no_gpu(rows, fly["wheel_cpu"])

    def rain(self, rows):
        self.title("One second of wheel scrolling")
        floor, tallest = -2.75, 3.25
        peak = max(rows[app]["rate"] for app in APPS)
        names, feeds, clouds = VGroup(), VGroup(), []
        scroll = ValueTracker(0)
        rng = random.Random(7)
        pulses = 8
        for app, x in zip(APPS, self.XS):
            names.add(label(NAME[app], x, 2.6, 32, COLOR[app], align="center", bold=True))
            cards = VGroup()
            for i in range(4):
                row = VGroup(bar(x - 1.2, 0, .55, COLOR[app], .26), bar(x - .5, 0, 1.4 if i % 2 else 1.0, GRID, .12))

                def roll(r, i=i, x=x):
                    offset = (i * .34 + scroll.get_value() * .34) % 1.36
                    r.move_to(point(x - .05, 2.05 - offset))
                    r.set_opacity(1 if .1 < offset < 1.0 else 0)

                row.add_updater(roll, call_updater=True)
                cards.add(row)
            feeds.add(VGroup(box(x, 1.55, 3.0, 1.35), cards))
            per_frame = max(1, round(rows[app]["rate"] / 60 / 1e6))
            dots = VGroup()
            for p in range(pulses):
                for _ in range(per_frame):
                    rest = point(x + rng.uniform(-1.25, 1.25), rng.uniform(floor + .15, .5))
                    dot = Dot(rest + point(0, .6), radius=.045, color=COLOR[app])

                    def fall(d, p=p, rest=rest):
                        age = scroll.get_value() - p
                        d.move_to(rest + point(0, max(0, 1 - age) * .6))
                        d.set_opacity(float(np.clip(age * 2, 0, 1)))

                    dot.add_updater(fall, call_updater=True)
                    dots.add(dot)
            clouds.append(dots)
        self.play(FadeIn(names), FadeIn(feeds), run_time=.5)
        self.add(*clouds)
        self.play(scroll.animate.set_value(pulses + .6), run_time=5.0, rate_func=linear)
        for mob in (*feeds, *clouds):
            for part in mob.get_family():
                part.clear_updaters()
        bars, values = VGroup(), VGroup()
        for app, x in zip(APPS, self.XS):
            height = tallest * rows[app]["rate"] / peak
            bars.add(bar(x, floor, height, COLOR[app], 1.7, vertical=True))
            values.add(label(f"{rows[app]['rate'] / 1e6:,.0f} M/s", x, floor + height + .4, 40, COLOR[app],
                             align="center", bold=True, role="value"))
        self.play(FadeOut(feeds), *(ReplacementTransform(dots, b) for dots, b in zip(clouds, bars)),
                  run_time=1.3, rate_func=rate_functions.ease_in_out_cubic)
        self.play(AnimationGroup(*(FadeIn(v, shift=point(0, .15)) for v in values), lag_ratio=.15), run_time=.5)
        most, least, ratio = callouts.scroll()
        mx, lx = self.XS[APPS.index(most)], self.XS[APPS.index(least)]
        top_hi = floor + tallest
        top_lo = floor + tallest * rows[least]["rate"] / peak
        side = 1 if lx > mx else -1
        edge = mx + side * 1.0
        guide = line(mx + side * .85, top_lo, lx - side * .85, top_lo, INK, 2).set_stroke(opacity=.55)
        mark = bracket([(edge - side * .1, top_lo), (edge, top_lo), (edge, top_hi), (edge - side * .1, top_hi)])
        tag = badge(f"≈{ratio:.1f}×", (mx + lx) / 2, (top_lo + top_hi) / 2 + .2, COLOR[most], 44)
        self.difference([guide, mark], [tag], (2.0, 0), worse=[bars[APPS.index(most)]], hold=1.3)
        self.clear_stage()

    def routes(self):
        self.foot("From the GPUI and Rune notes • counters also include thumbnail decoding")
        xs = (-4.85, 0, 4.85)
        steps = {"gpui": ("Scroll tick", "Notify PageView", "Render page tree"),
                 "rune": ("Scroll tick", "Write live offset", "Translate content")}
        self.title("GPUI rebuilds the page view per tick")
        chips = [chip(name, x, .35, 4.1, GOLD, 30, 1.2) for name, x in zip(steps["gpui"], xs)]
        links = [right_arrow(a + 2.12, .35, b - 2.12, .35, MUTED) for a, b in zip(xs, xs[1:])]
        self.play(AnimationGroup(*(FadeIn(c, shift=point(.2, 0)) for c in chips), *(Create(a) for a in links),
                                 lag_ratio=.15), run_time=1.0)
        self.play(pulse(chips[2], 1.06), run_time=.5)
        self.wait(1.4)
        self.title("Rune moves a live offset instead")
        self.play(*(Transform(c, chip(name, x, .35, 4.1, LILAC, 30, 1.2)) for c, name, x in zip(chips, steps["rune"], xs)),
                  run_time=.8, rate_func=rate_functions.ease_in_out_cubic)
        self.play(pulse(chips[1], 1.06), run_time=.5)
        self.wait(1.4)
        self.clear_stage()

    def no_gpu(self, laptop_rows, fly_rows):
        self.foot("Best explanation (mechanism not measured): on CPU rasterization, per-frame draw cost dominates.")
        gpu_rank = sorted(APPS, key=lambda app: -laptop_rows[app]["rate"])
        cpu_rank = sorted(APPS, key=lambda app: -fly_rows[app]["cpu_percent"])
        left, width = -3.3, 5.8
        y_at = lambda i: 1.4 - 1.15 * i
        self.title("With a GPU")
        tag = label("Laptop • instructions per second", SAFE_X, 2.45, 26, MUTED, align="right")
        rows = {}
        for i, app in enumerate(gpu_rank):
            y = y_at(i)
            w = width * laptop_rows[app]["rate"] / laptop_rows[gpu_rank[0]]["rate"]
            rows[app] = VGroup(label(NAME[app], -SAFE_X, y, 36, COLOR[app], bold=True), bar(left, y, w, COLOR[app], .62),
                               label(f"{laptop_rows[app]['rate'] / 1e6:,.0f} M/s", left + w + .2, y, 36, COLOR[app],
                                     bold=True, role="value"))
        self.play(FadeIn(tag), AnimationGroup(*(AnimationGroup(FadeIn(r[0]), GrowFromEdge(r[1], LEFT), FadeIn(r[2]))
                                                for r in rows.values()), lag_ratio=.2), run_time=1.1)
        self.wait(1.2)
        self.play(*(FadeOut(rows[app][2]) for app in APPS), run_time=.25)
        self.title("Without one", run_time=.4)
        new_tag = label("Fly VM • wheel CPU, no GPU", SAFE_X, 2.45, 26, MUTED, align="right")
        moves = [ReplacementTransform(tag, new_tag)]
        fresh = {}
        for i, app in enumerate(cpu_rank):
            y = y_at(i)
            w = width * fly_rows[app]["cpu_percent"] / fly_rows[cpu_rank[0]]["cpu_percent"]
            here = rows[app][0].get_center()
            there = point(here[0], y)
            if there[1] > here[1] + .01:
                lane = VMobject().set_points_smoothly([here, here + point(1.9, -.15), there + point(1.9, .15), there])
                moves.append(MoveAlongPath(rows[app][0], lane))
            else:
                moves.append(rows[app][0].animate.move_to(there))
            moves.append(Transform(rows[app][1], bar(left, y, w, COLOR[app], .62), path_arc=-.8))
            fresh[app] = label(normalized.cpu_text("fly_wheel", app), left + w + .2, y, 32, COLOR[app], bold=True,
                               role="value")
        self.play(*moves, run_time=1.5, rate_func=rate_functions.ease_in_out_cubic)
        self.play(AnimationGroup(*(FadeIn(fresh[app]) for app in cpu_rank), lag_ratio=.12), run_time=.4)
        flipped = cpu_rank[0]
        start_y, end_y = y_at(gpu_rank.index(flipped)), y_at(0)
        arrow = Arrow(point(left - .3, start_y), point(left - .3, end_y), buff=0, color=INK, stroke_width=5,
                      tip_length=.24)
        flip = badge("Order flips", 2.6, -2.55, INK, 40)
        self.difference([arrow], [flip], (0, -.4), worse=[rows[flipped][1]], hold=1.4)


# ------------------------------------------------------------------ Memory
class Memory(Page):
    XS = (-4.5, 0.0, 4.5)
    FLOOR, TALLEST, WIDTH = -2.6, 4.15, 1.7

    def construct(self):
        fly = data.fly_input()
        _, rows = data.memory({})
        self.rows = rows
        self.open("Laptop on battery • process-tree PSS • medians of three • PSS apportions shared memory")
        stages = (("cold", "Cold Home, player prewarm off"), ("warm", "Warm Home, player loaded"),
                  ("playing", "Playback: mint is the player alone"), ("peak", "Peak during the tour"))
        self.peak = max(rows[app][key] for app in APPS for key, _ in stages)
        self.columns = None
        self.extras = VGroup()
        callout = None
        for key, title in stages:
            callout = self.stage(key, title, callout)
        if "memory_compare" in fly:
            self.play(FadeOut(callout), run_time=.3)
            self.fly_stage(fly["memory_compare"])

    def height(self, mib, peak=None):
        return self.TALLEST * mib / (peak or self.peak)

    def stage(self, key, title, previous):
        rows = self.rows
        self.title(title)
        if self.columns is None:
            self.columns = VGroup(*(label(NAME[app], x, -3.0, 30, COLOR[app], align="center", bold=True)
                                    for app, x in zip(APPS, self.XS)))
            self.bars = VGroup(*(bar(x, self.FLOOR, .001, COLOR[app], self.WIDTH, vertical=True)
                                 for app, x in zip(APPS, self.XS)))
            self.values = VGroup()
            self.play(FadeIn(self.columns), run_time=.3)
            self.add(self.bars)
        fades = [FadeOut(m) for m in (previous, self.extras, self.values) if m is not None and len(m.submobjects)]
        new_values, grows = VGroup(), []
        for i, (app, x) in enumerate(zip(APPS, self.XS)):
            h = self.height(rows[app][key])
            grows.append(Transform(self.bars[i], bar(x, self.FLOOR, h, COLOR[app], self.WIDTH, vertical=True)))
            new_values.add(label(f"{rows[app][key]:.0f} MiB", x, self.FLOOR + h + .38, 40, COLOR[app], align="center",
                                 bold=True, role="value"))
        self.play(*fades, *grows, run_time=1.1, rate_func=rate_functions.ease_in_out_cubic)
        self.play(AnimationGroup(*(FadeIn(v, shift=point(0, .12)) for v in new_values), lag_ratio=.12), run_time=.45)
        self.values = new_values
        self.extras = VGroup()
        if key == "warm" and rows["rune"].get("warm"):
            top = self.FLOOR + self.height(rows["rune"]["warm"])
            tag = badge("libmpv only, no EGL surface", 0, 0, INK, 22)
            tag.move_to(point(SAFE_X - tag.width / 2, top + 1.15))
            self.play(FadeIn(tag, scale=.7), run_time=.4, rate_func=rate_functions.ease_out_back)
            self.extras.add(tag)
        high, low, gap = callouts.memory()[key]
        callout = self.gap(APPS.index(low), APPS.index(high), rows[low][key], rows[high][key], f"+{gap} MiB",
                           self.bars[APPS.index(high)], COLOR[high])
        if key == "playing":
            self.play(FadeOut(callout), run_time=.3)
            callout = VGroup()
            segments, notes = VGroup(), VGroup()
            for app, x in zip(APPS, self.XS):
                core = rows[app].get("core")
                if core is None:
                    continue
                base = self.height(core)
                segments.add(bar(x, self.FLOOR, base, MINT, self.WIDTH, vertical=True))
                notes.add(label(f"{core:.0f}", x - self.WIDTH / 2 - .18, self.FLOOR + base / 2, 30, MINT,
                                align="right", bold=True, role="value"))
            self.play(FadeIn(segments, shift=point(0, .1)), run_time=.7)
            self.play(FadeIn(notes, shift=point(.1, 0)), run_time=.4)
            self.wait(1.4)
            self.extras.add(segments, notes)
        return callout

    def gap(self, low_i, high_i, low_mib, high_mib, text, high_bar, color):
        """Guide from the lower column's top, bracket the excess on the higher one, badge in the gutter."""
        xs, width = self.XS, self.WIDTH
        low_top, high_top = self.FLOOR + self.height(low_mib), self.FLOOR + self.height(high_mib)
        hx, lx = xs[high_i], xs[low_i]
        side = 1 if high_i < len(xs) - 1 else -1
        edge = hx + side * (width / 2 + .14)
        guide = line(min(hx, lx) - width / 2, low_top, max(hx, lx) + width / 2, low_top, INK, 2).set_stroke(opacity=.5)
        mark = bracket([(edge - side * .1, low_top), (edge, low_top), (edge, high_top), (edge - side * .1, high_top)])
        tag = badge(text, 0, 0, color, 30)
        tag.move_to(point(edge + side * (.2 + tag.width / 2), max(self.FLOOR + .45, (low_top + high_top) / 2 - .35)))
        return self.difference([guide, mark], [tag], (hx, (low_top + high_top) / 2), worse=[high_bar], hold=1.0)

    def fly_stage(self, rows):
        self.title("Fly VM, next to YouTube web")
        self.foot(f"App and player peaks: local fixtures • YouTube: cold settled, peak {rows['youtube']['peak_mib']:,.0f} "
                  "MiB, live network")
        order = (*APPS, "ntctl", "youtube")
        xs = (-5.75, -2.9, -.05, 2.8, 5.65)
        width = 1.45
        peak = max(rows[app]["mib"] for app in order)
        colors = {**COLOR, "ntctl": MINT}
        names = {**NAME, "ntctl": "ntctl play"}
        moves = [FadeOut(m) for m in (self.extras, self.values) if len(m.submobjects)]
        for i, app in enumerate(APPS):
            moves += [Transform(self.bars[i], bar(xs[i], self.FLOOR, self.height(rows[app]["mib"], peak), colors[app],
                                                  width, vertical=True)),
                      self.columns[i].animate.move_to(point(xs[i], -3.0))]
        self.play(*moves, run_time=1.1, rate_func=rate_functions.ease_in_out_cubic)
        late = VGroup(*(bar(x, self.FLOOR, self.height(rows[app]["mib"], peak), colors[app], width, vertical=True)
                        for app, x in zip(order[3:], xs[3:])))
        late_names = VGroup(*(label(names[app], x, -3.0, 26, colors[app], align="center", bold=True)
                              for app, x in zip(order[3:], xs[3:])))
        values = VGroup(*(label(f"{rows[app]['mib']:.0f} MiB", x, self.FLOOR + self.height(rows[app]["mib"], peak) + .38,
                                34, colors[app], align="center", bold=True, role="value")
                          for app, x in zip(order, xs)))
        self.play(FadeIn(values[:3]), FadeIn(late_names[0]), GrowFromEdge(late[0], -UP), run_time=.6)
        self.play(FadeIn(values[3]), run_time=.2)
        self.play(FadeIn(late_names[1]), GrowFromEdge(late[1], -UP), run_time=1.3, rate_func=rate_functions.ease_out_cubic)
        live = badge("live network, not fixture", 0, 0, INK, 24)
        live.move_to(point(SAFE_X - live.width / 2, values[4].get_center()[1] + .78))
        self.play(FadeIn(values[4], shift=point(0, .15)), FadeIn(live, scale=.7), run_time=.5)
        high, low, gap = callouts.fly_memory({"memory_compare": rows})
        hi, lo = order.index(high), order.index(low)
        low_top = self.FLOOR + self.height(rows[low]["mib"], peak)
        guide = line(min(xs[hi], xs[lo]) - width / 2, low_top, max(xs[hi], xs[lo]) + width / 2, low_top,
                     INK, 2).set_stroke(opacity=.5)
        tag = badge(f"+{gap} MiB", xs[hi], low_top - .75, INK, 28)
        self.difference([guide], [tag], (xs[hi], low_top), worse=[self.bars[hi]], hold=1.2)


# ------------------------------------------------------------------ Hiccups
AREA = {"iced": "iced", "GPUI": "GPUI", "Rune": "Rune", "core": "core"}


class Hiccups(Page):
    def construct(self):
        catalog = data.read(data.HICCUPS)["incidents"]
        self.open("Project incident catalog • dated discovery and fix evidence")
        self.timeline(catalog)
        self.totals(catalog)
        self.stability()

    def timeline(self, catalog):
        self.title("A week of hiccups")
        areas = ("iced", "GPUI", "Rune", "core", "tooling")
        colors = {"iced": SKY, "GPUI": GOLD, "Rune": LILAC, "core": MINT, "tooling": INK}
        area_of = lambda row: AREA.get(row["area"], "tooling")
        days = sorted({row[key][:10] for row in catalog for key in ("date", "fixed_when") if row[key]})
        start, end = date.fromisoformat(days[0]), date.fromisoformat(days[-1])
        span = (end - start).days
        left, right = -3.9, 5.2
        x_at = {(start + timedelta(days=i)).isoformat(): left + (right - left) * i / span for i in range(span + 1)}
        ys = {area: 1.75 - .95 * i for i, area in enumerate(areas)}
        frame = VGroup(label("Nov", -SAFE_X, -2.95, 24, MUTED))
        for day, x in x_at.items():
            frame.add(label(day[-2:].lstrip("0"), x, -2.95, 24, MUTED, align="center", mono=True, role="value"))
        counts = Counter(area_of(row) for row in catalog)
        for area in areas:
            frame.add(label(area, -SAFE_X, ys[area], 30, colors[area], bold=True),
                      label(str(counts[area]), SAFE_X, ys[area], 34, colors[area], align="right", bold=True,
                            role="value"))
        legend = label("● found   ○ fixed", 1.2, 2.5, 24, MUTED)
        self.play(FadeIn(frame), FadeIn(legend), run_time=.5)
        bins = {}
        for row in sorted(catalog, key=lambda r: (r["date"], r["id"])):
            bins.setdefault((area_of(row), row["date"][:10]), []).append(row)
        found = {day: VGroup() for day in x_at}
        fixed = {day: VGroup() for day in x_at}
        for (area, day), group in bins.items():
            for j, row in enumerate(group):
                y = ys[area] + (.3 - .6 * j / (len(group) - 1) if len(group) > 1 else 0)
                a = x_at[day]
                found[day].add(Dot(point(a, y), radius=.075, color=colors[area]))
                if row["fixed_when"]:
                    b = max(x_at[row["fixed_when"][:10]], a + .25)
                    fixed[row["fixed_when"][:10]].add(VGroup(
                        line(a, y, b - .07, y, colors[area], 3).set_stroke(opacity=.7),
                        Dot(point(b, y), radius=.07, color=BG).set_stroke(colors[area], 3, 1)))
        cursor = line(left, 2.15, left, -2.55, INK, 2).set_stroke(opacity=.5)
        self.add(cursor)
        for day in x_at:
            steps = [cursor.animate.set_x(x_at[day])]
            if found[day]:
                steps.append(FadeIn(found[day], scale=.5))
            if fixed[day]:
                steps.append(AnimationGroup(*(Create(m) for m in fixed[day]), lag_ratio=.03))
            self.play(*steps, run_time=.45)
        self.wait(1.2)
        self.clear_stage()

    def totals(self, catalog):
        self.title("Where they ended up")
        open_count = sum(not row["fixed_when"] for row in catalog)
        numbers = ((len(catalog), "found", INK), (len(catalog) - open_count, "with fix evidence", MINT),
                   (open_count, "still open in the catalog", MUTED))
        items = VGroup()
        for i, (_, words, color) in enumerate(numbers):
            x = -SAFE_X + i * 4.9
            items.add(VGroup(Readout(x, .7, 0, 120, color, role="hero"), label(words, x, -.75, 28, MUTED)))
        self.play(*(FadeIn(item) for item in items), run_time=.4)
        clock = ValueTracker(0)
        for (value, _, _), item in zip(numbers, items):
            item[0].add_updater(lambda n, value=value: n.set_value(value * clock.get_value()))
        self.play(clock.animate.set_value(1), run_time=1.1, rate_func=rate_functions.ease_out_cubic)
        for item in items:
            item[0].clear_updaters()
        self.play(pulse(items[1][0], 1.06), run_time=.5)
        self.wait(1.5)
        self.clear_stage()

    def stability(self):
        self.title("A separate stability check")
        self.foot("Each run opened related videos 20 times • the check samples frames and can miss brief glitches")
        assessed = data.read(data.STABILITY)["results"]["NOW"]
        clean = sum(assessed[app]["clean"] for app in APPS)
        total = sum(assessed[app]["assessed"] for app in APPS)
        hero = label(f"{clean} of {total}", -SAFE_X, .7, 120, MINT, bold=True, role="hero")
        words = label("NOW runs clean", -SAFE_X, -.75, 36, INK, bold=True)
        self.play(FadeIn(hero, shift=point(0, .2), scale=.9), run_time=.6, rate_func=rate_functions.ease_out_back)
        self.play(FadeIn(words, shift=point(0, .12)), run_time=.4)
        self.wait(1.5)


# The runner passes stills.py to Manim separately to keep static and moving output independent.


class FrameworkUpdates(Page):
    """Explain invalidation by moving one feed through three cached representations."""

    title_font = FRAMEWORK_FONT

    def construct(self):
        register_font(str(data.ROOT / "assets/fonts/GoogleSans-Variable.ttf"))
        stages = ("View", "Layout", "Draw")
        colors = (SKY, GOLD, LILAC)
        ys = (1.75, .05, -1.65)
        names = VGroup(*(label(name, -6.85, y, VALUE, color, bold=True, font=self.title_font)
                         for name, y, color in zip(stages, ys, colors, strict=True)))
        strips = [self.strip(y, color) for y, color in zip(ys, colors, strict=True)]
        self.title("What has to run again?")
        self.play(FadeIn(names), *(FadeIn(group) for group in strips), run_time=.8)
        self.foot("View chooses the cards. Layout places them. Draw produces the frame.")

        self.title("Same cards, new offset")
        self.foot("iced translates cached geometry. Rune samples a bound offset.")
        draw_offset = .58
        self.play(strips[2].animate.shift(draw_offset * LEFT), run_time=1)
        self.play(pulse(names[2], 1.1), run_time=.45)
        self.wait(1.5)

        self.title("Same gesture in GPUI")
        self.foot("NativeTube invalidates PageView. Cached chrome can be reused.")
        for name, strip in zip(names, strips, strict=True):
            self.play(pulse(name, 1.1), pulse(strip, 1.045), run_time=.8)
        self.wait(1.5)

        self.title("Scroll far enough")
        self.foot("A different group of cards must be mounted.")
        for y, color, group in zip(ys, colors, strips, strict=True):
            incoming_x = 5.1 - (draw_offset if group is strips[2] else 0)
            self.play(
                FadeOut(group[0], shift=.35 * LEFT),
                FadeIn(self.tile(incoming_x, y, color, len(group)), shift=.35 * LEFT),
                run_time=.7,
            )
        self.wait(1.5)
        self.clear_stage(run_time=.45)

        self.title("What runs again?")
        self.foot("NativeTube feed paths · each row starts with a changed scroll offset")
        xs = (-1.7, 1.55, 4.8)
        header = VGroup(*(label(name, x, 1.95, VALUE, INK, align="center", bold=True, font=self.title_font)
                          for name, x in zip(stages, xs, strict=True)))
        self.play(FadeIn(header), run_time=.4)
        rows = (
            ("iced", SKY, (False, False, True)),
            ("GPUI", GOLD, (True, True, True)),
            ("Rune", LILAC, (False, False, True)),
        )
        markers = []
        for index, (name, color, work) in enumerate(rows):
            y = .8 - 1.24 * index
            self.play(FadeIn(label(name, -6.5, y, VALUE, color, bold=True, font=self.title_font)), run_time=.25)
            row = [Dot(point(x, y), radius=.19, color=color, fill_opacity=int(active),
                       stroke_color=color, stroke_opacity=1, stroke_width=2)
                   for x, active in zip(xs, work, strict=True)]
            self.play(*(FadeIn(dot, scale=.5) for dot in row), run_time=.55)
            markers.append(row)
        key = label("Filled = runs     Outline = reuses", -6.5, -3.02, LABEL, MUTED, font=self.title_font)
        self.play(FadeIn(key), run_time=.4)
        self.wait(1.5)
        self.title("When the mounted group changes")
        self.play(*(dot.animate.set_fill(opacity=1) for row in (markers[0], markers[2])
                    for dot in row[:2]), run_time=.75)
        self.foot("The view and layout run again when iced or Rune mounts new cards.")
        self.wait(1.5)

    @classmethod
    def strip(cls, y, color):
        return VGroup(*(cls.tile(x, y, color, index) for index, x in enumerate((-2.7, -.1, 2.5))))

    @staticmethod
    def tile(x, y, color, index):
        shell = RoundedRectangle(width=1.95, height=.95, corner_radius=.12,
                                 stroke_color=color, stroke_width=2.5, fill_color=color,
                                 fill_opacity=.18 if y < 0 else .04).move_to(point(x, y))
        glyphs = VGroup()
        for i in range(1 + index % 4):
            glyphs.add(Dot(point(x + .23 * (i - (index % 4) / 2), y),
                           radius=.075, color=color))
        return VGroup(shell, glyphs)
