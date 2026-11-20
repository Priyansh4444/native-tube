import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from performance.analysis import (
    compare_metric, navigation_growth, percentile, phase_metrics, phase_windows, playback_latencies, settled,
)
from performance.live import (
    Cooldown, RunLog, benchmark_env, check_log_line, comparable_key, stream_values, verify_live, verify_stream,
)
from performance.procfs import Sampler, snapshot
from performance.stories import live_steps, steps_for


def load_harness():
    script = Path(__file__).resolve().parents[1] / "check-performance.py"
    spec = importlib.util.spec_from_file_location("check_performance", script)
    harness = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(harness)
    return harness


def sample(at, pss, tick):
    return {
        "t_s": at, "span_s": 0,
        "processes": [
            {"id": "1:0", "pss_mib": pss, "rss_mib": 100, "private_mib": 50, "swap_mib": 0,
             "threads": 2, "fds": 9, "cpu_ticks": tick, "context_switches_by_task": {"2:0": tick}},
        ],
    }


class AnalysisTests(unittest.TestCase):
    def test_resource_metrics(self):
        points = [sample(0, 110, 0), sample(1, 70, 5), sample(2, 60, 10), sample(3, 65, 15)]
        summary = phase_metrics(points)
        self.assertEqual(summary["peak_pss_mib"], 110)
        self.assertEqual(summary["settled_pss_mib"], 65)
        self.assertEqual(summary["settled_processes"], 1)
        self.assertEqual(summary["context_switches_proxy_per_s"], 5)
        self.assertGreater(summary["cpu_seconds"], 0)
        self.assertEqual(summary["settled_swap_mib"], 0)

    def test_missing_child_fields_do_not_look_like_zero(self):
        points = [sample(0, 40, 1), sample(1, 50, 3), sample(2, 45, 6)]
        points[1]["processes"].append({**points[1]["processes"][0], "id": "3:0", "pss_mib": None})
        self.assertIsNone(settled(points, "pss_mib"))
        self.assertIsNone(phase_metrics(points)["peak_pss_mib"])
        self.assertEqual(phase_metrics([])["settled_pss_mib"], None)

    def test_partial_window_cannot_supply_budget_metrics(self):
        app = {"phases": [{"name": "startup", "seconds": 1}, {"name": "idle", "seconds": 3}]}
        expected = ["startup", "idle"]
        points = [sample(at, 42, tick) for tick, at in enumerate([1.2, 1.45, 1.7, 1.95])]
        self.assertIsNone(phase_windows(app, expected, 1, points)["idle"]["settled_pss_mib"])
        points += [sample(at, 43, tick) for tick, at in enumerate([2.5, 3.0, 3.5, 3.75], start=4)]
        self.assertEqual(phase_windows(app, expected, 1, points)["idle"]["settled_pss_mib"], 43)

    def test_plateau_and_late_growth(self):
        self.assertEqual(navigation_growth([25, 75, 102, 102, 103, 104, 103, 102]), 1)
        self.assertEqual(navigation_growth([25, 50, 75, 100, 125, 150, 175, 200]), 75)
        self.assertIsNone(navigation_growth([25, 75, 102, None, 103, 104, 103, 102]))
        self.assertEqual(percentile(list(range(1, 21)), 0.95), 19)
        self.assertIsNone(percentile([], 0.95))

    def test_playback_marks_cannot_cross_into_the_next_open(self):
        cmd = '{"cmd":"open_video"}'
        app = {"commands": [(10, cmd), (25, cmd)],
               "all_marks": [(30, "watch_loaded"), (35, "playback_started")]}
        with self.assertRaisesRegex(ValueError, "has no Watch/first-frame mark"):
            playback_latencies(app, 2)
        app["all_marks"][:0] = [(13, "watch_loaded"), (17, "playback_started")]
        self.assertEqual(playback_latencies(app, 2), [7, 10])

    def test_budget_deltas_caps_and_missing_data(self):
        self.assertEqual(compare_metric(20, 22, 2)["result"], "pass")
        self.assertEqual(compare_metric(20, 23, 2)["result"], "fail")
        self.assertEqual(compare_metric(20, 22, 4, 21)["result"], "fail")
        self.assertEqual(compare_metric(20, None, 4)["result"], "fail")
        self.assertEqual(compare_metric(20, None)["result"], "unavailable")
        self.assertEqual(compare_metric(0, 0)["result"], "report")

    def test_repeated_regression_fails_gate(self):
        harness = load_harness()
        samples = {
            "baseline": [{"metrics": {"pss_mib": value}} for value in (100, 101, 110)],
            "candidate": [{"metrics": {"pss_mib": value}} for value in (102, 120, 121)],
        }
        self.assertEqual(harness.compare_story(samples, {"pss_mib": 10}, {})["pss_mib"]["result"], "fail")
        samples["candidate"][2] = {"error": "sampler lost process"}
        result = harness.compare_story(samples, {"pss_mib": 10}, {})["pss_mib"]
        self.assertEqual((result["candidate"], result["result"]), (None, "fail"))


class LiveTests(unittest.TestCase):
    def test_live_height_arguments(self):
        harness = load_harness()
        live = ["--mode", "live", "live", "-", "out"]
        defaults = harness.parse_args(live)
        self.assertIsNone(defaults.height)
        self.assertEqual(defaults.apps, ["iced", "gpui", "rune"])
        args = harness.parse_args([*live, "--height", "360"])
        self.assertEqual((args.height, args.apps), (360, defaults.apps))
        args = harness.parse_args([*live, "iced", "--height=720", "rune"])
        self.assertEqual((args.height, args.apps), (720, ["iced", "rune"]))
        for invalid in (["--height"], ["--height=0"], ["--height=-1"], ["--height=720p"],
                        ["--height=4294967296"], ["unknown"]):
            with self.subTest(invalid=invalid), patch("sys.stderr", new_callable=io.StringIO):
                with self.assertRaises(SystemExit) as error:
                    harness.parse_args([*live, *invalid])
                self.assertEqual(error.exception.code, 2)
        with patch("sys.stderr", new_callable=io.StringIO) as stderr, self.assertRaises(SystemExit):
            harness.parse_args(["--height=360", "baseline", "binaries", "out", "iced"])
        self.assertIn("--height requires --mode live", stderr.getvalue())

    def test_live_reports_cap_source_and_propagates_unavailable_playback(self):
        harness = load_harness()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "apps/iced/target/release/nt-iced"
            binary.parent.mkdir(parents=True)
            binary.touch()
            for override, cap, source in (([], 1080, "core::resolve::default_max_height"),
                                          (["--height", "360"], 360, "--height")):
                with self.subTest(cap=cap):
                    out = root / f"report-{cap}"
                    args = harness.parse_args(["--mode", "live", "live", "-", str(out), "iced", *override])
                    failure = f"playback within the requested {cap}p cap was unavailable"
                    with patch.multiple(harness, ROOT=root, GUARD=binary, LIVE_COOLDOWN=root / "cooldown"), \
                            patch.object(harness, "WindowWatcher"), \
                            patch.object(harness, "measure", return_value={"error": failure}) as measure, \
                            patch("sys.stdout", new_callable=io.StringIO):
                        self.assertEqual(harness.live_usage(args), 1)
                    self.assertEqual(measure.call_args.kwargs["height_override"], cap if override else None)
                    report = json.loads((out / "results.json").read_text())
                    self.assertEqual((report["quality_cap_p"], report["quality_cap_source"]), (cap, source))
                    self.assertEqual(report["status"], "incomplete")
                    self.assertFalse(report["comparable"])
                    self.assertIn(failure, report["failure"])

    def test_mode_cleans_inherited_media_and_snapshot_overrides(self):
        inherited = {"NT_VIDEO_OVERRIDE": "bad", "NT_FIXTURES": "personal", "NT_MPV_OPTS": "mute=no",
                     "NT_MAX_HEIGHT": "240", "NT_SNAPSHOT_DIR": "bad",
                     "XDG_ACTIVATION_TOKEN": "old", "WAYLAND_DISPLAY": "wayland-1"}
        common = (inherited, "fresh-profile", "scenario", "report", "fixtures", "clip.mp4")
        replay = benchmark_env(*common)
        self.assertEqual((replay["NT_LIVE"], replay["NT_VIDEO_OVERRIDE"], replay["NT_FIXTURE_MODE"]),
                         ("0", "clip.mp4", "replay"))
        self.assertNotIn("NT_MAX_HEIGHT", replay)
        live = benchmark_env(*common, ipc="/tmp/example/mpv")
        for name in ("NT_VIDEO_OVERRIDE", "NT_FIXTURES", "NT_FIXTURE_MODE", "NT_SNAPSHOT_DIR",
                     "XDG_ACTIVATION_TOKEN", "NT_MAX_HEIGHT"):
            self.assertNotIn(name, live)
        self.assertEqual(live["NT_MPV_OPTS"], "input-ipc-server=/tmp/example/mpv")
        self.assertEqual([live[key] for key in ("NT_LIVE", "NT_MUTE", "NT_NO_MPRIS", "NT_PROBE")],
                         ["1"] * 4)
        capped = benchmark_env(*common, ipc="/tmp/example/mpv", height_override=360)
        self.assertEqual(capped, {**live, "NT_MAX_HEIGHT": "360"})

    def test_cooldown_is_acted_on_before_report(self):
        with self.assertRaises(Cooldown) as failure:
            check_log_line(b"nt-core: YouTube signalled a bot check; pausing all requests to it for 30 min\n", True)
        self.assertEqual(failure.exception.seconds, 31 * 60)
        with self.assertRaises(Cooldown):
            RunLog(io.BytesIO(b"mpv: HTTP error 429\n"), 0, ipc="test").poll()
        with self.assertRaisesRegex(RuntimeError, "timed out"):
            check_log_line(b"NT_AUTOPILOT: timed out waiting for watch_loaded\n", False)

    def test_verifies_media_and_scrubs_signed_url(self):
        properties = {"path": "https://x.googlevideo.com/videoplayback?sig=secret",
                      "video-params": {"w": 1280, "h": 720}, "video-codec": "h264",
                      "container-fps": 30, "hwdec-current": "vaapi-copy", "mute": True, "pause": False}
        samples = [stream_values({**properties, "time-pos": pos}) for pos in (1.0, 1.5, 2.0)]
        self.assertNotIn("secret", json.dumps(samples))
        self.assertEqual(verify_stream(samples, 1080), {
            "width": 1280, "height": 720, "codec": "h264", "fps": 30, "decoders": ["vaapi-copy"],
        })
        self.assertEqual(verify_stream(samples, 720)["height"], 720)
        with self.assertRaisesRegex(ValueError, r"requested 360p cap was unavailable \(decoded 720p\)"):
            verify_stream(samples, 360)
        samples[1]["source"] = stream_values({"path": "/tmp/local.mp4"})["source"]
        with self.assertRaisesRegex(ValueError, "could not verify"):
            verify_stream(samples, 1080)

    def test_live_validation_binds_streams_to_watches_and_restored_home(self):
        events = RunLog(io.BytesIO(), 0)
        phases = {"playing": {"start_s": 0, "end_s": 6}, "switched": {"start_s": 7, "end_s": 13},
                  "recovered": {"start_s": 15, "end_s": 18}}
        events.player = [
            {"t_s": offset + pos, "source": "youtube_cdn", "width": 640, "height": 360,
             "codec": "h264", "fps": 30, "decoder": "vaapi", "position_s": pos,
             "muted": True, "paused": False}
            for offset in (0, 7) for pos in (1, 2, 3)
        ]
        events.states = [
            {"t_s": at, "page": page, "loading": False, "error": None, "video": video}
            for at, page, video in [
                (3, "watch", {"id": "video1", "first_frame": True}),
                (10, "watch", {"id": "video2", "first_frame": True}),
                (16, "home", None),
            ]
        ]
        with tempfile.TemporaryDirectory() as profile:
            library = Path(profile) / "library.json"
            library.write_text(json.dumps({"history": [{"id": "video2"}, {"id": "video1"}]}))
            evidence = verify_live(events, phases, library, 360)
            self.assertTrue(evidence["returned_to_home"])
            self.assertEqual([(video["id"], video["height"]) for video in evidence["videos"]],
                             [("video1", 360), ("video2", 360)])
            with self.assertRaisesRegex(ValueError, "video video1: .* requested 240p cap was unavailable"):
                verify_live(events, phases, library, 240)
            events.states.pop()
            with self.assertRaisesRegex(ValueError, "return to the loaded Home feed"):
                verify_live(events, phases, library, 360)

    def test_comparable_videos_and_bounded_workload(self):
        first = {"id": "video1", "width": 1280, "height": 720, "codec": "h264", "fps": 30}
        second = {**first, "id": "video2"}
        expected = comparable_key({"videos": [first, second]})
        self.assertNotEqual(expected, comparable_key({"videos": [second, first]}))
        for key, value in (("id", "video3"), ("width", 1264), ("height", 480), ("codec", "vp9"), ("fps", 60)):
            with self.subTest(key=key):
                self.assertNotEqual(expected, comparable_key({"videos": [first, {**second, key: value}]}))
        steps = live_steps()
        self.assertEqual(sum(step.get("cmd") == "open_video" for step in steps), 2)
        self.assertEqual(sum(step.get("cmd") == "search" for step in steps), 1)
        for mode in ("quick", "soak"):
            nav = steps_for(mode)["navigation"]
            self.assertEqual(sum(step.get("cmd") == "open_video" for step in nav), 8 if mode == "quick" else 16)


@unittest.skipUnless(sys.platform.startswith("linux"), "samples actual Linux procfs")
class ProcfsTests(unittest.TestCase):
    def test_exit_between_opening_and_reading_process_stat(self):
        harness = load_harness()
        with subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"]) as process:
            stat = Path(f"/proc/{process.pid}/stat")
            with stat.open() as opened:
                def read_after_exit(*args, **kwargs):
                    process.terminate()
                    process.wait(timeout=5)
                    return opened.read()

                with patch.object(Path, "read_text", side_effect=read_after_exit):
                    self.assertFalse(harness.alive(process.pid))

        with patch.object(Path, "read_text", side_effect=PermissionError("not permitted")):
            with self.assertRaises(PermissionError):
                harness.alive(1)

    def test_samples_real_parent_and_memory_allocating_child(self):
        child = "import time; ram = bytearray(12 * 1024 * 1024); ram[::4096] = b'x' * 3072; time.sleep(5)"
        parent = f"""import subprocess, sys
with subprocess.Popen([sys.executable, '-c', {child!r}]) as child:
    print(child.pid, flush=True)
    try:
        sys.stdin.readline()
    finally:
        child.terminate()
"""
        with subprocess.Popen([sys.executable, "-c", parent], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, text=True) as process:
            try:
                child_pid = int(process.stdout.readline())
                with Sampler(process.pid, time.monotonic()) as sampler:
                    time.sleep(0.9)
                self.assertGreaterEqual(len(sampler.samples), 3)
                measured = snapshot(process.pid)
                self.assertEqual({int(row["id"].split(":")[0]) for row in measured},
                                 {process.pid, child_pid})
                row = next(row for row in measured if row["id"].startswith(f"{child_pid}:"))
                self.assertGreaterEqual(row["private_mib"], 12)
                self.assertGreaterEqual(row["rss_mib"], row["pss_mib"])
                self.assertGreater(phase_metrics(sampler.samples)["peak_pss_mib"], 12)
            finally:
                process.stdin.close()
                process.wait(timeout=5)


if __name__ == "__main__":
    unittest.main()
