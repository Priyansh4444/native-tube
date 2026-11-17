# Review council

Shared background for every reviewer. Paths are relative to the repository root.

NativeTube is a native YouTube client for Linux built three times, with iced, GPUI and Rune
(apps/iced, apps/gpui, apps/rune), on one shared Rust backend (core/, crate nt-core: YouTube
data, images, local library, libmpv playback, Chromium scroll physics port, metrics, autopilot).
The owner wants (1) the three frameworks compared on build cost with an AI agent, performance,
memory, CPU and complexity, including a bare "raw shell" baseline per framework and the real
YouTube website as a reference, and (2) three apps that are genuinely excellent to use.

The owner's verdict on the current state: start-up speed and other important metrics aren't
being measured, the apps don't feel like YouTube, and the UI is subpar. Scrolling must feel as
good as Chrome. The viewing experience must be uncompromised. Nothing ships until reviewed.

Read first: docs/frontend-spec.md (what the apps were built to), docs/scroll.md,
apps/*/NOTES.md (what each builder says they did), bench/ (scenario.json, run-scenario.sh,
playback.py, results/), core/src/lib.rs.

Rules for reviewers:
- You are reviewing, not fixing. Do not edit anything except your own findings file,
  docs/review/<your-name>.md. Do not commit.
- Be concrete: every finding names the file and line or the screenshot, says what is wrong,
  why it matters to the owner's goals, and what the fix is. Rank findings: P0 (broken or
  misleading), P1 (clearly below the bar), P2 (polish). Say what you verified by running
  something versus what you concluded from reading.
- Don't praise. Don't pad. If something is fine, one line saying so is enough.
- Hard constraints you must not recommend against: the apps run pure Rust plus native
  libraries only (no JavaScript, Lua, Python or helper processes at run time); the app never
  signs in to YouTube; traffic to YouTube stays small (use recorded data: test runs through
  tools/headless.sh replay .cache/fixtures and play a local clip); all playback during tests
  is muted; never open a window on the owner's real screen (tools/headless.sh puts test
  windows on off-screen outputs).
- Do the work yourself; do not start subagents.
