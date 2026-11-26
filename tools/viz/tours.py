"""Compose the shared-clock tour and a slowed replay, entirely from saved captures."""
from dataclasses import dataclass
import json
import math

import subprocess

import av

import data

WORK = data.OUT.parent / "compose"
REPLAY_FACTOR = 4
FPS = 60
W, H = 1920, 1080
TILE_W, TILE_H = 948, 533   # captures are cropped from 16:10 to 16:9 at the bottom
X0, Y0 = 8, 4
GAP_X, GAP_Y = 8, 6
BG = "000000"
DARK = "121212"
INK = "FFFFFF"
MUTED = "999999"
RIM = "2A2A2A"
MIN_FS = 34  # libass \fs is cell height; Roboto em ≈ fs / 1.2. Labels use 40+ (≥ 32 px em); 34 is the footnote.


@dataclass
class Tour:
    app: str
    video: str
    opened: float
    visible_frame: float | None
    sources: list[str]
    clock: str
    steps: dict  # step → (command time, mark time) on the capture clock; mark None if untimed


def lap_entries():
    entries = {}
    for app in data.APPS:
        directory = f"{data.MEDIA}/tours/{app}"
        video, report = f"{directory}/{app}.mp4", f"{directory}/report.json"
        entries[app] = {
            "video": video, "report": report, "scenario": "tools/media/tour.json",
            "sources": [video, report, f"{directory}/capture.json", "tools/media/tour.json",
                        "tools/media/recorder.py", "core/src/metrics.rs", "core/src/autopilot.rs"],
        }
    return entries


def duration(path):
    result = subprocess.check_output(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "json", path],
        text=True,
    )
    return float(json.loads(result)["format"]["duration"])


def clip_onsets(path, first_open, second_open):
    """Find the first fixture color bars recorded by Grim, independently of the player mark."""
    coords = [19, 37, 53, 70, 87, 102]  # Sampling the 1280×800 Watch rectangle at 1/8 scale.
    expected = ([255, 23, 0], [0, 215, 0], [255, 238, 0],
                [0, 13, 255], [255, 38, 255], [0, 230, 255])
    with av.open(path) as capture:
        capture.streams.video[0].codec_context.thread_count = 2
        for frame in capture.decode(video=0):
            t = float(frame.pts * frame.time_base)
            if t < first_open - .14 or t > second_open - .3:
                continue
            pixels = frame.reformat(width=160, height=100, format="rgb24").to_ndarray()[20, coords]
            matches = [max(abs(int(a) - b) for a, b in zip(pixel, color)) < 75
                       for pixel, color in zip(pixels, expected)]
            if sum(matches) >= 5:
                return t
    return None


def native(app, entry):
    video, report_path = entry["video"], entry["report"]
    report = data.read(report_path)
    if report.get("failed_waits"):
        raise ValueError(f"Incomplete tour: {report_path}")
    length = duration(video)
    raw = report["samples"]
    commands = [(t / 1000, json.loads(value)["cmd"]) for t, value in report["commands"]]
    scenario = data.read(entry.get("scenario", "tools/media/tour.json"))
    # The final scripted sleep ends at report creation, at which point the recorder
    # stops. Its last frame can precede this endpoint by a capture slot or more.
    last_step = scenario[-1]
    end = commands[-1][0] + last_step["sleep"] if "sleep" in last_step else raw[-1][0]
    offset = entry.get("offset_s", end - length)
    commands = [(t - offset, name) for t, name in commands]
    marks = [(t / 1000 - offset, name) for t, name in report["all_marks"]]
    wheels = [t for t, cmd in commands if cmd == "scroll"]
    opens = [t for t, cmd in commands if cmd == "open_video"]
    search = next(t for t, name in marks if name == "search_loaded")
    played = next(t for t, name in marks if name == "playback_started" and t >= opens[0])
    switched = next(t for t, name in marks if name == "playback_started" and t >= opens[1])
    visual = clip_onsets(video, *opens)
    searches = [t for t, cmd in commands if cmd == "search"]
    steps = {"scroll": (wheels[0], None, wheels[-1]), "search": (searches[0], search, None),
             "open": (opens[0], visual if visual is not None else played, None),
             "related": (opens[1], switched, None)}
    return Tour(app, video, opens[0], visual, entry["sources"], "end-aligned estimate", steps)


def normalize_web(entry):
    video = entry["video"]
    length = duration(video)
    wheel = entry["wheel_s"]
    if not 0 <= wheel < entry["open_s"] < length:
        raise ValueError("Invalid clock for reviewed web capture")
    cues = dict((name, at) for at, name in entry["stages"])
    # Only the cold open has a click time in the browser receipt; other steps are visual cues, not timed.
    steps = {"scroll": (wheel, None, wheel), "search": (None, cues.get("SEARCH SHOWN"), None),
             "open": (entry["open_s"], entry.get("visible_frame_s"), None),
             "related": (None, cues.get("WARM PRE-ROLL"), None)}
    return Tour("youtube", video, entry["open_s"], entry.get("visible_frame_s"),
                entry["sources"], entry.get("clock", "reviewed capture"), steps)


def rgb(color):
    value = color.lstrip("#")
    return "&H" + value[4:6] + value[2:4] + value[0:2] + "&"


def ass_time(t):
    centi = round(max(t, 0) * 100)
    return f"{centi // 360000}:{centi // 6000 % 60:02d}:{centi // 100 % 60:02d}.{centi % 100:02d}"


class Overlay:
    def __init__(self):
        self.events = []

    def event(self, start, end, tag, content, layer=1):
        if ass_time(start) != ass_time(end):
            self.events.append(f"Dialogue: {layer},{ass_time(start)},{ass_time(end)},Base,,0,0,0,,{{{tag}}}{content}")

    def text(self, content, x, y, start, end, *, size=MIN_FS, color=INK, align=4, mono=False, bold=False, layer=1):
        if size < MIN_FS:
            raise ValueError(f"Overlay text below {MIN_FS}: {content!r}")
        font = r"\fnRoboto" if mono else ""
        weight = r"\b1" if bold else ""
        safe = str(content).replace("\\", "/").replace("{", "(").replace("}", ")").replace("\n", r"\N")
        self.event(start, end, fr"\an{align}\pos({x},{y})\fs{size}\1c{rgb(color)}{font}{weight}", safe, layer=layer)

    def rect(self, x, y, w, h, start, end, color, *, layer=0, alpha=0):
        fade = fr"\1a&H{alpha:02X}&" if alpha else ""
        self.event(start, end, fr"\an7\pos({x},{y})\p1\1c{rgb(color)}{fade}",
                   f"m 0 0 l {w} 0 {w} {h} 0 {h}", layer=layer)

    def rounded(self, x, y, w, h, start, end, color, *, layer=0, outline=False, alpha=0, rim=None, width=3):
        r = 12
        path = (f"m {r} 0 l {w-r} 0 b {w-5} 0 {w} 5 {w} {r} l {w} {h-r} "
                f"b {w} {h-5} {w-5} {h} {w-r} {h} l {r} {h} b 5 {h} 0 {h-5} 0 {h-r} "
                f"l 0 {r} b 0 5 5 0 {r} 0")
        if outline:
            style = fr"\bord{width}\3c{rgb(color)}\1a&HFF&"
        elif rim:
            style = fr"\bord1\3c{rgb(rim)}" + (fr"\1a&H{alpha:02X}&" if alpha else "")
        else:
            style = fr"\1a&H{alpha:02X}&" if alpha else ""
        self.event(start, end, fr"\an7\pos({x},{y})\p1\1c{rgb(color)}{style}", path, layer=layer)

    def glass(self, x, y, w, h, start, end, *, layer=0, color=DARK, alpha=0):
        """Obsidian glass card: surface, rim at 8% white, and a faint top sheen line."""
        self.rounded(x, y, w, h, start, end, color, layer=layer, rim=RIM, alpha=alpha)
        self.rect(x + 12, y + 1, w - 24, 1, start, end, "2E2E2E", layer=layer)

    def write(self, path):
        path.write_text(f"""[Script Info]
ScriptType: v4.00+
PlayResX: {W}
PlayResY: {H}
WrapStyle: 2

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Base,Roboto,34,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,4,0,0,0,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
""" + "\n".join(self.events) + "\n")


def tiles():
    for i, app in enumerate(data.ALL_APPS):
        yield app, X0 + (i % 2) * (TILE_W + GAP_X), Y0 + (i // 2) * (TILE_H + GAP_Y)


STEP_NAMES = {"scroll": "Wheel scroll", "search": "Search", "open": "Click → first frame", "related": "Related video"}
LEAD, TAIL, OPEN_TAIL = .6, 1.6, 2.8


def segments(tours, replay):
    """The cut list: each step aligns every tile on its own command, so the timers race from one click."""
    if replay:
        return [("open", .15, 1.0)]
    cut = []
    for step in ("scroll", "search", "open", "related"):
        spans = []
        for tour in tours.values():
            cmd, mark, until = tour.steps[step]
            if cmd is not None and mark is not None:
                spans.append(mark - cmd)
            elif until is not None and cmd is not None:
                spans.append(until - cmd)
        if spans:
            cut.append((step, LEAD, max(spans) + (OPEN_TAIL if step == "open" else TAIL)))
    return cut


def frozen_values(tours, cut):
    """step → app → seconds from that app's own command to its mark, for every timed tile."""
    result = {}
    for step, _, _ in cut:
        for app, tour in tours.items():
            cmd, mark, _ = tour.steps[step]
            if app != "youtube" and step != "scroll" and cmd is not None and mark is not None:
                result.setdefault(step, {})[app] = mark - cmd
    return result


def anchor(tour, step):
    """Source time that lines up with the step's command on screen."""
    cmd, mark, _ = tour.steps[step]
    if cmd is not None:
        return cmd
    return (mark - .3) if mark is not None else tour.opened


def render(tours, *, replay, platform, web_reason, encode=True):
    fly_cut = platform == "Fly VM"
    name = variant_name(replay, fly_cut)
    speed = REPLAY_FACTOR if replay else 1
    cut = segments(tours, replay)
    lengths = [(lead + span) * speed for _, lead, span in cut]
    total = sum(lengths)
    frozen = frozen_values(tours, cut)
    if not encode:
        return name, total, frozen
    overlay = Overlay()
    args = ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-filter_complex_threads", "2",
            "-f", "lavfi", "-i", f"color=0x{BG}:s={W}x{H}:r={FPS}:d={total}"]
    graph, current, index = [], "0:v", 0
    for app, x, y in tiles():
        if app not in tours:
            summary_panel(overlay, cut, frozen, lengths, x, y, replay, web_reason if fly_cut else None)
            continue
        tour = tours[app]
        index += 1
        args += ["-threads", "2", "-i", tour.video]
        parts = []
        for k, ((step, lead, span), length) in enumerate(zip(cut, lengths)):
            start = max(0, anchor(tour, step) - lead)
            graph.append(f"[{index}:v]trim=start={start:.4f}:end={start + lead + span + .5:.4f},setpts={speed}*(PTS-STARTPTS),"
                         f"fps={FPS},tpad=stop_mode=clone:stop_duration={length:.3f},trim=duration={length:.4f},"
                         f"setpts=PTS-STARTPTS,crop=iw:iw*9/16:0:0,scale={TILE_W}:{TILE_H},setsar=1[s{index}_{k}]")
            parts.append(f"[s{index}_{k}]")
        graph.append(f"{''.join(parts)}concat=n={len(parts)}:v=1:a=0[t{index}]")
        graph.append(f"[{current}][t{index}]overlay={x}:{y}:eof_action=repeat[m{index}]")
        current = f"m{index}"
        annotate_tile(overlay, tour, x, y, cut, lengths, frozen, speed)
    note = "Timers start at each app's own command • clocks aligned approximately • open stops at the first visible frame"
    if fly_cut:
        note = "Native timers start at each app's own command • approximate clock alignment • YouTube web: live"
    # One small footnote, on the seam between the two rows of tiles.
    seam = Y0 + TILE_H + GAP_Y // 2
    overlay.rounded(W // 2 - 800, seam - 22, 1600, 44, 0, total, BG, layer=5, alpha=0x30, rim=RIM)
    overlay.text(note, W // 2, seam, 0, total, size=MIN_FS, color=MUTED, align=5, layer=6)
    ass = WORK / f"{name}.ass"
    overlay.write(ass)
    graph.append(f"[{current}]ass={ass.relative_to(data.ROOT)}:fontsdir=assets/fonts,"
                 "scale=1856:1044,pad=1920:1080:32:18:black[out]")
    (WORK / f"{name}.filters").write_text(";\n".join(graph))
    body = WORK / f"{name}-body.mp4"
    subprocess.run(["nice", "-n", "19", *args, "-filter_complex", ";".join(graph), "-map", "[out]",
                    "-t", f"{total:.4f}", "-an", "-c:v", "libx264", "-threads", "4", "-preset", "veryfast",
                    "-profile:v", "high", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(body)],
                   check=True)
    import obsidian  # The bookend cards live with the other Obsidian artwork.
    obsidian.bookend_tour(name, body, frozen, fly_cut=fly_cut, replay=replay, web="youtube" in tours)
    return name, total, frozen


def frosted(overlay, x, y, w, h, start, end, layer=2):
    overlay.rounded(x, y, w, h, start, end, BG, layer=layer, alpha=0x48, rim=RIM)


def annotate_tile(overlay, tour, x, y, cut, lengths, frozen, speed):
    """Overlays stay minimal: the app name, a big running timer, and the step label."""
    color = data.APP_COLORS[tour.app]
    total = sum(lengths)
    top = y + (14 if y == Y0 else 32)  # clear of the footnote on the seam
    frosted(overlay, x + 16, top, 260, 64, 0, total)
    overlay.rect(x + 30, top + 54, 232, 4, 0, total, color, layer=3)
    overlay.text(data.NAMES[tour.app], x + 32, top + 30, 0, total, size=46, bold=True, layer=3)
    at = 0.0
    for (step, lead, span), length in zip(cut, lengths):
        cmd, mark, _ = tour.steps[step]
        live = tour.app == "youtube"
        words = STEP_NAMES[step] + ("  •  live, not raced" if live else "")
        width = 30 + 23 * len(words)
        bottom = y + TILE_H - (90 if y == Y0 else 74)
        frosted(overlay, x + 16, bottom, width, 58, at, at + length)
        overlay.text(words, x + 34, bottom + 29, at, at + length, size=40, bold=True, layer=3)
        value = frozen.get(step, {}).get(tour.app)
        if value is not None:
            src0 = anchor(tour, step) - lead
            stop = at + (lead + value) * speed
            frosted(overlay, x + TILE_W - 330, top, 314, 92, at, min(stop, at + length))
            ticks = math.ceil(length * 30)
            for tick in range(ticks):
                t0, t1 = at + tick / 30, min(at + (tick + 1) / 30, at + length)
                if t0 >= stop:
                    break
                elapsed = max(0.0, src0 + (t0 - at) / speed - cmd)
                overlay.text(f"{elapsed * 1000:,.0f} ms", x + TILE_W - 36, top + 46, t0, min(t1, stop),
                             size=72, color=color, mono=True, bold=True, align=6, layer=3)
            if stop < at + length:
                # Frozen and highlighted: the pill turns the app's colour and the tile rim flashes white.
                overlay.rounded(x + TILE_W - 330, top, 314, 92, stop, at + length, color, layer=2)
                overlay.text(f"{value * 1000:,.0f} ms", x + TILE_W - 36, top + 46, stop, at + length, size=72,
                             color=BG, mono=True, bold=True, align=6, layer=3)
                overlay.rounded(x, y, TILE_W, TILE_H, stop, min(stop + .45, at + length), INK, layer=4,
                                outline=True, width=6)
        at += length


def summary_panel(overlay, cut, frozen, lengths, x, y, replay, web_reason):
    """The empty quadrant: the step in big type and each app's stopped time as it lands."""
    at = 0.0
    for (step, lead, span), length in zip(cut, lengths):
        speed = REPLAY_FACTOR if replay else 1
        overlay.text(STEP_NAMES[step], x + 40, y + 70, at, at + length, size=64, bold=True)
        values = frozen.get(step, {})
        if not values:
            overlay.text("no stop mark for this step", x + 40, y + 140, at, at + length, size=40, color=MUTED)
        best = min(values.values()) if values else None
        for i, app in enumerate(sorted(values, key=values.get)):
            land = at + (lead + values[app]) * speed
            if land >= at + length:
                continue
            gap = round(values[app] * 1000) - round(best * 1000)
            row = y + 170 + i * 96
            overlay.text(data.NAMES[app], x + 40, row, land, at + length, size=56, color=data.APP_COLORS[app], bold=True)
            overlay.text(f"{values[app] * 1000:,.0f} ms", x + 470, row, land, at + length, size=56,
                         color=data.APP_COLORS[app], mono=True, bold=True, align=6)
            if gap > 0:
                frosted(overlay, x + 500, row - 30, 220, 60, land, at + length)
                overlay.text(f"+{gap} ms", x + 610, row, land, at + length, size=44, bold=True, align=5, layer=3)
        at += length
    if web_reason:
        overlay.text(web_reason, x + 40, y + TILE_H - 34, 0, at, size=MIN_FS, color=MUTED)


def variant_name(replay, fly_cut):
    return "SideBySideFly" if fly_cut else "FirstSecondReplay" if replay else "SideBySide"


def first_frame_clip():
    """Show one archived video open per app at original speed."""
    WORK.mkdir(parents=True, exist_ok=True)
    lead, seconds = .75, 4.25
    panel_w, panel_h, top = 568, 356, 432
    overlay = Overlay()
    overlay.text("Opening a video", 84, 222, 0, seconds, size=78, bold=True)
    overlay.text("Laptop footage recorded before the benchmark rerun",
                 84, 304, 0, seconds, size=36, color=MUTED)
    args = ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-filter_complex_threads", "2",
            "-f", "lavfi", "-i", f"color=black:s={W}x{H}:r={FPS}:d={seconds}"]
    graph, current, sources = [], "0:v", {}
    for i, (app, entry) in enumerate(lap_entries().items(), 1):
        tour = native(app, entry)
        if tour.visible_frame is None or tour.visible_frame <= tour.opened:
            raise ValueError(f"{app} recording lacks a visible first frame after the open command")
        x = 84 + (i - 1) * 592
        args += ["-threads", "2", "-ss", f"{tour.opened - lead:.6f}", "-i", tour.video]
        graph.append(f"[{i}:v]fps={FPS},scale={panel_w}:{panel_h},setsar=1[v{i}]")
        graph.append(f"[{current}][v{i}]overlay={x}:{top}[m{i}]")
        current = f"m{i}"
        overlay.text(data.NAMES[app], x, top - 44, 0, seconds, size=54, bold=True)
        sources[f"{app} archived capture and edit alignment"] = tour.sources
    ass = WORK / "FirstFrameRealTime.ass"
    overlay.write(ass)
    graph.append(f"[{current}]ass={ass.relative_to(data.ROOT)}:fontsdir=assets/fonts[out]")
    subprocess.run(["nice", "-n", "19", *args, "-filter_complex", ";".join(graph), "-map", "[out]",
                    "-t", str(seconds), "-an", "-c:v", "libx264", "-threads", "2", "-preset", "veryfast",
                    "-profile:v", "high", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
                    str(data.OUT / "FirstFrameRealTime.mp4")], check=True)
    return sources


def main(fly, wanted=None):
    WORK.mkdir(parents=True, exist_ok=True)
    variants = [("Laptop", lap_entries(), False), ("Laptop", lap_entries(), True)]
    if "tours" in fly:
        variants.append(("Fly VM", fly["tours"], False))
    receipts = {}
    for platform, entries, replay in variants:
        fly_cut = platform == "Fly VM"
        captures = {app: native(app, entries[app]) for app in data.APPS}
        if "youtube" in entries:
            captures["youtube"] = normalize_web(entries["youtube"])
        status = fly.get("web_status", {}) if fly_cut else {}
        reason = f'Browser run blocked • {status["reason"]}' if status else "No comparable browser recording"
        name, total, frozen = render(captures, replay=replay, platform=platform, web_reason=reason,
                                     encode=wanted is None or variant_name(replay, fly_cut) in wanted)
        receipts[name] = {
            "platform": platform, "duration_s": total, "bookends_s": [2.0, 2.5],
            "steps": frozen, "slow_factor": REPLAY_FACTOR if replay else 1, "web_status": status,
            "tiles": {
                app: {
                    "sources": tour.sources, "steps": tour.steps, "report_open_s": tour.opened,
                    "first_visible_frame_s": tour.visible_frame, "alignment": tour.clock,
                } for app, tour in captures.items()
            },
        }
    (WORK / "tour-alignment.json").write_text(json.dumps(receipts, indent=2) + "\n")
    return receipts
