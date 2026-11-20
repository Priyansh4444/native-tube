"""Logged-out YouTube workload provenance, using read-only libmpv IPC."""
import json
import re
import socket
import time
from urllib.parse import urlsplit

# The built-in cap in core/src/resolve.rs (default_max_height).
DEFAULT_HEIGHT = 1080
PROPERTIES = ("path", "video-params", "video-codec", "container-fps",
              "hwdec-current", "time-pos", "mute", "pause")
STREAM_SHAPE = ("width", "height", "codec", "fps")
THROTTLED = re.compile(rb"^nt-core: YouTube signalled .+; pausing all requests to it for (\d+) min\r?\n$")
RENDER_SIGNALS = {
    b"Wayland GL video attached": "wayland_gl_attachment_logged",
    b"NT_EVENT gpu_surface_attached ": "gpu_surface_attachment_logged",
    b"software video fallback": "software_fallback_logged",
    b"keeping software video": "software_fallback_logged",
    b"Wayland video fell back to software": "software_fallback_logged",
}
HTTP_REFUSAL = re.compile(rb"HTTP(?: error)? (403|429)\b")


class Cooldown(RuntimeError):
    def __init__(self, seconds):
        self.seconds = seconds
        super().__init__(f"YouTube signalled a refusal or cooldown. Live test stopped for {seconds // 60} min.")


def benchmark_env(inherited, profile, scenario, report, fixtures, clip, ipc=None, *, height_override=None):
    env = {key: value for key, value in inherited.items()
           if not key.startswith("NT_") and key not in ("XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID")}
    env.update(NT_MUTE="1", NT_NO_MPRIS="1", NT_TRACE="1", NT_DATA_DIR=str(profile),
               NT_AUTOPILOT=str(scenario), NT_REPORT=str(report))
    if ipc is None:
        env.update(NT_LIVE="0", NT_PROBE="0", NT_FIXTURES=str(fixtures),
                   NT_FIXTURE_MODE="replay", NT_VIDEO_OVERRIDE=str(clip))
    else:
        env.update(NT_LIVE="1", NT_PROBE="1", NT_MPV_OPTS=f"input-ipc-server={ipc}")
        if height_override is not None:
            env["NT_MAX_HEIGHT"] = str(height_override)
    return env


def check_log_line(line, live):
    if live:
        cooldown = THROTTLED.fullmatch(line)
        if cooldown:
            # The core prints whole minutes rounded down.
            raise Cooldown((int(cooldown[1]) + 1) * 60)
        if not line.startswith((b"NT_STATE ", b"NT_MARK ")) and HTTP_REFUSAL.search(line):
            raise Cooldown(30 * 60)
    if line.startswith(b"NT_AUTOPILOT: timed out waiting for "):
        raise RuntimeError(line.decode(errors="replace").strip())


def stream_values(values):
    url = urlsplit(values.get("path") or "")
    params = values.get("video-params") or {}
    return {
        "source": "youtube_cdn" if url.scheme == "https" and (url.hostname or "").endswith(".googlevideo.com")
        else "unverified",
        "width": params.get("w"), "height": params.get("h"),
        "codec": values.get("video-codec"), "fps": values.get("container-fps"),
        "pixel_format": params.get("pixelformat"), "decoder": values.get("hwdec-current"),
        "position_s": values.get("time-pos"), "muted": values.get("mute"), "paused": values.get("pause"),
    }


def read_player(path):
    # Deliberately request only the properties we need. Never persist the
    # signed media URL or listen to mpv's potentially sensitive log events.
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(0.12)
        client.connect(str(path))
        requests = [
            {"command": ["get_property", name], "request_id": idx}
            for idx, name in enumerate(PROPERTIES)
        ]
        client.sendall(b"".join(json.dumps(request).encode() + b"\n" for request in requests))
        replies = {}
        deadline = time.monotonic() + 0.12
        with client.makefile("rb") as stream:
            while len(replies) < len(requests) and time.monotonic() < deadline:
                line = stream.readline()
                if not line:
                    break
                answer = json.loads(line)
                idx = answer.get("request_id")
                if idx is not None and 0 <= idx < len(PROPERTIES):
                    replies[PROPERTIES[idx]] = answer.get("data")
        return stream_values(replies)


class RunLog:
    def __init__(self, log, started, ipc=None):
        self.log = log
        self.started = started
        self.ipc = ipc
        self.home = None
        self.states = []
        self.player = []
        self.render_signals = set()
        self.next_ipc = 0

    def poll(self):
        while True:
            offset = self.log.tell()
            line = self.log.readline()
            if not line.endswith(b"\n"):
                self.log.seek(offset)
                break
            check_log_line(line, self.ipc is not None)
            if line.startswith(b"NT_MARK home_loaded ") and self.home is None:
                self.home = time.monotonic()
            if self.ipc and line.startswith(b"NT_STATE "):
                state = json.loads(line.removeprefix(b"NT_STATE "))
                self.states.append({"t_s": time.monotonic() - self.started,
                                    **{key: state.get(key) for key in ("page", "loading", "error", "video")}})
            if self.ipc and not line.startswith(b"NT_STATE "):
                self.render_signals.update(signal for marker, signal in RENDER_SIGNALS.items() if marker in line)
        now = time.monotonic()
        if self.ipc and self.home is not None and now >= self.next_ipc:
            self.next_ipc = now + 0.5
            try:
                player = read_player(self.ipc)
            except (OSError, ValueError):
                return
            self.player.append({"t_s": now - self.started, **player})

    def state_evidence(self, phase, page, video_id=None):
        for state in self.states:
            if not phase["start_s"] + 0.25 <= state["t_s"] <= phase["end_s"] - 0.15:
                continue
            if state["page"] != page or state["loading"] or state["error"] is not None:
                continue
            video = state["video"]
            if video_id is None and video is None:
                return True
            if video and video["id"] == video_id and video["first_frame"]:
                return True
        return False


def verify_stream(samples, height_cap):
    if len(samples) < 3:
        raise ValueError("too few live player observations during settled playback")
    if any(row["source"] != "youtube_cdn" or row["muted"] is not True or row["paused"] is not False
           or not all(row[key] for key in STREAM_SHAPE) for row in samples):
        raise ValueError("could not verify unpaused, muted YouTube CDN playback with decoded stream metadata")
    decoded_heights = {row["height"] for row in samples}
    if max(decoded_heights) > height_cap:
        decoded = ", ".join(f"{height}p" for height in sorted(decoded_heights))
        raise ValueError(f"playback within the requested {height_cap}p cap was unavailable (decoded {decoded})")
    shapes = {tuple(row[key] for key in STREAM_SHAPE) for row in samples}
    if len(shapes) != 1:
        raise ValueError("stream dimensions, codec or FPS changed during settled playback")
    positions = [row["position_s"] for row in samples if row["position_s"] is not None]
    if len(positions) < 3 or positions[-1] - positions[0] < 1:
        raise ValueError("decoded playback position did not advance")
    return {**dict(zip(STREAM_SHAPE, next(iter(shapes)))),
            "decoders": sorted({row["decoder"] or "unreported" for row in samples})}


def verify_live(log, phases, library, height_cap):
    history = json.loads(library.read_text())["history"]
    ids = [entry["id"] for entry in reversed(history)]
    if len(ids) != 2 or len(set(ids)) != 2:
        raise ValueError("expected two distinct YouTube video IDs in the fresh session's history")
    opened = []
    for video_id, name in zip(ids, ("playing", "switched")):
        phase = phases[name]
        observations = [row for row in log.player
                        if phase["start_s"] + 0.75 <= row["t_s"] <= phase["end_s"] - 0.5]
        if not log.state_evidence(phase, "watch", video_id):
            raise ValueError(f"app state probe did not verify Watch for video {video_id}")
        try:
            stream = verify_stream(observations, height_cap)
        except ValueError as error:
            raise ValueError(f"video {video_id}: {error}") from error
        opened.append({"id": video_id, **stream})
    if not log.state_evidence(phases["recovered"], "home"):
        raise ValueError("the app state probe did not verify return to the loaded Home feed")
    return {"videos": opened, "returned_to_home": True,
            "render_signals": sorted(log.render_signals)}


def comparable_key(evidence):
    return tuple(tuple(video[key] for key in ("id", *STREAM_SHAPE)) for video in evidence["videos"])
