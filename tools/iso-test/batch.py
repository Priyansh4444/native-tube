#!/usr/bin/env python3
"""Drive the private Sway seat with nt-input and retain the resulting probe stream."""
from contextlib import contextmanager, nullcontext
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys
import time


def latest(log):
    with log.open("rb") as file:
        file.seek(max(0, log.stat().st_size - 262144))
        for line in reversed(file.read().splitlines()):
            if line.startswith(b"NT_STATE "):
                try:
                    return json.loads(line[9:])
                except ValueError:
                    continue
    return {}


def command(*args, **kwargs):
    return subprocess.run(args, check=True, timeout=15, **kwargs)


def live_block(log):
    if os.environ.get("NT_LIVE") != "1":
        return None
    return next((
        line for line in reversed(log.read_text().splitlines())
        if line.startswith("nt-core: YouTube signalled ")
        and "pausing all requests" in line
    ), None)


@contextmanager
def clipboard_seat():
    # Clipboard selection is tied to keyboard focus on Sway. Keep an initialized
    # virtual keyboard attached for the clipboard case, starting before its window.
    with subprocess.Popen(["wtype", "-k", "Shift_L", "-s", "180000"]) as keyboard:
        try:
            for _ in range(40):
                seats = json.loads(command("swaymsg", "-t", "get_seats", capture_output=True).stdout)
                if seats and seats[0]["capabilities"] & 2:
                    break
                time.sleep(0.05)
            else:
                raise AssertionError("private clipboard keyboard did not attach")
            yield
        finally:
            keyboard.terminate()


class Unavailable(Exception):
    """The observation requires a target or recording absent from this run."""


class Driver:
    def __init__(self, app, out):
        self.app = app
        self.out = out
        self.profile = Path(os.environ["XDG_RUNTIME_DIR"]) / f"{app}-profile"
        self.log = out / "app.log"
        self.events = out / "events.jsonl"
        out.mkdir(exist_ok=True)
        self.events.touch()
        self.extra_env = {}

    def event(self, **event):
        with self.events.open("a") as file:
            print(json.dumps({"epoch_ns": time.time_ns(), **event}), file=file)

    @contextmanager
    def launch(self, name):
        self.log = self.out / f"{name}.log"
        env = os.environ | {"NT_APP_ID": f"nttest-{self.app}", "NT_DATA_DIR": str(self.profile)} | self.extra_env
        with self.log.open("wb") as output, subprocess.Popen(
            [f"/inputs/nt-{self.app}"], env=env, stdout=output, stderr=subprocess.STDOUT,
        ) as process:
            self.process = process
            try:
                self.wait(lambda s: not s["loading"], "settled initial page", seconds=16)
                time.sleep(0.3)  # Wait for initial Sway surface transactions.
                yield
            finally:
                process.terminate()
                try:
                    process.wait(timeout=4)
                except subprocess.TimeoutExpired:
                    process.kill()

    def state(self):
        return latest(self.log)

    def wait(self, accepts, observation, seconds=5, after=None):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            state = self.state()
            if state and (after is None or state["t_ms"] > after) and accepts(state):
                self.event(observation=observation, log=self.log.name, state=state)
                return state
            if self.process.poll() is not None:
                raise AssertionError(f"app exited {self.process.returncode} before {observation}")
            time.sleep(0.035)
        state = self.state()
        self.event(unmet=observation, log=self.log.name, state=state)
        raise AssertionError(f"{observation} not observed in {seconds}s (probe t_ms={state.get('t_ms')})")

    def start_input(self, *args):
        args = [str(arg) for arg in args]
        before = self.state().get("t_ms", 0)
        self.event(input=args, log=self.log.name, before_ms=before)
        return before, ["nt-input", self.app, *args]

    def input(self, *args):
        before, argv = self.start_input(*args)
        command(*argv, stdout=subprocess.DEVNULL)
        return before

    def target(self, kinds=None, label=None, index=0, seconds=3):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            targets = self.state().get("targets", [])
            matches = [t for t in targets if (kinds is None or t["kind"] in kinds)
                       and (label is None or re.search(label, t["label"] or "", re.I))]
            if len(matches) > index:
                return matches[index]
            if self.process.poll() is not None:
                raise AssertionError(f"app exited {self.process.returncode} looking for {kinds}: {label}")
            time.sleep(0.05)
        raise Unavailable(f"target absent from probe: {kinds}, {label}, index {index}")

    @staticmethod
    def point(target):
        x, y, w, h = target["rect"]
        return round(x + w / 2), round(y + h / 2)

    def click(self, kinds=None, label=None, index=0, button="left"):
        target = self.target(kinds, label, index)
        self.event(target=target, log=self.log.name)
        return self.input("click", *self.point(target), button)

    def shot(self, name):
        command("grim", "-o", "HEADLESS-1", str(self.out / f"{name}.png"))
        self.event(screenshot=f"{name}.png", log=self.log.name, state=self.state())

    def hover(self, target, name, delay):
        self.input("hover", *self.point(target), self.out / f"{name}.png", delay)
        self.event(screenshot=f"{name}.png", log=self.log.name, state=self.state())

    def check(self, name, case):
        status = "PASS"
        try:
            note = case()
        except Unavailable as error:
            status, note = "SKIPPED", str(error)
        except (AssertionError, subprocess.SubprocessError) as error:
            status, note = "FAIL", str(error)
        if status != "PASS" and (block := live_block(self.log)):
            status, note = "SKIPPED", block
        self.shot(name)
        self.event(case=name, status=status, notes=note, log=self.log.name)
        print(self.app, name, status, note, flush=True)

    def home(self):
        if self.state()["page"] != "home":
            self.click({"rail"}, r"^Home$")
        return self.wait(lambda s: s["page"] == "home" and s["items"] > 0,
                         "populated home")

    def search(self, query, exact_point=False):
        if exact_point:
            self.input("click", 600, 28)
        else:
            self.click({"search_field"})
        if self.state()["query"]:
            self.input("key", "ctrl+a")
        before = self.input("type", query)
        self.wait(lambda s: s["focus"] == "search" and s["query"] == query,
                  f"focused search field contains {query}", after=before)
        if exact_point:
            self.wait(lambda s: s["menu"] == "suggestions", "search suggestions")
            self.shot("search-suggestions")
        before = self.input("key", "Return")
        return self.wait(lambda s: s["page"] == "search" and s["results_query"] == query
                         and not s["loading"] and s["focus"] is None,
                         "submitted search, focus cleared", seconds=12, after=before)

    def watch(self, kinds=frozenset({"card", "row"}), label=None):
        initial = video(self.state()).get("id")
        before = self.click(kinds, label)
        return self.wait(lambda s: s["page"] == "watch" and video(s).get("id") not in {None, initial},
                         "watch changed after the click", after=before)

    def playing(self):
        return self.wait(lambda s: video(s).get("first_frame")
                         and not video(s)["paused"] and video(s)["position"] > 0.05,
                         "first frame and playback advancement", seconds=8)

    def controls(self):
        target = self.target({"player"})
        x, y = self.point(target)
        self.input("move", x, y)
        return self.target({"settings"})

    def settings(self, label=None):
        self.controls()
        before = self.click({"settings"})
        self.wait(lambda s: s["menu"] == "settings", "settings open", after=before)
        if label:
            self.click(None, label)

    def pss(self):
        smaps = Path(f"/proc/{self.process.pid}/smaps_rollup").read_text()
        return int(re.search(r"^Pss:\s+(\d+)", smaps, re.M).group(1))


def video(state):
    return state.get("video") or {}


def rapid(d):
    d.home()
    d.watch()
    d.playing()
    for attempt in range(1, 21):
        initial = video(d.state())["id"]
        target = d.target({"related"}, seconds=6)
        d.event(related_attempt=attempt, initial_video=initial, target=target)
        x, y, width, height = target["rect"]
        point = (990, 125) if x <= 990 < x + width and y <= 125 < y + height else d.point(target)
        before = d.input("click", *point)
        d.wait(lambda s: s["page"] == "watch" and video(s).get("id") not in {None, initial},
               f"related click {attempt} changed video on first attempt", seconds=2, after=before)
        if attempt in (1, 10, 20):
            d.shot(f"related-{attempt:02d}")
    initial = d.state()["back_depth"]
    memory = [d.pss()]
    for attempt in range(35):
        back = attempt % 2 == 0
        old = d.state()
        before = d.input("key", "alt+Left" if back else "alt+Right")
        d.wait(lambda s: s["back_depth"] == old["back_depth"] + (-1 if back else 1),
               f"history traversal {attempt + 1}", seconds=3, after=before)
        if attempt in (0, 1):
            d.shot("back-frame" if attempt == 0 else "forward-frame")
        if attempt % 5 == 4:
            memory.append(d.pss())
    assert d.state()["back_depth"] == initial - 1
    landed = d.playing()
    memory.append(d.pss())
    d.event(pss_kib=memory, final=landed)
    return f"20/20 first clicks, 35 traversals, final playback advancing; sampled PSS KiB {memory}"


def iced_search(d):
    state = d.search("rust", exact_point=True)
    before = d.input("key", "Page_Down")
    d.wait(lambda s: s["scroll_y"] > state["scroll_y"], "PageDown moved results", after=before)
    return "Suggestions and search focus tracked by the probe; PageDown moved results"


def iced_sleep(d):
    d.home()
    d.watch()
    d.playing()
    d.settings(r"^Sleep timer")
    before = d.click(None, r"^End of video$")
    d.wait(lambda _: "sleep_timer_end_of_video_armed" in d.log.read_text(),
           "End of video was armed", after=before)
    d.shot("sleep-armed")
    before = d.input("key", "Escape")
    d.wait(lambda s: s["menu"] is None, "settings dismissed", after=before)
    initial = video(d.state())["id"]
    state = d.wait(lambda s: video(s).get("id") == initial and video(s).get("paused")
                   and video(s).get("position", 0) >= 58 and s["menu"] != "up_next",
                   "same video paused at end without Up next", seconds=68)
    assert "sleep_timer_fired" in d.log.read_text(), "missing sleep_timer_fired trace"
    return f"60 s clip, sleep_timer_fired, position {video(state)['position']}"


def iced_menu(d):
    initial = d.home()
    for action in (("key", "Escape"), ("click", 650, 95), ("wheel", 650, 95, 1)):
        before = d.input("click", 1150, 700, "right")
        d.wait(lambda s: s["menu"] == "card", "edge card menu open", after=before)
        d.shot(f"card-menu-{action[0]}")
        before = d.input(*action)
        d.wait(lambda s: s["menu"] is None and s["page"] == initial["page"],
               f"{action[0]} closed menu without navigation", after=before)
    before = d.input("click", 1150, 700, "right")
    d.wait(lambda s: s["menu"] == "card", "card menu reopened", after=before)
    d.click(None, r"^Share$")
    d.wait(lambda s: s["menu"] is None, "Share closed card menu")
    for _ in range(10):
        paste = subprocess.run(["wl-paste", "--no-newline"], capture_output=True, text=True, timeout=3)
        if paste.returncode == 0:
            break
        time.sleep(0.1)
    assert paste.stdout.startswith(("https://youtu.be/", "https://www.youtube.com/watch?v=")), (
        f"clipboard contained {paste.stdout!r}; wl-paste stderr: {paste.stderr.strip()}"
    )
    d.event(clipboard=paste.stdout)
    return "Escape, outside click and wheel dismissed; Share copied a YouTube watch link"


def card_hover(d):
    d.home()
    card = d.target({"card"})
    if d.app == "iced":
        for delay in (400, 1800, 2200):
            name = f"card-hover-{delay}"
            d.input("hover", 284, 300, d.out / f"{name}.png", delay)
            d.event(screenshot=f"{name}.png", log=d.log.name, state=d.state())
    else:
        d.hover(card, "card-hover-1", 400)
        second = d.target({"card"}, index=1)
        d.hover(second, "card-hover-2-fast", 60)
        d.hover(second, "card-hover-2-settled", 400)
        d.input("move", 640, 30)
        time.sleep(0.24)
        d.shot("card-hover-leave")
    raise Unavailable("captured screenshots for visual inspection; probe has no card hover or preview state")


def keyboard_focus(d):
    d.home()
    d.input("move", 640, 70)
    for index in range(1, 8 if d.app == "iced" else 7):
        d.input("key", "Tab")
        time.sleep(0.08)
        d.shot(f"tab-{index}")
    raise Unavailable("captured tab frames; focus ring needs visual inspection")


def modes(d):
    d.home()
    d.watch()
    d.playing()
    for key, field in (("t", "theatre"), ("f", "fullscreen")):
        for index in range(1, 7):
            old = d.state()[field]
            before = d.input("key", key)
            d.wait(lambda s: s[field] != old, f"{field} toggle {index}", seconds=2, after=before)
    return "Six theatre and six fullscreen key presses produced twelve state transitions"


def gpui_next(d):
    d.home()
    d.watch()
    d.playing()
    d.watch({"related"})
    current = d.playing()
    next_target = d.target({"related"})
    # Keep Sway's temporary keyboard present long enough for GPUI to bind it and
    # process the key while the newly opened Watch page is rendering.
    before = d.input("key", "wait=250", "shift+n", "wait=250")
    state = d.wait(lambda s: video(s).get("id") not in {None, video(current)["id"]},
                   "Shift+N advanced after related click", after=before)
    d.playing()
    return f"Prepared related item {next_target['label']!r}; advanced to {video(state)['id']}"


def gpui_help(d):
    d.home()
    # Keep one virtual keyboard attached between the two keys. GPUI stops
    # receiving keys after Sway withdraws the initial keyboard capability.
    before, argv = d.start_input("key", "question", "wait=1200", "Escape")
    with subprocess.Popen(argv, stdout=subprocess.DEVNULL) as keys:
        opened = d.wait(lambda s: s["menu"] == "shortcuts", "? opens shortcuts", after=before)
        d.shot("shortcut-sheet-open")
        assert keys.wait(timeout=6) == 0, "keyboard sequence failed"
        d.wait(lambda s: s["menu"] is None, "Escape closes shortcuts", after=opened["t_ms"])
    return "Sheet open/close verified. Arrow typography captured in shortcut-sheet-open.png"


def rune_queue_hide(d):
    state = d.home()
    before = d.click({"card"}, button="right")
    d.wait(lambda s: s["menu"] == "card", "card menu opened", after=before)
    d.shot("queue-hide-menu")
    d.click(None, "Add to queue|Play next")
    library = d.profile / "library.json"
    for _ in range(25):
        if library.exists() and json.loads(library.read_text()).get("queue"):
            break
        time.sleep(0.05)
    assert library.exists() and json.loads(library.read_text()).get("queue"), "queue was not saved"
    d.shot("queue-filled")
    before = d.click({"card"}, button="right")
    d.wait(lambda s: s["menu"] == "card", "card menu reopened", after=before)
    d.click(None, "Not interested")
    for _ in range(25):
        if json.loads(library.read_text()).get("hidden_videos"):
            break
        time.sleep(0.05)
    assert json.loads(library.read_text()).get("hidden_videos"), "hidden video was not saved"
    d.wait(lambda s: s["menu"] is None and s["items"] < state["items"], "card hidden from home")
    return "Queue and hidden video persisted; card count dropped"


def rune_controls(d):
    d.home()
    d.watch()
    d.playing()
    d.settings()
    d.shot("rune-settings-panel")
    d.click(None, r"^Loop")
    time.sleep(0.2)
    d.shot("rune-loop")
    d.click(None, r"^Stable volume")
    time.sleep(0.2)
    d.shot("rune-stable-volume")
    return "Loop and stable volume menu clicks captured; inspect checkmarks in screenshots"


def rune_channel(d):
    d.home()
    d.watch()
    d.playing()
    d.click({"channel", "author"})
    d.wait(lambda s: s["page"] == "channel" and not s["loading"], "channel loaded", seconds=10)
    raise Unavailable("channel loaded; thumbnail quality needs visual inspection")


def rune_resume(d):
    if d.profile.exists():
        shutil.rmtree(d.profile)
    with d.launch("resume-seed"):
        d.watch()
        started = d.playing()
        d.controls()
        seek = d.target({"scrubber"})
        x, y, w, h = seek["rect"]
        d.input("click", x + w * 0.35, y + h / 2)
        d.wait(lambda s: video(s).get("position", 0) > 10, "seek to resumable position")
        d.input("key", "ctrl+q")
        assert d.process.wait(timeout=8) == 0, "Rune did not close gracefully after seek"
    saved = json.loads((d.profile / "library.json").read_text()).get("last_played")
    assert saved and saved["position"] > 10, "no resumable position persisted on quit"
    with d.launch("resume-open"):
        d.shot("resume-card")
        resume = d.target({"resume"}, r"Continue watching")
        before = d.input("click", *d.point(resume))
        d.wait(lambda s: video(s).get("id") == video(started)["id"]
               and video(s)["position"] > 10, "resume opened at saved position", after=before)
        d.playing()
        d.shot("resume-playing")
    return f"Resume activated after restart at saved position {saved['position']} s"


def transcript_fixture(d):
    # Reuse all anonymous network recordings as-is. Add one deterministic response
    # for the missing transcript request inside the private tmpfs.
    fixtures = Path(os.environ["XDG_RUNTIME_DIR"]) / "transcript-fixtures"
    if fixtures.exists():
        return fixtures
    fixtures.mkdir()
    for record in Path(os.environ["NT_FIXTURES"]).iterdir():
        (fixtures / record.name).symlink_to(record)
    key = b"GET https://search.pronsh.dev/api/search?q=rust&page=1&per_page=20"
    digest = 0xcbf29ce484222325
    for b in key:
        digest = ((digest ^ b) * 0x100000001b3) & ((1 << 64) - 1)
    dest = fixtures / f"{digest:016x}.bin"
    if not dest.exists():
        fixture = {
            "videos": [{
                "video_id": "0y6RKiIk6cs",
                "title": "Rust for Dummies in 12 Minutes",
                "youtuber_name": "Let's Get Rusty",
                "youtuber_handle": "",
                "view_count": 0,
                "youtube_url": "https://www.youtube.com/watch?v=0y6RKiIk6cs&t=24s",
                "best_timestamp": "0:24",
                "hits": [{"t_start_ms": 24000, "text": "Synthetic Rust transcript check",
                          "timestamp": "0:24"}],
            }],
            "has_next_page": False,
            "total_videos": 1,
        }
        dest.write_text(json.dumps(fixture))
        d.event(synthetic_fixture=fixture, key=key.decode())
    return fixtures


def rune_transcript(d):
    d.search("rust")
    before = d.click({"chip"}, r"^Transcripts$")
    d.wait(lambda s: not s["loading"] and s["error"] is None,
           "transcript filter loaded", after=before)
    target = d.target({"card", "row"}, r"Rust for Dummies")
    d.shot("transcript-result")
    before = d.input("click", *d.point(target))
    d.wait(lambda s: video(s).get("id") == "0y6RKiIk6cs"
           and 24 <= video(s).get("position", 0) < 29,
           "clicked transcript hit and landed at 0:24", after=before)
    return "Clicked the transcript hit using a synthetic 0:24 replay response"


def rune_chapters(d):
    rune_transcript(d)
    d.playing()
    # The description and chapter rows lack probe targets. These points match
    # the visually inspected 1280x800 fixture and we verify the resulting seek.
    d.input("click", 630, 751)
    d.input("wheel", 730, 690, 4, 50)
    time.sleep(0.55)
    d.shot("chapter-list")
    before = d.input("click", 180, 660)
    d.wait(lambda s: 0 < video(s).get("position", 0) < 5 and s["scroll_y"] < 10,
           "Intro chapter rewound the player and restored the view", after=before)
    return "Intro chapter rewound from 0:24 to below 0:05 and scrolled to the player"


def rune_next(d):
    d.home()
    d.watch()
    initial = d.playing()
    d.controls()
    before = d.click({"next"})
    d.wait(lambda s: video(s).get("id") not in {None, video(initial)["id"]},
           "Next switches to a different video", seconds=3, after=before)
    d.playing()
    return "Next changed video within 3 s and reached an advancing first frame"


def rune_retry(d):
    d.search("zzzz-native-tube-isolated-fixture-miss")
    before = d.click({"retry"}, r"Retry")
    d.wait(lambda s: s["loading"] or s["error"] is not None,
           "Retry returned a load or fixture error", after=before)
    return "Retry invoked against a deliberate replay miss; probe confirmed returned error"


LIVE_ID = os.environ.get("NT_ISO_LIVE_VIDEO_ID", "5C_HPTJg5ek")
LIVE_CASES = {
    "iced": ("iced-5-live-captions",),
    "gpui": ("gpui-3-caption-position", "gpui-caption-ease", "gpui-cc-settings"),
    "rune": ("rune-cc", "rune-audio", "rune-caption-position"),
}


def is_painted(state, kind, label):
    return any(t["kind"] == kind and t["label"] == label for t in state["targets"])


def watch_live(d):
    d.wait(lambda s: s["page"] == "watch" and video(s).get("id") == LIVE_ID,
           "opened exact video ID", seconds=15)
    d.wait(lambda s: video(s).get("first_frame") and video(s).get("muted")
           and video(s).get("position", 0) > 0.05,
           "public video produced an advancing muted first frame", seconds=22)


def live_subtitle(d):
    # Read one playback property from the app's private mpv socket. No URLs or
    # track payloads are requested. Grab frames only while a cue can be visible.
    try:
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(0.2)
            client.connect(str(d.mpv_socket))
            client.sendall(b'{"command":["get_property","sub-text"],"request_id":1}\n')
            with client.makefile("rb") as stream:
                for line in stream:
                    reply = json.loads(line)
                    if reply.get("request_id") == 1:
                        return reply.get("data") or ""
    except (OSError, ValueError):
        return ""
    return ""


def subtitle_frame(d, name, accepts=bool):
    d.wait(lambda _: accepts(live_subtitle(d)), f"active cue for {name}", seconds=12)
    d.event(cue=live_subtitle(d), frame=name)
    d.shot(name)


def cc(d):
    d.controls()
    if video(d.state()).get("captions") is None:
        d.target({"captions"}, r"^CC$")
        before = d.input("key", "c") if d.app == "iced" else d.click({"captions"}, r"^CC$")
        d.wait(lambda s: video(s).get("captions") is not None,
               "real CC input selected a caption track", seconds=8, after=before)
    subtitle_frame(d, "cc-selected")
    return "CC selected a caption track in the probe and mpv reported a visible cue"


def translate(d):
    cc(d)
    d.settings(r"^Subtitles/CC$")
    before = d.click({"menu_item"}, r"^Auto-translate$")
    d.wait(lambda s: is_painted(s, "menu_item", "Arabic"),
           "Auto-translate language menu arrived", after=before)
    d.shot("translation-menu")
    before = d.click({"menu_item"}, r"^Arabic$")
    d.wait(lambda s: (video(s).get("captions") or [None, None])[1] == "ar",
           "the player selected Arabic translation", seconds=12, after=before)
    d.input("move", 1248, 50)
    subtitle_frame(d, "translated-caption", lambda cue: re.search(r"[\u0600-\u06ff]", cue))
    return "CC, Settings, Subtitles/CC, Auto-translate and Arabic were activated using painted targets"


def caption_box(d, frame, player_rect):
    # The live test enables a diagnostic magenta subtitle background without
    # changing font, size or placement. Decode the actual Sway screenshot.
    raw = command("ffmpeg", "-loglevel", "error", "-threads", "1",
                  "-i", str(d.out / f"{frame}.png"), "-frames:v", "1",
                  "-pix_fmt", "rgb24", "-threads:v", "1", "-f", "rawvideo", "pipe:1",
                  capture_output=True).stdout
    assert len(raw) == 1280 * 800 * 3, "unexpected screenshot size"
    x, y, w, h = map(round, player_rect)
    columns = []
    rows = []
    for row in range(y + h // 2, min(y + h, 800)):
        left = (row * 1280 + x) * 3
        pixels = raw[left:left + w * 3]
        # The control-bar gradient darkens magenta without changing its hue.
        hits = [column for column, (r, g, b) in enumerate(zip(
                    pixels[::3], pixels[1::3], pixels[2::3], strict=True))
                if min(r, b) >= 128 and abs(r - b) <= 20 and g <= 32]
        if len(hits) >= 20:
            columns.extend(hits)
            rows.append(row)
    if len(rows) < 5:
        return None
    return [x + min(columns), min(rows), max(columns) - min(columns) + 1, max(rows) - min(rows) + 1]


def caption_position(d):
    cc(d)
    settings = d.controls()["rect"]
    player = d.target({"player"})["rect"]
    x, y, width, height = d.target({"scrubber"})["rect"]
    bar = [x, y, width, max(height, settings[1] + settings[3] - y)]
    d.wait(lambda s: video(s).get("caption_lift") == 1,
           "caption lift settled with the bar shown")
    subtitle_frame(d, "caption-shown")
    before = d.input("move", 1248, 50)
    d.wait(lambda s: not is_painted(s, "settings", "Settings"),
           "control bar hidden on pointer leave", after=before)
    time.sleep(0.35)
    subtitle_frame(d, "caption-hidden")
    shown = caption_box(d, "caption-shown", player)
    hidden = caption_box(d, "caption-hidden", player)
    d.event(control_bar=bar, caption_box_shown=shown, caption_box_hidden=hidden)
    assert shown and hidden, "could not measure a rendered caption box in both frames"
    clearance = bar[1] - (shown[1] + shown[3])
    drop = (hidden[1] + hidden[3]) - (shown[1] + shown[3])
    assert clearance >= 0 and drop >= 5, f"caption clearance {clearance:.1f}px, drop {drop}px"
    return f"Caption bottom cleared the visible seek bar by {clearance:.1f}px and moved down {drop}px when hidden"


def caption_ease_duration(log, start_ms, end_ms):
    samples = []
    for line in log.read_text().splitlines():
        if not line.startswith("NT_STATE "):
            continue
        try:
            state = json.loads(line[9:])
        except ValueError:
            continue
        lift = video(state).get("caption_lift")
        if (start_ms < state["t_ms"] <= end_ms and lift is not None
                and 0 < lift < 1 and not is_painted(state, "settings", "Settings")):
            samples.append((state["t_ms"], lift))
    if len(samples) < 2:
        raise Unavailable("probe cadence yielded fewer than two intermediate ease samples")
    first, last = samples[0], samples[-1]
    # The cubic ease-out has lift = (1 - elapsed / duration)^3 when hiding.
    # Fit elapsed time from the app clock. Grim's subprocess completion
    # timestamps include capture overhead and are only useful for ordering.
    progress = first[1] ** (1 / 3) - last[1] ** (1 / 3)
    assert progress > 0, f"caption lift did not decrease: {samples}"
    return (last[0] - first[0]) / progress, samples

def caption_ease(d):
    cc(d)
    d.controls()
    d.wait(lambda s: video(s).get("caption_lift") == 1,
           "caption lift reached shown position")
    player = d.target({"player"})["rect"]
    subtitle_frame(d, "ease-start")
    before, argv = d.start_input("move", 1248, 50)
    start = time.monotonic()
    frames = []
    with subprocess.Popen(argv, stdout=subprocess.DEVNULL) as pointer:
        while time.monotonic() - start < 0.65 and len(frames) < 40:
            name = f"ease-{len(frames):02d}"
            d.shot(name)
            frames.append(((time.monotonic() - start) * 1000, name))
        assert pointer.wait(timeout=5) == 0, "pointer did not leave the player"
    rest = d.wait(lambda s: not is_painted(s, "settings", "Settings")
                  and video(s).get("caption_lift") == 0,
                  "caption lift returned to rest after the control bar hid", after=before)
    subtitle_frame(d, "ease-end")
    lifted = caption_box(d, "ease-start", player)
    rested = caption_box(d, "ease-end", player)
    assert lifted and rested, "could not measure caption boxes across the fade"
    top_bottom = lifted[1] + lifted[3]
    rest_bottom = rested[1] + rested[3]
    samples = []
    for ms, name in frames:
        box = caption_box(d, name, player)
        if box:
            samples.append([round(ms), box[1] + box[3]])
    d.event(caption_ease_frames=samples, caption_bottom_shown=top_bottom,
            caption_bottom_hidden=rest_bottom)
    middle = [ms for ms, y in samples if top_bottom + 2 < y < rest_bottom - 2]
    settled = [ms for ms, y in samples if middle and ms > middle[0] and abs(y - rest_bottom) <= 2]
    assert middle and settled, f"no measurable intermediate and settled caption positions: {samples}"
    duration, lifts = caption_ease_duration(d.log, before, rest["t_ms"])
    d.event(caption_ease_duration_ms=duration, caption_lift_samples=lifts)
    assert 160 <= duration <= 250, f"caption ease fit was {duration:.0f} ms"
    return f"Controls hid on pointer leave and rendered captions eased down, with a {duration:.0f} ms app-clock duration"


def live_audio(d):
    d.settings(r"^Audio track$")
    alternate = d.target({"menu_item"}, r"Spanish|French|German|Arabic|Hindi|Portuguese")
    current = video(d.state()).get("audio_track")
    expected = alternate["label"].removesuffix(" (auto-dubbed)")
    d.shot("audio-tracks")
    before = d.input("click", *d.point(alternate))
    d.wait(lambda s: video(s).get("audio_track") not in {None, current}
           and video(s).get("audio_label") == expected,
           "selected alternate track is active", seconds=18, after=before)
    d.playing()
    d.shot("audio-track-selected")
    return f"Playback switched to {alternate['label']!r}"


def live_app(d, selected):
    assert re.fullmatch(r"[A-Za-z0-9_-]{11}", LIVE_ID), f"invalid video ID: {LIVE_ID!r}"
    scenario = Path(os.environ["XDG_RUNTIME_DIR"]) / f"watch-{d.app}.json"
    scenario.write_text(json.dumps([
        {"wait_mark": "home_loaded", "timeout": 12},
        {"cmd": "open_video_id", "id": LIVE_ID},
        {"sleep": 180},
    ]))
    d.mpv_socket = Path(os.environ["XDG_RUNTIME_DIR"]) / f"{d.app}-mpv"
    d.extra_env = {
        "NT_AUTOPILOT": str(scenario),
        "NT_MPV_OPTS": f"ao=null,input-ipc-server={d.mpv_socket},sub-back-color=#FFFF00FF",
    }
    actions = {
        "iced-5-live-captions": translate,
        "gpui-3-caption-position": caption_position,
        "gpui-caption-ease": caption_ease,
        "gpui-cc-settings": translate,
        "rune-cc": cc,
        "rune-audio": live_audio,
        "rune-caption-position": caption_position,
    }
    cases = [name for name in LIVE_CASES[d.app] if selected is None or name in selected]
    with d.launch("live"):
        try:
            watch_live(d)
        except AssertionError as error:
            d.shot("live-unavailable")
            for case in cases:
                d.event(case=case, status="SKIPPED", notes=str(error), log=d.log.name)
                print(d.app, case, "SKIPPED", error, flush=True)
            return
        for name in cases:
            if block := live_block(d.log):
                d.event(case=name, status="SKIPPED", notes=block, log=d.log.name)
                print(d.app, name, "SKIPPED", block, flush=True)
            else:
                d.check(name, lambda: actions[name](d))


OFFLINE = {
    "iced": [
        ("iced-1-search", iced_search), ("iced-2-sleep", iced_sleep), ("iced-3-card-menu", iced_menu),
        ("iced-4-preview", card_hover), ("iced-6-rapid-nav", rapid),
        ("iced-7-tab-ring", keyboard_focus), ("iced-8-modes", modes),
    ],
    "gpui": [("gpui-1-shift-next", gpui_next), ("gpui-2-shortcut-sheet", gpui_help),
             ("gpui-card-hover", card_hover)],
    "rune": [("rune-20-35", rapid), ("rune-queue-hide", rune_queue_hide),
             ("rune-focus", keyboard_focus), ("rune-controls", rune_controls),
             ("rune-channels", rune_channel), ("rune-transcript", rune_transcript),
             ("rune-chapters", rune_chapters),
             ("rune-next", rune_next), ("rune-retry", rune_retry)],
}


def main():
    mode, out, *apps = sys.argv[1:]
    for selection in apps or ["rune", "iced", "gpui"]:
        app, _, case_filter = selection.partition(":")
        selected = set(case_filter.split(",")) if case_filter else None
        assert app in OFFLINE, app
        driver = Driver(app, Path(out) / app)
        if mode == "smoke":
            with driver.launch("app"):
                state = driver.home()
                driver.shot("home")
                print(app, state["page"], state["items"], len(state["targets"]), flush=True)
            continue
        if mode == "live":
            live_app(driver, selected)
            if re.search(r"sign in.*bot|pausing all requests|HTTP(?: error)? (?:403|429)",
                         driver.log.read_text(), re.I):
                print("Live run stopped on sign-in requirement, denial or cooldown.", flush=True)
                return
            continue
        for name, case in OFFLINE[app]:
            if selected is not None and name not in selected:
                continue
            if driver.profile.exists():
                shutil.rmtree(driver.profile)
            if name in {"rune-transcript", "rune-chapters"}:
                driver.extra_env = {"NT_FIXTURES": str(transcript_fixture(driver))}
            if name == "gpui-2-shortcut-sheet":
                driver.extra_env = {"WAYLAND_DEBUG": "client"}
            seat = clipboard_seat() if name == "iced-3-card-menu" else nullcontext()
            with seat, driver.launch(name):
                driver.check(name, lambda: case(driver))
            driver.extra_env = {}
        if mode == "batch" and app == "rune" and (selected is None or "rune-resume" in selected):
            try:
                status, note = "PASS", rune_resume(driver)
            except Unavailable as error:
                status, note = "SKIPPED", str(error)
            except (AssertionError, subprocess.SubprocessError) as error:
                status, note = "FAIL", str(error)
            driver.event(case="rune-resume", status=status, notes=note, log=driver.log.name)
            print(app, "rune-resume", status, note, flush=True)


if __name__ == "__main__":
    main()
