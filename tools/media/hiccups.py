#!/usr/bin/env python3
"""Build the historical incident catalog from cited project records."""
from datetime import datetime
from pathlib import Path
import subprocess

from metrics import MILESTONES, OUT, ROOT, write_json

LEDGER = ROOT / ".cache/orchestrate/native-tube/ledger.tsv"
PICKUP = ".cache/orchestrate/native-tube/pickup-2026-11-20.md"
UPSTREAM = "docs/upstream/"
BLOG = "docs/blog/coordinator-notes.md"
REVIEW = "review-sol"
KINDS = ("crash", "freeze/stall", "wrong behaviour", "perf regression", "review finding",
         "test-tool or machine incident", "model/usage-limit interruption", "privacy/process incident")


def git_date(rev):
    return subprocess.check_output(
        ["git", "show", "-s", "--format=%aI", rev], cwd=ROOT, text=True).strip()


def catalog():
    ledger = LEDGER.read_text().splitlines()
    observed_now = datetime.now().astimezone().isoformat(timespec="seconds")
    evidence = {f"L{i}": (line.split("\t")[0], f"{LEDGER.relative_to(ROOT)}:{i}")
                for i, line in enumerate(ledger, 1) if line.startswith("2026-")}
    evidence["pickup-rune"] = (datetime.fromtimestamp((ROOT / PICKUP).stat().st_mtime).astimezone()
                               .isoformat(timespec="seconds"), PICKUP)

    def stamp(ref):
        if ref is None:
            return None
        return evidence[ref][0] if ref in evidence else git_date(ref)

    rows = [
        ("GPUI could retain the previous video's picture after a switch", "GPUI", KINDS[2],
         "frontend review", "cb25193", "2caf0fa", ["docs/review/frontends.md"]),
        ("A failed GPUI player creation could disable playback for the session", "GPUI", KINDS[4],
         "frontend review", "cb25193", "2caf0fa", ["docs/review/frontends.md"]),
        ("Rune could load a previous topic's results under the newly selected chip", "Rune", KINDS[4],
         "frontend review", "cb25193", "2caf0fa", ["docs/review/frontends.md"]),
        ("Previous video could publish a stale frame after load or stop", "core", KINDS[2],
         "core review", "48d80a8", "48d80a8", ["docs/review/core.md"]),
        ("Test playback could be unmuted through iced", "core", KINDS[4],
         "core review", "48d80a8", "48d80a8", ["docs/review/core.md"]),
        ("Play state was interpreted as Ended after a dropped or expired URL", "core", KINDS[2],
         "core review", "48d80a8", "bf276b1", ["docs/review/core.md", BLOG]),
        ("Image cache byte accounting caused churn", "core", KINDS[3],
         "core review", "48d80a8", "714ad01", ["docs/review/core.md"]),
        ("Playback seek snapped to keyframes instead of the requested time", "core", KINDS[2],
         "viewing review", "312b946", "312b946", ["docs/review/viewing.md"]),
        ("The screen could lock while a video played", "core", KINDS[2],
         "coordinator", "4c94cbc", "4c94cbc", [BLOG, "docs/review/viewing.md"]),
        ("GPUI re-entrant PlayerView update", "GPUI", KINDS[0], "owner / GPUI agent", "2caf0fa", "2caf0fa", [UPSTREAM+"gpui-reentrant-keystroke.md", BLOG]),
        ("Search field lost focus on layout change", "iced", KINDS[2], "iced agent", "2caf0fa", "2caf0fa", [UPSTREAM+"iced-focus-sibling-width.md", BLOG]),
        ("Wrapped LiveText aborted in release", "Rune", KINDS[0], "Rune agent", "fb30202", "fb30202", [UPSTREAM+"rune-wrapped-livetext.md", "apps/rune/NOTES.md", "git:6ab17e4"]),
        ("GPU related-video switches intermittently stalled", "Rune", KINDS[1], "coordinator", "L11", "L12", ["apps/rune/NOTES.md"]),
        ("GPU switches initially showed black frames", "Rune", KINDS[2], "coordinator", "L11", "L12", ["apps/rune/NOTES.md"]),
        ("GPU playback mark preceded the first real frame (#5)", "GPUI", KINDS[2], REVIEW, "L20", "L21", ["apps/gpui/NOTES.md"]),
        ("GPU playback mark preceded the first real frame (#5)", "iced", KINDS[2], REVIEW, "L20", "L23", ["apps/iced/NOTES.md"]),
        ("Storyboard size overflow (#1)", "core", KINDS[0], REVIEW, "L20", "L26", [PICKUP]),
        ("Dub switch could block the UI (#3)", "core", KINDS[1], REVIEW, "L20", "L26", [PICKUP]),
        ("Dub and quality changes raced (#4)", "core", KINDS[2], REVIEW, "L20", "L26", [PICKUP]),
        ("Resume position could be discarded on exit (#8)", "core", KINDS[2], REVIEW, "L20", "L26", [PICKUP]),
        ("Thumbnail CDN cooldown policy (#11)", "core", KINDS[4], REVIEW, "L20", "L26", [PICKUP]),
        ("Empty first comments page replayed page 1 (#10)", "iced", KINDS[2], REVIEW, "L20", "L22", ["apps/iced/NOTES.md"]),
        ("Playlist requests could apply out of click order (#9)", "Rune", KINDS[2], REVIEW, "L20", "pickup-rune", [PICKUP]),
        ("GPU video spilled beyond the visible region (#6)", "GPUI", KINDS[2], REVIEW, "L20", "L24", ["apps/gpui/NOTES.md"]),
        ("GPU video lacked partial-scroll clipping (#6)", "Rune", KINDS[2], REVIEW, "L20", "pickup-rune", [PICKUP]),
        ("GPU video disappeared during partial scroll (#7)", "iced", KINDS[2], REVIEW, "L20", "L22", ["apps/iced/NOTES.md"]),
        ("Fixture keys depended on serde_json object order", "core", KINDS[2], "benchmark run", "48d80a8", "48d80a8", [BLOG, "docs/review/core.md"]),
        ("libmpv v0.42 parsing broke loadfile compatibility", "core", KINDS[2], "Opus core agent", "L13", "L13", []),
        ("CUDA probing added approximately 1.1 s during GPU attach", "core", KINDS[3], "Opus core agent", "L13", "L13", []),
        ("Idle PSS rose approximately 54 MiB", "iced", KINDS[3], "no-input perf gate", "L17", "L19", [PICKUP]),
        ("First frame rose approximately 201 ms", "Rune", KINDS[3], "no-input perf gate", "L17", "L28", [PICKUP]),
        ("Early injected keys were dropped", "test tooling", KINDS[5], "coordinator", "L6", "L6", []),
        ("Test input left the pointer on a headless output", "test tooling", KINDS[5], "owner / coordinator", "408263d", "408263d", [str(LEDGER.relative_to(ROOT).parent/"preferences.md"), BLOG]),
        ("Claude Opus usage cap paused active units", "orchestration", KINDS[6], "coordinator", "L2", "L7", [BLOG]),
        ("Codex Sol usage cap paused active units", "orchestration", KINDS[6], "coordinator", "L4", "L7", [BLOG]),
        ("iced Sol session reached its usage cap", "orchestration", KINDS[6], "coordinator", "L10", "L19", []),
        ("Lint (hawk) Sol session reached its usage cap; coordinator finished the check", "orchestration",
         KINDS[6], "coordinator", "L29", "L29", []),
        ("iced retained memory after leaving Watch regressed", "iced", KINDS[3], "coordinator", "L33", "L36", []),
        ("GPUI wheel-scroll CPU regressed", "GPUI", KINDS[3], "coordinator", "L33", "L38", []),
        ("iced click-to-first-frame and warm switch over gate budget (core GPU attach)", "iced", KINDS[3],
         "no-input perf gate", "L36", "L44", []),
        ("Commits were pushed with a private author identity", "orchestration", KINDS[7], "coordinator",
         "L34", "L35", []),
        ("Input pause file was lost on reboot", "test tooling", KINDS[5], "coordinator", "L37", "L37", []),
        ("Opus weekly usage limit paused GPUI, core and media work", "orchestration", KINDS[6],
         "coordinator", "L37", "L37", []),
    ]
    incidents = []
    for i, (title, area, kind, finder, found, fixed, sources) in enumerate(rows, 1):
        sources = list(dict.fromkeys(sources + [evidence.get(ref, (None, f"git:{ref}"))[1]
                                               for ref in (found, fixed) if ref]))
        incidents.append(dict(id=i, title=title, date=stamp(found), area=area, kind=kind,
                              found_by=finder, fixed_when=stamp(fixed), sources=sources))
    return {
        "milestones": {rev: git_date(rev) if rev != "NOW" else observed_now for rev in MILESTONES},
        "incidents": incidents,
        "unverified": ["Cursor flicker as a separate incident", "Gemini usage-limit interruption"],
        "date_basis": "First dated record available. Same-day code and documentation may bound an earlier event.",
    }


if __name__ == "__main__":
    write_json(OUT / "hiccups.json", catalog())
