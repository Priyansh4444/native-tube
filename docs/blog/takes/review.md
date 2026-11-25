# Fact-check of the two takes (neon.md, carbon.md)

Checked 2026-11-24 against the evidence set in the take brief, the raw receipts beside it, the
Cargo.lock files, `git` line counts and the source. Read-only: nothing was built or launched.

Verdicts: **C** confirmed · **CORR** corrected · **U** unsupported · **M** misleading (the number is
right but the framing is wrong).

Abbreviations: `README` = docs/media/README.md · `metrics` = .cache/media/out/final/metrics.json ·
`res` = .cache/media/out/resources/resources.json · `attr` = .../resources/attribution.json ·
`ctr` = .cache/media/out/counters/counters.json · `isDk` / `F70` = .cache/perf/out/compare-isDkA304
and compare-F70v7wbg summary.md · `coord` = docs/blog/coordinator-notes.md · `ledger` =
.cache/orchestrate/native-tube/ledger.tsv (line numbers include the header) · `hic` =
.cache/media/out/hiccups.json · `parity` = docs/review/youtube-parity.md.

## 0. Points that apply across both takes

These are the facts behind most of the corrections. They aren't written down anywhere else.

1. **Two power conditions.** Milestone timings (`metrics`) and counters (`ctr`) ran on AC with
   `balance_performance`. All 39 resource runs (`res`: 12 per app plus 3 `ntctl`) ran on battery
   with the `power` profile (README:28–30, and every `res` run has `"ac": false`). "44–45% software
   CPU", "36% ntctl" and "9.5–10% GPU playback" come from the battery runs. "41–42% at 2caf0fa"
   and "9.0/10.0/10 at NOW" come from the AC milestone table.
2. **Different harnesses.** The perf gate (`isDk`, `F70`, `tools/check-performance.py`) and the media
   collector measure different scenarios. The gate's `idle_pss_mib`, `click_first_frame_ms` and
   `wheel_cpu_percent` aren't the README's Idle PSS, Open→frame and wheel instruction counts. For
   example, the gate puts Rune's click→frame at 277.3 ms, while the README has 189 ms for the same
   build.
3. **Counter medians are computed per key** (`tools/media/counters.py:397–400`, `median_tree`). The
   three runs of each app and scenario ran back to back (`counters.py:414–420`). A median row's RCS,
   VCS and joules can therefore come from different runs.
4. **Stale code-size figures.** `docs/review/frontends.md` was committed at cb25193 (2026-11-16) and
   describes the apps as first delivered. Its line and crate counts no longer hold:

   | | iced | GPUI | Rune | source |
   |---|---:|---:|---:|---|
   | App source lines, 96df4ec (first delivery) | 3,762 | 3,045 | 2,508 | `git show 96df4ec:apps/*/src/*.rs \| wc -l` |
   | App source lines, 2caf0fa | 8,853 | 7,026 | 5,587 | same |
   | **App source lines, NOW (HEAD = working tree)** | **13,518** | **12,653** | **10,631** | `find apps/*/src -name '*.rs' \| xargs cat \| wc -l` |
   | Lockfile packages, 96df4ec–bf276b1 | 431 | 811 | 500 | `grep -c '^\[\[package\]\]'` |
   | Lockfile packages, 2caf0fa | 479 | 811 | 488 | same |
   | **Lockfile packages, NOW** | **480** | **812** | **490** | apps/*/Cargo.lock |
   | Bare-shell lockfile packages | 319 | 640 | 434 | shells/*/Cargo.lock; coord:71–74 |

5. **Binary units.** The app sizes in the README are true MiB: iced 31,012,120 B = 29.58 MiB,
   GPUI 55,256,920 B = 52.70 MiB, Rune 44,263,384 B = 42.21 MiB (`metrics` NOW
   `stripped_bytes`). The bare-shell sizes in coord:71–74 are decimal MB (`bench/shells.py:210`,
   `/1e6`). 12.5 / 27.7 / 34.1 MB is 11.9 / 26.4 / 32.5 MiB. The shell "PSS MB" column is really
   MiB (`shells.py:150`, kB/1024).
6. **Why the software-playback apps run fewer instructions than `ntctl`.** Yes: `ntctl` renders
   the full video size and the apps render at the size of the player box.
   - `ntctl play` creates `Player::new` and never calls `set_target_size`
     (core/src/bin/ntctl.rs:541, and no call anywhere in the file). The default target is
     1280×720 (core/src/player.rs:806). Its log confirms `VideoFrame(1280x720 Rgba)`
     (.cache/media/out/counters/core/playback-software/1/ntctl.log:1).
   - All three apps pass the player box in physical pixels to `set_target_size`
     (apps/iced/src/player_view.rs:71, apps/gpui/src/player.rs:492, apps/rune/src/video.rs:797).
     They share one Watch geometry: the sidebar is `avail × 0.29` clamped to 300–402
     (apps/iced/src/view.rs:2397–2421, apps/gpui/src/pages.rs:439, apps/rune/src/pages.rs:1039).
     At a 1280×800 window the player is about 858×482. `render_size` (core/src/player.rs:1945)
     floors the width to a multiple of 16, so mpv renders **848×476**: 44% of 1280×720's pixels,
     and **≈1.6 MB per RGBA frame, not 3.7 MB**.
   - **P/E-core scheduling doesn't explain it.** `ntctl`'s per-PMU running shares (P-core
     23–30%, E-core 53–54%, LP 16–22%) match the apps' (P 22–31%, E 52–62%, LP 13–20%). Its IPC is
     higher on *both* core types (run 1: E-core 3,341M / 1,887M = 1.77 against iced's 1.40; P-core
     2.56 against 1.57) (`counters/{core,iced}/playback-software/*/sample.json`).
   - So the smaller render target removes instructions, which is why the app-minus-reference
     residual is −488 to −540 M. The frontends still add about 850–950 M cycles of low-IPC work
     (copying the frame, uploading it, presenting). That last step is an inference, because no
     stall counters were collected.
7. **The wheel story, run by run.** All three apps draw about 162 frames at about 60 fps in the
   2.7 s wheel phase (`counters/*/wheel/*/report.json`, `phases[0].frames` 161–163). GPUI's lower
   GPU work therefore isn't explained by dropped frames. In these runs the app's own wheel CPU was
   Rune 20.7–21.4%, iced 24.8–28.1% and GPUI 44.0–45.5%. Rune had 1–2 long frames per run (max
   29–64 ms), while iced and GPUI had 0. iced's three wheel runs had RCS 348.0 / 123.6 / 38.5
   Mcycles, VCS 18.1 / 14.3 / 0.46 and package energy 48.1 / 48.8 / 23.2 J. Run 3 is clean, at
   GPUI's level (GPUI 28.4–46.7, Rune 67.8–77.4).
8. **The energy counters can't judge the video subsurface.** Package energy for 10 s of idle
   (iced 83.8, GPUI 70.0, Rune 74.5 J) is about the same as for 10 s of playback (62–96 J). Single
   idle runs spike 5–60× in RCS (iced idle run 3: 82 Mcycles; GPUI idle run 2: 59.2). GPU joules
   barely moved from the software path to the GPU path (iced 1.74 → 1.56, GPUI 1.27 → 1.56, Rune
   1.75 → 1.54). Neither take claims an energy saving, and the article shouldn't either.

## 1. Every claim, with its verdict

### neon.md

| # | Claim | V | Evidence |
|---|---|---|---|
| N1 | iced 0.14, gpui-pre 0.3.12 + gpui-component 0.9.1, Rune rev 4fbd2d6 | C | coord:11–13 |
| N2 | Subsurface cut playback CPU ~4.5×, from 44–45% to 9.0–10.0% | C | Within `res` (battery, NOW, same clip): 45.08→10.47, 45.23→10.08, 44.14→9.45, i.e. **4.3× / 4.5× / 4.7×**. The "9.0" is Rune's AC milestone value (`metrics` NOW 9.0), not the same table; the low end in `res` is 9.5 |
| N3 | CPU instructions fell ~19×, from ~6,550M to 328–353M | C | `ctr`: 6,545.96/352.61 = 18.6×, 6,525.11/352.92 = 18.5×, 6,574.66/327.54 = 20.1×. These are user-space instructions only; cycles fell 6.2–7.1× |
| N4 | Parity UI ~28 h GPUI, ~44 h iced, ~40 h Rune | C | parity:117. Estimates, ±50% (parity:112–113) |
| N5 | GPUI uses 3× Rune's instructions while scrolling | C | 3,020.93/1,013.41 = 2.98× (README:202,208) |
| N6 | GPUI has the largest stripped binary, 52.7 MiB | C | 55,256,920 B = 52.70 MiB (`metrics`) |
| N7 | GPUI re-entrancy panic at entity_map.rs:192 | C | docs/upstream/gpui-reentrant-keystroke.md:10 |
| N8 | iced leads at NOW: window 190, Home 209, open 95, warm 36 ms, idle 153 / tour 267 MiB, binary 29.6 MiB | C | README:50–52, 178; `metrics` NOW iced 189.99/209.27/94.70/35.5/152.96 |
| N9 | iced: "zero crashes in the 43-incident catalog" (also "iced never crashed or stalled") | M | True per area: `hic` lists no crash or freeze tagged `iced` (4 wrong behaviour, 3 perf). But crash #17 (storyboard size overflow) is in nt-core, which iced runs. There are only 3 crashes in the whole catalog, and the catalog holds only what was written down (README:99). Review also removed a "potential deadlock" from iced that was never counted as an incident (ledger:23) |
| N10 | 0.14 lacks ellipsis, blur, an `unfocus` helper; the HighPerformance default woke the dGPU (339→199 ms) | C | iced-power-preference.md:28–31, 80–85; iced-unfocus-helper.md:3–8; parity:136–138 |
| N11 | Rune posts the lowest scrolling **and GPU-playback** CPU, thanks to Live/Pixels | M | Scroll instructions: yes (1,013.41M). But the gate's `wheel_cpu_percent` puts iced (21.95%) below Rune (26.54%) (`isDk`). GPU playback: 9.45% against 10.08–10.47% in `res`, with overlapping runs (Rune 9.3–10.1, GPUI 9.3–10.4), and Pixels isn't used on the GPU path |
| N12 | Rune ~2,500 initial lines | C | apps/rune/NOTES.md:12; 2,508 at 96df4ec. It is **10,631 now** |
| N13 | Rune peaked highest in the tour, 337 MiB | C | README:178; `res` rune tour peak 336.6 |
| N14 | Rune: crates.io patches, no raw display callback, wrapped LiveText release abort at `flatten.rs:259` | CORR | Line **258** at the pinned rev 4fbd2d6; 259 is upstream HEAD (rune-wrapped-livetext.md:20–24). The rest: rune NOTES:5–7, 223–226 |
| N15 | serde_json `preserve_order` broke GPUI's replay; 1,145 fixtures | C | coord:137–140 |
| N16 | A CPU-vs-GPU inversion during scrolling | C | Holds for **GPUI vs Rune only**, in every run: GPUI RCS 28.4/46.7/35.6 against Rune 77.4/68.3/67.8, with GPUI's CPU 2.98× Rune's. iced's median is contaminated; its clean run 3 (38.5) is at GPUI's level (§0.7). Mechanism not measured |
| N17 | Rune 238/252/189/50 ms; GPUI 293/310/118/43 ms | C | README:51–52 |
| N18 | Bare iced 460 ms (339 in the repro) → 175 (199); GPUI 253, Rune 259 | C | coord:71–74 (8 runs, matches bench/results/shells.json medians 459.9/175.0/252.8/259.4); iced-power-preference.md:30–31. coord:66–67 marks this table "re-run" |
| N19 | isDk: iced `click_first_frame_ms` 209.8 (+112.5 against a +75 budget), the lone failure | C | `isDk` iced/play row; the only `fail` |
| N20 | select_egl_vendor: renderer init iced 171→106, Rune 505→99 ms | C | ledger:42. One coordinator line with no receipt and no run count (see C18) |
| N21 | iced re-enabled prewarm and passed all 14 at 112.1 (+17.6) | C | `F70` (14 rows, all pass); ledger:44 |
| N22 | Cold Home 99 / 131 / 143 MiB | C | README:168 |
| N23 | Warm Home 153 / 169 / 188 | C | README:169 |
| N24 | Software playback PSS 211 / 244 / 255 against ntctl 116; 45–55% share; 95–139 residual | C | README:170, 184–186; `attr` |
| N25 | GPU playback PSS 217 / 253 / 274 | C | README:174 |
| N26 | Tour peak 267 / 289 / 337 | C | README:178 |
| N27 | Rune is heaviest in GPU playback and the tour "(+70 MiB over iced)" | CORR | +70 applies to the tour (337−267). GPU playback is **+57** (274−217) |
| N28 | Rune bare shell 78 MB / 11 threads against GPUI 96 MB / 32 | C | coord:73–74 (the values are MiB, §0.5) |
| N29 | Rune plateaus at 453–474 MiB over 60 opens: the 96 MiB cache, 40-page stack, ~160 MiB of libraries | C | rune NOTES:296–308 |
| N30 | Binaries 29.6 / 42.2 / 52.7 MiB "(12.5 / 34.1 / 27.7 MB shell, 319 / 434 / 640 crates)"; "scale similarly" | M | The crate counts are the **bare shells'** lockfiles. The app lockfiles are 480 / 490 / 812 now (§0.4). The shells don't follow the apps' order: GPUI's shell is smaller than Rune's, but GPUI's app is larger |
| N31 | Idle: 5.97–6.02M instructions per 10 s (0.60M/s) | C | README:195,201,207 |
| N32 | 0.00% idle CPU in the final gates | C | `F70` iced 0.00; `isDk` gpui and rune 0.00 (iced was 0.44 in `isDk`) |
| N33 | 2–5 wakeups/s in low-power shells against 107/s for default iced | C | coord:71–74 |
| N34 | Software: 6,525–6,575M instructions (~21.8M/frame, 44–45% CPU), "80–82% from ntctl play" | M | 80–82% is ntctl's share of **CPU %** on the battery runs (README:184–186). For instructions, ntctl's reference *exceeds* the app total (residual −488 to −540M, `ctr` `attribution`) |
| N35 | GPU path: Rune 327.54M (1.09M/frame), iced 352.61M (1.18M), GPUI 352.92M (1.18M); 1.54–1.56 J; RCS 69.63–75.06 | C | README:197,203,209,223,229,235 |
| N36 | Wheel: 1,013.41 / 562.40, 1,540.33 / 760.37, 3,020.93 / 1,407.45 M | C | README:196,202,208. These are per wheel phase (~2.71 s), not per second |
| N37 | "Gate `wheel_cpu_percent` agrees: iced 21.95–23.98, Rune 26.54, GPUI 45.87" | M | The numbers are right, but the gate ranks **iced below Rune**, the reverse of the instruction counts. Only "GPUI highest" agrees. In the counter runs Rune was lowest (§0.7) |
| N38 | Wheel GPU: GPUI 35.56 Mc / 0.42 J / 19.29 J; Rune 68.33 / 0.72 / 20.22; iced 123.57 + 14.27 VCS / 1.25 / 48.12 | C | README:222,228,234 (per-key medians) |
| N39 | iced's VCS suggests contamination; GPUI's ~2× GPU advantage over Rune looks real | C | Per run: §0.7 |
| N40 | All assessable builds score 100 (3/3, w=0); NOW 0 black / 0 stale / 0 frozen of 9 | C | README:91–93, 246–248; stability.json |
| N41 | 43 incidents, all fixed | C | README:99 ("43 with dated fix evidence") |
| N42 | core 14: 1 crash, 1 freeze, 8 wrong, 2 perf, 2 review | C | `hic` |
| N43 | iced 7: 4 wrong, 3 perf "(focus loss, prewarm/attach budgets)" | CORR | Focus loss (#11) is **wrong behaviour**. The 3 perf regressions are #30 (idle PSS +54 MiB), #38 (retained memory after Watch) and #40 (click→frame / warm-switch budget) |
| N44 | Rune 7: 1 crash, 1 stall, 3 wrong, 1 review, 1 perf | C | `hic` |
| N45 | GPUI 6: 1 crash, 3 wrong, 1 review, 1 perf | C | `hic` |
| N46 | orchestration + tooling 9: 5 usage-limit, 3 tooling, 1 author rewrite | C | `hic` #32–37, 41–43 |
| N47 | First builds $23.54 / 49 min, $27.99 / 55, $28.41 / 59 | C | coord:83–86 |
| N48 | Parity $780 / $1,100 / $1,220 + core 36 h / $1,000 | C | parity:117–118. The rate is "~$28 per agent hour" (parity:19), but the first builds actually ran $28.8–30.5/h |
| N49 | GPUI built-ins; the heatmap in 20 lines | C | gpui NOTES:129–132 |
| N50 | iced: Message plumbing, stack/Overlay, Clamp, 1×1 images (quads before images), manual hover redraws | C | iced NOTES:129–141; hover fix in the "Hover tint" entry |
| N51 | Rune ~2,500 against iced ~3,800 at the start; parity effort near iced | C | rune NOTES:12; iced NOTES:12; parity:117. Now 10,631 against 13,518 |
| N52 | Zero `#[allow]`/`#[expect]` under crash-proof lints | C | ledger:45; grep finds none in apps/*/src, core, tools; `unwrap_used`/`expect_used`/`panic` = deny in every Cargo.toml |
| N53 | Seven upstream reports | C | 7 files in docs/upstream |
| N54 | iced: zero vendored crates or patches | C | No `[patch]` in apps/iced/Cargo.toml |
| N55 | iced master fixed PowerPreference::None and ellipsis | C | iced-power-preference.md:44–49, 82–85 |
| N56 | No `unfocus()` in iced_runtime 0.14.0; `row!` drops zero-width children (0.14.2) | C | iced-unfocus-helper.md:3–8; iced-focus-sibling-width.md:30–36 |
| N57 | GPUI drops axis_stop; ~30-line vendored patch | C | gpui-axis-stop.md; the diff has 30 ± lines; apps/gpui/Cargo.toml:37 |
| N58 | No public cursor-hide → synthetic keystroke → panic inside `defer_in` | C | gpui-hide-cursor.md:33–38; gpui-reentrant-keystroke.md:14–16 |
| N59 | `paint_surface` is macOS-only; variable fonts broke | C | gpui NOTES:280; coord:125 |
| N60 | Rune: duplicate patches, leaked `'static`, libloading `wl_proxy_get_display` | C | rune NOTES:5–7, 223–226, 290–292 |
| N61 | Only Rune had readable text on macOS (GPUI none, iced dots) | C | coord:126–127 |
| N62 | GPUI: even with five cached views, scrolling invalidates PageView and "burns 3,020.93M" | M | The structure is right (gpui NOTES:178–180), and the pre-fix profile has Taffy on top (.cache/perf/out/gpui-investigate/wheel-profile.txt, 8.8%). But the GPUI agent put the *remaining* wheel CPU down to core poster decoding (hq720 WebP + Lanczos3, ledger:38), which also shows in that profile (nt-core WebP, resize). Each app also decodes a different number of thumbnails (iced builds −1 to +2 viewports; Rune ±1; GPUI visible rows). The whole 3,021M isn't tree rebuilds |
| N63 | iced's Scroller: `with_translation`, no message or layout per scroll frame between bands | C | iced NOTES:62–75. In the measured wheel story, though, each autopilot `Scroll` command goes through `update` (apps/iced/src/app.rs:3506) and triggers a `view` rebuild (frontends.md I4) |
| N64 | Rune: `poll` writes a Live offset with no view commit → 1,013.41M | C | rune NOTES:54–68 |
| N65 | Software playback uploaded "~3.7 MB per 720p frame" (iced texture, GPUI RenderImage, Rune Pixels::set) | M | The NOTES give 3.7 MB *at 720p*, but the benchmark rendered at the player box, 848×476 ≈ **1.6 MB** (§0.6) |
| N66 | The subsurface (wp_viewporter + EGL + painting around a hole) removed 30 fps presentation from the UI loop | C | yt-music-gpui-notes.md:12–34 |
| N67 | HighPerformance woke the dGPU (+70 MB PSS, +103 wakeups/s) | C | 120→50 MB, 106→3/s (iced-power-preference.md:30–31). The fix set two env vars (`WGPU_POWER_PREF=low` **and** `WGPU_BACKEND=vulkan`) |
| N68 | Unfiltered EGL: +38–50 MiB PSS, +90–400 ms init | CORR | PSS: iced ~38 MiB (iced NOTES "Idle memory on Home"), Rune ~50 MiB of NVIDIA libraries (rune NOTES:308) ✓. Init deltas per ledger:42: iced **65**, Rune **406**, GPUI **2** ms (ledger:36 gives iced ~90 ms) |
| N69 | CUDA probing ~1.1 s until vaapi (1,129→79 ms) | C | ledger:13 |
| N70 | Prewarm trades 38–54 MiB of idle PSS for ~100 ms faster first playback | M | 38–54 is cold→warm Home (README:168–169). For Rune that is libmpv only, with no surface. The ~100 ms (209.8→112.1) is iced's **surface-only** A/B, with libmpv prewarmed on both sides. The two numbers measure different things |
| N71 | GPUI's leases (`lease_erased`) turn re-entrant redraws into panics; iced's Elm loop prevents this structurally | C | gpui-reentrant-keystroke.md:52–53. iced's `update(&mut self)` returns Tasks and can't re-enter, which is true by design. The crash counts (1 vs 0) are too few to show it |
| N72 | "Use iced master (or ≥0.15)" | U | Only master (98bd48fe) was checked. No 0.15 release appears in the evidence |
| N73 | The shared core "paid off equally across all three (~36 h once)" | U | 36 h is a remaining-work *estimate* (parity:118). No payoff was measured |
| N74 | Mixed power profiles; 39 battery runs | C | README:28–30; `res` |
| N75 | System-wide i915/RAPL; iced wheel VCS 14.27, 48.12 J | C | README:129, 222 |
| N76 | ntctl 7,065.18M instructions against 6,525–6,575M; ~20% fewer cycles; 36% against 44–45% | C | README:212, 172. 3,741/4,591 to 4,695 = 18.5–20.3% fewer |
| N77 | ntctl anomaly "likely P/E-core scheduling or viewport scaling" | CORR | Viewport scaling: yes (ntctl 1280×720, apps 848×476). P/E scheduling: refuted by the per-PMU shares (§0.6) |
| N78 | n=3; warm switch spans 33–88 ms within one build | C | README:59 |
| N79 | stability.py: 3 software-path passes per historical build, so it misses GPU stalls | C | README:95 |
| N80 | One Arrow Lake-P + NVIDIA Hyprland laptop | C | README:129; iced-power-preference.md:3 |
| N81 | Parity ±50%; 1440p/2160p still open; real-input checks still open | C | parity:113, 57; ledger:48–49 |
| N82 | Diagram 1: stack instructions per frame into ntctl ~80% + app residual ~20% | CORR | The 80/20 split holds only for CPU %. For instructions the residual is negative, so the bar can't be stacked (§0.6) |
| N83 | Diagram 3: the ladder lists 50/78/96 and 99/131/143 (iced/Rune/GPUI), then 217/253/274 and 267/289/337 (iced/GPUI/Rune) | M | The app order flips mid-ladder. Rune is 274 and 337, GPUI 253 and 289 |

### carbon.md

| # | Claim | V | Evidence |
|---|---|---|---|
| C1 | Versions; nt-core = visionOS client, gate, cache, ScrollAnimator, libmpv software + Wayland subsurface | C | coord:9–13, 39. The subsurface is in core/src/video_surface.rs; the cited coord lines predate it (coord:48–49 still calls it "the next step") |
| C2 | "Over twelve days"; Opus 5.5, GPT 6.2 Sol, Gemini 4 Neon | C | First commit 2978b56 at 2026-11-11 20:33, last at 2026-11-24 (git log); coord:17–19 |
| C3 | 720p30 playback CPU 41–42% (2caf0fa) → 9.0–10.0% (NOW); 32.72–35.24 M instr/s | C | `metrics` 2caf0fa 42.00/41.33/42.33, NOW 10.00/10.17/9.00 (same AC table, default paths); README:197,203,209 |
| C4 | Wheel 1,013 / 1,540 / 3,021 M; cold 99 / 143 / 131; binary 29.6 / 52.7 / 42.2 | C | README:168, 196–208, 50–52 |
| C5 | iced: smallest binary, fastest Home 209, lowest cold/warm/tour 99/153/267 | C | README:50, 168–178 |
| C6 | iced: "zero crashes across 43 project incidents" / "0 crashes and 0 stalls" | M | Same as N9 |
| C7 | iced pays a "1.57× UI velocity tax" | M | 44/28 = 1.571 ✓, but these are ±50% T-shirt-size estimates made 2026-11-18 (parity:3, 112–113). The ranges (22–66 h, 14–42 h) overlap, so the "tax" can't carry three significant figures |
| C8 | Missing ellipsis, blur, `unfocus`, floating popups → ~44 h ($1,220) | C | parity:117, 136–138 |
| C9 | GPUI at 2–3× the scroll CPU | C | Instructions: 1.96× iced, 2.98× Rune. Gate CPU %: 2.09× iced, 1.73× Rune |
| C10 | GPUI parity ~28 h (~$780) | C | parity:117 |
| C11 | GPUI burns 1,112.34 M instr/s on wheel scroll | C | README:202. Mechanism: see C61 |
| C12 | GPUI pulls 811 lockfile crates | C | 811 from 96df4ec to 2caf0fa; **812** in apps/gpui/Cargo.lock now |
| C13 | GPUI turns synchronous entity re-entrancy into runtime panics | C | gpui-reentrant-keystroke.md:10 |
| C14 | Rune has "the smallest codebase (2,508 lines)" (also in Recommendations) | CORR | 2,508 is the 96df4ec count. **NOW: Rune 10,631, GPUI 12,653, iced 13,518**. Rune is still the smallest |
| C15 | Rune has the lowest scroll cost, 373.52 M/s | C | README:208 |
| C16 | Rune: patched winit/wgpu-hal, libloading, leaked app IDs, LiveText workarounds | C | rune NOTES:4–7, 224–226, 290; rune-wrapped-livetext.md:26–30 |
| C17 | Overriding iced's HighPerformance default "halved bare-shell PSS (120 → 50 MB)" | M | It's more than half: −58% (2.4×), or 113→50 in the 8-run table (coord:71–72). And it's **two** knobs (power preference *and* Vulkan-only backend), on a bare shell, not the app |
| C18 | "Pinning Mesa EGL cut Rune renderer init 5× (505 → 99 ms)" | M | 505/99 = 5.1× ✓, but the only source is one coordinator ledger line (ledger:42) with no receipt or run count. The same change did almost nothing for GPUI (27→25). Don't use it as a headline without a re-measurement |
| C19 | Asymmetric prewarm "explains nearly all" of Rune's 189 ms against iced's 95 ms | M | The asymmetry is real (README:79). Rune's own measurement says surface prewarm "saved another ~60 ms" (apps/rune/src/video.rs:265–268), about two-thirds of the 94 ms gap. iced's surface A/B is ~98 ms (`isDk` 209.8 → `F70` 112.1). No Rune ablation exists, so "nearly all" is unproven |
| C20 | NOW 190/209, 238/252, 293/310 ms | C | README:50–52 |
| C21 | Bare iced 460 (339 in isolation) → 175 (199); GPUI 253, Rune 259; HighPerformance + GL probing | C | coord:71–74; iced-power-preference.md:30–38 |
| C22 | Open→frame 95 / 118 / 189 | C | README:50–52 |
| C23 | "All crushing YouTube in Helium (4170 ms cold, 3885 warm)" | M | Not like for like. The app figures use the local clip and replayed fixtures (README:22–23); Helium is a live web player. The comparable row is **core alone 451 / 218 ms against Helium 4170 / 3885** (coord:31–36) |
| C24 | iced and GPUI prewarm libmpv + surface; Rune only the Player | C | README:79 |
| C25 | `isDk`: iced 97.30 → 209.80 ms (fails +75), then 112.10 | C | `isDk`, `F70`; ledger:43–44. The 97.30 is that gate's 2caf0fa baseline, not iced's own pre-deferral value |
| C26 | Rune's 2caf0fa baseline skipped libmpv prewarm (273.20 → 277.30); it pays ~99 ms of on-demand EGL init | C | README:78; `isDk`; ledger:42; rune video.rs:265–268 (the surface attaches on the first load) |
| C27 | Warm switch 36 / 43 / 50 is "inside 3-run noise (33–88 ms)" | M | The NOW runs are tight: iced 32.5–37.0, GPUI 42.2–44.8, Rune 30.1–54.0 (`metrics`). iced and GPUI don't overlap. The 33–88 span belongs to a historical build (README:59) |
| C28 | Binaries 29.6 (431 crates), 42.2 (500), 52.7 (811) | CORR | NOW binaries paired with 96df4ec crate counts. NOW lockfiles: **480 / 490 / 812** |
| C29 | Cold 99/131/143; warm 153/169/188 | C | README:168–169 |
| C30 | ntctl 116 MiB; residuals 95 (55%), 129 (47%), 139 (45%) | C | `attr`: 95.28 / 128.68 / 138.74 |
| C31 | GPU playback 217/253/274; tour 267/289/337 | C | README:174, 178 |
| C32 | `warmed_retained_pss_mib` −68.45 (to 48.86) GPUI, −53.37 (to 77.75) Rune | C | `isDk` |
| C33 | Idle 6.02 / 5.97 / 5.98 M (0.60 M/s) | C | README:195,201,207 |
| C34 | Wheel 373.52 / 568.05 (1.52×) / 1,112.34 (2.98×); Home 173.43 / 178.11 / 252.05 | C | README:194–208; the ratios check out |
| C35 | Software 6,525.11–6,574.66 M (21.75–21.92 M/frame, 44–45% CPU, 80–82% core share) | C | README:172, 184–186, 198–210. CPU % and share come from the battery runs, the instructions from AC |
| C36 | GPU path cuts user instructions 18.5–20× … 1.09–1.18 M/frame, 9.5–10%, 1.54–1.56 J | C | Arithmetic in N3. GPU joules on the software path were 1.27–1.75, so the energy didn't drop (§0.8) |
| C37 | Residuals −539.9 / −515.9 / −487.6 M | C | `ctr` `attribution.app_minus_reference_instructions` |
| C38 | ntctl 3,741.26 M cycles against 4,591.19–4,695.03; 36% against 44–45% | C | README:212, 172 |
| C39 | ntctl renders full 1280×720 at 1.89 IPC; the Watch page scales smaller at ~1.41 IPC, "stalling memory bandwidth on ~3.7 MB/frame copies" | CORR | Render size and IPC ✓ (7,065/3,741 = 1.89; apps 1.40–1.42; §0.6). The frames are **~1.6 MB** (848×476), not 3.7 MB. "Stalling memory bandwidth" wasn't measured |
| C40 | iced wheel 48.12 J, 123.57 RCS, 14.27 VCS against 0.00 VCS and 19.29–20.22 J for the others | C | README:222, 228, 234 |
| C41 | iced: zero video frames; load 4.61 against 2.76 on GPUI | C | `counters/{iced,gpui}/wheel/1/sample.json` loadavg; report `video_frames_rendered: 0`. But iced's clean run 3 also had load 4.43, so load doesn't mark contamination |
| C42 | counters.py runs its 3 slices back to back per app | C | tools/media/counters.py:414–420 |
| C43 | "Concurrent desktop video activity polluted" iced's wheel rows | U | The contamination is supported (runs 1–2 against run 3, §0.7). Its source isn't recorded anywhere |
| C44 | stability.py: 100 "across all 30 assessable historical cells" | CORR | **30 runs in 10 cells** (README:95; stability.json: 2+2+3+3 cells × 3 runs) |
| C45 | The historical runs used software rendering | C | README:95 |
| C46 | 43 incidents: 14 / 7 / 7 / 6 / 6 / 3; orchestration = 5 caps + 1 author rewrite | C | `hic` |
| C47 | GPUI #10: a production crash from the double lease inside `defer_in` | C | `hic` #10; gpui-reentrant-keystroke.md:9–16 |
| C48 | Rune #12: wrapped LiveText `assert!` at flatten.rs:258 | C | rune-wrapped-livetext.md:20–26 |
| C49 | Rune #13 stall "3/5 timeouts"; #14 2.6 s black holes | C | ledger:11 says "3/5 waits timed out". **The evidence conflicts**: rune NOTES:239 says "2 of 5 and 1 of 5 waits timed out in two baseline runs". The 2.6 s figure is rune NOTES:243 |
| C50 | First builds $23.54 / 49, $27.99 / 55, $28.41 / 59 | C | coord:83–86 |
| C51 | Parity 28 / 40 / 44 h atop 36 h of core | C | parity:117–118 |
| C52 | "At review" Rune 2,508, GPUI 3,045 (+30), iced 3,762 | C | frontends.md:412; `git` at 96df4ec matches exactly. Stale: see §0.4 |
| C53 | GPUI heatmap in 20 lines, hole in ~50 | C | gpui NOTES:131–132 |
| C54 | iced: Clamp, Scroller virtualization, 1×1 badges for iced_wgpu's layer order | C | iced NOTES:129–134 |
| C55 | "Agents **filed** 7 upstream repros" | M | They are seven local reports in docs/upstream. Nothing shows any was filed, and iced-power-preference.md:48 says "Nothing to report for master" |
| C56 | iced (3): power preference and ellipsis fixed on master 98bd48fe; zero crate patches | C | iced-power-preference.md:44–49, 82–85; iced Cargo.toml |
| C57 | GPUI (3): paint_surface, axis_stop, private hide-cursor, dispatch panic | C | Four items under a count of 3 (paint_surface has no report); the facts check out: gpui NOTES:279–280 and the three reports |
| C58 | On macOS "GPUI initially showed no text until static Roboto weights were bundled" | U | coord:124–127 lists the static-Roboto fix (lost variable weights) and macOS "no text at all" as separate bullets. Nothing says the bundle fixed macOS |
| C59 | Rune 0.0.1 git, patches at Cargo.toml:24–26, `&'static`, libloading, wrap/truncate | C | apps/rune/Cargo.toml:22–26; rune NOTES:224–226, 290 |
| C60 | GPUI: "0.4–0.7 ms build + 2–3 ms Taffy every scroll frame, even with TopBar/Rail cached" | M | The ms figures are from the Nov-16 single-view build (frontends.md:160–166, G2), from before the split into five views. The pre-fix profile still puts Taffy on top, but ledger:38 credits the remaining wheel CPU to core poster decoding (see N62) |
| C61 | iced: zero `view()` passes between bands; autopilot Scroll messages force rebuilds that real wheel input skips | C | iced NOTES:61–75; frontends.md I4; app.rs:3506 still routes them through `update` |
| C62 | Rune: `Live<[f32;2]>` via `Tx::live_only()`, no view or layout between bands | C | rune NOTES:54–68 |
| C63 | The subsurface removed CPU RGBA buffers, per-frame uploads and 30 Hz redraws → 9.5–10% | C | yt-music-gpui-notes.md:16–34; iced NOTES "Review fixes" (`set_clip`). The per-frame size was ~1.6 MB here (§0.6) |
| C64 | iced's `&mut App` lets rustc forbid re-entrancy "(0 crashes)"; GPUI leases and Rune's flatten asserts are runtime panics | M | The design claim is fine. Tying it to the crash count isn't: Rune's crash was an assertion against an unsupported text-option combination, not re-entrancy, and 0 against 1 is too small a sample |
| C65 | RAM: "35–55 MiB jumps" from fonts (~34 MB), the 96 MiB cache, 11–46 MiB copies, dual EGL "(505 → 99 ms attach)" | M | Each part is sourced (iced NOTES "Fonts"; rune NOTES:304–306; gpui NOTES:111), but the "jumps" are never defined. 505→99 is renderer **init**, not attach (ledger:42). Rune's 46 MiB was its old bound; it's 18 MiB now (rune NOTES:306) |
| C66 | GPUI takes ~one-third less effort (28 vs 44 h) | C | 1 − 28/44 = 36% (estimates) |
| C67 | GPUI binaries ~53 MiB | C | 52.70 MiB |
| C68 | iced: override the HighPerformance defaults "until 0.15" | U | The fix is on master (98bd48fe); no 0.15 release appears in the evidence |
| C69 | Rune "worked immediately on macOS Metal" | C | coord:127 ("Rune was usable on macOS"); rune NOTES:8–9 |
| C70 | Universal rule: deny unwrap/expect/panic | C | `[lints.clippy]` in core and every app Cargo.toml; ledger:45 |
| C71 | Split power regimes; 39 battery runs; the earlier pass was ~50% slower | C | README:28–30 |
| C72 | Per-PMU medians don't sum (9.74 M + 87.88 M ≠ 173.43 M) | C | `ctr` results.iced.home; per-key `median_tree` (counters.py:397–400) |
| C73 | 1280×800 at scale 1; one Arrow Lake-P + NVIDIA laptop | C | README:24, 129 |
| C74 | HiDPI, 4K VP9/AV1, X11, Windows and macOS unmeasured | C | No data for any of them; parity:57 |
| C75 | Input batch (Shift+N, `?`, iced sleep timer) still queued | C | ledger:48–49; .cache/orchestrate/native-tube/input-batch.md:7, 16–17 |
| C76 | Outside YouTube's ToS | C | coord:207–208; README.md:55 |
| C77 | Diagram 1: `instructions_per_scroll_second` from counters.json = 373.5 / 568.1 / 1,112.3 | CORR | That key holds **374.73 / 569.72 / 1,117.05** (divided by `scenario_seconds`). The README's 373.52 / 568.05 / 1,112.34 divide by the CPU window. Pick one and say which |

### Counts

| | Confirmed | Corrected | Unsupported | Misleading | Total |
|---|---:|---:|---:|---:|---:|
| neon.md | 66 | 6 | 2 | 9 | 83 |
| carbon.md | 58 | 5 | 3 | 11 | 77 |
| **Both** | **124** | **11** | **5** | **20** | **160** |

## 2. Consensus: both takes agree and the evidence confirms

1. **Taking video out of the UI framework is the biggest single win.** On the GPU subsurface all
   three apps converge: 9.5–10% CPU (res), 1.09–1.18 M user instructions per frame against
   21.75–21.92 M in software (README:197–210), and 1.54–1.56 J GPU energy.
2. **Once video is out of the way, GPUI is the most expensive app to scroll.** In wheel scrolling
   it retires 3,020.93 M instructions against 1,540.33 (iced) and 1,013.41 (Rune), and its CPU is
   45.87% in the gate against 21.95% and 26.54%. Of all the scroll comparisons, this is the one
   that holds across every source.
3. **iced is the leanest at runtime.** It has the smallest binary (29.6 MiB), the lowest cold,
   warm, GPU-playback and tour PSS (99 / 153 / 217 / 267 MiB), and the fastest window, Home,
   open and warm switch at NOW (190 / 209 / 95 / 36 ms) (README:50, 168–178).
4. **Rune is the heaviest on the GPU path and in the tour** (274 and 337 MiB), despite having the
   smallest bare shell after iced's low-power one.
5. **GPUI had the cheapest parity estimate** (~28 h) against Rune (~40 h) and iced (~44 h), plus
   ~36 h of shared core work. These are estimates, ±50%. First builds cost about the same
   ($23.54–28.41, 49–59 min).
6. **Incidents:** 43, all fixed. Three crashes: core storyboard overflow, GPUI's re-entrant
   PlayerView and Rune's wrapped LiveText. Two stalls (a core dub switch and Rune's GPU switch).
   None are tagged to the iced frontend.
7. **Framework maturity.** iced 0.14 needs no patches but ships a HighPerformance GPU default and
   lacks ellipsis. GPUI on Linux needs a vendored axis_stop patch and has no public cursor-hide
   call. Rune is a pinned git dependency with crates.io patches, a libloading display lookup, a
   leaked `&'static str` app ID and a release assert on wrapped or truncated LiveText.
8. **The bare iced shell was the slowest out of the box** (460 ms, 113 MB, 107 wakeups/s) and the
   fastest with wgpu's two env knobs (175 ms, 50 MB, 2/s) (coord:71–72).
9. **The instruments have limits.** Two power regimes, system-wide GPU/RAPL counters (iced's wheel
   row is contaminated), n = 3 medians, a coarse stability.py (every cell 100), one
   machine at 1280×800.
10. **The ntctl reference retires more instructions** than the apps on the software path, with
    fewer cycles.

## 3. Where the takes disagree, and who is right

| Topic | neon.md | carbon.md | Verdict |
|---|---|---|---|
| Playback CPU baseline | 44–45% → 9.0–10.0% (NOW software vs GPU) | 41–42% (2caf0fa) → 9.0–10.0% (NOW) | **Both are real**, but they're different comparisons. Neon's is the clean A/B: the same build and clip, forced software against GPU, all on battery: 45/45/44 → 10/10/9.5, i.e. 4.3–4.7×. Carbon's spans milestones on the AC table (42.0/41.3/42.3 → 10.0/10.2/9.0) and includes everything else that changed. Use Neon's pair and drop the 9.0, which comes from the other table. NOW-software (battery) and 2caf0fa (AC) can't be compared with each other |
| Crate counts | 319 / 434 / 640 | 431 / 500 / 811 | **Neither is the NOW app.** Neon's are the bare shells; Carbon's are first delivery (96df4ec). NOW app lockfiles: **480 / 490 / 812** |
| Lines of code | "~2,500 initial" (labelled) | "smallest codebase (2,508)" | **Neon** labels it correctly. NOW: 10,631 / 12,653 / 13,518 (Rune / GPUI / iced) |
| Rune's 189 ms open→frame | Lists it, with no cause | Prewarm asymmetry "explains nearly all" | **Partly Carbon.** The asymmetry is documented, but Rune's own figure for surface prewarm is ~60 ms of the 94 ms gap |
| ntctl instruction anomaly | "P/E-core scheduling or viewport scaling" | Full 1280×720 in ntctl against the smaller Watch box | **Carbon**, on render size (proved by the code). The per-PMU data refutes P/E scheduling. Carbon's "3.7 MB" and "memory stalls" parts are wrong or unmeasured |
| Wheel CPU agreement | The gate "agrees" with the counters | Makes no gate claim | **Neon is wrong.** The gate has iced below Rune; the counters have Rune below iced |
| iced's wheel GPU spike | Unverified until a clean re-run | Polluted by desktop video | **Both half right.** Run 3 is clean (38.5 Mcycles, 0.46 VCS, 23.2 J), which supports contamination, but its source isn't known |
| Rune's GPU-switch stall | n/a | "3/5 timeouts" | **The evidence conflicts:** the ledger says 3/5, Rune's NOTES say 2 of 5 and 1 of 5 in two runs |
| Rune LiveText assert line | flatten.rs:259 | flatten.rs:258 | **Carbon** (258 at the pinned rev; 259 is upstream HEAD) |
| GPUI's scroll cost | Tree rebuilds | Rebuilds + Taffy at 0.4–0.7 + 2–3 ms | **Partly both.** Taffy leads the profile, but the GPUI agent's last word (ledger:38) is that what remains is core poster decoding. Neither has a NOW profile |

## 4. Safe to publish

Each line gives the number and its source. Use these phrasings.

1. With the video on a Wayland subsurface, playback CPU fell from 45% / 45% / 44% to 10% / 10% /
   9.5% of one core (iced / GPUI / Rune). That's the same NOW build and the same 720p30 clip, on
   battery (README:172, 176; `res`).
2. The same switch cut user-space instructions per frame from about 21.8 M to 1.09–1.18 M, about
   19× (README:197–210, AC).
3. In software mode, the player alone (`ntctl play`) accounts for about 80% of each app's CPU
   (36 of 44–45%) and 45–55% of its memory (116 of 211–255 MiB) (README:184–186).
4. On the 27-tick wheel story, GPUI retired 3,020.93 M instructions, iced 1,540.33 M and Rune
   1,013.41 M (README:196–208, medians of 3, AC). Per second of scrolling: 1,112 / 568 / 374 M.
5. In the perf gate, GPUI's wheel-scroll CPU was 45.87% of one core, about twice iced's (21.95%)
   and Rune's (26.54%) (`isDk`). Don't rank iced against Rune: the two instruments disagree.
6. At idle all three are event-driven: about 6.0 M instructions per 10 s (README:195, 201, 207).
7. At NOW, iced was fastest to a window (190 ms), to Home (209 ms), to the first video frame
   (95 ms) and on a related-video switch (36 ms). Rune: 238 / 252 / 189 / 50. GPUI: 293 / 310 /
   118 / 43 (README:50–52, medians of 3).
8. Home memory with the player prewarmed: iced 153, Rune 169, GPUI 188 MiB. Without the prewarm:
   99 / 131 / 143 MiB (README:168–169).
9. Peak memory over the scripted tour: iced 267, GPUI 289, Rune 337 MiB (README:178).
10. Stripped binaries: iced 29.6, Rune 42.2, GPUI 52.7 MiB (`metrics` stripped_bytes). Lockfile
    packages: 480 / 490 / 812 (apps/*/Cargo.lock).
11. App source today: Rune 10,631 lines, GPUI 12,653, iced 13,518 (`wc -l`). As first delivered:
    2,508 / 3,045 / 3,762.
12. A bare iced 0.14 window on this hybrid laptop mapped in 460 ms, used 113 MiB and woke 107
    times a second. With `WGPU_POWER_PREF=low` and `WGPU_BACKEND=vulkan` it took 175 ms, 50 MiB
    and 2 wakeups (coord:71–72, 8 runs). The cause is `iced_wgpu` 0.14 defaulting to
    `HighPerformance` and probing every backend (iced-power-preference.md:35–38).
13. Bare shells: GPUI 253 ms / 96 MiB / 32 threads; Rune 259 ms / 78 MiB / 11 threads (coord:73–74).
14. The project catalogued 43 incidents, all fixed: 3 crashes (core, GPUI, Rune) and 2 stalls
    (core, Rune). None was in iced's own code (`hic`).
15. GPUI's crash: dispatching a synthetic keystroke from `cx.defer_in` panicked at
    `entity_map.rs:192` ("already being updated"). The fix was `Window::defer`
    (gpui-reentrant-keystroke.md:10; coord:118–123).
16. Rune's crash: `text().wrap().live_text()` trips an assert at `flatten.rs:258` in release
    builds (rune-wrapped-livetext.md:20).
17. Estimated UI work to YouTube parity, ±50%: GPUI ~28 h, Rune ~40 h, iced ~44 h, plus ~36 h of
    shared core work (parity:112–118).
18. First full builds: iced $23.54 in 49 min, Rune $27.99 in 55 min, GPUI $28.41 in 59 min
    (coord:83–86).
19. GPUI drew the heatmap in about 20 lines and the transparent hole in about 50 (gpui NOTES:131–132).
    iced 0.14 needed a hand-written binary-search ellipsis (`Clamp`) and 1×1-image badges to work
    around iced_wgpu's layer order (iced NOTES:129–134).
20. GPUI needs a ~30-line vendored patch so touchpad flings start on Wayland
    (gpui-axis-stop.md; apps/gpui/Cargo.toml:37). Rune requires its own `[patch.crates-io]` for
    winit and wgpu-hal (rune NOTES:5–7). iced needs no patch.
21. Forcing libmpv onto VA-API interop cut GPU attach from 1,129 to 79 ms (ledger:13; one
    coordinator measurement).
22. Prewarming iced's GPU surface after Home brought click-to-first-frame from 209.8 ms back to
    112.1 ms, and all 14 gate budgets passed (`isDk`, `F70`).
23. Over 60 live opens Rune's memory levelled off at 453–474 MiB. That was its caches and the
    40-page back stack filling up, not a leak (rune NOTES:296–308).
24. Every stability run (`stability.py`, 20 related-video switches each) was clean across all
    builds, and the NOW frame checks found 0 black, 0 stale and 0 frozen frames in 9 per app
    (README:91–93, 246–248). It's a coarse test, and on the historical builds it ran the
    software path only.
25. The software-path apps retire *fewer* instructions than the player alone (6.53–6.57 G against
    7.07 G) because they render the video at the Watch player size (≈848×476), while `ntctl`
    renders the full 1280×720 (core/src/player.rs:806; ntctl.log:1). They still spend more cycles
    (4.59–4.70 G against 3.74 G).

Don't publish without a re-measurement: renderer init 505→99 ms (ledger:42 only); any iced wheel
GPU or energy figure; any energy claim for the subsurface; the "1.57×" and dollar figures as
anything better than estimates; the cause of Rune's 189 ms open.

## 5. Diagram ideas, ranked by how well the data supports them

| Rank | Idea | Data file | Support |
|---:|---|---|---|
| 1 | **carbon #1**: video converges, scroll diverges (instructions per frame, software → GPU, beside wheel instructions per second) | `ctr` (`normalization.instructions_per_rendered_video_frame`, `instructions_per_scroll_second`) | Strong. User-space CPU counters, n=3, AC, tight. Use either the counters.json key values (374.7 / 569.7 / 1,117.1) or the README's (373.5 / 568.1 / 1,112.3), and label which |
| 2 | **neon #1**: software vs GPU playback, paired bars | CPU %: `res` + `attr`; instructions per frame: `ctr` | Strong, with one fix: stack ntctl + residual **for CPU % only** (36 + 8–9). For instructions, show ntctl as a reference line, because the residual is negative. Don't mix the battery CPU % and AC instructions on one axis |
| 3 | **carbon #3**: lifecycle memory, stacked bars | bench/results/shells.json, `res`, `attr` | Good. The app order (iced/Rune/GPUI) is consistent. Label the shells as bare (no core), and note the shells and apps came from different tools and power states |
| 4 | **neon #3**: memory escalation ladder | same, plus iced-power-preference.md | Good data, but fix the app order (the GPU-playback and tour numbers are in iced/GPUI/Rune order), and the 113 vs 120 MB default-shell figure comes from two different runs |
| 5 | **neon #4 / carbon #5**: incidents by area × kind, beside the flat stability strip | `hic`, .cache/media/out/stability/stability.json | Data exact (43 incidents; kind counts 18/7/5/4/3/3/2/1). Small counts. Note that the catalog records what was written down (README:99), and that core incidents affect all three apps |
| 6 | **neon #5**: before → after dumbbells for defaults | iced-power-preference.md, ledger:13, 42, `isDk`, `F70` | Mixed. The iced shell (3-run doc) and the prewarm gate are measured. VA-API 1,129→79 and EGL init are single ledger lines with no receipts. The units and scales differ (ms, MB, wakeups/s), so use a log axis or separate panels |
| 7 | **carbon #2**: prewarm latency–memory frontier | `isDk`, `F70`, `metrics` | Values check out (126.3/209.8, 162.1/112.1, 153/95, 186/118, 167/189), but the gate's idle PSS and the collector's idle PSS are different harnesses. Plot them as two series or not at all. Five points |
| 8 | **neon #2**: parity hours vs wheel instructions, bubbles for tour PSS and binary | parity, README | Weak. The x-axis is a ±50% estimate, and four encodings on three points invite a correlation the data can't carry |
| 9 | **carbon #4**: velocity vs scroll CPU quadrant, labelled with LOC | parity, frontends.md, `ctr` | Weakest. Same estimate problem, and the LOC labels (3,045 / 2,508 / 3,762) are from first delivery. NOW: 12,653 / 10,631 / 13,518 |
