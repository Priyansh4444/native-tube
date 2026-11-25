# Fact-check of docs/blog/x-article.md

This review covers the November 24 draft. The current article uses the later
[matched results and laptop confirmation](../media/fair.md) and the
[three real-time clips](x-clips.json).

Checked 2026-11-24 against the sources named in the task: `docs/blog/takes/review.md` (binding),
`docs/media/fly-bench.md` and `.cache/fly-bench/out/summary.json`, `docs/media/README.md` and
`.cache/media/out/**`, `docs/blog/coordinator-notes.md`, `bench/results/playback.json`,
`.cache/perf/out/compare-{isDkA304,F70v7wbg}/summary.md`, `apps/*/NOTES.md`, `docs/upstream/*.md`,
`.cache/media/out/hiccups.json`, `tools/fly-bench/bench.py`, `bench/cdp.mjs` and the code. Read-only:
nothing was built or launched and there was no GUI input. Stills were viewed directly; every video
was sampled with ffmpeg into contact sheets (6–9 frames each).

**Counts:** about 130 claims checked (numbers, units, ratios, mechanisms, captions, privacy). 62
edits: 41 corrections, 13 unsupported claims removed or replaced (rows 1, 16, 22, 23, 30, 43, 45,
46, 47, 50, 55, 57, 60), and 8 rows that swap 21 values for `<!--PENDING-NORMALIZE-->` on the
owner's instruction (CPU "% of one core" and warm numbers). `docs/media/normalized.md` did not
exist at 22:08.

## 1. Changes (before → after)

| # | Where | Before | After | Reason and source |
|---|---|---|---|---|
| 1 | §1 | "every difference … comes from the UI framework" | "most of the difference … plus prewarm policy and thumbnail decoding differ" | Prewarm differs by app (README:79); each app decodes a different number of thumbnails (review N62) |
| 2 | §1 SideBySide caption | "the exact same scripted YouTube tour" | laptop tour replaying fixtures and a local clip | Frames show laptop overlay "local fixture clip in native apps" and an empty YouTube tile |
| 3 | §2 | "44–45% of a CPU core … (iced 45%, GPUI 45%, Rune 44%)" | `<!--PENDING-NORMALIZE-->` | Owner: every % of one core pending normalization |
| 4 | §2 | "from 45 / 45 / 44% down to 10 / 10 / 9.5%" | ratios kept (4.3× / 4.5× / 4.7×), values pending | Ratios are unit-free (README:172,176; review N2) |
| 5 | §2 | "converged to within a rounding error" | "within about 8% … in instructions" | 352.92 / 327.54 = 1.077 (README:197,203,209) |
| 6 | §2 InstructionsStill caption | "look almost identical" | "within about 8%", plus a note that the still's wheel column (570 / 1,117 / 375) is per scenario second, not the README's 568 / 1,112 / 374 | Review C77: two normalizations exist; the still uses counters.json's |
| 7 | §2 caveat 2 | "292–294% CPU (171–176 billion instructions) vs 91–159%" | CPU pending; instructions labelled "whole test run", compared with software 58–103 B | fly-bench.md:18 ("`perf stat` measured each complete scenario"); summary.json `perf.*.default/software` |
| 8 | §3 | Warm switch 36 / 43 / 50 ms | pending | Owner: warm numbers pending (README:50–52; values correct as measured) |
| 9 | §3 FirstFrame caption | "Latency breakdown from launch to Home to first video frame" | "Open → first frame: laptop GPU and Fly CPU rendering, beside YouTube web's cold first content frame on Fly (full page load, live network, not like for like)" | Re-rendered video (38.5 s, 18:38) shows that, plus a stages panel; no launch→Home timing |
| 10 | §3 FirstSecondReplay caption | "first second after launch, iced mapping and painting Home first" | "first second after opening a video: iced and GPUI show their first frame before Rune" | Video title "First second after open … aligned to each open event"; at +0.37 s iced and GPUI show bars, Rune still "OPEN SENT" |
| 11 | §3 Fly | "mapped in 142 ms and hit first frame in 191 ms" | "window map / open → first frame" | `open_ms` is open command → frame, not launch → frame (summary.json; fly-bench.md:9) |
| 12 | §3 Fly | Warm switches 161–194 ms | pending | Owner instruction (fly-bench.md:13–15) |
| 13 | §3 Chromium | "burning 123.8% CPU" | pending | Raw % of one core above 100% |
| 14 | §3 Chromium | "Warm … timed out past 60 s … 96.7 s median (69.3, 96.7, 240 s timeout)" | "measured mostly unskipped pre-roll ads: an ad's first frame in about 1 s, but the video took `<!--PENDING-NORMALIZE-->` … That number is ad time, not player speed" | summary.json: every warm run `"ad": true`; `any_ms` 968.6–1,320.1; measurement runs don't press Skip (bench.py `web_open` default `skip_visible_ads=False`; fly-bench.md:45); decisions.txt:5 "preroll-censored" |
| 15 | §3 | (no Fly side-by-side) | added `[MEDIA: .cache/viz/out/SideBySideFly.mp4 …]` | Brief lists it; output name from tools/viz/tours.py:235. PENDING |
| 16 | §3 | "For a live-network apples-to-apples check" | "A closer live-network check … still compares a bare player with a whole web app" | coord:188–189; review C23 |
| 17 | §3 | Helium "4,171 ms" | 4,170 ms | bench/results/playback.json web cold median 4,170.45 (fly-bench.md:39 rounds up to 4,171; coord:36 has 4170) |
| 18 | §3 | nt-core "141 MB PSS" | "about 141 MiB" | playback.py:85 divides kB by 1024 |
| 19 | §3 | Helium "978 MiB PSS" | "978 MiB PSS on the cold runs" | playback.json cold median 978.0; warm runs 1,023.5. (coord:36's "987 MB" is a typo) |
| 20 | §3 | "(with ad blocking)" | "(with ad blocking, so no pre-roll, a best case for YouTube)" | playback.json `notes.web` |
| 21 | §3 | nt-core 218 ms warm; Helium 3,885 ms warm; 63.8% CPU | pending | Owner instruction |
| 22 | §3 prewarm | "Mostly prewarming policy" | "a likely part … we haven't run the Rune experiment" | Review C19 and the "don't publish … the cause of Rune's 189 ms" list |
| 23 | §3 prewarm | "buy ~100 ms … for ~40–55 MiB" | split: Cold→Warm Home is the player cost; the gate's surface A/B is ~36 MiB idle (126.32 → 162.14) for ~98 ms (209.8 → 112.1) | Review N70 (the two numbers measure different things); isDk and F70 iced rows |
| 24 | §4 | "~162 frames at 60 fps) on both machines … leaderboard invert" | "~162 frames … on the laptop … leaderboard change" | Frames counted only on the laptop (review §0.7); Fly ran the same 27-tick story (stories.py, bench.py `scenario`) |
| 25 | §4 | Gate 45.87 / 21.95 / 26.54%, "about twice iced and Rune", "consistently 2–3×" | CPU pending; "about 2.1× iced's and 1.7× Rune's"; "about 1.7–3×" | 45.87/21.95 = 2.09, 45.87/26.54 = 1.73 (isDk); instructions 1.96× / 2.98× |
| 26 | §4 | GPUI "decodes thumbnails on the CPU" as a framework cost | "Not all of the gap is the framework: … nt-core decoding poster thumbnails, a different number per app" | Review N62, C60; ledger:38 |
| 27 | §4 | GPU counters unlabelled | "(system-wide, so they include anything else the GPU was doing)" | README:129 |
| 28 | §4 | iced "polluted by concurrent background activity" | "other system activity, source unrecorded" | Review C43 |
| 29 | §4 Fly | 134.7 / 196.1 / 280.0% CPU | pending; instructions labelled "whole run including launch and Home" | Raw >100%; fly-bench.md:18 |
| 30 | §4 Fly | "that explains the surprise"; "order flipped completely"; "more than twice … nearly twice" | "our best explanation … we didn't measure the mechanism"; "flipped: iced cheapest, Rune most expensive"; 2.1× and 1.9×, same order after subtracting each idle-only run | Review N16 (mechanism not measured). 65.19/31.04 = 2.10, 65.19/34.50 = 1.89; wheel − cold: 27.86 / 30.66 / 60.88 B |
| 31 | §5 | "0.00% CPU in the final gates" | "no measurable idle CPU" | Avoids a raw % figure; F70 iced 0.00, isDk GPUI/Rune 0.00 (review N32) |
| 32 | §5 Memory caption | "from bare shell to Cold Home … Rune overtaking GPUI once video and browsing caches warm up" | "from Cold Home … Rune overtaking GPUI by the tour peak" | Current video has no bare-shell stage, and its playback stage (software) shows GPUI 255 > Rune 244. PENDING re-render |
| 33 | §5 | ntctl "36% CPU" | pending | Owner instruction |
| 34 | §5 | "the other half is the framework" | "the frontend—the framework plus each app's own code" | README:115 (residual includes frontend and integration costs) |
| 35 | §5 | "~160 MiB of mapped libraries filling up" | caches and back stack fill up "on top of ~160 MiB of mapped libraries" | rune NOTES:300–308 |
| 36 | §5 | Fly native peaks "stayed under half of Chromium's peak" | kept, labelled as different workloads (fixture run with playback and switch vs one cold live page load) | 485 < 507 holds; workloads differ (bench.py; fly-bench.md:33–35) |
| 37 | §6 | "100 (3/3) across all 30 assessable historical runs" | "in every cell—30 runs across 10 build × app cells" | Review C44 |
| 38 | §6 Hiccups caption | "crashes and stalls hit core, GPUI, and Rune, while iced's own code logged zero" | "3 crashes (core, GPUI, Rune) and 2 stalls (core, Rune); none in iced's own code, though core's crash hit every app" | hiccups.json tally; review N9 |
| 39 | §6 | iced focus: "shrinking its width to zero … omit zero-width children before positional diffing, shifting widget IDs" | "the icon slot changed width from zero; `row!` skips zero-width children, so the input's position shifted and iced's positional tree diff gave it fresh state" | iced-focus-sibling-width.md:3–6, 30–36 |
| 40 | §6 | "its own code logged zero crashes … though remember nt-core's crash" | adds: only three crashes in the whole catalog; the catalog holds only what got written down | Review N9, C64; README:99 |
| 41 | §6 | Rune "stalls and 2.6 s black holes when incoming GPU frames didn't wake Rune" | stalls = frames didn't wake Rune; black hole = hole opened before mpv decoded a frame | rune NOTES:236–243 (two different bugs) |
| 42 | §6 | `select_egl_vendor()` "to pin Mesa EGL" | "to load only the compositor's EGL vendor (Mesa here)" | iced NOTES:303; EGLStill |
| 43 | §6 EGLStill caption | "pinning Vulkan low-power, VA-API interop, and a single EGL vendor eliminated huge startup and RAM penalties" | mechanism only (device → DRM driver → vendor) | Still shows the mechanism only, with footer "EGL timing awaits remeasurement"; review "don't publish" 505→99 |
| 44 | §6 | "GPUI showed no text … and GPUI lost variable font weights until we bundled static Roboto" | macOS text separated from the font-weight fix | Review C58 (nothing says the bundle fixed macOS) |
| 45 | §7 CostStill caption | "… while estimated effort to full YouTube parity spreads wide" | "$23.54–$28.41 in 49–59 minutes" | The still shows first builds only and says later work is outside it |
| 46 | §7 | "the frameworks split sharply (±50% estimates …)" | "the estimates spread out—but they are ±50% estimates, so the ranges overlap" | Review C7 (22–66 h vs 14–42 h overlap) |
| 47 | §7 | "Why did agents move fastest in GPUI?" | "Why does GPUI's estimate come out lowest?" | No speed measured; GPUI's first build was the slowest (59 min, coord:86) |
| 48 | §7 | "zero `#[allow]` or `#[expect]` overrides across the repo" | "in our own code; only the vendored GPUI patch keeps upstream's" | grep: 10 in apps/gpui/vendor/gpui-pre-linux; none elsewhere |
| 49 | §8 iced | "lowest memory … fastest startup (190 ms)" | labelled "on the laptop" / "to a window" | Fly numbers differ; avoid mixing |
| 50 | §8 GPUI | "where developer and agent velocity come first" | "where the lowest estimated UI effort matters most" | Velocity was estimated, not measured (#47) |
| 51 | §8 Rune | "posted the lowest laptop scroll CPU" | "retired the fewest CPU instructions to scroll on the laptop" | Gate CPU ranks iced below Rune (review N11, N37: scroll order differs by measure) |
| 52 | §9 | "19–20% fewer cycles" | "18–20%" | 1 − 3,741.26/4,591.19 = 18.5%; 1 − 3,741.26/4,695.03 = 20.3% |
| 53 | §9 | ntctl "36% vs 44–45%" | pending | Owner instruction |
| 54 | §9 | "the apps spend ~850–950 M extra cycles copying, uploading, and presenting" | "most likely … (an inference—no stall counters were collected)" | Review §0.6 |
| 55 | §9 PerCoreStill caption | "scheduling shares match across apps and ntctl, proving render resolution drives the gap" | "Per-PMU split of GPU-subsurface playback instructions … similar spread" | Still shows GPU playback, three apps, no ntctl, no shares |
| 56 | §9 | iced wheel "external contamination" | "other system activity, source unrecorded" | Review C43 |
| 57 | §9 GPUStill caption | "… while GPUI cuts clean scroll render work roughly in half vs. Rune" | playback only | Still has no wheel data |
| 58 | §9 | llvmpipe "(292–294% CPU, 171.5–176.4 B instructions)" | CPU pending; instructions "over the whole default run" | fly-bench.md:18 |
| 59 | §9 GPUI | "scrolling still pays Taffy layout plus core poster decoding" | "… and the GPUI agent traced the remaining wheel CPU to core poster decoding" | ledger:38 via review N62 |
| 60 | §9 SizesStill caption | "… and Rust source lines" | binaries and lockfile packages only | The still has no line counts |
| 61 | §9 methods | "median of n = 3 round-robin runs" | "round-robin for timings; back to back per app for counters", plus Fly, 8-run shells, ±50% parity, single VA-API measurement | counters.py:414–420 (review §0.3); README:28 |
| 62 | Sources | five broad entries | 14 entries mapping each number family to its file (adds playback.json, summary.json, perf compares, NOTES, code paths) | Task 4 |

Checked and left unchanged (sample): versions and rev (coord:11–13); twelve days (review C2);
private web API, logged out, visionOS client, outside YouTube's ToS (coord:207–208); 6.55/6.53/6.57 B
and ~21.8 M/frame; 18.5×–20.1×; 0.33–0.35 B and 1.09–1.18 M/frame; RAPL 70–84 J idle vs 62–96 J and
GPU 1.27–1.75 vs 1.54–1.56 J (no energy claim); laptop 190/209/95, 238/252/189, 293/310/118; bare
shells 460/113/107 → 175/50/2, GPUI 253/96, Rune 259/78 (MiB per review §0.5; iced sets both
`WGPU_*` vars in apps/iced/src/main.rs:49); Fly 142/229/264 map and 191/189/300 open; Chromium
1,857 ms at 854×480, 865/1,014 MiB; 1,013/1,540/3,021 M wheel and 1.52×/2.98×; GPU wheel RCS
35.6 (28.4–46.7) vs 68.3 (67.8–77.4), iced run 3 38.5; Fly 31.0/34.5/65.2 B; idle 6.0 M per 10 s;
the PSS table (README:168–178); ntctl 116 MiB and 95/129/139 MiB residuals (55/47/45%); +57 / +70;
453–474 MiB; Fly peaks 422–485; 0/0/0 of 9; 43 incidents 14/7/7/6/9 (hiccups.json); entity_map.rs:192;
flatten.rs:258; ~38 MiB and ~50 MiB; 1,129 → 79 ms (single measurement, labelled); 1,145 fixtures;
first builds $23.54/49, $27.99/55, $28.41/59; parity 28/40/44 h, $780/$1,100/$1,220, 36 h/$1,000;
~20 and ~50 lines; ~30-line patch; 2,508/3,045/3,762 labelled "first delivery" beside NOW
10,631/12,653/13,518 (re-counted with `wc -l`); 480/490/812 and 319/434/640 (re-counted); 29.6/42.2/
52.7 MiB; ntctl 11.0 MiB; per-PMU shares; 1.89 vs 1.40–1.42 IPC; 848×476 ≈ 1.6 MB; i915 playback
RCS/VCS/RC6; subsurface, `wp_viewporter`, `set_clip`, `place_below`, mpv options (code); 7 local
upstream reports, "not yet filed".

## 2. Media slots

| Slot | File | Status | Caption vs content |
|---|---|---|---|
| SideBySide.mp4 | exists (22.0 s) | OK | Fixed (#2). YouTube tile reads "No comparable browser recording" |
| FramePath.mp4 | exists | OK | Matches: 848×476 ≈ 1.6 MB, 6.55/6.53/6.57 B → 0.35/0.35/0.33 B, "about 19×" |
| InstructionsStill.png | exists | OK | Fixed (#6) |
| FirstFrame.mp4 | exists, re-rendered 18:38 (38.5 s vs 33.5 s in verification.json) | OK | Fixed (#9). Note it shows "YouTube warm pre-roll: 69 s" on screen, a warm number the normalization may change |
| FirstSecondReplay.mp4 | exists | OK | Fixed (#10) |
| SideBySideFly.mp4 | **missing** | **PENDING** (re-render in progress) | Caption must be checked against the render |
| ScrollCost.mp4 | exists, unchanged since 18:04 | **PENDING** (re-render in progress) | Current render is laptop only; the caption's "on a software-rendered VM, the order flips" isn't shown yet. Recheck after the re-render |
| cpu-instructions.png | exists | OK | Matches |
| Memory.mp4 | exists, unchanged since 18:06 | **PENDING** (re-render in progress) | Fixed (#32); recheck after the re-render |
| attribution-NOW-settled_pss_mib.png | exists | OK | Matches (116 MiB reference; 45–55%) |
| Hiccups.mp4 | exists | OK | Fixed (#38); video shows 43 found / 43 fixed, 7/6/7/14/9 |
| EGLStill.png | exists | OK | Fixed (#43) |
| CostStill.png | exists | OK | Fixed (#45) |
| PerCoreStill.png | exists | OK | Fixed (#55) |
| GPUStill.png | exists | OK | Fixed (#57) |
| SizesStill.png | exists | OK | Fixed (#60) |
| UpstreamStill.png | exists | OK | Caption fine. The still's title "What went upstream" implies filing; the reports are local only |

## 3. Spots marked `<!--PENDING-NORMALIZE-->` (21)

Raw values, for whoever fills them from `docs/media/normalized.md`:

| # | Section | Raw value now removed | Kind |
|---|---|---|---|
| 1 | §2 | software playback 45 / 45 / 44% of one core (laptop, battery) | CPU |
| 2 | §2 | before/after 45/45/44 → 10/10/9.5% (ratios 4.3–4.7× kept in text) | CPU |
| 3–4 | §2 caveat 2 | Fly EGL path 291.8–294.0%; Fly forced software 90.7–159.0% | CPU >100% |
| 5–7 | §3 | laptop warm switch iced 36, GPUI 43, Rune 50 ms | warm |
| 8 | §3 | Fly warm switch 161–194 ms | warm |
| 9 | §3 | Chromium cold 123.8% | CPU >100% |
| 10 | §3 | Chromium warm 96.7 s median (69.3 s, 96.7 s, 240 s timeout; ads included) | warm |
| 11 | §3 | nt-core warm 218 ms (live, laptop) | warm |
| 12 | §3 | Helium warm 3,885 ms | warm |
| 13 | §3 | Helium cold 63.8% | CPU |
| 14 | §4 | gate wheel CPU GPUI 45.87% (iced 21.95, Rune 26.54; ratios kept) | CPU |
| 15–17 | §4 | Fly wheel 134.7 / 196.1 / 280.0% | CPU >100% |
| 18 | §5 | ntctl play 36% | CPU |
| 19–20 | §9 | ntctl 36% vs apps 44–45% | CPU |
| 21 | §9 | Fly llvmpipe 292–294% | CPU >100% |

Warm-labelled values kept on purpose: "Warm Home" PSS (153 / 188 / 169 MiB and the +54/+45/+38
deltas). It is a memory figure with a documented definition (libmpv mapped throughout the idle
window, README:110), not a warm timing. If normalized.md redefines "warm", revisit it.

Raw "% of one core" still visible in media (not text): SideBySide and FirstSecondReplay overlays
("Sampled CPU", up to 174% in FirstSecondReplay, footnoted "CPU uses one-core units" in small
type) and ScrollCost's gate panel (21.95 / 45.87 / 26.54%, labelled "Percent of one CPU core").

## 4. Policy and privacy

- Owner appears only as @Priyansh4444; no email or real name (grep). The one other personal name is
  the prior-art author (from coord:210), credited for his public project.
- The article never claims ToS compliance and says plainly it calls YouTube's private web API,
  logged out, outside YouTube's Terms of Service (§1).
- No energy-saving claim; parity hours carry ±50%; the "iced zero crashes" line carries the review's
  caveats; Rune's 2,508 is labelled "first delivery" next to the NOW 10,631; the scroll order is
  stated as measure-dependent; the YouTube warm switch is now described as pre-roll ad time.

## 5. Ready to post?

**Not yet. Ready once the placeholders and pending media are filled; the prose and remaining
numbers are sound.**

Blocking:
1. 21 `<!--PENDING-NORMALIZE-->` placeholders. They are HTML comments, so posting now would leave
   visibly empty bold spots and broken sentences in §2–§5 and §9.
2. SideBySideFly.mp4 doesn't exist yet; ScrollCost.mp4 and Memory.mp4 are still the old renders.
   Check all three captions against the new renders (ScrollCost's caption depends on the Fly panel).

Remaining risks (not blocking):
- Length: about 4,900 words with the appendix, well above the brief's 1,800–2,800. The appendix
  could move to a thread or a link.
- FirstFrame.mp4 plots native open → frame (fixture replay, in-app open) and YouTube web's cold
  first frame (full page load, live network) on one axis. The caption says so; a reader may still
  read it as a race.
- "iced won every latency metric" includes the pending warm switch; recheck once it's normalized.
- Media overlays show raw one-core CPU (up to 174%) in small type (§3 above).
- UpstreamStill's title "What went upstream" suggests the reports were filed; they're local.
- Fly instruction counts are per whole run, not per wheel phase or playback phase; the text says
  so, but per-phase numbers would be cleaner if normalized.md provides them.
- Several figures rest on one source with no receipt: VA-API 1,129 → 79 ms (labelled single
  measurement), the GPUI agent's poster-decoding attribution (ledger:38), and the 14 h video's
  16.6 s → 0.7 s (coord:41–42).

## 6. Final verification (after Neon's final pass and the re-render)

Checked 2026-11-24 against `docs/media/normalized.md`, `.cache/media/out/normalized.json`,
`docs/media/fly-bench.md`, `docs/media/README.md`, the per-run CSVs in
`.cache/media/out/counters/*/`, and `.cache/viz/out/sources.json`. Read only: no builds, no launches,
no GUI input.

### Neon's numbers (all match)

- **Laptop playback:** software 0.441–0.452 cores (2.76–2.83%, shown as 2.8%, battery), GPU
  0.095–0.105 cores (0.59–0.65%). Ratios 4.3× / 4.5× / 4.7× (unrounded 4.31 / 4.49 / 4.67).
  652.49–657.45 M instr/video-s and 21.8 M/frame on AC; 53.88–54.29 instr/frame-pixel at 848×476
  vs `ntctl play` 25.51 at 1280×720; GPU 32.75–35.29 M/s and 1.09–1.18 M/frame; 18.5×–20.1×.
- **No-GPU finding:** Fly EGL 2.918–2.940 cores (72.96–73.50% of 4 vCPUs) vs software 0.907–1.590
  (22.67–39.75%); whole-scenario instructions 171.46–176.42 B vs 57.64–103.11 B, with the note that
  Fly per-unit rates are unavailable. Laptop about 0.1 vs about 0.45 in §8. Laptop GPU and Fly
  software rendering are labelled everywhere they appear.
- **Warm switch:** laptop 35.5 / 43.1 / 50.5 ms and Fly 161.4 / 193.8 / 182.4 ms are the medians
  of the per-run values; the article says they switch an already-running player on the local clip
  and that prepared-stream click-to-frame wasn't measured.
- **YouTube rerun:** 0.613 / 0.717 / 0.735 s first frames; 15.371 s ad, 16.064 s wall, 0.693 s
  excluding ad; Warm 1 a >130 s timeout behind an ad; cold 2.234–3.297 s; "no ads skipped or
  blocked". The Skip click only appears in the earlier Fly tour recording, now noted in the
  SideBySideFly caption.
- **Scroll and other CPU:** laptop wheel 0.266 / 0.444 / 0.211 cores (1.66 / 2.77 / 1.32%, from
  the AC counter runs) and 569.72 / 1,117.05 / 374.73 M/s, 9.45 / 18.65 / 6.22 M/frame; Fly wheel
  1.347 / 1.961 / 2.800 cores (33.69 / 49.04 / 70.01%); `ntctl` 0.362 cores / 2.26%; Chromium cold
  1.238 cores / 30.94%, warm 0.784 / 19.59%; Helium 0.638 / 3.99%; idle ≤0.019%.
- **Prior fixes:** none of rows 1–62 were reverted. Row 36 had drifted: Neon's "fixture tour" for
  the Fly peaks is the default playback scenario (fly-bench.md, the Memory render). Fixed below.

### Changes (10)

| # | Where | Before → after | Reason |
|---|---|---|---|
| F1 | §2 FramePath caption | "removes 30 fps frame uploads and UI redraws" → copy/upload skipped, about 19× fewer instructions, plus the no-GPU coda (2.92–2.94 vs 0.91–1.59 cores on Fly) | The final render ends with a "What if there's no GPU?" coda the caption left out |
| F2 | §3 | "Trial 1 timed out after an ad" → "behind an ad" | normalized.md: no observed ad exit; matches FirstFrame's on-screen wording |
| F3 | §3 SideBySideFly caption | adds fixtures + local clip vs live YouTube, and that this recording's browser clicked Skip on one ad while the rerun skipped nothing | The render's footer says "Browser clicked Skip on one ad in this recording"; the tile shows "WARM PRE-ROLL" |
| F4 | §4 ScrollCost caption | adds "(wheel CPU, a different measure)" and that the on-screen redraw line is our best explanation | The final act sets laptop instructions/s beside Fly cores and says "how much each framework redraws per frame dominates"; §4 says the mechanism wasn't measured |
| F5 | §5 Memory caption | adds "Laptop", "software Playback", and the Fly coda (865 settled / 1,014 peak) | The render's Playback stage is software (GPUI 255 > Rune 244) and it ends on a Fly VM panel |
| F6 | §5 | "fixture tour" → "default fixture playback scenario" | fly-bench.md "Peak PSS covers the default scenario"; Memory render "App peaks come from the default playback scenario" (restores row 36) |
| F7 | §9 | "Per-PMU shares" → "Per-PMU scheduled-time shares" | P 23–30 / E 53–54 / LP 16–22% are the `perf` running-time percentages in `cpu.csv`; instruction-count shares are P 37–40% for `ntctl` and 35–41% for the apps |
| F8 | §9 | removed the PerCoreStill slot | Stale: the PNG (17:48) shows independent per-PMU medians (GPUI 149 + 177 + 35 = 361 M against a 353 M total; Rune 148/145/34) while sources.json now lists the repaired partition (GPUI 126/177/51, Rune 180/120/27), which §9 itself describes. It also showed GPU playback under a paragraph about software playback |
| F9 | §9 EGLStill caption | "so EGL stops probing the other GPU's driver" → "aimed at skipping the other GPU's EGL vendor" | The still says "This targets unnecessary vendor probing" and "an ambiguous probe keeps the loader's default" |
| F10 | §9 | added the UpstreamStill slot after the seven-report list | Neon's trim dropped it; the re-render (22:18) is titled "Framework bugs we found (written up, not yet filed)" and lists 3 iced / 3 GPUI / 1 Rune, matching the text |

### Media checked (frames sampled with ffmpeg)

| Slot | Frames viewed (s) | Result |
|---|---|---|
| SideBySide.mp4 (22.0 s) | 10 | OK: laptop, local fixture clip, "No comparable browser recording", CPU in cores of 16 |
| FramePath.mp4 (42.6 s) | 5, 10, 15, 20, 25, 30, 35, end | Fixed (F1). 48.4 MB/s, 6.55/6.53/6.57 B → 0.35/0.35/0.33 B, "about 19×"; coda 0.09–0.10 vs 0.44–0.45 cores (laptop), 2.92–2.94 vs 0.91–1.59 (Fly) |
| InstructionsStill.png | still | OK (6.55 B vs 353/353/328 M; within 8%) |
| FirstFrame.mp4 (34.5 s) | 1, 5, 10, 15, 25, 30 | OK: 95/118/189 laptop GPU, 191/189/300 Fly software, YouTube 1,857 ms; warm footnote "about 0.6–0.7 s … 15.4 s pre-roll … timed out behind an ad"; stages panel says stage durations weren't isolated |
| FirstSecondReplay.mp4 (4 s) | 1.5 | OK: +0.38 s iced and GPUI "FIRST VISIBLE FRAME", Rune "OPEN SENT" |
| SideBySideFly.mp4 (38 s) | 1, 10, 20, 30, end | Fixed (F3). Signed-out ("Sign in" button), no account content; cores of 4 |
| ScrollCost.mp4 (46.6 s) | 5, 10, 15, 20, 25, 30, 35, 40, end | Fixed (F4). 570/1,117/375 M/s, 9.4/18.6/6.2 M/frame; gate 0.22/0.46/0.27 cores ("iced and Rune swap places"); Fly 1.35/1.96/2.80 cores, 33.7/49.0/70.0% |
| Memory.mp4 (45.6 s) | 5, 15, 25, 30, 35, 40, end | Fixed (F5). 99/143/131 → 153/188/169 → 211/255/244 (116 core) → 267/289/337; Fly 427/485/422, ntctl 124, YouTube 865 (peak 1,014) |
| Hiccups.mp4 (32.4 s) | 20, end | OK: 43 found / 43 fixed, 7/6/7/14/9; "9 of 9 NOW runs clean" agrees with 9 checks per app |
| CostStill, GPUStill, SizesStill, attribution PNG | stills | OK |
| EGLStill.png | still | Fixed (F9) |
| PerCoreStill.png | still | Removed (F8) |
| UpstreamStill.png | still | Added (F10) |

### Policy, privacy, X readiness

- Only @Priyansh4444; no email or real name. "Private web API … logged out … outside YouTube's
  Terms of Service"; no compliance claim. No energy claim. ±50% on parity. "2,508 lines at first
  delivery". The iced crash line keeps its caveats. The local reports are "not yet filed upstream",
  and the restored still's title says the same.
- Title 69 characters. §§1–8 are 2,699 words with captions (2,337 without); the appendix runs
  long by design. No HTML comments, TODOs or placeholders. All 15 media files exist. Each slot
  follows the paragraph it supports.

### Remaining risks (not blocking)

- The media round numbers more coarsely than the text (FramePath 0.09–0.10 cores and 0.6–0.7% vs
  0.095–0.105 and 0.59–0.65%; ScrollCost 9.4 vs 9.45 M/frame; FirstFrame "about 0.6–0.7 s" vs
  0.61–0.74 s). The values agree; only the rounding differs.
- FramePath's footer reads "Laptop • AC", but its coda's laptop cores come from the battery
  resource runs. The text labels this correctly. A re-render would fix the footer.
- ScrollCost's last line asserts the redraw mechanism. F4 caveats it in the caption; a re-render
  could soften it on screen.
- Memory's stage tag reads "LAPTOP • GPU" on its software-playback stage. That tag names the
  machine, not the render path.
- To restore PerCoreStill, re-render it from the repaired partition first.

**Verdict: READY TO POST.** No blockers remain.
