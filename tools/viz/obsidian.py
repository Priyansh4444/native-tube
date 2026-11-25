"""Obsidian publication artwork: bookend cards, the cover and the NativeTube demo, from archived footage."""
from functools import lru_cache
import json
import subprocess

from PIL import Image, ImageDraw, ImageFilter, ImageFont

import data
import tours

WORK = data.OUT.parent / "obsidian"
FONT = data.ROOT / "assets/fonts"
CAPTION = ".cache/iso-test/out/live-KTJdkAE8/rune/caption-shown.png"
ARTICLE = "docs/blog/x-article.md"
WHITE = "#ffffff"
SECONDARY = "#999999"
RING = "#3d3d3d"
MINT = "#4FE3B0"
FPS = 60
SIZE = (1920, 1080)
WORDMARK = "NativeTube · iced vs GPUI vs Rune"
QUESTIONS = {
    "SideBySide": "Same clicks, three native apps: who gets there first?",
    "SideBySideFly": "The same clicks on a GPU-less Fly VM, next to YouTube web",
    "FirstSecondReplay": "Click to first frame, slowed down four times",
}
STEPS = {"search": "Search", "open": "First frame", "related": "Related"}


@lru_cache(maxsize=None)
def face(size, bold=False):
    return ImageFont.truetype(str(FONT / ("Roboto-Bold.ttf" if bold else "Roboto-Regular.ttf")), size)


def ease_out(t):
    t = min(max(t, 0), 1)
    return 1 - (1 - t) ** 3


def ease_back(t):
    t = min(max(t, 0), 1)
    c = 1.70158
    return 1 + (c + 1) * (t - 1) ** 3 + c * (t - 1) ** 2


def glass(image, rect, fill=18):
    """#121212 surface, 175° sheen (white 5.5% fading by 65%), 1 px rim at 8% plus a 6% top line."""
    x, y, right, bottom = (round(v) for v in rect)
    w, h = right - x, bottom - y
    if w <= 2 or h <= 2:
        return
    tile = Image.new("RGB", (w, h), (fill,) * 3)
    draw = ImageDraw.Draw(tile)
    for row in range(round(h * .65)):
        tone = round(fill + 13 * (1 - row / (h * .65)))
        draw.line((0, row, w, row), fill=(tone,) * 3)
    mask = Image.new("L", tile.size)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, w - 1, h - 1), radius=12, fill=255)
    image.paste(tile, (x, y), mask)
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((x, y, right - 1, bottom - 1), radius=12, outline="#262626", width=1)
    draw.line((x + 13, y + 1, right - 14, y + 1), fill="#333333", width=1)


def glow_text(image, xy, text, font, color, anchor="ls", strength=.45):
    """Accent text with a soft glow against the black."""
    x, y = xy
    left, top, right, bottom = font.getbbox(text, anchor=anchor)
    padding = 54
    layer = Image.new("RGBA", (right - left + 2 * padding, bottom - top + 2 * padding))
    ImageDraw.Draw(layer).text((padding - left, padding - top), text, font=font, fill=color, anchor=anchor)
    halo = layer.filter(ImageFilter.GaussianBlur(18))
    image.paste(halo, (x + left - padding, y + top - padding),
                mask=halo.getchannel("A").point(lambda a: round(a * strength)))
    ImageDraw.Draw(image).text(xy, text, font=font, fill=color, anchor=anchor)


def blend(image, alpha):
    if alpha >= 1:
        return image
    return Image.blend(Image.new("RGB", image.size, "black"), image, max(alpha, 0))


def encode_frames(output, seconds, draw):
    """Pipe PIL frames (draw(t) → Image) into an X-spec H.264 file."""
    process = subprocess.Popen([
        "nice", "-n", "19", "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y",
        "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{SIZE[0]}x{SIZE[1]}", "-r", str(FPS), "-i", "pipe:0",
        "-an", "-c:v", "libx264", "-threads", "2", "-preset", "veryfast", "-profile:v", "high",
        "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(output),
    ], stdin=subprocess.PIPE)
    cache = {}
    for frame in range(round(seconds * FPS)):
        image = draw(frame / FPS, cache)
        process.stdin.write(image.convert("RGB").tobytes())
    process.stdin.close()
    if process.wait():
        raise subprocess.CalledProcessError(process.returncode, "ffmpeg")


def rails(draw, x, y, progress, width=230):
    for i, app in enumerate(data.APPS):
        grow = ease_out((progress - i * .12) / .6)
        if grow > 0:
            draw.rounded_rectangle((x + i * (width + 18), y, x + i * (width + 18) + round(width * grow), y + 10),
                                   radius=5, fill=data.APP_COLORS[app])


def intro_frame(question, t, seconds=2.0):
    image = Image.new("RGB", SIZE, "black")
    draw = ImageDraw.Draw(image)
    draw.text((160, 250), WORDMARK, font=face(40, True), fill=SECONDARY)
    rise = ease_back((t - .2) / .55)
    words = question
    font = face(96, True)
    lines, line = [], ""
    for word in words.split():
        trial = f"{line} {word}".strip()
        if draw.textlength(trial, font=font) > 1580 and line:
            lines.append(line)
            line = word
        else:
            line = trial
    lines.append(line)
    for i, text in enumerate(lines):
        draw.text((160, 420 + i * 118 + round(40 * (1 - rise))), text, font=font, fill=WHITE)
    rails(draw, 160, 780, (t - .35) / .8)
    fade = min(1, (seconds - t) / .3)
    return blend(image, min(fade, ease_out(t / .3)))


def outro_frame(heading, rows, caveat, t, seconds=2.5, columns=None):
    """End card on black: heading, rows of counting values (accent glow), the fastest ringed, difference badges."""
    image = Image.new("RGB", SIZE, "black")
    draw = ImageDraw.Draw(image)
    draw.text((150, 165), WORDMARK, font=face(36, True), fill=SECONDARY)
    draw.text((150, 250), heading, font=face(74, True), fill=WHITE)
    count = ease_out((t - .15) / .85)
    xs = columns or (470, 790, 1110)
    if rows and rows[0].get("names"):
        for name, color, x in zip(rows[0]["names"], rows[0]["colors"], xs, strict=False):
            draw.text((x, 420), name, font=face(42, True), fill=color)
    for r, row in enumerate(rows):
        y = 565 + r * 118
        draw.text((150, y), row["label"], font=face(50, True), fill=WHITE, anchor="ls")
        best = row.get("best")
        for i, (value, fmt, color) in enumerate(row["values"]):
            x = xs[i]
            text = fmt.format(value * count)
            glow_text(image, (x, y + 8), text, face(80, True), color, anchor="ls")
            if best == i and t > .95:
                ring = ease_back((t - .95) / .4)
                width = ImageDraw.Draw(image).textlength(fmt.format(value), font=face(80, True))
                pad = 18 + round(10 * (1 - min(ring, 1)))
                ImageDraw.Draw(image).rounded_rectangle((x - pad, y - 76 - pad // 2, x + width + pad, y + 24),
                                                        radius=12, outline=RING, width=2)
        if row.get("badge") and t > 1.0:
            pop = ease_back((t - 1.0) / .4)
            font = face(38, True)
            text = row["badge"]
            w = ImageDraw.Draw(image).textlength(text, font=font) + 44
            x0 = 1830 - 60 - w
            glass(image, (x0, y - 60 - 8 * (1 - pop), x0 + w, y + 6 - 8 * (1 - pop)), fill=26)
            d = ImageDraw.Draw(image)
            d.rounded_rectangle((x0, y - 60, x0 + w, y + 6), radius=12, outline=RING, width=2)
            d.text((x0 + 22, y - 14), text, font=font, fill=WHITE, anchor="ls")
    draw = ImageDraw.Draw(image)
    if caveat:
        draw.text((150, 905), caveat, font=face(30), fill=SECONDARY)
    return blend(image, ease_out(t / .25))


def card_clip(output, seconds, frame):
    encode_frames(output, seconds, lambda t, _cache: frame(t))


def concat(parts, output):
    listing = output.with_suffix(".txt")
    listing.write_text("".join(f"file '{part}'\n" for part in parts))
    subprocess.run([
        "nice", "-n", "19", "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-xerror", "-y",
        "-f", "concat", "-safe", "0", "-i", str(listing), "-filter_threads", "2",
        "-vf", "setpts=N/(60*TB)", "-r", "60", "-fps_mode", "cfr", "-an", "-c:v", "libx264", "-threads", "2",
        "-preset", "veryfast", "-profile:v", "high", "-crf", "18", "-pix_fmt", "yuv420p",
        "-movflags", "+faststart", str(output),
    ], check=True)


def race_rows(frozen):
    """End-card rows from the same stopped timers the tiles show; native apps only."""
    rows = []
    for step, title in STEPS.items():
        values = {app: frozen.get(step, {}).get(app) for app in data.APPS}
        if any(v is None for v in values.values()):
            continue
        best = min(data.APPS, key=values.get)
        worst = max(data.APPS, key=values.get)
        gap = round(values[worst] * 1000) - round(values[best] * 1000)
        rows.append({"label": title, "best": data.APPS.index(best),
                     "values": [(values[app] * 1000, "{:,.0f} ms", data.APP_COLORS[app]) for app in data.APPS],
                     "names": [data.NAMES[app] for app in data.APPS],
                     "colors": [data.APP_COLORS[app] for app in data.APPS],
                     "badge": f"{data.NAMES[worst]} +{gap} ms"})
    return rows


def bookend_tour(name, body, frozen, *, fly_cut, replay, web):
    """Wrap a side-by-side body in the 2 s intro and the 2.5 s per-step end card."""
    WORK.mkdir(parents=True, exist_ok=True)
    intro, outro = WORK / f"{name}-intro.mp4", WORK / f"{name}-outro.mp4"
    card_clip(intro, 2.0, lambda t: intro_frame(QUESTIONS[name], t))
    rows = race_rows({"open": frozen.get("open", {})} if replay else frozen)
    machine = "Fly VM" if fly_cut else "laptop"
    heading = f"Command to finish, per step • {machine}"
    caveat = ("Archived tours • native fixtures • YouTube web ran live outside the race"
              if web else "Archived fixture tours • one run per app • approximate clock alignment")
    card_clip(outro, 2.5, lambda t: outro_frame(heading, rows, caveat, t))
    concat([intro, body, outro], data.OUT / f"{name}.mp4")


def bezier(p0, p1, p2, p3, steps=120):
    pts = []
    for i in range(steps + 1):
        t = i / steps
        a, b, c, d = (1 - t) ** 3, 3 * (1 - t) ** 2 * t, 3 * (1 - t) * t ** 2, t ** 3
        pts.append((a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0], a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1]))
    return pts


def glow_line(image, points, color, width, blur, strength):
    layer = Image.new("RGBA", image.size)
    ImageDraw.Draw(layer).line(points, fill=color, width=width, joint="curve")
    layer = layer.filter(ImageFilter.GaussianBlur(blur))
    image.paste(layer, mask=layer.getchannel("A").point(lambda a: round(min(255, a * strength))))


def strands(image, core, starts):
    """Three glowing app strands that converge into one shared core."""
    for (x, y), app in zip(starts, data.APPS, strict=True):
        color = data.APP_COLORS[app]
        pts = bezier((x, y), (x + 520, y), (core[0] - 420, core[1]), core)
        glow_line(image, pts, color, 44, 34, .7)
        glow_line(image, pts, color, 16, 8, 1.0)
        ImageDraw.Draw(image).line(pts, fill=color, width=7, joint="curve")
    tail = [core, (image.size[0] + 10, core[1])]
    glow_line(image, tail, MINT, 30, 24, .5)
    ImageDraw.Draw(image).line(tail, fill=MINT, width=6)
    for radius, blur, strength in ((120, 50, .55), (70, 22, .8), (38, 8, 1.0)):
        layer = Image.new("RGBA", image.size)
        ImageDraw.Draw(layer).ellipse((core[0] - radius, core[1] - radius, core[0] + radius, core[1] + radius),
                                      fill=MINT)
        layer = layer.filter(ImageFilter.GaussianBlur(blur))
        image.paste(layer, mask=layer.getchannel("A").point(lambda a: round(min(255, a * strength))))
    ImageDraw.Draw(image).ellipse((core[0] - 22, core[1] - 22, core[0] + 22, core[1] + 22), fill="#E9FFF7")


def cover():
    """Draw the article title and three app strands converging into one shared core."""
    title = (data.ROOT / ARTICLE).read_text().splitlines()[0].removeprefix("# ")
    head, suffix = title.split(": ", 1)
    image = Image.new("RGB", (1600, 900), "black")
    strands(image, (1180, 650), ((-20, 480), (-20, 650), (-20, 820)))
    draw = ImageDraw.Draw(image)
    font = face(64, True)
    size = 64
    while draw.textlength(head + ":", font=font) > 1460:
        size -= 2
        font = face(size, True)
    draw.text((70, 150), head + ":", font=font, fill=WHITE, anchor="ls")
    x = 70
    big = face(112, True)
    for i, (word, app) in enumerate(zip(suffix.split(" vs. "), data.APPS, strict=True)):
        glow_text(image, (x, 290), word, big, data.APP_COLORS[app], anchor="ls", strength=.6)
        x += round(draw.textlength(word, font=big)) + 26
        if i < 2:
            draw.text((x, 290), "vs.", font=face(64, True), fill=SECONDARY, anchor="ls")
            x += round(draw.textlength("vs.", font=face(64, True))) + 26
    ImageDraw.Draw(image).text((70, 375), "One YouTube client, built three ways.",
                               font=face(36), fill="#bdbdbd", anchor="ls")
    ImageDraw.Draw(image).text((1180, 785), "one shared Rust core", font=face(30, True), fill=SECONDARY, anchor="ms")
    image.save(data.OUT / "cover.png")
    return title


def encode(args, output, duration):
    subprocess.run([
        "nice", "-n", "19", "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-xerror", "-y",
        "-filter_complex_threads", "2", *args, "-t", str(duration), "-r", "60", "-an",
        "-c:v", "libx264", "-threads", "2", "-preset", "veryfast", "-profile:v", "high",
        "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(output),
    ], check=True)


def feature_clip(path, start, seconds, heading, detail, *, picture=False):
    """Leave the whole app visible and set the two labels outside the capture."""
    clip = WORK / f"{path.stem}-{start:g}s.mp4"
    overlay = tours.Overlay()
    overlay.text(heading, 95, 83, 0, seconds, size=54, bold=True)
    overlay.text(detail, 1825, 83, 0, seconds, size=36, color=tours.MUTED, align=6)
    ass = clip.with_suffix(".ass")
    overlay.write(ass)
    input_args = ["-loop", "1", "-framerate", "60"] if picture else ["-ss", str(start), "-threads", "2"]
    filter_graph = ("scale=1360:850:force_original_aspect_ratio=decrease:out_range=tv,"
                    "pad=1920:1080:(ow-iw)/2:142:color=black,setsar=1,"
                    f"ass={ass.relative_to(data.ROOT)}:fontsdir=assets/fonts")
    encode([*input_args, "-i", str(path), "-vf", filter_graph], clip, seconds)
    return clip


def demo():
    """A short product tour with one shot per feature."""
    WORK.mkdir(parents=True, exist_ok=True)
    laptop = tours.lap_entries()
    shots = [
        (data.ROOT / laptop["iced"]["video"], .8, 2.4, "Home", "iced"),
        (data.ROOT / laptop["gpui"]["video"], 5.3, 2.6, "Search", "GPUI"),
        (data.ROOT / laptop["rune"]["video"], 14.1, 2.8, "Related video", "Rune · fixture playback"),
        (data.ROOT / CAPTION, 0, 2.6, "Captions", "Rune · live test still"),
    ]
    track = [feature_clip(*shot, picture=shot[0].suffix == ".png") for shot in shots]
    concat(track, data.OUT / "Demo.mp4")
    return {"feature captures": [str(shot[0].relative_to(data.ROOT)) for shot in shots]}


def scroll_clip():
    """Keep the recorder's wall-clock timing for the continuous touchpad gesture."""
    WORK.mkdir(parents=True, exist_ok=True)
    capture = data.OUT.parent / "scroll-capture"
    video = capture / "rune-scroll.mkv"
    receipt = data.read(capture / "edit.json")
    clip = feature_clip(video, receipt["start_s"], receipt["duration_s"], "Touchpad scroll",
                        "Rune · laptop · real time")
    concat([clip], data.OUT / "ScrollRealTime.mp4")
    return {"60 fps capture and edit": [str(path.relative_to(data.ROOT)) for path in
                                       (video, capture / "report.json", capture / "edit.json")]}


if __name__ == "__main__":
    print(cover())
    print(json.dumps(demo(), ensure_ascii=False))
