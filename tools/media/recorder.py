"""Record a verified 1280×800 headless output as silent H.264."""
import signal
import subprocess
import time
from pathlib import Path

WIDTH, HEIGHT = 1280, 800
FPS = 30


def record(destination, capture, running):
    """Write a constant-rate stream. Repeat the preceding frame when capture misses a slot."""
    pending = destination.with_suffix(".pending.mp4")
    photos = frames = 0
    previous = None
    started = time.monotonic()
    with destination.with_suffix(".ffmpeg.log").open("wb") as log, subprocess.Popen([
        "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y",
        "-f", "image2pipe", "-c:v", "ppm", "-framerate", str(FPS), "-i", "pipe:0",
        "-an", "-c:v", "libx264", "-preset", "ultrafast", "-crf", "18",
        "-pix_fmt", "yuv420p", "-threads", "2", "-movflags", "+faststart", str(pending),
    ], stdin=subprocess.PIPE, stderr=log) as encoder:
        try:
            while running():
                if time.monotonic() - started >= 90:
                    raise RuntimeError("recording exceeded 90 seconds")
                shot = capture()
                if not running():
                    break
                if not shot.startswith(f"P6\n{WIDTH} {HEIGHT}\n255\n".encode()):
                    raise RuntimeError("Grim returned an unexpected frame format or output size")
                # Skip encoder catch-up if the first frame itself was delayed.
                slot = max(frames, round((time.monotonic() - started) * FPS))
                if previous is not None:
                    for _ in range(frames, slot):
                        encoder.stdin.write(previous)
                else:
                    slot = 0
                    started = time.monotonic()
                encoder.stdin.write(shot)
                previous = shot
                photos += 1
                frames = slot + 1
                time.sleep(max(0, started + frames / FPS - time.monotonic()))
            encoder.stdin.close()
            if encoder.wait(timeout=8) != 0 or not photos:
                raise RuntimeError(f"encoding failed for {destination}")
            pending.replace(destination)
        finally:
            if encoder.poll() is None:
                encoder.send_signal(signal.SIGKILL)
            pending.unlink(missing_ok=True)
    return {"captures": photos, "encoded_frames": frames, "fps": FPS}


def grim(output):
    if output not in {"NT-ICED", "NT-GPUI", "NT-RUNE", "NT-BENCH"}:
        raise ValueError("capture requires a named NativeTube headless output")
    return subprocess.run(
        ["grim", "-t", "ppm", "-o", output, "-"],
        capture_output=True, check=True, timeout=3,
    ).stdout
