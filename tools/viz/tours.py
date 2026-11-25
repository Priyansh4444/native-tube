"""Compose the shared-clock tour and a slowed replay, entirely from saved captures."""
import bisect
from dataclasses import dataclass
import json
import math

import subprocess

import av

import data
import normalized

WORK = data.OUT.parent / "compose"
START_LEAD = 1.35
REPLAY_FACTOR = 4
REPLAY_SECONDS = 1
FPS = 60
W, H = 1920, 1080
TILE_W, TILE_H = 688, 430
X0, Y0 = 251, 118
GAP_X, GAP_Y = 42, 58
BG = "10151F"
DARK = "1B2535"
INK = "F4F4EE"
MUTED = "ACB9CB"
MINT = "82ECC3"


@dataclass
class Tour:
    app: str
    video: str
    length: float
    wheel: float
    opened: float
    visible_frame: float | None
    stages: list[tuple[float, str]]
    samples: list[tuple[float, float, float]]
    memory_label: str
    sources: list[str]
    clock: str

    def cursor(self, replay, fly_cut):
        return self.opened if replay or fly_cut else self.wheel

    def start(self, replay, fly_cut):
        return self.cursor(replay, fly_cut) - (0 if replay else 7 if fly_cut else START_LEAD)


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
    backs = [t for t, cmd in commands if cmd == "back"]
    search = next(t for t, name in marks if name == "search_loaded")
    played = next(t for t, name in marks if name == "playback_started" and t >= opens[0])
    switched = next(t for t, name in marks if name == "playback_started" and t >= opens[1])
    visual = clip_onsets(video, *opens)
    stages = [(0, "HOME  ✓"), (wheels[-1], "SCROLL  ✓"), (search, "SEARCH  ✓"),
              (visual if visual is not None else played, "PLAY  ✓"), (switched, "RELATED  ✓"),
              (backs[0], "BACK 1 SENT"), (backs[-1], "BACK 2 SENT")]
    samples = [
        (at - offset, (cpu - before_cpu) * 100 / (at - before_at), rss)
        for (before_at, before_cpu, _), (at, cpu, rss) in zip(raw, raw[4:])
        if at > before_at and cpu >= before_cpu
    ]
    return Tour(app, video, length, wheels[0], opens[0], visual, stages, samples,
                "RSS", entry["sources"], "end-aligned estimate")


def normalize_web(entry):
    video = entry["video"]
    length = duration(video)
    wheel = entry["wheel_s"]
    if not 0 <= wheel < entry["open_s"] < length:
        raise ValueError("Invalid clock for reviewed web capture")
    return Tour("youtube", video, length, wheel, entry["open_s"], entry.get("visible_frame_s"),
                entry["stages"], entry.get("samples", []),
                entry.get("memory_label", "PSS"), entry["sources"], entry.get("clock", "reviewed capture"))


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

    def text(self, content, x, y, start, end, *, size=22, color=INK, align=4, mono=False, bold=False):
        font = r"\fnDejaVu Sans Mono" if mono else ""
        weight = r"\b1" if bold else ""
        safe = str(content).replace("\\", "/").replace("{", "(").replace("}", ")").replace("\n", r"\N")
        self.event(start, end, fr"\an{align}\pos({x},{y})\fs{size}\1c{rgb(color)}{font}{weight}", safe)

    def rect(self, x, y, w, h, start, end, color, *, layer=0):
        self.event(start, end, fr"\an7\pos({x},{y})\p1\1c{rgb(color)}",
                   f"m 0 0 l {w} 0 {w} {h} 0 {h}", layer=layer)

    def write(self, path):
        path.write_text(f"""[Script Info]
ScriptType: v4.00+
PlayResX: {W}
PlayResY: {H}
WrapStyle: 2

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Base,DejaVu Sans,22,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,4,0,0,0,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
""" + "\n".join(self.events) + "\n")


def tiles():
    for i, app in enumerate(data.ALL_APPS):
        yield app, X0 + (i % 2) * (TILE_W + GAP_X), Y0 + (i // 2) * (TILE_H + GAP_Y)


def annotate_tile(overlay, tour, x, y, total, replay, fly_cut):
    clock_rate = REPLAY_FACTOR if replay else 1
    start = tour.start(replay, fly_cut)
    times = [t for t, _, _ in tour.samples]
    stages = [t for t, _ in tour.stages]
    color = data.APP_COLORS[tour.app]
    overlay.rect(x, y + TILE_H, TILE_W, 29, 0, total, DARK)
    cursor = tour.cursor(replay, fly_cut)
    first = tour.visible_frame
    if first is not None and first >= start:
        flash = (first - start) * clock_rate
        for bx, by, bw, bh in (
            (x, y, TILE_W, 4), (x, y + TILE_H - 4, TILE_W, 4),
            (x, y, 4, TILE_H), (x + TILE_W - 4, y, 4, TILE_H),
        ):
            overlay.rect(bx, by, bw, bh, flash, min(flash + .45, total), MINT, layer=2)
    for tick in range(math.ceil(total * 30)):
        at = tick / 30
        end = min((tick + 1) / 30, total)
        original = min(tour.length, start + at / clock_rate)
        elapsed = original - cursor
        timer = f"{'−' if elapsed < 0 else '+'}{abs(elapsed):05.2f} s"
        overlay.text(timer, x + TILE_W, y - 17, at, end, size=26, color=color, mono=True, align=6)
        stage_index = bisect.bisect_right(stages, original) - 1
        status = tour.stages[stage_index][1] if stage_index >= 0 else "READY"
        if (replay and first is not None and original >= first) or (first is not None and first <= original < first + .65):
            status = "FIRST CONTENT FRAME" if tour.app == "youtube" else "FIRST VISIBLE FRAME"
        elif original >= tour.length - .09:
            status = "TOUR DONE"
        elif first is not None and tour.opened <= original < first:
            status = "OPEN SENT"
        elif original < tour.wheel and not replay and tour.app != "youtube":
            status = "HOME  ✓"
        overlay.text(status, x + TILE_W - 162, y - 17, at, end, size=19, color=MINT, align=6, bold=True)
        i = bisect.bisect_right(times, original) - 1
        if i < 0 or original - times[i] > 1:
            overlay.text("CPU / memory samples unavailable", x + 12, y + TILE_H + 14, at, end, size=16, color=MUTED)
            continue
        _, cpu, memory = tour.samples[i]
        meter_y = y + TILE_H + 12
        # One-core percent becomes cores busy out of the machine's logical CPUs, so no value exceeds its
        # scale. The laptop count comes from the normalized receipt; until then the label is a placeholder.
        cores = normalized.value("fly_logical_cpus" if fly_cut else "laptop_logical_cpus")
        if cores:
            cpu_label, cpu_frac = f"CPU {cpu / 100:4.2f}/{cores:.0f} cores", cpu / (100 * cores)
        else:
            cpu_label, cpu_frac = "CPU pending normalization", 0
        overlay.text(cpu_label, x + 10, meter_y + 2, at, end, size=17, mono=True)
        overlay.text(f"{tour.memory_label} {memory:.0f} MiB", x + 370, meter_y + 2, at, end, size=17, mono=True)
        meters = ((x + 510, memory / 512, MINT),) if not cores else ((x + 193, cpu_frac, color), (x + 510, memory / 512, MINT))
        for left, frac, fill in meters:
            overlay.rect(left, meter_y, 150, 4, at, end, "344256")
            if frac > 0:
                overlay.rect(left, meter_y, min(150, round(150 * frac)), 4, at, end, fill, layer=1)


def variant_name(replay, fly_cut):
    return "SideBySideFly" if fly_cut else "FirstSecondReplay" if replay else "SideBySide"


def render(tours, *, replay, platform, web_reason, encode=True):
    fly_cut = platform == "Fly VM"
    name = variant_name(replay, fly_cut)
    total = REPLAY_SECONDS * REPLAY_FACTOR if replay else math.ceil(
        max(t.length - t.start(replay, fly_cut) for t in tours.values())
    )
    if not encode:
        return name, total
    overlay = Overlay()
    title = "First second after open" if replay else "Same tour, side by side"
    subtitle = (f"{platform}  /  {REPLAY_FACTOR}× slow motion  /  aligned to each open event" if replay
                else f"{platform}  /  aligned to first scroll command  /  local fixture clip in native apps")
    if fly_cut:
        subtitle = "Fly VM  /  aligned to the cold open  /  native fixture clip and live YouTube"
    overlay.text(title, X0, 31, 0, total, size=38, bold=True)
    overlay.text(subtitle, X0, 68, 0, total, size=20, color=MUTED)
    logical = normalized.value("fly_logical_cpus" if fly_cut else "laptop_logical_cpus")
    cpu_note = f"CPU = sampled cores busy of {logical:.0f} logical CPUs." if logical else "CPU pending normalization."
    footer = (f"Clock zero: native open command; YouTube navigation start. {cpu_note} Browser clicked Skip on one ad in this recording."
              if fly_cut else f"Native capture clocks aligned approximately from report end. {cpu_note} "
              "RSS is resident memory.")
    overlay.text(footer, X0, 1067, 0, total, size=17, color=MUTED)
    args = [
        "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-filter_complex_threads", "2",
        "-f", "lavfi", "-i", f"color=0x{BG}:s={W}x{H}:r={FPS}:d={total}",
    ]
    filter_graph = []
    current = "0:v"
    for app, x, y in tiles():
        overlay.text(data.NAMES[app], x, y - 17, 0, total, size=28, color=data.APP_COLORS[app], bold=True)
        if app not in tours:
            overlay.rect(x, y, TILE_W, TILE_H, 0, total, DARK)
            msg = web_reason if app == "youtube" else "Tour unavailable"
            overlay.text(msg, x + TILE_W // 2, y + TILE_H // 2, 0, total, size=25, color=MUTED, align=5)
            continue
        tour = tours[app]
        i = 1 + len(filter_graph) // 2
        args.extend(["-threads", "2", "-ss", f"{max(0, tour.start(replay, fly_cut)):.6f}", "-i", tour.video])
        speed = REPLAY_FACTOR if replay else 1
        filter_graph.append(
            f"[{i}:v]setpts={speed}*(PTS-STARTPTS),fps={FPS},"
            f"scale={TILE_W}:{TILE_H}:force_original_aspect_ratio=decrease,"
            f"pad={TILE_W}:{TILE_H}:(ow-iw)/2:(oh-ih)/2:color=0x{BG},setsar=1[v{i}]"
        )
        filter_graph.append(f"[{current}][v{i}]overlay={x}:{y}:eof_action=repeat[m{i}]")
        current = f"m{i}"
        annotate_tile(overlay, tour, x, y, total, replay, fly_cut)
    ass = WORK / f"{name}.ass"
    overlay.write(ass)
    filter_graph.append(f"[{current}]ass={ass.relative_to(data.ROOT)}[out]")
    (WORK / f"{name}.filters").write_text(";\n".join(filter_graph))
    output = data.OUT / f"{name}.mp4"
    subprocess.run([*args, "-filter_complex", ";".join(filter_graph), "-map", "[out]", "-t", str(total),
                    "-an", "-c:v", "libx264", "-threads", "2", "-preset", "veryfast", "-crf", "19",
                    "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(output)], check=True)
    return name, total


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
        name, total = render(captures, replay=replay, platform=platform, web_reason=reason,
                             encode=wanted is None or variant_name(replay, fly_cut) in wanted)
        receipts[name] = {
            "platform": platform, "duration_s": total,
            "slow_factor": REPLAY_FACTOR if replay else 1, "web_status": status,
            "tiles": {
                app: {
                    "sources": tour.sources, "video_start_s": tour.start(replay, fly_cut),
                    "first_scroll_s": tour.wheel, "report_open_s": tour.opened,
                    "first_visible_frame_s": tour.visible_frame, "stages": tour.stages,
                    "alignment": tour.clock,
                } for app, tour in captures.items()
            },
        }
    (WORK / "tour-alignment.json").write_text(json.dumps(receipts, indent=2) + "\n")
    return receipts
