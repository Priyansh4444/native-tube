# Choosing a Rust GUI: iced vs. GPUI vs. Rune

For a Rust desktop app on Linux, I'd start with **iced**, reach for **GPUI** if layered controls are central to the design, and try **Rune** if I wanted reactive bindings for a custom UI and could pin a pre-1.0 toolkit.

- **iced** had the lowest idle memory on both test machines with the video player prewarmed. On the CPU-rendered VM, it used the least CPU for scrolling and opened video fastest. Its message-driven architecture made floating controls more work to wire up.
- **GPUI** made floating menus, video overlays, and a custom seek bar easiest to build. It used the most idle memory and spent the most CPU time scrolling on the laptop. Two video opens timed out with prewarm disabled.
- **Rune** used the fewest CPU instructions per drawn scroll frame on the laptop and had the lowest peak memory in the short playback test. This build required a git pin and two crate forks.

On the laptop, all three opened the same video fixture in about **100 ms** with full prewarm. For a video-heavy app, the playback architecture was the bigger decision. Giving `libmpv` its own Wayland surface for GPU rendering cut CPU instructions per video frame by **16–19×** across all three clients.

[MEDIA: .cache/viz/out/cover.png — One shared Rust core feeding three native UI toolkits: iced, GPUI, and Rune.]

---

## 1. The Same App, Three UI Architectures

[MEDIA: .cache/viz/out/Demo.mp4 — NativeTube in ten seconds. Home in iced, search in GPUI, and related playback and captions in Rune.]

I built **NativeTube**, a signed-out desktop YouTube client with Home feeds, search, watch pages, related videos, scrubbing, and keyboard navigation, in **iced 0.14**, **GPUI** (`gpui-pre` 0.3.12 and `gpui-component` 0.9.1), and **Rune** (`anomalyco/rune`, git rev `4fbd2d6`).

All three use the same Rust core for networking, image caching, scroll physics, and `libmpv` playback. I benchmarked the clients on my Intel GPU laptop and a GPU-less Fly VM using the same recorded feeds and local video. Each frontend still decides when to lay out a view, mount thumbnails, and repaint.

### How each framework updates the UI

[MEDIA: .cache/viz/out/FrameworkUpdates.mp4 — See how a feed becomes a frame, what each client reuses on scroll, and how the measured CPU work changes between the laptop and VM.]

Picture a feed at rest. Keep three snapshots in mind: **which cards are mounted**, **the sizes and positions layout computed for them**, and **the last frame you saw**. Now scroll a little. The frame has to change. The mounted card list can stay the same, and the renderer can shift cards using the geometry it already has. Scroll farther and the app needs a different group of cards. The view must describe that group, layout must place it, and draw must produce a new frame.

Read the scroll handler as a state transition. The app starts with **(mounted cards, layout, scroll offset)**. If the new offset fits the existing group, iced and Rune keep its cards and layout and change the offset used by draw. If the offset crosses the group’s rebuild threshold, they update the mounted cards and compute layout for the new group. GPUI’s NativeTube handler takes the page rebuild path whenever the snapped offset changes, even if the same cards remain visible.

| Scroll event in NativeTube | iced | GPUI | Rune |
|---|---|---|---|
| Offset changes within the mounted group | Reuse view and layout. Draw with a translation. | Rebuild the page view, run Taffy layout, draw. | Reuse view and layout. Sample the live offset, draw. |
| Scroll reaches the next mounted group | Send a message, rebuild the group, lay it out, draw. | The page view and layout still run. Cached chrome can be reused. | Commit a view with the new group, lay it out, draw. |

**View** describes what belongs in the interface, such as a thumbnail and title for each mounted video. **Layout** gives those elements sizes and positions, such as a 160-pixel thumbnail beside a title that wraps to fit. **Draw** turns those descriptions and positions into drawing commands. The renderer produces the pixels, clipping cards at the viewport edge and applying the scroll offset. For a small scroll, iced and Rune can draw the same cards at a new offset using their existing geometry.

The transition table tracks which stages run. Each stage can cost a different amount. Rune had the lowest instructions per drawn frame on the GPU laptop (**8.25 million**), with iced close at **8.56 million** and GPUI at **17.5 million**. Rune and iced overlapped on laptop CPU time. On the software-rendered VM, iced led at **175 million**, with Rune at **442 million** and GPUI at **481 million**. View reuse can save CPU, and the renderer and the work the app submits still affect the total.

**React on the web.** A state setter queues work. React runs the relevant components to produce element descriptions, reconciles them with the previous render, and commits the needed DOM changes. The DOM is the browser’s tree of page objects. What changed determines the subsequent browser work. Changing text can affect layout. A transform can sometimes be handled in compositing. Rendering a React component therefore tells you little by itself about the cost of the final frame. [React’s render and commit guide](https://react.dev/learn/render-and-commit)

**iced.** A click usually emits a `Message`. The app handles it in `update(&mut self, Message)`, and `view` supplies the next widget description. iced can diff it against retained widget state and reuse cached work. NativeTube’s custom scroller takes a shorter route for movement inside the existing card group. It updates the offset and requests a redraw, then clips the viewport, applies `with_translation`, and draws the intersecting children from their existing layouts. A band change sends a message to rebuild the needed rows. Even on the short route, iced still visits and draws widgets.

**GPUI.** Entity state has stable identity, and a view renders from it. `cx.notify()` tells GPUI an entity changed. The runtime checks which windows track that entity, then schedules the relevant invalidation or queues an observer notification. When the snapped scroll offset changes, NativeTube marks the page view and root dirty. The page builds elements for the currently visible cards and GPUI uses Taffy to resolve geometry before painting. Cached views like the top bar can reuse their work. Timing `notify()` alone would miss the work it schedules.

**Rune.** `Tx` collects requests from an event handler. The body of `tx.live_only()` sets one Boolean. It tells the runtime that this handler changed nothing the view reads, so the event can skip rebuilding the view and running layout. A numeric `Live::set` is a separate operation. At the pinned revision it encodes the value and atomically swaps a 128-bit word without allocating. An unchanged ordinary write returns without notifying. A changed write walks listeners, and each subscribed runtime coalesces wakes. NativeTube publishes the content offset and scrollbar geometry this way. On a live frame, Rune walks its retained nodes, samples their bound properties, and applies changes to the scene. Paint walks the scene to emit GPU draw instances and update hit-test data at the new positions. A band change asks for a view commit so the right cards get mounted. [Rune’s numeric Live source](https://github.com/anomalyco/rune/blob/4fbd2d6ac4dd12595dd8ed0b7849b28a309c7c00/crates/rune/src/live.rs) · [Rune’s frame sampling](https://github.com/anomalyco/rune/blob/4fbd2d6ac4dd12595dd8ed0b7849b28a309c7c00/crates/rune/src/runtime/motion/live_latch.rs)

Rune’s numeric `Live` is signal-like in the sense that one source feeds a specific property. Here the binding is explicit, stores the latest value, and is sampled on a frame. That differs from a signal system where reading a value inside an effect registers a dependency and a write schedules that effect. These native toolkits have their own widget, element, or scene structures. None of these paths goes through a browser DOM.

**How expensive is a Rune publication?** On my laptop on battery, a release-build microbenchmark of `Live<[f32; 2]>::set` took a median **12 ns per call** for an unchanged value without listeners, **21 ns** for a changing value without listeners, and **40 ns** for a changing value bound to one runtime with a wake already pending. Each case ran in nine tight-loop batches of four million calls pinned to one logical CPU. These are publication timings, including the loop overhead. They exclude platform wake delivery, frame sampling, and paint. The `live_only()` setter is the single Boolean assignment described above and has no separately measured timing here. [Benchmark source](../../bench/rune-live-publication.rs) · [Raw samples](../../bench/results/rune-live-publication.json)

**How expensive is the frame?** The matched laptop run yielded median CPU time per drawn frame of **3.44 ms for iced, 5.75 ms for GPUI, and 3.21 ms for Rune**. These are whole-process ratios across the scroll window. They include work such as thumbnail decoding and tell us how much CPU the app spent, while click-to-pixel latency and stage-by-stage attribution require separate measurements. Section 4 has the ranges and instruction counts.

---

## 2. How the Video Path Changes CPU Cost

Originally, `libmpv` decoded frames into CPU memory, and each toolkit uploaded them as textures and redrew the window. At 848×476, one uncompressed RGBA picture is about 1.6 MB (~48 MB/s at 30 fps).

I moved video to a Wayland subsurface. On the laptop, `libmpv` renders into its own GPU surface, and the compositor places that surface behind a transparent area in the UI.

[MEDIA: .cache/viz/out/FramePathStill.png — Software copies each 848×476 RGBA frame (~1.6 MB) through RAM and the UI toolkit. With a Wayland subsurface, libmpv renders directly into a separate compositor surface below the transparent hole.]

That single architectural change dwarfed every toolkit difference:

- **Intel GPU laptop (shipped defaults, matched rerun):** the native apps used **0.087–0.100 cores** on the subsurface path vs. **0.418–0.422 cores** on forced software. The CPU retired **1.16–1.41 million instructions per rendered video frame** on the GPU path vs. **22.1–22.4 million** on forced software—a **16–19× reduction** across all three clients.
- **GPU-less Fly VM (the flip):** Mesa's `llvmpipe` emulated OpenGL on the CPU, so the default EGL path burned **about 3.27 cores** and rendered roughly **150–154 frames in six seconds**, whereas forced software used **1.20–2.11 cores** and rendered **about 180 frames**.
- **Battery life remains an open question.** In the earlier AC counter study, package energy varied enough that the idle and playback readings overlapped.

---

## 3. Startup and Time to First Frame

[MEDIA: .cache/viz/out/FirstFrameRealTime.mp4 — One video open per app at normal speed, recorded before the matched benchmark below.]

I matched the comment loading, prewarm timing, and first-frame markers. I also fixed double thumbnail decoding in Rune at 2× scale and sent iced’s scripted wheel events through its normal scroller.

The matched suite used identical binaries, recorded fixtures, and the same local 720p30 clip on both machines. The first laptop run was on AC power:

| Full prewarm, laptop | iced | GPUI | Rune |
|---|---:|---:|---:|
| Window map | 100 ms | 176 ms | 126 ms |
| Home loaded | 167 ms | 318 ms | 180 ms |
| Click to first frame | 108 ms | 105 ms | 96 ms |
| Local-clip switch | 49 ms | 53 ms | 44 ms |

These are medians of five warm repeats after round 1. Under matched full prewarm, the first-frame and local-clip switch ranges overlap across all three apps. The switch reuses a running player and local clip. Prepared live-stream switching was outside this test.

I repeated the open and wheel tests on the same laptop on battery. All 36 attempts completed. The first-frame ranges still overlap with full prewarm. The wheel runs drew about **58–60 fps** across the three apps, with median p95 frame intervals of **17.5–19.3 ms**. Full ranges and the power settings are in [the matched benchmark notes](../media/fair.md).

Under each app's shipped default, iced and GPUI still overlap on click to first frame while Rune takes **224 ms** with prewarm off. With `NT_PREWARM=off`, **GPUI timed out twice in six laptop trials** (rounds 3 and 6 reached Ready and attached the Wayland surface, rendered zero video frames before the 12 s deadline, and then played after the video switched in both trials). Because of those two open timeouts, I left the prewarm-off ranking open.

[MEDIA: .cache/viz/out/matched-results.png — The matched full-prewarm rerun: medians (dots) and observed ranges (lines) across five warm repeats per app for laptop and Fly click-to-first-frame, plus laptop Home idle and playback-scenario peak PSS.]

One startup lesson almost bit iced before the app even existed: out of the box, bare iced 0.14 was *slowest* (**460 ms**, **113 MiB**, **107 wakeups/s**) because `iced_wgpu` defaulted to `HighPerformance` and woke our NVIDIA GPU. Setting `WGPU_POWER_PREF=low` and `WGPU_BACKEND=vulkan` dropped it to **175 ms**, **50 MiB**, and **2 wakeups/s**, beating bare GPUI (**253 ms / 96 MiB**) and Rune (**259 ms / 78 MiB**).

On the CPU-rendered Fly VM, median click to first frame was **240 ms** on iced, **363 ms** on GPUI, and **393 ms** on Rune with full prewarm and local fixtures. iced's observed range sat entirely below the other two.

---

## 4. Scrolling: CPU Time and Instructions per Frame

[MEDIA: .cache/viz/out/ScrollRealTime.mp4 — One continuous scripted touchpad scroll in Rune, captured on the laptop at 60 fps and shown at normal speed.]

For the measured workload, I replayed 27 wheel notches with prewarm off and counted CPU time and user-space instructions per UI frame:

| Wheel workload | iced | GPUI | Rune |
|---|---:|---:|---:|
| Laptop cores | 0.203 | 0.340 | 0.192 |
| Laptop CPU ms / drawn frame (observed range) | 3.44 (3.13–3.91) | 5.75 (5.69–6.10) | 3.21 (3.01–3.87) |
| Laptop million instructions / drawn frame | 8.56 | 17.5 | 8.25 |
| Laptop million user-space cycles / drawn frame | 4.13 | 8.02 | 3.77 |
| Fly cores | 1.60 | 3.06 | 3.02 |
| Fly million instructions / drawn frame | 175 | 481 | 442 |

On the laptop, iced and Rune overlap on CPU cores, and GPUI burns more CPU than both; Rune retires the fewest instructions per drawn frame, closely followed by iced. On the CPU-rendered Fly VM, iced wins both metrics decisively. Because the apps drew different frame counts on Fly, per-frame cost sits right next to total CPU time.

For each warm laptop run I divided the wheel phase’s total process CPU time by its number of drawn UI frames, then took the median of five run-level ratios. Total CPU time is wheel duration multiplied by cores used. I computed cycles per frame the same way using the wheel-window `cycles:u` counter as the numerator. The viewports mount different numbers of cards, decode different amounts of thumbnail data, and use different antialiasing settings. There is no per-stage attribution in these totals.

---

## 5. Memory: Idle and Playback Peaks Tell Different Stories

With full prewarm, iced had the lowest Home idle memory on both machines. Rune had the lowest peak in the short playback scenario (Home, open a video, play for six seconds, then switch). The table includes both machines, and the chart in Section 3 shows the laptop results.

| Full prewarm, process-tree PSS | iced | GPUI | Rune |
|---|---:|---:|---:|
| Laptop Home idle | 132 MiB | 227 MiB | 144 MiB |
| Laptop playback scenario peak | 276 MiB | 321 MiB | 238 MiB |
| Fly Home idle | 229 MiB | 309 MiB | 242 MiB |
| Fly playback scenario peak | 419 MiB | 478 MiB | 409 MiB |

PSS apportions shared memory across processes. With its shipped prewarm-off default, Rune's laptop Home idle median was **80 MiB**. Flipping Rune to full prewarm raised idle memory to **144 MiB** and cut its first-open median from **224 ms to 96 ms**. Artwork, browser load, and longer multi-page tours carry different footprints.

---

## 6. Stability and Failure Modes

Our stability harness (`stability.py`) scored **100 (3/3 clean)** across all **30 assessable historical runs** (10 build × app cells, software path), and NOW switch checks found **0 black, 0 stale, 0 frozen frames** out of 9 per app.

At the earlier benchmark checkpoint, the incident catalog recorded **43 incidents** with fixes: **14** in `nt-core`, **7** iced, **7** Rune, **6** GPUI, and **9** orchestration/tooling. The two GPUI prewarm-off timeouts from the matched rerun are still open.

Frontend incident counts tied (6–7 each), but failure modes diverged sharply:

- **GPUI's re-entrancy panic:** Lacking a public cursor-hide API, our agent dispatched a synthetic keystroke inside `cx.defer_in`; `dispatch_keystroke` redrew `PlayerView` mid-update and panicked at `entity_map.rs:192` ("already being updated"). `Window::defer` fixed it. In iced's `&mut App` Elm loop, the compiler forbids that bug.
- **iced's disappearing focus:** Focusing search changed an adjacent icon's width from zero; `row!` (0.14.2) skips zero-width children, shifting the input's index so positional diffing reset focus. (0.14 also lacks `unfocus()`. None of the catalog's crashes or stalls were in iced's own code—but there were only three crashes total, the catalog holds only what got written down, and `nt-core`'s storyboard crash hit all three apps, iced included.)
- **Rune's release abort and GPU stalls:** Combining `.wrap()`/`.truncate()` with `.live_text()` hit a release `assert!` (`flatten.rs:258`). Separately, incoming GPU frames failed to wake Rune to open its hole (stalls), and opening the hole before decode left a **2.6 s black hole**.
- **Drivers, Cargo, and macOS:** Unfiltered EGL probing loaded both Mesa and NVIDIA (~38–50 MiB wasted) until `select_egl_vendor()` pinned the compositor's vendor; a GPUI transitive dependency enabled `serde_json` `preserve_order`, breaking fixture replay until we canonicalized keys; and on macOS, **only Rune** rendered readable text out of the box (GPUI showed none; iced rendered dots).

---

## 7. Building With AI Agents: Cost, Speed, and Ergonomics

[MEDIA: .cache/viz/out/CostStill.png — First full agent builds under Claude Opus 5.5 cost nearly the same: $23.54–$28.41 in 49–59 minutes. Later reviews, fixes, and parity work are outside this table.]

First full builds under Claude Opus 5.5 landed almost on top of each other: **iced $23.54 (49 min)**, **Rune $27.99 (55 min)**, **GPUI $28.41 (59 min)**.

Review rounds and polish drove most of the total spend. Our review council's **±50% estimates** for full YouTube parity (context menus, miniplayer, chapters, heatmaps, storyboards, guide drawer)—overlapping ranges, atop **~36 h / ~$1,000** of shared core work—put **GPUI** lowest (**~28 h / ~$780**), **Rune** next (**~40 h / ~$1,100**), and **iced** highest (**~44 h / ~$1,220**).

- **GPUI** ships layered chrome (`anchored()`, `deferred()`, blur, `PathBuilder`), yielding a **~20-line** heatmap and **~50-line** video hole, trading against runtime borrow panics and a ~30-line Wayland fling patch.
- **iced 0.14** needed zero patches and prevented the re-entrant mutable update that crashed GPUI, but taxed overlays with Elm `Message` plumbing plus hand-rolled ellipsis (`Clamp`) and 1×1 image-badge workarounds.
- **Rune** started most concise (**2,508 lines** at first delivery vs. GPUI's **3,045** and iced's **3,762**; now **10,631 / 12,653 / 13,518**) with natural reactive bindings, but needed manual theming and `[patch.crates-io]` forks (`winit`, `wgpu-hal`).

Across five usage-cap pauses and handoffs (Opus 5.5 → GPT 6.2 Sol → Gemini 4 Neon), I kept agents honest with an audited spec, frozen fixtures, headless virtual-pointer runs, mandatory screenshots, Bombadil-style property tests, and strict Clippy lints (`unwrap_used`, `expect_used`, `panic` denied—**zero** `#[allow]`/`#[expect]` in our own code; only the vendored GPUI patch keeps upstream's).

---

## 8. Benchmark Methods and Detailed Results

### Live YouTube in Chromium on Fly

Chromium loaded live YouTube over the network. The native runs replayed local fixtures. Both used the same Fly VM class with four vCPUs, 8 GiB RAM, and headless Sway with software rendering.

Chromium 158 took **1,857 ms** to show the first content frame (854×480) from a cold start. It used **1.24 cores** (**30.9%** of the VM) at **865 MiB PSS** (**1,014 MiB peak**).

Early warm trials sat behind pre-roll ads. On a fresh VM, I measured time to any frame, time to the requested video, and ad duration without skipping or blocking ads.

| Warm trial (Chromium on Fly) | Any first frame | Content first frame (wall time) | Content first frame (excluding ad) | Ad duration |
|---|---:|---:|---:|---:|
| Trial 1 | 0.613 s | >130 s (timeout) | — | — |
| Trial 2 | 0.717 s | 0.717 s | 0.717 s | 0 s |
| Trial 3 | 0.735 s | 16.064 s | 0.693 s | 15.371 s |

The ad-free cold trials on the fresh VM took **2.23–3.30 s** to show the first frame.

### Earlier normalized CPU and per-unit instruction tables

CPU share is `CPU-seconds ÷ wall-seconds ÷ logical CPUs` (16 logical CPUs on the laptop; 4 AMD EPYC vCPUs on Fly), covering the benchmarked app or browser process and excluding the compositor. "Cores busy" is `CPU-seconds ÷ wall-seconds`. The legacy tables below use the earlier three-run study. The matched results in sections 2–5 come from the later six-round rerun. Here, parenthetical deltas compare against that machine's software `ntctl play` baseline.

**Playback (local 30 fps 720p H.264 clip):**

| Machine | Path | App | Cores busy (Δ) | Machine % (Δ pp) | M instr / video s | M instr / expected frame | Instr / frame-pixel |
|---|---|---|---:|---:|---:|---:|---:|
| Laptop | GPU | iced | 0.105 (−0.257) | 0.65 (−1.60) | 35.26 | 1.18 | — |
| Laptop | GPU | GPUI | 0.101 (−0.261) | 0.63 (−1.63) | 35.29 | 1.18 | — |
| Laptop | GPU | Rune | 0.095 (−0.267) | 0.59 (−1.67) | 32.75 | 1.09 | — |
| Laptop | Software | iced | 0.451 (+0.089) | 2.82 (+0.56) | 654.58 | 21.82 | 54.06 |
| Laptop | Software | GPUI | 0.452 (+0.091) | 2.83 (+0.57) | 652.49 | 21.75 | 53.88 |
| Laptop | Software | Rune | 0.441 (+0.080) | 2.76 (+0.50) | 657.45 | 21.92 | 54.29 |
| Laptop | Software | `ntctl play` | 0.362 | 2.26 | 705.43 | 23.51 | 25.51 |
| Fly | EGL (`llvmpipe`) | iced | 2.925 (+2.778) | 73.12 (+69.46) | — | — | — |
| Fly | EGL (`llvmpipe`) | GPUI | 2.940 (+2.793) | 73.50 (+69.83) | — | — | — |
| Fly | EGL (`llvmpipe`) | Rune | 2.918 (+2.772) | 72.96 (+69.29) | — | — | — |
| Fly | Software | iced | 0.907 (+0.760) | 22.67 (+19.00) | — | — | — |
| Fly | Software | GPUI | 1.143 (+0.997) | 28.58 (+24.92) | — | — | — |
| Fly | Software | Rune | 1.590 (+1.443) | 39.75 (+36.08) | — | — | — |
| Fly | Software | `ntctl play` | 0.147 | 3.66 | — | — | — |

On the laptop, playback CPU samples (battery) and instruction samples (AC) come from separate run sets. The per-pixel column spreads all measured work—including fixed decoding and UI—across target pixels (848×476 for the apps, 44% of `ntctl play`'s 1280×720). On Fly, CPU samples isolate playback and scrolling, but archived PMU counters cover each whole scenario (startup, idle, and workload, including a second open in playback runs): **171.46 / 172.89 / 176.42 B** instructions for EGL, **57.64 / 61.60 / 103.11 B** for software, and **7.15 B** for `ntctl play`. Per-unit instruction rates on Fly are therefore unavailable.

**Scrolling, Home reload, and related opens:**

| App | Laptop wheel cores / machine % | Laptop wheel M instr / s | Laptop wheel M instr / drawn frame | Fly wheel cores / machine % | Laptop M instr / Home reload | Laptop M instr / related open→mark |
|---|---:|---:|---:|---:|---:|---:|
| iced | 0.266 / 1.66 | 569.72 | 9.45 | 1.347 / 33.69 | 173.43 | 154.72 |
| GPUI | 0.444 / 2.77 | 1,117.05 | 18.65 | 1.961 / 49.04 | 252.05 | 185.14 |
| Rune | 0.211 / 1.32 | 374.73 | 6.22 | 2.800 / 70.01 | 178.11 | 205.70 |

(Note on wheel denominators: `normalized.md` divides by `counters.json`'s active scenario seconds, yielding 570 / 1,117 / 375 M/s; `docs/media/README.md` divides by full counter wall time, yielding 568 / 1,112 / 374 M/s.)

**Fly YouTube Chromium frame-timing rerun (fresh `performance-4x`, signed out, muted, no ads skipped or blocked):**

| Trial | (a) Click → any frame (s) | Actual frame, wall (s) | (b) Actual frame, excl. ad (s) | (c) Ad-state time (s) |
|---|---:|---:|---:|---:|
| Cold 1 | 3.297 | 3.297 | 3.297 | 0 |
| Cold 2 | 2.234 | 2.234 | 2.234 | 0 |
| Cold 3 | 2.290 | 2.290 | 2.290 | 0 |
| Warm 1 | 0.613 | >130 (timeout) | — | — |
| Warm 2 | 0.717 | 0.717 | 0.717 | 0 |
| Warm 3 | 0.735 | 16.064 | 0.693 | 15.371 |

In the earlier Fly Chromium benchmark, cold playback (1,857 ms at 854×480) used **1.238 cores** (**30.94%** of the VM, +27.28 pp vs. local `ntctl play`), and extended warm playback (two completed 720p runs after pre-roll ads) used **0.784 cores** (**19.59%**, +15.93 pp; 896 MiB settled / 903 MiB peak PSS).

### Per-core CPU splits and the `ntctl` IPC paradox

[MEDIA: .cache/viz/out/PerCoreStill.png — Forced software playback, by instructions retired rather than scheduled time: P-cores retire 35.1–38.8% of each app's instructions and 39.9% of ntctl's, so the extra work isn't a scheduling artefact.]

Our laptop runs an Intel Arrow Lake-P hybrid CPU (`cpu_core` P-cores, `cpu_atom` E-cores, `cpu_lowpower` LP-E cores). `perf stat -p` recorded unscaled user-space counts per PMU and summed them; in the normalized aggregate, each median total carries the PMU partition from its own observed run rather than independent per-PMU medians.

In forced software playback, standalone `ntctl play` retired **more** total instructions (**7,065.18 M**) than any GUI app (**6,525–6,575 M**), yet used **18–20% fewer cycles** (**3,741.26 M** vs. **4,591–4,695 M**) and less battery CPU (**0.362 cores / 2.26%** vs. **0.441–0.452 cores / 2.76–2.83%**). Per-PMU scheduled-time shares refute P/E-core scheduling as the cause: `ntctl` (P 23–30%, E 53–54%, LP 16–22%) matched the apps (P 22–31%, E 52–62%, LP 13–20%). Instead, `ntctl` never calls `set_target_size` and decodes full **1280×720** at **1.89 IPC** (**25.51 instructions/frame-pixel**), whereas all three Watch pages size `mpv` to the player box—floored to a multiple of 16 at **848×476** (44% of 720p pixels, **~1.6 MB/frame** RGBA)—retiring **53.88–54.29 instructions/frame-pixel** at **1.40–1.42 IPC**. The apps spend ~850–950 M extra cycles, most likely copying, uploading, and presenting frames (an inference—no stall counters were collected).

### GPU engine busy cycles (Intel i915 PMU)

[MEDIA: .cache/viz/out/GPUStill.png — System-wide i915 render and video-decode busy cycles during GPU-subsurface playback: all three apps converge.]

On Arrow Lake-P, `GT0` hosts render (`RCS`), copy (`BCS`), and compute (`CCS`); `GT1` hosts video decode (`VCS`). Estimated busy cycles equal `busy_ns × avg_GT_MHz / 1000` (system-wide, so they include anything else on the GPU).

- **GPU playback (10 s):** all three converged tightly—RCS **69.63** (iced) / **74.71** (GPUI) / **75.06** (Rune) Mcycles; VCS **16.19 / 9.12 / 8.70** Mcycles; BCS and CCS 0.00; RC6 sleep residency **7.75–7.82 s** out of 10 s.
- **Wheel scroll (~2.7 s, 161–163 frames at 60 fps):** GPUI logged **35.56 Mcycles** RCS (runs: 28.4, 46.7, 35.6; 0.00 VCS) vs. Rune's **68.33 Mcycles** (runs: 77.4, 68.3, 67.8; 0.00 VCS). Rune also logged 1–2 long frames per run (max 29–64 ms), while iced and GPUI had zero. iced's wheel runs 1–2 were contaminated by other system activity, source unrecorded (RCS 348.0 / 123.6, VCS 18.1 / 14.3, package 48.1 / 48.8 J); clean run 3 dropped to **38.5 Mcycles** RCS and **0.46** VCS (23.2 J).

In the earlier AC counter study, Intel RAPL measured **70–84 J** of package energy over 10 seconds at idle and **62–96 J** over 10 seconds of playback. During playback, GPU-domain energy was **1.27–1.75 J** on the software path and **1.54–1.56 J** on the GPU subsurface path.

### Subsurface, EGL vendor selection, and playback pipeline details

[MEDIA: .cache/viz/out/EGLStill.png — How nt-core picks one EGL vendor on a hybrid machine: Wayland main device → DRM driver → matching EGL vendor, aimed at skipping the other GPU's EGL vendor at startup.]

`nt-core` embeds `libmpv` into a desync child `wl_subsurface` placed below the window, clipped to the player rectangle via `wp_viewporter` (`VideoSurface::set_clip`), with `video-timing-offset=0` and every `load-*` mpv script explicitly disabled (`load-scripts=no` alone still ran 6 Lua scripts). Against `googlevideo`'s open-ended `Range: bytes=0-` throttling, bounded requests (`request_size` 10 MB, initial 1 MB) cut first frame on a 14-hour video from **16.6 s to 0.7 s**.

On our hybrid Intel + NVIDIA laptop, unfiltered EGL probing pulled in both Mesa/LLVM and NVIDIA libraries (~38 MiB extra in iced, ~50 MiB of NVIDIA libraries in Rune) until `nt-core` added `select_egl_vendor()` to load only the compositor's EGL vendor (Mesa here). Earlier, `libmpv`'s CUDA probing cost **1,129 ms** on GPU attach until forcing VA-API interop dropped it to **79 ms** (a single coordinator measurement). On Fly's headless Sway + Pixman VM (`Mesa 26.3.1`), GBM initialization failed and EGL fell back to CPU `llvmpipe` (**2.92–2.94 cores / 73.0–73.5%** of the VM during playback; 171.5–176.4 B whole-scenario instructions).

Two more multi-app integration notes from the build: a transitive GPUI dependency enabled `serde_json`'s `preserve_order`, altering JSON key order in GPUI alone until we canonicalized keys and re-recorded **1,145 fixtures**; and GPUI separately lost variable-font weights on Linux until we bundled static Roboto weights.

### Memory attribution and prewarm receipts

[MEDIA: docs/media/attribution-NOW-settled_pss_mib.png — Earlier NOW player attribution on software playback: standalone ntctl play accounts for 45–55% of total memory across all three apps.]

Cold Home (`NT_NO_PREWARM=1`, `libmpv` unmapped) → Warm Home (default prewarm, `libmpv` mapped throughout the idle window) adds **+54 MiB** in iced (99 → 153 MiB), **+45 MiB** in GPUI (143 → 188 MiB), and **+38 MiB** in Rune (131 → 169 MiB); on Fly, Warm Home settled at 235 / 315 / 220 MiB (vs. Cold Home 175 / 273 / 182 MiB). Separately, in our performance gate A/B on iced, disabling post-Home GPU surface prewarm dropped idle memory from **162 MiB to 126 MiB** (−36 MiB) while raising click-to-first-frame from **112.1 ms to 209.8 ms** (+98 ms, failing its +75 ms budget).

### Sizes, crate counts, and local upstream reports

[MEDIA: .cache/viz/out/SizesStill.png — Stripped release binaries and lockfile package counts for the three NOW apps.]

| Metric (NOW) | iced | Rune | GPUI |
|---|---:|---:|---:|
| Stripped release binary | 29.6 MiB | 42.2 MiB | 52.7 MiB |
| Cargo.lock packages (bare shell) | 480 (319) | 490 (434) | 812 (640) |
| App Rust source lines (first delivery) | 13,518 (3,762) | 10,631 (2,508) | 12,653 (3,045) |

(`ntctl` stripped binary: **11.0 MiB**.)

[MEDIA: .cache/viz/out/UpstreamStill.png — Framework bugs we found (written up, not yet filed): the seven local reports in docs/upstream/, three each for iced and GPUI and one for Rune.]

I wrote up **7 local upstream bug reports** in `docs/upstream/` (not yet filed upstream):

- **iced (3):** `HighPerformance` GPU default (fixed on `master` `98bd48fe`), zero-width `row!` child resetting sibling focus (0.14.2), missing `unfocus()` helper (0.14.0). Zero patches needed.
- **GPUI (3 + Linux gap):** Wayland dropping `wl_pointer.axis_stop` (~30-line vendored patch), private `hide_cursor_until_mouse_moves`, re-entrant `dispatch_keystroke` panic (`entity_map.rs:192`), plus `paint_surface` being macOS-only.
- **Rune (1 + git pins):** wrapped/truncated `LiveText` release abort (`flatten.rs:258`), alongside `[patch.crates-io]` forks for `winit`/`wgpu-hal`.

### Methods, warm audit, sanity checks, and limitations

The matched fairness rerun in sections 2–5 ran **6 rotated rounds** (108 attempts per machine: 108/108 completed on Fly; 106/108 on laptop due to the two GPUI `NT_PREWARM=off` first-frame timeouts) using identical binary hashes, recorded fixtures, and the muted 720p30 clip on both an isolated rootless Podman Sway container on the laptop (Intel render node, `--cpus 4`, AC power) and a temporary `performance-4x` Fly VM (Pixman/llvmpipe/lavapipe). Warm statistics use medians and min–max ranges of valid rounds 2–6.

The earlier laptop benchmark metrics in this appendix are medians of **n = 3** runs (round-robin across apps for timings; back to back per app for counters) on one 16-CPU Arrow Lake-P + NVIDIA Hyprland laptop (1280×800, scale 1, off-screen `NT-BENCH` output), driven by `NT_AUTOPILOT` against frozen fixtures (index `8e94547e…`) and one muted 1280×720 30 fps H.264 clip (`a9766ffa…`), `NT_NO_MPRIS=1`, autoplay off. Timing milestones and `perf` counters ran on **AC (`balance_performance`)**; all 39 resource/PSS runs ran on **battery (`power`)**—never compare raw numbers across those two power regimes. Fly numbers are medians of 3 runs per workload on one `performance-4x` VM with CPU rendering; never mix them with laptop GPU numbers. Bare-shell figures are 8 runs, parity hours are ±50% estimates, and the VA-API figure is a single measurement.

**Warm-switch audit:** Per-run local-clip switch times (command → `playback_started` render/paint hook) in the earlier benchmark were **35.5 / 37.0 / 32.5 ms** (iced), **43.1 / 44.8 / 42.2 ms** (GPUI), and **54.0 / 50.5 / 30.1 ms** (Rune) on the laptop, and **161.4 / 172.4 / 154.2 ms** (iced), **188.2 / 197.7 / 193.8 ms** (GPUI), and **182.4 / 170.5 / 224.3 ms** (Rune) on Fly. Because `NT_VIDEO_OVERRIDE` bypasses `Player::prepare_next`, no run in either set verifies prepared-stream click-to-frame. Earlier live-network laptop warm timings (`nt-core` and Helium) were outside this warm audit and are omitted above.

**Sanity checks:** All **39,265** readable process samples passed `PSS ≤ RSS`; **135** native open→frame marker pairs passed frame-later-than-open; all **152** hybrid PMU splits summed cleanly; a pinned synthetic steady workload scaled **1.988×** in instructions from 1 to 2 CPU seconds (14.81 B → 29.44 B); peak-to-peak repeat spread was ≤0.47% for laptop playback instructions/s, ≤0.63% for laptop scroll instructions/s, and ≤5.07% for Fly whole-scenario instructions (one notable timing outlier: laptop iced Home run 1 took 1,003 ms vs. 207/209 ms). HiDPI scaling, 4K VP9/AV1 playback, X11, Windows, and macOS benchmarks remain unmeasured.

---

## Sources and Methods

1. **Binding fact-check, power regimes, per-PMU IPC proof, NOW line/crate counts, per-run GPU wheel checks, the 848×476 render size, and the incident breakdown:** `docs/blog/takes/review.md`
2. **Normalized CPU (cores busy and whole-machine share), per-unit instruction rates, warm-switch audit, Fly YouTube frame-timing rerun, and sanity checks:** `docs/media/normalized.md` and `.cache/media/out/normalized.json`
3. **Laptop NOW timings (window, Home, open → frame, local-clip switch):** `docs/media/README.md` milestone table, backed by `.cache/media/out/final/metrics.json`
4. **Laptop Cold/Warm Home, playback and tour PSS, playback CPU, `ntctl` attribution:** `docs/media/README.md` final tables, backed by `.cache/media/out/resources/resources.json` and `.cache/media/out/resources/attribution.json` (battery runs)
5. **Laptop CPU instructions and cycles, i915 GPU busy cycles, RAPL energy:** `docs/media/README.md` counter tables, backed by `.cache/media/out/counters/counters.json` (AC runs; per-run wheel values in `.cache/media/out/counters/*/wheel/*/`)
6. **Stability scores and NOW frame checks:** `.cache/media/out/stability/stability.json`; **incident catalog (43, by area and kind):** `.cache/media/out/hiccups.json`
7. **Fly.io VM benchmark (native map/open/switch, Fly PSS, Fly CPU, whole-run instructions) and YouTube web in Chromium 158 (cold first frame, historical ad-dominated warm runs, frame-timing rerun):** `docs/media/fly-bench.md`, backed by `.cache/fly-bench/out/summary.json` and `.cache/fly-bench/out/normalize-web/`
8. **Matched rerun (prewarm policies, first-frame timeouts, timing ranges, memory, and per-window counters):** `docs/media/fair.md`
9. **Capture and clip receipts:** `.cache/viz/out/sources.json` and `docs/blog/x-clips.json`
10. **Bare shells (460/175/253/259 ms, 113/50/96/78 MiB, wakeups, threads, shell crate counts), first-build cost and time, bounded-range 16.6 s → 0.7 s, mpv script settings, serde `preserve_order` and 1,145 fixtures, macOS text:** `docs/blog/coordinator-notes.md` (shells also in `bench/results/shells.json`)
11. **Parity estimates (±50%):** `docs/review/youtube-parity.md`
12. **Dirty details and framework mechanics:** `docs/upstream/*.md` (7 reports), `apps/gpui/NOTES.md` (heatmap ~20 lines, hole ~50 lines), `apps/iced/NOTES.md` (Clamp, 1×1 badges, ~38 MiB EGL cost, surface prewarm), `apps/rune/NOTES.md` (GPU-switch stall, 2.6 s black hole, 453–474 MiB plateau, ~50 MiB NVIDIA libraries)
13. **Performance gate A/B (iced surface prewarm 209.8 → 112.1 ms at 126 → 162 MiB idle; wheel CPU ratios; idle CPU):** `.cache/perf/out/compare-isDkA304/summary.md` and `.cache/perf/out/compare-F70v7wbg/summary.md`
14. **VA-API 1,129 → 79 ms, usage-cap pauses, zero lint allows:** `.cache/orchestrate/native-tube/ledger.tsv` (verified in `docs/blog/takes/review.md`)
15. **Mechanism claims:** `core/src/video_surface.rs` (desync subsurface placed below the window, `wp_viewporter`, `set_clip`), `core/src/player.rs` (mpv options), `apps/rune/src/view.rs` (`tx.live_only()`), `apps/iced/src/scroller.rs` (`with_translation`), `apps/iced/src/main.rs` (the two `WGPU_*` variables)
