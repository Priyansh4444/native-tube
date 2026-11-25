# Normalized media measurements

CPU share is CPU-seconds ÷ wall-seconds ÷ logical CPUs, expressed as a percent. Cores busy is CPU-seconds ÷ wall-seconds. The laptop has 16 logical CPUs in `/proc/cpuinfo`. Fly has four vCPUs. Both measures cover the benchmarked app or browser process and leave out the compositor. Logical CPU share measures occupancy. The laptop's hybrid cores and Fly's vCPUs deliver different amounts of work at the same occupancy.

User-space retired instructions are a rough work measure on these two x86-64 machines. Cycles depend on the processor, clock and scheduling, so use them for IPC checks within a machine. The laptop uses an iGPU for the GPU path. Fly renders EGL through llvmpipe. Both native benchmarks use the same local 30 fps clip.

## Playback

Values are medians of three runs. Parentheses show deltas from that machine's software `ntctl play` reference. GPU/EGL comparisons to this reference compare different render paths. On the laptop the playback CPU samples and instruction samples come from separate sets of runs.

| Machine | Path | App | Cores busy (Δ) | Machine % (Δ pp) | M instructions / video s (Δ) | M instructions / expected frame (Δ) | Instructions / expected frame-pixel (Δ) |
|---|---|---|---:|---:|---:|---:|---:|
| Laptop | GPU | iced | 0.105 (−0.257) | 0.65 (−1.60) | 35.26 (−670.17) | 1.18 (−22.34) | — |
| Laptop | GPU | GPUI | 0.101 (−0.261) | 0.63 (−1.63) | 35.29 (−670.14) | 1.18 (−22.34) | — |
| Laptop | GPU | Rune | 0.095 (−0.267) | 0.59 (−1.67) | 32.75 (−672.68) | 1.09 (−22.42) | — |
| Laptop | Software | iced | 0.451 (+0.089) | 2.82 (+0.56) | 654.58 (−50.86) | 21.82 (−1.70) | 54.06 (+28.54) |
| Laptop | Software | GPUI | 0.452 (+0.091) | 2.83 (+0.57) | 652.49 (−52.94) | 21.75 (−1.76) | 53.88 (+28.37) |
| Laptop | Software | Rune | 0.441 (+0.080) | 2.76 (+0.50) | 657.45 (−47.98) | 21.92 (−1.60) | 54.29 (+28.78) |
| Laptop | Software | ntctl play | 0.362 | 2.26 | 705.43 | 23.51 | 25.51 |
| Fly | EGL | iced | 2.925 (+2.778) | 73.12 (+69.46) | — | — | — |
| Fly | EGL | GPUI | 2.940 (+2.793) | 73.50 (+69.83) | — | — | — |
| Fly | EGL | Rune | 2.918 (+2.772) | 72.96 (+69.29) | — | — | — |
| Fly | Software | iced | 0.907 (+0.760) | 22.67 (+19.00) | — | — | — |
| Fly | Software | GPUI | 1.143 (+0.997) | 28.58 (+24.92) | — | — | — |
| Fly | Software | Rune | 1.590 (+1.443) | 39.75 (+36.08) | — | — | — |
| Fly | Software | ntctl play | 0.147 | 3.66 | — | — | — |

The frame estimate uses 30 × playback seconds. The laptop's renderer counters give 1.18/1.18/1.09 M instructions per GPU frame and 21.84/21.75/21.92 M per software frame for iced/GPUI/Rune. No compositor presentation count was recorded. The apps' 848×476 software target has 44% of the 1280×720 `ntctl play` pixels. The per-pixel column spreads all measured work across those pixels, including fixed work such as decoding and UI. The CLI has no render-size option, so the pixel calculation uses saved measurements.

Fly's CPU samples isolate playback and scrolling. Its archived PMU counters combine startup, idle and the workload, including a second open in playback scenarios. The `ntctl play` total also includes startup. The [raw Fly table](fly-bench.md#native-medians) keeps these totals. Per-unit PMU rates and deltas require a new bounded native collection.

## Scrolling, Home and related opens

| App | Laptop wheel cores / machine % | Laptop wheel M instructions / s | Laptop wheel M instructions / drawn frame | Fly wheel cores / machine % | Laptop M instructions / Home reload | Laptop M instructions / related open→frame mark |
|---|---:|---:|---:|---:|---:|---:|
| iced | 0.266 / 1.66 | 569.72 | 9.45 | 1.347 / 33.69 | 173.43 | 154.72 |
| GPUI | 0.444 / 2.77 | 1,117.05 | 18.65 | 1.961 / 49.04 | 252.05 | 185.14 |
| Rune | 0.211 / 1.32 | 374.73 | 6.22 | 2.800 / 70.01 | 178.11 | 205.70 |

Home instructions cover one scripted Home reload. The related-open value divides the five-slice PMU total by five. The slice adds perf boundary overhead to the open→mark interval. The first open has no isolated PMU window. `ntctl play` has no scrolling, Home or related-open workload, so these rows have no baseline delta.

## Warm audit

| App | Laptop related open→mark, 3 runs (ms) | Fly related open→mark, 3 runs (ms) | Laptop warm Home (MiB PSS) | Fly warm Home (MiB PSS) |
|---|---:|---:|---:|---:|
| iced | 35.5 / 37.0 / 32.5 | 161.4 / 172.4 / 154.2 | 153 | 235 |
| GPUI | 43.1 / 44.8 / 42.2 | 188.2 / 197.7 / 193.8 | 188 | 315 |
| Rune | 54.0 / 50.5 / 30.1 | 182.4 / 170.5 / 224.3 | 169 | 220 |

- **Native related switch.** The six-second playback phase ends, autopilot invokes `open_video(index=0)` on the Watch page, and the app records its first picture render/paint marker, `playback_started`. NOW laptop and current Fly reports pair both opens with later markers and show a loaded Watch page before the switch. The laptop counter receipts also pair all five related opens in each run with later markers before Back. The autopilot command is the click proxy. Laptop crop checks found zero black, stale-candidate or frozen frames in 27 checks. The timestamps come from app render/paint hooks.
- **Prepared stream.** These runs used `NT_VIDEO_OVERRIDE` and the same local clip for both titles. The override bypasses `Player::prepare_next` stream resolution. Fly also set autoplay off. The numbers measure a local-file switch on an already-running player. No run in this set verifies click→frame for a prefetched stream. The local fixture eliminates network ads and network buffering. Local loading and decoding still contribute to the window.
- **Warm Home.** On the laptop the window is 2.0–5.5 s after the collector sees `home_loaded`. On Fly it is 2.0–3.8 s after that observation. PSS sampling trims 0.2 s at both ends. Every current default run had readable, mapped libmpv throughout the window. The matching cold runs require it unmapped. This verifies the idle memory window and library residency. Surface prewarm differs by frontend.
- **Historical YouTube warm.** The driver dispatched a YouTube SPA navigation event and polled for media time advancing with the requested ID and `.ad-showing` clear. The historical extended run's first-any proxies were 1.320/0.969/1.082 s and ads were seen in every warm trial. Actual-content proxies were a 240 s timeout, 69.294 s, and 96.745 s. Those windows include ads and any other wait before content. The gap between the two proxies cannot establish ad duration. The original 60 s run timed out in all three warm trials.

## Fly YouTube rerun

Three fresh signed-out, muted Chromium profiles each clicked a link to the same cold title from a blank tab, then clicked the first eligible related title in the existing Watch page. The related targets varied across trials. The browser recorded the trusted click and the first callback for a compositor-submitted video frame. The probe checked the requested video ID and excluded stale frames from the outgoing video. Ad-state transitions use `.ad-showing` or visible player ad UI. Ad duration is wall time in that state, including any wait while the state remains active.

| Trial | (a) Click→any frame (s) | Actual frame, wall (s) | (b) Actual frame, excluding ad-state time (s) | (c) Ad-state time (s) |
|---|---:|---:|---:|---:|
| Cold 1 | 3.297 | 3.297 | 3.297 | 0 |
| Cold 2 | 2.234 | 2.234 | 2.234 | 0 |
| Cold 3 | 2.290 | 2.290 | 2.290 | 0 |
| Warm 1 | 0.613 | >130 (timeout) | — | — |
| Warm 2 | 0.717 | 0.717 | 0.717 | 0 |
| Warm 3 | 0.735 | 16.064 | 0.693 | 15.371 |

Warm 1 showed an ad frame and had no observed ad exit or requested-video frame before the deadline. Its final ad duration is unknown. Warm 3 showed an ad entry, an ad exit, and a requested-video frame. No ads were skipped or blocked. [Fly rerun details](fly-bench.md#frame-timing-rerun)

## Sanity checks

| Check | Result |
|---|---|
| Idle CPU near zero | Maximum 0.019% of laptop capacity in the counter idle windows and 0.500% of Fly capacity in cold Home idle windows. |
| Instructions scale with time | A pinned no-GUI steady workload retired 14.81 B in 1 CPU s and 29.44 B in 2 CPU s, a 1.988× ratio. Native Fly had no controlled duration sweep. |
| Repeats | Maximum peak-to-peak spread as a fraction of median was 0.47% for laptop playback instructions/s, 0.63% for laptop scrolling instructions/s, and 5.07% for Fly whole-scenario instructions. Warm-switch ranges appear above. Laptop iced Home run 1 took 1,003 ms versus 207/209 ms. The archive does not establish cache state. iced wheel GPU work varied substantially in runs 1–2, so the system-wide GPU totals are unsuitable for a clean frontend comparison. |
| PSS ≤ RSS | All 39,265 readable process samples in the audited laptop resources and Fly native/web trees passed. |
| Frame later than open | 135 native command→marker pairs passed across the NOW, historical, Fly and five-slice counter runs. The fresh web results below use trusted click and browser frame timestamps. |
| Hybrid PMU sums | All 114 per-run and 38 recomputed aggregate instruction/cycle splits add up, treating an unscheduled PMU as zero contribution. The previous aggregate mixed independent PMU medians. Each repaired median total carries the partition from its own observed run. |
| Fly PMU counters | All 39 native user-space pairs were non-zero. Observed IPC was 1.52–2.52, inside the broad 0.1–6 consistency check. The Fly collector now requests user-only events by default. |

Run `nice -n 19 ionice -c3 python tools/media/normalize.py` to regenerate `.cache/media/out/normalized.json` from the raw counter, resource, milestone and Fly receipts. It also rebuilds the aggregate CPU counter summary from its saved runs.
