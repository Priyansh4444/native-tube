# NativeTube media measurements

Measured 2026-11-24. NOW uses the landed working-tree snapshot from the media build manifest. The historical benchmark runs and their receipts are preserved.

## Normalized results

The laptop's GPU playback uses **0.59–0.65% of its 16 logical CPUs** (0.095–0.105 cores busy). Software playback uses **2.76–2.83%** (0.44–0.45 cores), versus **2.26%** (0.36 cores) for `ntctl play`. The apps retire about **54 instructions per expected frame-pixel** at 848×476, versus **26** for `ntctl play` at 1280×720. The ratio includes fixed per-frame work. [Normalized tables, controls and warm audit](normalized.md)

## Builds

| Milestone | Commit date | What it is |
|---|---|---|
| 96df4ec | 2026-11-12 | Frontends as first delivered |
| cb25193 | 2026-11-16 | Lint gate and review fixes |
| bf276b1 | 2026-11-16 | Spec v2 |
| 2caf0fa | 2026-11-18 | Baseline before the recent rounds |
| NOW | measured last | `tools/media/build.sh --with-core NOW`: HEAD plus the working-tree changes under apps, core and assets. The SHA-256 of every source file is in `.cache/media/out/build/NOW-source.sha256` |

Each build is a `--locked --release` build in its own target directory under `.cache/media/target/<rev>/<app>`. Build commands run under `nice -n 19 ionice -c3` with `CARGO_BUILD_JOBS=4`. Before 66f442f, 96df4ec lacked the `nttest-<app>` app ID that the off-screen window rules need. Its runtime tests therefore use `96df4ec-routed`, which is 96df4ec plus only the three `main.rs` app-ID lines from 66f442f. Its stripped size uses the unmodified build. The history rewrite of 2026-11-20 touched only commits after 2caf0fa, so all four milestone hashes are unchanged.

## Conditions

Every run follows the same rules:

- Muted, `NT_NO_MPRIS=1`, its own temporary data directory, and autoplay off.
- `NT_FIXTURE_MODE=replay` against a frozen copy of the fixture corpus (`.cache/media/fixtures`, index SHA-256 `8e94547e…`).
- `NT_VIDEO_OVERRIDE` uses the same 1280×720, 30 fps H.264 clip for every app and build (SHA-256 `a9766ffa…`). All measured slices and recordings use replay. One untimed warm-up used the app's normal gated client to fill missing tour metadata in a private fixture copy while playback stayed on the local clip.
- Apps run on the off-screen `NT-BENCH` output (1280×800, scale 1) under the `nt-explore --guarded-app` lease, exactly as `tools/check-performance.py` runs them.
- Each run is one slice. It takes the shared `bench.explore.lock` and refuses to start while any `nttest-*` window or live headless PID exists. It is capped at 110 s by an alarm, and it releases the lock before the next run.
- No synthetic input. The apps are driven only by `NT_AUTOPILOT`.

Runs are interleaved round-robin (run 1 of every build, then run 2, then run 3), so changing background load spreads across all builds. Each receipt records power state and load. An earlier pass was taken partly on battery with the power-saver profile, where the same builds measured about 50% slower. It was discarded and is kept only as `.cache/media/out/metrics-v1` and `metrics` for reference. All 36 historical runs in the final pass (`.cache/media/out/metrics-final`) ran on AC with `balance_performance`, at a 1-minute load of 3.6–5.9 on 16 threads.

The NOW timing and counter runs also used AC with `balance_performance`. All 39 endpoint resource runs used battery with the `power` profile, including the matched standalone `ntctl` reference. Keep the two power conditions in view when comparing across tables.

## Milestone metrics

Medians of three runs (`tools/media/collect.py metrics`, scenario `tools/media/measure.json`: Home, wait 4 s, open video 1, 6 s playback phase, open the first related video).

| Build | App | Window ms | Home ms | Open→frame ms | Warm switch ms | Idle PSS MiB | Peak PSS MiB | Playback CPU % | Stripped MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 96df4ec | iced | 334 | 362 | 116† | n/a | 89 | 218 | 35 | 19.6 |
| 96df4ec | GPUI | 396 | n/a | n/a | n/a | n/a | n/a | n/a | 46.4 |
| 96df4ec | Rune | 309 | 337 | 99† | n/a | 128 | 235 | 40 | 38.5 |
| cb25193 | iced | 261 | 287 | 103† | n/a | 89 | 217 | 37 | 19.6 |
| cb25193 | GPUI | 419 | n/a | n/a | n/a | n/a | n/a | n/a | 46.4 |
| cb25193 | Rune | 320 | 342 | 129† | n/a | 126 | 242 | 40 | 38.5 |
| bf276b1 | iced | 278 | 300 | 112 | 38 | 89 | 225 | 36 | 19.7 |
| bf276b1 | GPUI | 417 | 442 | 103 | 35 | 140 | 257 | 52 | 46.5 |
| bf276b1 | Rune | 296 | 313 | 106 | 88 | 124 | 231 | 39 | 38.6 |
| 2caf0fa | iced | 189 | 200 | 102 | 76 | 100 | 186 | 42 | 27.9 |
| 2caf0fa | GPUI | 350 | 377 | 164 | 83 | 113 | 248 | 41 | 50.7 |
| 2caf0fa | Rune | 183 | 209 | 161 | 44 | 109 | 262 | 42 | 40.2 |
| NOW | iced | 190 | 209 | 95 | 36 | 153 | 248 | 10.0 | 29.6 |
| NOW | GPUI | 293 | 310 | 118 | 43 | 186 | 281 | 10 | 52.7 |
| NOW | Rune | 238 | 252 | 189 | 50 | 167 | 295 | 9.0 | 42.2 |

How each number is measured:

- **Window ms**: from the guard's `CLOCK_MONOTONIC` stamp just before `spawn` to Hyprland's matching `openwindow` event for `nttest-bench`. PID and monitor are verified afterwards.
- **Home ms**: from the same stamp until the collector sees `NT_MARK home_loaded` in the app log, polled every 10 ms.
- **Open→frame**: from the autopilot's `open_video` command timestamp to the first `playback_started` before the next command. † 96df4ec and cb25193 don't log command timestamps. For them the command time is reconstructed as `home_loaded` + 4000 ms, since the scenario sends the command right after a 4 s sleep that follows the Home wait. On the 2caf0fa runs this reconstruction matched the logged command timestamps within 0.2 ms.
- **Warm switch**: the same measurement for the related-video open after six seconds of playback. The local override bypasses successor stream preparation, so the window measures switching on an already-running player. It is n/a for 96df4ec and cb25193, whose reports keep only the first mark of each name. Across the three NOW runs per app, the saved related switches span 30.1–54.0 ms. The earlier timestamped builds span 27.8–104.9 ms.
- **Idle PSS**: process-tree `Pss` from `smaps_rollup`, taken 2 s after Home.
- **Peak PSS**: the highest process-tree PSS sampled every 250 ms (the gate's `tools/performance/procfs.Sampler`) over the whole run: Home, playback and the warm switch.
- **Playback CPU**: the app's own CPU seconds over the six-second `playback` phase divided by wall time. 100% equals one core. It is counted only when video frames were rendered.
- **Stripped MiB**: the size of a `strip -o` copy of the release binary.

GPUI at 96df4ec and cb25193 shows "every home topic failed to load" (frame checked). These builds predate 48d80a8, which made fixture keys independent of serde_json object order, so their Home request key is missing from the replay corpus. Measuring them would need live YouTube through an outdated client. Only window map time and size are reported for them.

Charts (one line per app, a faint bar for the lowest to highest run, gaps where n/a): `window_ms.png`, `home_ms.png`, `open_ms.png`, `warm_ms.png`, `idle_pss_mib.png`, `peak_pss_mib.png`, `playback_cpu_percent.png`, `stripped_bytes.png`.

Startup got faster at 2caf0fa for iced and Rune. Home loaded in about 200 ms, down from about 300–360 ms. iced's binary grew from 19.6 to 27.9 MiB while its sampled peak PSS fell from about 220 to 186 MiB. GPUI playback CPU fell from 52% to 41%. Open→frame for GPUI and Rune rose to about 160 ms at 2caf0fa. Playback CPU stayed between 35% and 42% of one core for iced and Rune at every milestone.

### Home prewarm by build

| Build | iced default | GPUI default | Rune default | Cold Home hook |
|---|---|---|---|---|
| 96df4ec | Linked libmpv, player created on playback | Same | Same | None |
| cb25193 | Linked libmpv, player created on playback | Same | Same | None |
| bf276b1 | Linked libmpv, player created on playback | Same | Same | None |
| 2caf0fa | Player prewarmed after Home | libmpv loaded on first playback | libmpv loaded on first playback | iced only |
| NOW | Player and GPU surface prewarm | Player and GPU surface prewarm | Player prewarm | All three |

ELF dependency inspection confirms that all nine builds before 2caf0fa link libmpv at launch. The saved 96df4ec iced and Rune probes also observed it mapped at the idle-PSS sample. At 2caf0fa, libmpv loads dynamically and only iced explicitly prewarms its player. The idle-PSS chart marks this change. Its values describe each build's default Home footprint. The drop from 140 to 113 MiB for GPUI spans two different player loading regimes.

## Stability

`tools/media/stability.py`, three runs per build. Each run scrolls and flings on Home, opens a video, then 20 times opens the first related video and goes Back (waits of 1.6 s). On switches 1, 10 and 20 the autopilot screenshots `NT-BENCH` 250 ms after playback starts and again 2.05 s later. A central player crop counts as black when every channel mean is ≤ 8/255, and as frozen when the two crops are byte-identical. The test clip shows a running timecode, so a playing video always changes between the two crops.

Event weights: crash ×5 (exit before the report, or a panic or signal in the log), failed wait ×2, freeze ×3, black frame ×2, probe error ×1 (malformed `NT_STATE`, or a new visible error episode; probes exist only in newer builds), report timeout ×2. **Score = 100 × clean runs ÷ assessed runs.** A clean run has a report, all three visual checks and zero weighted events.

| App | 96df4ec | cb25193 | bf276b1 | 2caf0fa | NOW |
|---|---:|---:|---:|---:|---:|
| iced | 100 (3/3, w=0) | 100 (3/3, w=0) | 100 (3/3, w=0) | 100 (3/3, w=0) | 100 (3/3, w=0) |
| GPUI | n/a (replay key) | n/a (replay key) | 100 (3/3, w=0) | 100 (3/3, w=0) | 100 (3/3, w=0) |
| Rune | 100 (3/3, w=0) | 100 (3/3, w=0) | 100 (3/3, w=0) | 100 (3/3, w=0) | 100 (3/3, w=0) |

Chart: `stability-by-app.png`. All 30 assessable historical runs were clean. This is a coarse instrument: three runs per cell, and sampled frames that can miss a glitch between samples. It runs the default render path, which is software for these builds. The GPU-path stalls the coordinator found in Rune on 2026-11-19 were on a later build and the GPU path, so they don't appear here. They do appear in the incident history.

## Incident history

`tools/media/hiccups.py` builds the catalog from dated project records: `.cache/orchestrate/native-tube/ledger.tsv` lines, review documents, app NOTES, upstream reports and git dates. It contains 43 recorded incidents, 43 with dated fix evidence. The iced GPU attach regression closed when the follow-up gate passed (ledger line 44). Each incident is assigned to the first milestone at or after its record date. It counts as open at a milestone when it was found on or before that milestone's date and fixed later or not at all. The NOW checkpoint is the extraction time and spans a longer, better-documented period than the earlier intervals, so more incidents land there partly because more was written down.

Two candidate entries stay **unverified** and are not counted: "cursor flicker" as an incident separate from the stranded test pointer, and "a Gemini usage-limit stop". The ledger and coordinator notes hold no dated record of either.

Charts: `incidents-by-app.png` (found per interval and open at each milestone, for iced, GPUI, Rune and core), `incidents-open.png` (open by area), `incidents-found-fixed.png`, `incidents-by-kind.png`.

## RAM and player attribution

`tools/media/resources.py` on the NOW build measured, from three runs each:

- cold Home with `NT_NO_PREWARM=1`, available only on binaries with the hook and with every recorded `/proc/<pid>/maps` check readable and showing libmpv unmapped throughout the idle window
- warm Home with libmpv mapped at every check in that window. Current GPUI schedules player and GPU-surface prewarm 600 ms after startup and honors `NT_NO_PREWARM`. A diagnostic observed zero libmpv mappings with the hook and five without it. This confirms warm and cold Home can be measured when the final build is ready.
- six-second playback PSS and CPU on the software path (`NT_SOFTWARE_VIDEO`, or `NT_RUNE_SOFTWARE` for Rune) and on the GPU path (counted only when the app logs a GPU attach and no fallback)
- the tour peak
- `ntctl play` alone on the same clip, as the core plus libmpv reference

The attribution bars stack the core reference under the app's software-playback total. The signed residual includes frontend and integration costs. The approximate player share is core reference ÷ app total. Separate renderer settings, library sharing and viewport size can affect the reference.

## Media

The tours (`tools/media/tour.json`: Home, scroll, search "rust", open a result, watch 6 s, open a related video, Back twice) were recorded full screen on each app's own 1280×800 headless output. FFmpeg encoded silent H.264 at 30 fps, repeating the last frame when Grim missed a slot. The three recordings each contain about 350 distinct captures over 23 seconds. The 1280×1200 comparison uses bf276b1 for START, the earliest build with all five overlay metrics in all three apps. Videos stay in `.cache/media/out/`.

## CPU instructions, GPU work, and energy

`tools/media/counters.py` takes one guarded slice per app and scenario. It measures a scripted Home reload after startup, ten seconds of idle Home, the existing 27-tick wheel story, ten seconds each of GPU and forced-software playback on the same local clip, and five related-video opens from command to first frame. Back and recovery between switches fall outside the counted windows. The app's autopilot blocks at each boundary until perf acknowledges the requested state. For these slices only, the snapshot hook is a timing marker and produces no screen capture. The raw CSV and enabled-window durations stay beside each receipt.

CPU counters use `perf stat -p` for user-space instructions and cycles. The collector records unscaled values from `cpu_core`, `cpu_atom`, and `cpu_lowpower` and sums the observed counts. Unscaled values avoid extrapolating time spent on one core type to the rest of the window. It reports instructions per Home reload, per second of the wheel phase, and per rendered video frame reported by core. Core counts software-rendered frames and GPU swaps. A compositor presentation count is unavailable, so the per-frame metric uses renderer output.

`ntctl play` uses the same clip and collects ten seconds after the first frame. The software playback bars show this player-only CPU reference, adjusted to the app's enabled duration, and the signed app-minus-reference result. The app and control render different target sizes. The signed difference includes that geometry effect. GPU playback and the UI-only scenarios show their full app totals because `ntctl play` has no matching workload for them.

System-wide i915 and RAPL collection requires the coordinator's measurement window and `perf_event_paranoid <= 0`. With the usual setting it records “not measured.” Each available engine reports busy nanoseconds. Estimated busy cycles are busy ns × average GT frequency in MHz ÷ 1000. Average MHz is the i915 frequency accumulator divided by its enabled seconds. On this Arrow Lake-P iGPU, render, copy and compute use GT0 and video decode uses GT1. RC6 is residency time. Engines can be busy concurrently, so their sum is engine work. Package and GPU energy are separate, overlapping RAPL domains. Both GPU work and energy can include other system activity. [Linux i915 PMU implementation](https://raw.githubusercontent.com/torvalds/linux/master/drivers/gpu/drm/i915/i915_pmu.c) · [Linux i915 PCI device definitions](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/i915/i915_pci.c)

`tools/media/counter_charts.py` renders 1500 px PNGs for CPU instructions, estimated GPU busy cycles and joules.

## Privacy review

The retained recordings were screened frame by frame in both right corners with `tools/media/notify_scan.py` and visually reviewed with whole-frame contact sheets. Earlier captures with desktop overlays or unresolved flags were discarded and re-recorded. The final three tours had zero flags across 2,108 frames. The 705-frame comparison had two flags that matched the source app's scrolling thumbnails on inspection. The 81 retained NOW switch stills also passed review. The replacement Rune slice saved only the 270×128 player crop used by the test.

## Reproduce

```sh
tools/media/build.sh 96df4ec cb25193 bf276b1 2caf0fa 96df4ec-routed && tools/media/build.sh --with-core NOW
py=.cache/media/venv/bin/python   # matplotlib + Pillow
$py tools/media/collect.py --fixtures .cache/media/fixtures metrics --out .cache/media/out/metrics-final
$py tools/media/stability.py --fixtures .cache/media/fixtures --out .cache/media/out/stability
$py tools/media/resources.py --milestones NOW --fixtures .cache/media/fixtures
$py tools/media/hiccups.py
$py tools/media/charts.py .cache/media/out/final/metrics.json
$py tools/media/extra_charts.py --stability .cache/media/out/stability/stability.json --resources .cache/media/out/resources/resources.json
$py tools/media/tables.py
```

Interrupted passes resume: every run writes a `sample.json` receipt, and finished runs are never repeated. `stability.py --cached-only` summarizes without launching anything.

During the coordinated NOW counter window:

```sh
nice -n 19 ionice -c3 "$py" tools/media/counters.py --fixtures .cache/media/fixtures
"$py" tools/media/counter_charts.py .cache/media/out/counters/counters.json
```

## Final measurement tables

### Cold and warm Home, playback, and tour resources

Warm Home uses the default GPU-mode startup and requires readable libmpv mappings throughout the idle window. CPU peaks use sliding windows of 0.75–1.25 seconds.

| Measure | iced | GPUI | Rune | ntctl play |
|---|---:|---:|---:|---:|
| Cold Home PSS | 99 | 143 | 131 | n/a |
| Warm Home PSS | 153 | 188 | 169 | n/a |
| Software playback PSS | 211 | 255 | 244 | 116 |
| Software playback peak PSS | 211 | 256 | 245 | 116 |
| Software playback CPU % | 45 | 45 | 44 | 36 |
| Software playback peak CPU % | 48 | 49 | 47 | 39 |
| GPU playback PSS | 217 | 253 | 274 | n/a |
| GPU playback peak PSS | 217 | 253 | 274 | n/a |
| GPU playback CPU % | 10 | 10 | 9.5 | n/a |
| GPU playback peak CPU % | 12 | 12 | 11 | n/a |
| Tour peak PSS | 267 | 289 | 337 | n/a |

### Software playback attribution

| App | PSS total | core ref | app residual | core share | CPU total | core ref | core share |
|---|---:|---:|---:|---:|---:|---:|---:|
| iced | 211 | 116 | 95 | 55% | 45 | 36 | 80% |
| GPUI | 255 | 116 | 139 | 45% | 45 | 36 | 80% |
| Rune | 244 | 116 | 129 | 47% | 44 | 36 | 82% |

### CPU counters

Three runs per app and scenario. Each cell is a median. Full per-PMU and normalized values are in the receipts.

| App | Scenario | Instructions M | Cycles M | M instructions / s | Instructions / frame |
|---|---|---:|---:|---:|---:|
| iced | home | 173.43 | 79.94 | 3,917.82 | n/a |
| iced | idle | 6.02 | 9.71 | 0.60 | n/a |
| iced | wheel | 1,540.33 | 760.37 | 568.05 | n/a |
| iced | playback-gpu | 352.61 | 747.54 | 35.23 | 1,175,359.06 |
| iced | playback-software | 6,545.96 | 4,644.53 | 653.90 | 21,843,709.39 |
| iced | warm-switch | 773.60 | 484.77 | 3,518.08 | n/a |
| GPUI | home | 252.05 | 98.73 | 5,123.71 | n/a |
| GPUI | idle | 5.97 | 10.90 | 0.60 | n/a |
| GPUI | wheel | 3,020.93 | 1,407.45 | 1,112.34 | n/a |
| GPUI | playback-gpu | 352.92 | 681.75 | 35.24 | 1,180,326.83 |
| GPUI | playback-software | 6,525.11 | 4,591.19 | 651.53 | 21,750,362.02 |
| GPUI | warm-switch | 925.69 | 635.52 | 3,717.13 | n/a |
| Rune | home | 178.11 | 77.91 | 4,041.23 | n/a |
| Rune | idle | 5.98 | 10.24 | 0.60 | n/a |
| Rune | wheel | 1,013.41 | 562.40 | 373.52 | n/a |
| Rune | playback-gpu | 327.54 | 657.15 | 32.72 | 1,091,815.56 |
| Rune | playback-software | 6,574.66 | 4,695.03 | 656.73 | 21,915,539.12 |
| Rune | warm-switch | 1,028.51 | 652.63 | 3,552.95 | n/a |
| ntctl | playback-software | 7,065.18 | 3,741.26 | 705.43 | n/a |

### GPU and energy counters

Three system-wide samples per app and scenario. The engine columns are estimated busy cycles in millions.

| App | Scenario | RCS Mcycles | VCS Mcycles | BCS Mcycles | CCS Mcycles | RC6 ms | Package J | GPU J |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| iced | home | 0.22 | 0.00 | 0.00 | 0.00 | 42.65 | 0.73 | 0.01 |
| iced | idle | 22.21 | 0.00 | 0.00 | 0.00 | 8,899.02 | 83.83 | 0.75 |
| iced | wheel | 123.57 | 14.27 | 0.00 | 0.00 | 1,733.13 | 48.12 | 1.25 |
| iced | playback-gpu | 69.63 | 16.19 | 0.00 | 0.00 | 7,757.86 | 84.02 | 1.56 |
| iced | playback-software | 85.85 | 10.45 | 0.00 | 0.00 | 7,731.03 | 96.30 | 1.74 |
| iced | warm-switch | 13.08 | 6.24 | 0.00 | 0.00 | 137.50 | 3.16 | 0.11 |
| GPUI | home | 15.31 | 0.00 | 0.00 | 0.00 | 45.96 | 0.48 | 0.01 |
| GPUI | idle | 5.04 | 0.00 | 0.00 | 0.00 | 8,918.02 | 69.99 | 0.54 |
| GPUI | wheel | 35.56 | 0.00 | 0.00 | 0.00 | 1,945.90 | 19.29 | 0.42 |
| GPUI | playback-gpu | 74.71 | 9.12 | 0.00 | 0.00 | 7,815.40 | 62.36 | 1.56 |
| GPUI | playback-software | 77.82 | 8.71 | 0.00 | 0.00 | 7,783.48 | 67.70 | 1.27 |
| GPUI | warm-switch | 9.65 | 5.49 | 0.00 | 0.00 | 153.69 | 3.14 | 0.09 |
| Rune | home | 0.60 | 0.00 | 0.00 | 0.00 | 32.98 | 0.37 | 0.01 |
| Rune | idle | 7.29 | 0.00 | 0.00 | 0.00 | 8,601.31 | 74.50 | 0.59 |
| Rune | wheel | 68.33 | 0.00 | 0.00 | 0.00 | 1,812.13 | 20.22 | 0.72 |
| Rune | playback-gpu | 75.06 | 8.70 | 0.00 | 0.00 | 7,754.23 | 72.37 | 1.54 |
| Rune | playback-software | 100.81 | 8.35 | 0.00 | 0.00 | 7,691.73 | 69.06 | 1.75 |
| Rune | warm-switch | 11.97 | 5.14 | 0.00 | 0.00 | 176.25 | 3.00 | 0.11 |
| ntctl | playback-software | 9.02 | 7.60 | 0.00 | 0.00 | 8,644.97 | 64.29 | 0.59 |

### Related-switch frame checks

The NOW runs capture the outgoing player immediately before the related open, a crop about 250 ms after the playback mark, and a follow-up crop after at least another 2.05 seconds. A byte-identical outgoing and early crop is a stale-frame candidate (weight 2).

| App | Checked | Black | Stale candidate | Frozen |
|---|---:|---:|---:|---:|
| iced | 9 | 0 | 0 | 0 |
| GPUI | 9 | 0 | 0 | 0 |
| Rune | 9 | 0 | 0 | 0 |

### Files

The app tours are in `.cache/media/out/tours/{iced,gpui,rune}/`. The composite is `.cache/media/out/side-by-side.mp4`. Counter samples and raw perf CSV are in `.cache/media/out/counters/`.
