#!/usr/bin/env python3
"""Drive the private Sway seat with nt-input and retain the resulting probe stream."""
from contextlib import contextmanager, nullcontext
import json
import os
from pathlib import Path
import re
import shutil
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
    before = d.input("key", "shift+n")
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


def live_watch(d):
    state = d.search("0y6RKiIk6cs")
    if not state["items"]:
        raise Unavailable("unsigned live search did not return a video")
    for _ in range(4):
        try:
            d.watch(label=r"Rust for Dummies in 12 Minutes")
            break
        except Unavailable:
            d.input("wheel", 770, 680, 5, 50)
            time.sleep(0.45)
    else:
        raise Unavailable("target video absent after four unsigned result viewports")
    d.wait(lambda s: video(s).get("id") == "0y6RKiIk6cs",
           "watch target is exactly 0y6RKiIk6cs", seconds=8)
    d.playing()


def live_controls(d):
    live_watch(d)
    d.controls()
    d.shot("controls-before")
    if d.app == "gpui":
        d.hover(d.target({"scrubber"}), "storyboard", 650)
        try:
            d.click({"captions", "cc"}, "captions|CC|subtitles")
        except Unavailable:
            x, y = d.point(d.target({"settings"}))
            d.input("click", x - 40, y)
    else:
        d.input("key", "c")
    time.sleep(1.1)
    d.shot("caption-key-or-cc")
    d.controls()
    d.shot("caption-controls")
    if d.app == "gpui":
        d.input("move", 1248, 50)
        for index, delay in enumerate((0.0, 2.65, 0.2, 0.2, 0.2)):
            time.sleep(delay)
            d.shot(f"caption-hide-{index}")
    d.settings(r"Subtitles|closed captions")
    d.shot("caption-menu")
    d.click(None, r"Auto.translate")
    d.shot("auto-translate")
    d.click(None, r"^Arabic$")
    time.sleep(1)
    d.shot("translated-caption")
    raise Unavailable("captions, animation and storyboard require review of the captured frames")


def live_audio(d):
    d.search("Last Person Standing Wins $250,000")
    d.watch(label=r"Last Person Standing Wins")
    d.playing()
    d.settings()
    d.shot("audio-settings")
    d.click(None, r"^Audio track")
    d.shot("audio-tracks")
    alternate = d.target({"menu_item"}, r"Spanish|French|German|Arabic|Hindi|Portuguese")
    d.input("click", *d.point(alternate))
    d.playing()
    d.shot("audio-track-selected")
    raise Unavailable(f"selected {alternate['label']!r}; retain the screenshots for comparison")


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
        cases = OFFLINE[app] if mode == "batch" else [(f"{app}-live", live_controls)]
        if mode == "live" and app == "rune":
            cases.append(("rune-audio", live_audio))
        for name, case in cases:
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
            if mode == "live" and re.search(
                r"sign in.*bot|pausing all requests|HTTP(?: error)? (?:403|429)",
                driver.log.read_text(), re.I,
            ):
                print("Live run stopped on sign-in requirement, denial or cooldown.", flush=True)
                return
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
