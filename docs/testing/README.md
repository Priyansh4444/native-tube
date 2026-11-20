# NativeTube property tests

Run either command from the repository root.

```bash
tools/test-all.sh
tools/test-all.sh --baseline 2caf0fa
```

The command runs `./check.sh`, the explorer's Rust checks, core tests, Bombadil and the native
explorer. It prints a summary and writes per-stage logs to `tests/bombadil/out/all-*/`. It exits
nonzero when a stage fails. Native stages use the existing `apps/*/target/release/nt-*` binaries
and report a skip for a missing binary or a running binary that emits no probe. The second command
also runs the performance gate against the supplied baseline revision.

The terminal and native stages replay `.cache/fixtures` and the native stage uses
`.cache/test-clip-720p30.mp4` through `tools/headless.sh`. All native playback is muted and MPRIS
is disabled. Use `NT_FIXTURES` for another recording directory. The aggregate runner installs the
pinned Bombadil npm package on first use. It defaults to 180 seconds total for Bombadil
and at most two minutes per native app. Set `NT_BOMBADIL_SECONDS=30`,
`NT_EXPLORE_MINUTES=0.5`, or `NT_EXPLORE_SEED=19` for shorter or seeded runs. Native runs require a Linux Hyprland session, `grim`, `jq`, `wtype`, and util-linux.
Run one GUI driver at a time. The aggregate runner lowers CPU and I/O priority and sets
`CARGO_BUILD_JOBS=4`.

## Core proptest

```bash
cd core
cargo test --locked --offline
PROPTEST_SEED=859 PROPTEST_CASES=1024 cargo test --locked --offline proptests::scroll_any_finite
```

A failure prints the shrunk input. The core unit properties try saved seeds first from
`tests/proptest-regressions/proptests.txt`. The cache and other integration tests print their
counterexamples in the parent test output. Parser mutations use recorded search, watch, comment,
channel, and continuation replies when available. A minimal search renderer is also included so
a clean checkout can exercise the parser without recordings. Core tests never need YouTube or a
local HTTP server.

| Property | Why it matters |
| --- | --- |
| Scroll operations keep offset and target finite and inside the range, and report finite velocity | A malformed delta or geometry change cannot strand the view at an invalid offset. |
| Scrolling runs out of frames after at most 151 ticks at 60 Hz | The UI can return to idle after input stops. |
| Programmatic scrolling reaches the exact clamped destination | Keyboard, navigation and restoration can rely on the endpoint. |
| An individual wheel, touchpad, keyboard or fling gesture moves in the direction of its sign | A one-way gesture cannot jump backward. |
| Decoded image admission, reuse, eviction and retries match the reference LRU | Byte counts reflect cached entries. A failed request leaves other entries intact and can be retried. |
| Thumbnail loading follows the first usable source and preserves the selected source's pixels through resizing | A missing or invalid variant does not hide a recorded thumbnail. |
| The polite gate allows at most three in-flight requests and spaces starts by at least 150 ms | Bursts of queued requests still respect the request budget. |
| Cooldowns reject immediately, retain the latest deadline and expire at that deadline in paused time | A new request cannot bypass or shorten a 429, 403 or bot-check pause. |
| Replay misses return a fixture error and the same continuation works once recorded | A failed continuation leaves its cursor reusable by the Retry row. |
| Canonical JSON preserves the value and is idempotent and key-order invariant | Fixture keys survive dependency feature changes. |
| Mutated recorded responses never panic or emit an empty video ID | Unexpected payloads cannot crash the UI or create unopenable entries. |
| Stream selection excludes AV1, obeys the height cap when possible and ranks resolution, FPS and AVC before VP9 | A choice stays playable and respects quality settings. |
| Audio prefers the original language, then ordinary audio, AAC and bitrate | The default audio selection avoids switching the viewer to a dub. |
| Loudness gain is in −96 to 0 dB and never increases as the input gets louder | Quiet media is never boosted and loud media is attenuated consistently. |
| The library survives loading, writing and reloading arbitrary contents | Saving one setting preserves the rest of the file. |
| A corrupt library is preserved byte for byte and loads safe defaults | Recovery leaves the user's original data available. |
| Printed times parse back to whole seconds, relative time floors to the correct unit, and counts round without displaying `1000K` or `1000M` | Labels remain consistent at rounding and unit boundaries. |

## Terminal through Bombadil

```bash
CARGO_BUILD_JOBS=4 nice -n 19 ionice -c3 cargo build --locked --manifest-path core/Cargo.toml --bin ntrepl
npm ci --prefix tests/bombadil
cargo run --locked --manifest-path core/Cargo.toml --bin ntrepl
tests/bombadil/run.sh explore 180 tests/bombadil/out/terminal
tests/bombadil/run.sh test --exit-on-violation --reproduce tests/bombadil/out/terminal/session-1 \
  --output-path tests/bombadil/out/replay
tests/bombadil/run.sh shrink --trace tests/bombadil/out/terminal/session-1 \
  --property backRestoresPreviousTitle --time-limit 1m --output-path tests/bombadil/out/shrunk
```

Paths passed to `run.sh` are relative to your working directory. Keep the same
binary, fixtures and terminal dimensions when reproducing. The wrapper keeps those dimensions and
the quiescence setting consistent across tests, replay and shrink. Provide `--property` if more
than one property failed. A normal trace finishes when the REPL exits or the time limit is
reached.

The REPL supports `home [topic]`, `search <query>`, `more`, `open <n>`, `watch <id>`,
`comments`, `replies <n>`, `channel <id>`, `back`, `state`, and `quit`. Its output makes
fixture misses visible. The spec draws choices from screen numbers, history, channels and
continuations, and also types random input.

- Default properties require valid terminal output and a successful exit. A separate property
  rejects panic diagnostics on screen.
- Each command returns to its numbered prompt within five seconds. This exposes a stuck request
  or a command that forgets to redraw.
- Back restores the previous title and decrements history depth. A skipped redraw or no-op
  restore fails even when the same title appears twice.
- Search shows the submitted query and `state` reports the same query. This detects stale search
  state leaking across commands.
- Quit exits with code zero, so the terminal driver can stop cleanly.

## Native explorer

```bash
cargo build --locked --release --manifest-path tools/nt-input/Cargo.toml
cargo test --locked --manifest-path tools/nt-explore/Cargo.toml
cargo build --locked --manifest-path tools/nt-explore/Cargo.toml
explore=tools/nt-explore/target/debug/nt-explore
nice -n 19 ionice -c3 "$explore" apps/iced/target/release/nt-iced --seed 19 --steps 100 --minutes 2
"$explore" --replay tools/nt-explore/out/run-EXAMPLE/explore/trace.json
"$explore" --shrink tools/nt-explore/out/run-EXAMPLE/explore/trace.json
```

Each run writes the input actions, health samples, probe states and violation to `trace.json`,
with `app.log` and a PNG if a violation was captured. The explorer owns NT-BENCH for the run,
verifies the window's app ID and monitor before sending input via `nt-input`. The detached
guardian stops the app when the explorer closes its lease, including after SIGKILL.
The input driver's own watchdog restores the desktop if an input command is killed. Each
attempt uses a fresh library and has a two-minute cap. Fixed seeds pick the same actions given the same probe
observations. Replay uses the recorded actions and target labels because timing or a changed
layout can change the seed's path. Shrink restarts the app for each candidate and removes
contiguous action blocks only when the same property still fails. An optional binary argument
overrides the path in the trace. Exit 3 indicates no probe from a live process; exit 4 means
another GUI test (any `nttest-*` window or an NT-BENCH session) holds the desktop, so nothing
ran, and `test-all.sh` reports that stage as `SKIP (desktop busy)`.

| Property | Why it matters |
| --- | --- |
| Process remains alive and logs no panic | Detects crashes regardless of the current page. |
| `0 ≤ scroll_y ≤ max_scroll` | Scrolling and page restores keep content inside the viewport range. |
| While rail entries are drawn, `rail_selected` is the root page's id (`home`, `subscriptions`, `history`, `watch_later`); search, watch and channel select nothing; with no rail drawn (narrow window, fullscreen) a root page may report `null` | Navigation chrome always represents the current page, and the probe reports the same ids in every app. |
| PSS stays below 1500 MB | Large allocations are visible during repeated navigation and playback. |
| Alt+Left restores the modeled page, query, scroll within 4 px and video ID within 2 s | Back remembers the view the user actually left. |
| Clicking a card, row or related item produces a watch page with a video ID and displayed frame within 6 s | A click cannot pass by leaving the previous video playing. |
| Escape dismisses an open menu within 500 ms | Focus or overlays cannot trap the user. |
| Typing and submitting search shows results for that query within 6 s | Old results cannot satisfy the new request. |
| Loading clears within 10 s of each action | A stalled continuation or transition is reported. |

### Leaving the desktop as it was

The owner uses the desktop during runs, so every way a GUI run can end must give back the real
cursor and focus and stop the app:

- Only `tools/nt-input` touches the pointer or focus; the explorer never calls `hyprctl`
  dispatchers. Before each input it re-reads the window's geometry and refuses if any other
  `nttest-*` window is open, the window moved, or it is not on NT-BENCH.
- nt-input children are bound to the explorer with `PR_SET_PDEATHSIG`. An input that hangs past
  5 s gets SIGTERM, then SIGKILL; nt-input's detached watchdog restores the cursor and focus
  however it exits.
- The app runs under a guardian holding a socket lease from the explorer; when the explorer
  exits or is SIGKILLed the lease closes and the guardian stops the app (tested in
  `tools/nt-explore/tests/guard.rs`).
- A session stops by itself at 118 s; shrinking runs at most 12 replays, with a pause between.
- `tools/test-all.sh` runs each stage niced, bound to the script (`setpriv --pdeathsig`), and
  stops the running stage on Ctrl-C or kill, so a killed test run leaves no explorer behind.

## RAM while using YouTube

Build the release binaries you want to compare, then run one of these from the repository root.

```bash
tools/check-performance.sh --live
tools/check-performance.sh --live iced
tools/check-performance.sh --live --height 360
```

Each app gets one fresh logged-out profile and a 1280×800 off-screen NT-BENCH window. Its
autopilot opens Home, scrolls through thumbnails, searches for `Big Buck Bunny Blender Foundation`,
plays the first two video results for seven seconds each after startup, and returns to Home.
Playback is muted, autoplay and MPRIS are disabled, and autopilot leaves the desktop pointer and
focus alone. The harness sets `NT_LIVE=1` and clears inherited fixture and video override
settings. It relies on the app's existing request pacing and stops the batch on a timeout,
refusal, or core cooldown signal. A cooldown reported by the app also delays later harness runs
until it expires. Each window is bounded to a short slice under two minutes.

For typical use, omit `--height` to use the app's Auto quality setting with its built-in 1080p
cap. Auto picks a quality using the drawn player height, so the decoded stream in the test
window can be lower than 1080p. Pass `--height 360` to repeat the cap used for the preliminary
single-journey measurements, which decoded 360p. Both forms clear inherited `NT_MAX_HEIGHT`.

The live report records the requested cap and its source, then verifies two distinct video IDs
from fresh history, advancing CDN playback, and a return to the loaded Home page. Read-only
libmpv IPC provides the actual decoded dimensions, codec, FPS and decoder. The report retains
the CDN classification without the signed URL and records available render-path log signals.
A decoded height above the cap makes the run incomplete. Apps are marked comparable when both
ordered video IDs and their actual dimensions, codecs and FPS match. Search rankings, thumbnail
sets, buffering and decode or rendering paths can still vary. Each app's default renderer is
part of the measured workload. The IPC socket and state probe introduce measurement overhead.

The table in `.cache/perf/out/live-*/summary.md` shows PSS, RSS and private RAM before browsing,
while scrolling, in both settled playback windows, at the sampled playback and journey peaks,
and after returning to Home. Retained memory compares the settled Home values within the same
run. Treat one short return to Home as a retention observation. See `results.json` for the exact
binary hashes and comparable status, and the per-app directory for the resource samples,
media metadata and app trace. An incomplete or different-stream run exits nonzero. Live
results describe the running release binaries. Rebuild from the intended app and core revisions
before using these numbers to evaluate a code change.

## Repeatable performance gate

```bash
tools/check-performance.sh 2caf0fa
tools/check-performance.sh 2caf0fa iced
tools/check-performance.sh --soak 2caf0fa iced
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=tools python3 -m unittest discover -s tools/performance -v
```

The gate snapshots the existing release binaries and builds the requested revision in a
detached worktree under `.cache/perf/`. Builds use locked Cargo dependencies, four Cargo jobs
and low CPU and I/O priority. The worktree is removed before measurement. The harness holds
the NT-BENCH explorer lock and checks for other native test windows before each launch.

The gate records fixture replay and the local muted clip as its provenance. Each story and
repeat gets a fresh profile. Three baseline and three candidate runs alternate order, and
the gate compares their medians. Quick mode runs eight Watch/Back cycles and six seconds of
settled playback before switching videos. Soak mode runs 16 cycles and 20 seconds of playback
before switching. Both modes bound each GUI launch to a slice under two minutes.

| Story | Workload and primary metrics |
| --- | --- |
| Load | Launch, first UI draw and first loaded Home content. Measure settled Home RAM and idle CPU. |
| Browse | Scroll and fling through Home, then return to the top. Measure memory, CPU and draw gaps. |
| Play | Open Home's second card, play the local clip, then open the first related video. Measure the two app first-picture delays, playback peaks and CPU, and RAM retained after Back returns to Home. |
| Navigation | Open the same fixture-backed Watch page and go Back repeatedly. Compare settled RAM, FD and thread counts across cycles. |

Limits in `tools/perf-budget.json` are allowed increases relative to the chosen baseline,
with an absolute stripped-size cap per app. Existing per-app PSS headroom is applied to
idle, playback, retained and navigation memory. CPU allowance is in percentage points on
the same machine. Additional measured metrics can be budgeted by their story and metric
key. Missing required data, a failed story or an exceeded budget causes a nonzero exit.
Report-only metrics carry an explicit unavailable status when data is incomplete. The
output directory contains the scenarios, binary and clip hashes, fixture index hash, budget
decisions, raw samples and original app logs and reports.

### How measurements map to Chromium

- Independent UX stories and latency from launch or an action to an app milestone follow
  Chromium's System Health approach. Home and search content marks represent screen content
  here, and playback marks indicate a video picture at the app's draw or render hook.
  [Chromium System Health](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/docs/speed/benchmark/harnesses/system_health.md)
- The load, browse, play and repeat-navigation stories use external Linux process-tree
  accounting. PSS apportions shared mappings, RSS includes the full resident size in each
  process, and private RAM sums private clean and dirty resident pages. Swap is sampled
  separately. The harness requests samples every 250 ms and reports sampled maxima and the
  median of the last three samples in a settled phase. Phase windows align to the observed
  Home mark and exclude their edges. In navigation, the last three settled values are
  compared with the preceding three to measure late growth after warm-up. The warmed retained
  value also reports the final window versus initial Home. A high retained value with flat
  late growth is consistent with bounded caches over the tested duration.
  [Chromium memory benchmarks](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/docs/memory-infra/memory_benchmarks.md)
- CPU time is user plus system CPU and 100% is one fully used core. CPU deltas and the
  context-switch proxy use observed process and task identities in adjacent reads and may
  miss short-lived work. Threads, FDs and process counts are snapshots. Context switches
  measure scheduling activity. The gate has no wakeup or energy instrumentation. The
  headless window stays unfocused throughout, so there is no background-transition metric.
  [Chromium System Health metrics](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/docs/speed/benchmark/harnesses/system_health.md)
- The rendering reports use the existing app draw hooks for p50, p95 and p99 gaps during
  wheel, fling and playback. Video picture counts come from the existing libmpv render
  hook where available. These hooks cannot attribute per-input scroll latency, compositor
  presentation, missed vsyncs, dropped pictures or GPU allocations. The baseline replay
  navigation uses Back commands and post-action sampling. Live mode also verifies the
  restored page through the current app's state probe.
  [Chromium rendering benchmarks](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/tools/perf/benchmarks/rendering.py)
