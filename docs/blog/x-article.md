# We Built Native YouTube Three Times in Rust: iced vs. GPUI vs. Rune

*One shared Rust core, three native UI toolkits, real hardware counters, and a clean cloud VM against YouTube in Chromium—here is what actually separated them, down to the CPU instruction.*

---

## 1. What We Built—and How

Debates about Rust GUI frameworks usually end in vibes because nobody builds the same real app three times. Over twelve days, @Priyansh4444 and a team of AI agents (Claude Opus 5.5, GPT 6.2 Sol, and Gemini 4 Neon under one coordinator) did exactly that.

We built **NativeTube**—a logged-out desktop YouTube client with Home feeds, search, watch pages, related videos, scrubbing, and keyboard navigation—in **iced 0.14**, **GPUI** (`gpui-pre` 0.3.12 + `gpui-component` 0.9.1), and **Rune** (`anomalyco/rune`, git rev `4fbd2d6`).

All three share one Rust core (`nt-core`) handling networking, a polite traffic gate, image caching, Chromium's scroll physics, and `libmpv` playback—no JavaScript, Electron, webview, `yt-dlp`, or Lua. Plainly: it calls YouTube's private web API logged out (specifically the visionOS client path), outside YouTube's Terms of Service.

Because the backend is identical, most differences in startup, scrolling, memory, crashes, and effort come from the UI toolkit and how each app drives it (though prewarm policy and thumbnail decoding counts also differ).

[MEDIA: .cache/viz/out/SideBySide.mp4 — All three native Rust clients running the same scripted tour side by side on the laptop, replaying recorded fixtures and a local test clip on the shared core.]

---

## 2. The One Decision That Mattered Most: Route Video Around the UI (When You Have a GPU)

Originally, `libmpv` decoded RGBA frames into CPU memory thirty times a second, and each toolkit uploaded them as textures and redrew the window. On our 16-CPU Linux laptop, that burned **0.44–0.45 cores** (**2.8%** of the machine; battery runs) just to play a muted 720p30 clip, retiring **652–657 M instructions/video-s** on AC—**21.8 M/frame**, or **53.88–54.29 instructions/frame-pixel** at 848×476 (vs. `ntctl play`'s **25.51** at 1280×720).

Borrowing Akshar Patel's `yt-music-gpui` trick, we stopped pushing video through the UI toolkit: `libmpv` renders on the GPU into an independent Wayland subsurface below a transparent UI window, and each app paints chrome around a transparent hole.

[MEDIA: .cache/viz/out/FramePath.mp4 — Moving video off the UI frame loop onto a GPU subsurface skips the per-frame copy and upload: about 19× fewer instructions on the laptop. The no-GPU coda shows the flip: on the Fly VM, the emulated GPU path takes 2.92–2.94 cores vs. 0.91–1.59 for software.]

On the laptop GPU, playback CPU collapsed **4.3× to 4.7×** on the same build and clip (iced 4.3×, GPUI 4.5×, Rune 4.7×)—down to **0.095–0.105 cores** (**0.59–0.65%** of the machine). User-space instructions plunged **18.5× to 20.1×**, to **32.75–35.29 M/video-s** (**1.09–1.18 M/frame**), leaving all three toolkits within about **8%**.

[MEDIA: .cache/viz/out/InstructionsStill.png — CPU instructions per 10 s of playback collapse ~19× once video bypasses the UI framework, leaving all three toolkits within about 8% of each other.]

**The twist:** that win **flips into a loss without a real GPU**. On our headless Fly.io VM (4 AMD EPYC vCPUs, Mesa `llvmpipe` software EGL), the GPU/EGL subsurface path burned **2.92–2.94 cores** (**73.0–73.5%** of the VM)—far *more* than forced software playback (**0.91–1.59 cores**, **22.7–39.7%**; whole-scenario instructions 171–176 B vs. 58–103 B; Fly per-unit instruction rates are unavailable because its counters span whole scenarios). When Mesa emulates OpenGL on the CPU, routing video around the UI hurts. It is a huge win on real GPU silicon and a clear loss without one.

**No energy claim:** System-wide Intel RAPL counters were too noisy to separate playback from idle (70–84 J idle vs. 62–96 J playing; GPU 1.27–1.75 vs. 1.54–1.56 J). We proved CPU work saved, not battery hours.

---

## 3. Startup, First Frame, and Real YouTube in Chromium

On the laptop (AC, frozen fixtures and the local clip), **iced won every latency metric**:

- **Window map:** iced **190 ms**, Rune **238 ms**, GPUI **293 ms**
- **Home loaded:** iced **209 ms**, Rune **252 ms**, GPUI **310 ms**
- **Open → first frame:** iced **95 ms**, GPUI **118 ms**, Rune **189 ms**
- **Local-clip switch (running player):** iced **35.5 ms**, GPUI **43.1 ms**, Rune **50.5 ms**

Those switch times (and Fly's below) measure switching on an already-running player with the local clip (`NT_VIDEO_OVERRIDE` bypasses stream preparation). Prepared-stream click-to-frame wasn't measured.

[MEDIA: .cache/viz/out/FirstFrame.mp4 — Open → first frame: the native apps on the laptop GPU and on the Fly VM's CPU rendering, beside YouTube web's cold first content frame on the same Fly VM (a full page load over the live network, so not like for like).]

[MEDIA: .cache/viz/out/FirstSecondReplay.mp4 — Quarter-speed replay of the first second after opening a video on the laptop: iced and GPUI show their first frame before Rune.]

iced almost lost startup: out of the box, bare iced 0.14 was *slowest* (**460 ms**, **113 MiB**, **107 wakeups/s**) because `iced_wgpu` defaulted to `HighPerformance` and woke our NVIDIA GPU. Setting `WGPU_POWER_PREF=low` and `WGPU_BACKEND=vulkan` dropped it to **175 ms**, **50 MiB**, and **2 wakeups/s**, beating bare GPUI (**253 ms / 96 MiB**) and Rune (**259 ms / 78 MiB**).

### Against real YouTube in Chromium

On one clean 4-vCPU Fly.io VM (8 GiB RAM, headless Sway, software rendering), we ran all three binaries beside signed-out, muted **YouTube web in Chromium 158**:

- **Native apps on Fly (fixtures):** map / open → first frame were iced **142 / 191 ms**, GPUI **229 / 189 ms**, Rune **264 / 300 ms**; local-clip switches on a running player took **161.4 / 193.8 / 182.4 ms** (prepared-stream click-to-frame wasn't measured).
- **Live YouTube in Chromium (cold):** **1,857 ms (~1.9 s)** to first content frame (854×480), burning **1.24 cores** (**30.9%** of the VM) at **865 MiB PSS** (**1,014 MiB peak**).
- **Live YouTube warm switch (clean rerun on a fresh Fly VM):** Early warm trials sat behind pre-roll ads, so a trusted-click probe separated (a) any frame, (b) the requested video excluding ad time, and (c) ad duration. **No ads were skipped or blocked:**

| Warm trial (Chromium on Fly) | (a) Any frame | Actual (wall) | (b) Excl. ad | (c) Ad duration |
|---|---:|---:|---:|---:|
| Trial 1 | 0.613 s | >130 s (timeout) | — | — |
| Trial 2 | 0.717 s | 0.717 s | 0.717 s | 0 s |
| Trial 3 | 0.735 s | 16.064 s | 0.693 s | 15.371 s |

Warm trials hit a first frame in **0.61–0.74 s** (cold rerun trials: **2.23–3.30 s**, ad-free). Trial 2 reached the requested video in **0.717 s**; Trial 3 showed a **15.4 s** ad (**16.1 s** wall, **0.693 s** excl. ad); Trial 1 timed out behind an ad.

[MEDIA: .cache/viz/out/SideBySideFly.mp4 — The same tour on the Fly VM in four tiles: iced, GPUI and Rune replaying fixtures and the local clip, and signed-out YouTube web in Chromium on the live network. In this recording the browser clicked Skip on one ad; the timing rerun above skipped and blocked nothing.]

Fly compares fixtures to live YouTube, so it isn't like for like. On the laptop over the live network (still bare player vs. web app), standalone `nt-core` hit a cold first frame in **451 ms** (~141 MiB PSS), while YouTube in Helium (with ad blocking, so no pre-roll—a best case for YouTube) took **4,170 ms** cold, using **0.638 cores** (**3.99%** of the laptop) and **978 MiB PSS**.

**Prewarm trade-off:** iced and GPUI prewarm both `libmpv` and the GPU surface after Home, while Rune creates its EGL surface on first click—likely part of Rune's 189 ms open time, though unisolated. Mapping `libmpv` adds **38–54 MiB** (Cold → Warm Home), and disabling iced's surface prewarm in our gate saved ~36 MiB idle (162 → 126 MiB) while costing ~98 ms on first play (112.1 → 209.8 ms).

---

## 4. Scrolling: The Order Flips Between GPU and Software Rendering

On the same scripted 27-tick mouse-wheel scroll (~2.7 s; ~162 frames at 60 fps on the laptop), the leaderboard flipped between machines.

On the **GPU laptop**, CPU work split wide open:

- **Rune:** **375 M instr/s** (**6.22 M/frame**, 1,013 M total) — **0.211 cores** (**1.32%** of the 16-CPU laptop)
- **iced:** **570 M instr/s** (**9.45 M/frame**, 1,540 M total; 1.52× Rune) — **0.266 cores** (**1.66%**)
- **GPUI:** **1,117 M instr/s** (**18.65 M/frame**, 3,021 M total; **2.98× Rune**) — **0.444 cores** (**2.77%**)

In our separate performance gate, GPUI's wheel CPU was **2.1× iced's** and **1.7× Rune's** (iced slightly below Rune), so we don't call a winner between iced and Rune on laptop scroll CPU—only that GPUI is **1.7–3× heavier**.

[MEDIA: .cache/viz/out/ScrollCost.mp4 — On a GPU laptop, Rune uses a third of GPUI's CPU instructions to scroll; on the software-rendered Fly VM (wheel CPU, a different measure), the order flips. The video's per-frame redraw line is our best explanation, not a measured mechanism.]

Why? **Rune** writes `[0, -offset]` into a `Live<[f32; 2]>` GPU binding without rebuilding views or layout between virtualization bands; **iced** translates children in a custom `Scroller`; **GPUI**, even split into five cached views, invalidates its page view and runs Taffy layout. Not all of the gap is the toolkit: GPUI's agent traced much of the remaining wheel CPU to `nt-core` decoding poster thumbnails, and each app decodes a different number while scrolling.

System-wide laptop **GPU** counters (including other GPU work) showed the flip side: **GPUI did roughly half Rune's render work**—**35.6 M** cycles vs. **68.3 M** (iced's clean run 3 hit **38.5 M**, after runs 1–2 saw unrecorded system activity).

That is our best explanation for the **software-rendered Fly VM**, where Mesa rasterizes on the CPU (CPU is wheel-phase; instructions span the whole run since Fly lacks per-unit PMU rates):

- **iced:** **1.35 cores** (**33.7%** of the 4-vCPU VM; **31.0 B** whole-run instructions)
- **GPUI:** **1.96 cores** (**49.0%**; **34.5 B** instructions)
- **Rune:** **2.80 cores** (**70.0%**; **65.2 B** instructions—**2.1×** iced, **1.9×** GPUI; same order minus idle)

Without a GPU, Rune's heavier draw scene lands on the CPU and the order flips: iced cheapest, Rune costliest. (We didn't measure the mechanism directly, though it fits the laptop GPU counters.)

---

## 5. Memory: What the Player Costs vs. What the Framework Costs

At idle, all three sleep cleanly (**6.0 M instructions / 10 s**, ≤0.019% of laptop capacity, no measurable idle CPU in final gates). In memory, **iced stayed leanest at every stage** on the laptop (battery runs):

| Stage (laptop PSS) | iced | GPUI | Rune |
|---|---:|---:|---:|
| Cold Home (`NT_NO_PREWARM=1`) | 99 MiB | 143 MiB | 131 MiB |
| Warm Home (default prewarm) | 153 MiB | 188 MiB | 169 MiB |
| Software playback | 211 MiB | 255 MiB | 244 MiB |
| GPU playback | 217 MiB | 253 MiB | 274 MiB |
| Scripted tour peak | 267 MiB | 289 MiB | 337 MiB |

[MEDIA: .cache/viz/out/Memory.mp4 — Laptop memory climbs from Cold Home to Warm Home, software Playback, and Tour Peak—with Rune overtaking GPUI by the tour peak—then the Fly VM's native peaks beside signed-out YouTube web (865 MiB settled, 1,014 MiB peak; different workloads).]

Standalone `ntctl play` took **116 MiB PSS** (**0.362 cores / 2.26%** of the laptop in software mode)—**45–55%** of software playback RAM—leaving **95 / 129 / 139 MiB** of frontend overhead for iced / Rune / GPUI (framework plus app code).

**Rune** is the surprise: lighter than GPUI as a bare shell (**78 MiB**, 11 threads vs. **96 MiB**, 32 threads), yet heaviest in GPU playback (**274 MiB**) and tour peak (**337 MiB**). Over 60 live opens it plateaued at **453–474 MiB**—not a leak, just `nt-core`'s 96 MiB image cache and 40-page back stack on top of ~160 MiB of mapped libraries.

On software-rendered Fly (graphics buffers in system RAM), native peaks (**422–485 MiB**, default fixture playback scenario) stayed under half Chromium's **1,014 MiB** peak (865 MiB settled) for one cold live YouTube load (different workloads, so read as a rough gap).

---

## 6. Stability and the Week of Hiccups

Our stability harness (`stability.py`) scored **100 (3/3 clean)** across all **30 assessable historical runs** (10 build × app cells, software path), and NOW switch checks found **0 black, 0 stale, 0 frozen frames** out of 9 per app.

The real story is our **43-incident catalog** (all fixed): **14** in `nt-core`, **7** iced, **7** Rune, **6** GPUI, and **9** orchestration/tooling.

[MEDIA: .cache/viz/out/Hiccups.mp4 — All 43 recorded project incidents from discovery to fix, by area: 3 crashes (core, GPUI, Rune) and 2 stalls (core, Rune); none in iced's own code, though core's crash hit every app.]

Frontend counts tied (6–7 each), but failure modes diverged:

- **GPUI's re-entrancy panic:** Lacking a public cursor-hide API, our agent dispatched a synthetic keystroke in `cx.defer_in`; `dispatch_keystroke` redrew `PlayerView` mid-update and panicked at `entity_map.rs:192` ("already being updated"). `Window::defer` fixed it. In iced's `&mut App` Elm loop, the compiler forbids that bug.
- **iced's disappearing focus:** Focusing search changed an adjacent icon's width from zero; `row!` (0.14.2) skips zero-width children, shifting the input's index so positional diffing reset focus. (0.14 also lacks `unfocus()`. None of the catalog's crashes or stalls were in iced's own code—but there were only three crashes total, the catalog holds only what got written down, and `nt-core`'s storyboard crash hit all three apps, iced included.)
- **Rune's release abort and GPU stalls:** Combining `.wrap()`/`.truncate()` with `.live_text()` hit a release `assert!` (`flatten.rs:258`). Separately, GPU frames failed to wake Rune to open its hole (stalls), and opening the hole before decode left a **2.6 s black hole**.
- **Drivers, Cargo, and macOS:** Unfiltered EGL probing loaded both Mesa and NVIDIA (~38–50 MiB wasted) until `select_egl_vendor()` pinned the compositor's vendor; a GPUI transitive dep enabled `serde_json` `preserve_order`, breaking fixture replay until we canonicalized keys; and on macOS, **only Rune** rendered readable text out of the box (GPUI showed none; iced rendered dots).

---

## 7. Building With AI Agents: Cost, Speed, and Ergonomics

First full builds under Claude Opus 5.5 cost nearly the same: **iced $23.54 (49 min)**, **Rune $27.99 (55 min)**, **GPUI $28.41 (59 min)**.

[MEDIA: .cache/viz/out/CostStill.png — First full builds cost nearly the same: $23.54–$28.41 in 49–59 minutes.]

Review rounds and polish drove most of the spend. Our review council's **±50% estimates** for full YouTube parity (context menus, miniplayer, chapters, heatmaps, storyboards, guide drawer)—overlapping ranges, atop **~36 h / ~$1,000** of shared core work—put **GPUI** lowest (**~28 h / ~$780**), **Rune** next (**~40 h / ~$1,100**), and **iced** highest (**~44 h / ~$1,220**).

**GPUI** ships layered chrome (`anchored()`, `deferred()`, blur, `PathBuilder`), yielding a **~20-line** heatmap and **~50-line** video hole, trading against borrow panics and a ~30-line Wayland fling patch. **iced 0.14** needed zero patches and prevented re-entrancy crashes, but taxed overlays with Elm `Message` plumbing plus hand-rolled ellipsis (`Clamp`) and 1×1 image-badge workarounds. **Rune** started most concise (**2,508 lines** at first delivery vs. GPUI's **3,045** and iced's **3,762**; now **10,631 / 12,653 / 13,518**) with natural reactive bindings, but needed manual theming and `[patch.crates-io]` forks (`winit`, `wgpu-hal`).

Across five usage-cap pauses and handoffs (Opus 5.5 → GPT 6.2 Sol → Gemini 4 Neon), agents stayed honest via an audited spec, frozen fixtures, headless virtual-pointer runs, mandatory screenshots, Bombadil-style property tests, and strict Clippy lints (`unwrap_used`, `expect_used`, `panic` denied—**zero** `#[allow]`/`#[expect]` in our own code; only the vendored GPUI patch keeps upstream's).

---

## 8. The Verdict: Who Should Pick Which?

- **Pick iced** for lean binaries (**29.6 MiB**), lowest laptop memory (**99 MiB** cold / **267 MiB** peak), fastest laptop startup to a window (**190 ms**), zero vendored patches, and compile-time safety against state re-entrancy. On 0.14, set `WGPU_POWER_PREF=low` and `WGPU_BACKEND=vulkan` (upstream `master` has since fixed the power default and added ellipsis).
- **Pick GPUI** for chrome-dense desktop workspaces—popovers, floating menus, custom vector UI—where the lowest estimated UI effort matters most. Accept larger binaries (**52.7 MiB**), heavier scroll CPU, small Linux Wayland patches, and strict entity-borrow discipline.
- **Watch Rune, don't bet production on it yet.** Its `Live` architecture retired the fewest CPU instructions to scroll on the laptop and rendered cleanly on macOS out of the box, but git pins, `[patch.crates-io]` forks, heavier software-rendering draw cost, and pre-1.0 panics make it an R&D pick.
- **No matter which toolkit you pick:** put networking, caching, scroll physics, and playback in a shared headless Rust core—and **match the video path to the hardware**. Routing video around the UI onto a GPU subsurface is a **4.3–4.7× CPU win** on a real GPU (~0.1 vs. ~0.45 cores), and a **clear loss without one** (~2.9 vs. 0.9–1.6 cores under CPU-emulated EGL).

---

## 9. For the Nerds: Hardware Counters, Normalized Tables, Internals, and Receipts

### Normalized CPU and per-unit instruction tables

CPU share is `CPU-seconds ÷ wall-seconds ÷ logical CPUs` (16 logical CPUs on the laptop; 4 AMD EPYC vCPUs on Fly), covering the benchmarked app or browser process and excluding the compositor. "Cores busy" is `CPU-seconds ÷ wall-seconds`. All native values below are medians of 3 runs; parenthetical deltas compare against that machine's software `ntctl play` baseline.

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

(Note on wheel denominators: `normalized.md` and `InstructionsStill.png` divide by `counters.json`'s active scenario seconds, yielding 570 / 1,117 / 375 M/s; `docs/media/README.md` divides by full counter wall time, yielding 568 / 1,112 / 374 M/s.)

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

Our laptop runs an Intel Arrow Lake-P hybrid CPU (`cpu_core` P-cores, `cpu_atom` E-cores, `cpu_lowpower` LP-E cores). `perf stat -p` recorded unscaled user-space counts per PMU and summed them; in the normalized aggregate, each median total carries the PMU partition from its own observed run rather than independent per-PMU medians.

In forced software playback, standalone `ntctl play` retired **more** total instructions (**7,065.18 M**) than any GUI app (**6,525–6,575 M**), yet used **18–20% fewer cycles** (**3,741.26 M** vs. **4,591–4,695 M**) and less battery CPU (**0.362 cores / 2.26%** vs. **0.441–0.452 cores / 2.76–2.83%**). Per-PMU scheduled-time shares refute P/E-core scheduling as the cause: `ntctl` (P 23–30%, E 53–54%, LP 16–22%) matched the apps (P 22–31%, E 52–62%, LP 13–20%). Instead, `ntctl` never calls `set_target_size` and decodes full **1280×720** at **1.89 IPC** (**25.51 instructions/frame-pixel**), whereas all three Watch pages size `mpv` to the player box—floored to a multiple of 16 at **848×476** (44% of 720p pixels, **~1.6 MB/frame** RGBA)—retiring **53.88–54.29 instructions/frame-pixel** at **1.40–1.42 IPC**. The apps spend ~850–950 M extra cycles, most likely copying, uploading, and presenting frames (an inference—no stall counters were collected).

[MEDIA: .cache/viz/out/PerCoreStill.png — Forced software playback, by instructions retired rather than scheduled time: P-cores retire 35.1–38.8% of each app's instructions and 39.9% of ntctl's, so the extra work isn't a scheduling artefact.]

### GPU engine busy cycles (Intel i915 PMU)

On Arrow Lake-P, `GT0` hosts render (`RCS`), copy (`BCS`), and compute (`CCS`); `GT1` hosts video decode (`VCS`). Estimated busy cycles equal `busy_ns × avg_GT_MHz / 1000` (system-wide, so they include anything else on the GPU).

- **GPU playback (10 s):** all three converged tightly—RCS **69.63** (iced) / **74.71** (GPUI) / **75.06** (Rune) Mcycles; VCS **16.19 / 9.12 / 8.70** Mcycles; BCS and CCS 0.00; RC6 sleep residency **7.75–7.82 s** out of 10 s.
- **Wheel scroll (~2.7 s, 161–163 frames at 60 fps):** GPUI logged **35.56 Mcycles** RCS (runs: 28.4, 46.7, 35.6; 0.00 VCS) vs. Rune's **68.33 Mcycles** (runs: 77.4, 68.3, 67.8; 0.00 VCS). Rune also logged 1–2 long frames per run (max 29–64 ms), while iced and GPUI had zero. iced's wheel runs 1–2 were contaminated by other system activity, source unrecorded (RCS 348.0 / 123.6, VCS 18.1 / 14.3, package 48.1 / 48.8 J); clean run 3 dropped to **38.5 Mcycles** RCS and **0.46** VCS (23.2 J).

[MEDIA: .cache/viz/out/GPUStill.png — System-wide i915 render and video-decode busy cycles during GPU-subsurface playback: all three apps converge.]

### Subsurface, EGL vendor selection, and playback pipeline details

`nt-core` embeds `libmpv` into a desync child `wl_subsurface` placed below the window, clipped to the player rectangle via `wp_viewporter` (`VideoSurface::set_clip`), with `video-timing-offset=0` and every `load-*` mpv script explicitly disabled (`load-scripts=no` alone still ran 6 Lua scripts). Against `googlevideo`'s open-ended `Range: bytes=0-` throttling, bounded requests (`request_size` 10 MB, initial 1 MB) cut first frame on a 14-hour video from **16.6 s to 0.7 s**.

On our hybrid Intel + NVIDIA laptop, unfiltered EGL probing pulled in both Mesa/LLVM and NVIDIA libraries (~38 MiB extra in iced, ~50 MiB of NVIDIA libraries in Rune) until `nt-core` added `select_egl_vendor()` to load only the compositor's EGL vendor (Mesa here). Earlier, `libmpv`'s CUDA probing cost **1,129 ms** on GPU attach until forcing VA-API interop dropped it to **79 ms** (a single coordinator measurement). On Fly's headless Sway + Pixman VM (`Mesa 26.3.1`), GBM initialization failed and EGL fell back to CPU `llvmpipe` (**2.92–2.94 cores / 73.0–73.5%** of the VM during playback; 171.5–176.4 B whole-scenario instructions).

[MEDIA: .cache/viz/out/EGLStill.png — How nt-core picks one EGL vendor on a hybrid machine: Wayland main device → DRM driver → matching EGL vendor, aimed at skipping the other GPU's EGL vendor at startup.]

Two more multi-app integration notes from the build: a transitive GPUI dependency enabled `serde_json`'s `preserve_order`, altering JSON key order in GPUI alone until we canonicalized keys and re-recorded **1,145 fixtures**; and GPUI separately lost variable-font weights on Linux until we bundled static Roboto weights.

### Memory attribution and prewarm receipts

Cold Home (`NT_NO_PREWARM=1`, `libmpv` unmapped) → Warm Home (default prewarm, `libmpv` mapped throughout the idle window) adds **+54 MiB** in iced (99 → 153 MiB), **+45 MiB** in GPUI (143 → 188 MiB), and **+38 MiB** in Rune (131 → 169 MiB); on Fly, Warm Home settled at 235 / 315 / 220 MiB (vs. Cold Home 175 / 273 / 182 MiB). Separately, in our performance gate A/B on iced, disabling post-Home GPU surface prewarm dropped idle memory from **162 MiB to 126 MiB** (−36 MiB) while raising click-to-first-frame from **112.1 ms to 209.8 ms** (+98 ms, failing its +75 ms budget).

[MEDIA: docs/media/attribution-NOW-settled_pss_mib.png — Player attribution on software playback: standalone ntctl play accounts for 45–55% of total memory across all three apps.]

### Framework mechanics under the hood

- **Rune:** `poll` pushes `[0, -offset]` into `Live<[f32; 2]>` via `Tx::live_only()`, bypassing view and layout commits between half-viewport bands. Combining `.wrap()`/`.truncate()` with `.live_text()` hits a release `assert!` at `flatten.rs:258` (`4fbd2d6`). To reach Wayland from Rune, our agent leaked a `'static` string for `set_app_id` and used `libloading` for `wl_proxy_get_display`.
- **GPUI:** Entities live in an `EntityMap` guarded by runtime borrow leases (`lease_erased`); synchronous re-entry via `cx.defer_in` → `dispatch_keystroke` panics at `entity_map.rs:192`. Even split into five cached views, scrolling still pays Taffy layout, and the GPUI agent traced much of the remaining wheel CPU to `nt-core` poster decoding.
- **iced:** `update(&mut self, Message) -> Task` prevents re-entrancy at compile time. Our `Scroller` shifts laid-out children with `with_translation(0, -offset)` between bands—note that autopilot `Scroll` messages still route through `update` and trigger `view` rebuilds that real wheel input skips. Overlaying the search icon and changing only padding worked around `row!` skipping zero-width children in 0.14.2.

### Sizes, crate counts, and local upstream reports

| Metric (NOW) | iced | Rune | GPUI |
|---|---:|---:|---:|
| Stripped release binary | 29.6 MiB | 42.2 MiB | 52.7 MiB |
| Cargo.lock packages (bare shell) | 480 (319) | 490 (434) | 812 (640) |
| App Rust source lines (first delivery) | 13,518 (3,762) | 10,631 (2,508) | 12,653 (3,045) |

(`ntctl` stripped binary: **11.0 MiB**.)

[MEDIA: .cache/viz/out/SizesStill.png — Stripped release binaries and lockfile package counts for the three NOW apps.]

We wrote up **7 local upstream bug reports** in `docs/upstream/` (not yet filed upstream):

- **iced (3):** `HighPerformance` GPU default (fixed on `master` `98bd48fe`), zero-width `row!` child resetting sibling focus (0.14.2), missing `unfocus()` helper (0.14.0). Zero patches needed.
- **GPUI (3 + Linux gap):** Wayland dropping `wl_pointer.axis_stop` (~30-line vendored patch), private `hide_cursor_until_mouse_moves`, re-entrant `dispatch_keystroke` panic (`entity_map.rs:192`), plus `paint_surface` being macOS-only.
- **Rune (1 + git pins):** wrapped/truncated `LiveText` release abort (`flatten.rs:258`), alongside `[patch.crates-io]` forks for `winit`/`wgpu-hal`.

[MEDIA: .cache/viz/out/UpstreamStill.png — Framework bugs we found (written up, not yet filed): the seven local reports in docs/upstream/, three each for iced and GPUI and one for Rune.]

### Methods, warm audit, sanity checks, and limitations

Every laptop benchmark metric is a median of **n = 3** runs (round-robin across apps for timings; back to back per app for counters) on one 16-CPU Arrow Lake-P + NVIDIA Hyprland laptop (1280×800, scale 1, off-screen `NT-BENCH` output), driven by `NT_AUTOPILOT` against frozen fixtures (index `8e94547e…`) and one muted 1280×720 30 fps H.264 clip (`a9766ffa…`), `NT_NO_MPRIS=1`, autoplay off. Timing milestones and `perf` counters ran on **AC (`balance_performance`)**; all 39 resource/PSS runs ran on **battery (`power`)**—never compare raw numbers across those two power regimes. Fly numbers are medians of 3 runs per workload on one `performance-4x` VM with CPU rendering; never mix them with laptop GPU numbers. Bare-shell figures are 8 runs, parity hours are ±50% estimates, and the VA-API figure is a single measurement.

**Warm-switch audit:** Per-run local-clip switch times (command → `playback_started` render/paint hook) were **35.5 / 37.0 / 32.5 ms** (iced), **43.1 / 44.8 / 42.2 ms** (GPUI), and **54.0 / 50.5 / 30.1 ms** (Rune) on the laptop, and **161.4 / 172.4 / 154.2 ms** (iced), **188.2 / 197.7 / 193.8 ms** (GPUI), and **182.4 / 170.5 / 224.3 ms** (Rune) on Fly. Because `NT_VIDEO_OVERRIDE` bypasses `Player::prepare_next`, no run in this set verifies prepared-stream click-to-frame. Earlier live-network laptop warm timings (`nt-core` and Helium) were outside this warm audit and are omitted above.

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
8. **Live `nt-core` vs. YouTube in Helium on the laptop (451 ms, 4,170 ms, 0.638 cores / 3.99%, 978 MiB):** `bench/results/playback.json` and `docs/media/fly-bench.md` (Helium ran uBlock Origin; see `notes.web`)
9. **Bare shells (460/175/253/259 ms, 113/50/96/78 MiB, wakeups, threads, shell crate counts), first-build cost and time, bounded-range 16.6 s → 0.7 s, mpv script settings, serde `preserve_order` and 1,145 fixtures, macOS text:** `docs/blog/coordinator-notes.md` (shells also in `bench/results/shells.json`)
10. **Parity estimates (±50%):** `docs/review/youtube-parity.md`; **`yt-music-gpui` prior art:** `docs/review/yt-music-gpui-notes.md`
11. **Dirty details and framework mechanics:** `docs/upstream/*.md` (7 reports), `apps/gpui/NOTES.md` (heatmap ~20 lines, hole ~50 lines), `apps/iced/NOTES.md` (Clamp, 1×1 badges, ~38 MiB EGL cost, surface prewarm), `apps/rune/NOTES.md` (GPU-switch stall, 2.6 s black hole, 453–474 MiB plateau, ~50 MiB NVIDIA libraries)
12. **Performance gate A/B (iced surface prewarm 209.8 → 112.1 ms at 126 → 162 MiB idle; wheel CPU ratios; idle CPU):** `.cache/perf/out/compare-isDkA304/summary.md` and `.cache/perf/out/compare-F70v7wbg/summary.md`
13. **VA-API 1,129 → 79 ms, usage-cap pauses, zero lint allows:** `.cache/orchestrate/native-tube/ledger.tsv` (verified in `docs/blog/takes/review.md`)
14. **Mechanism claims:** `core/src/video_surface.rs` (desync subsurface placed below the window, `wp_viewporter`, `set_clip`), `core/src/player.rs` (mpv options), `apps/rune/src/view.rs` (`tx.live_only()`), `apps/iced/src/scroller.rs` (`with_translation`), `apps/iced/src/main.rs` (the two `WGPU_*` variables)
