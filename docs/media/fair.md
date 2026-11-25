# Matched fairness re-measurement

This page answers the two fairness audits ([Neon](../blog/fairness/neon.md),
[Carbon](../blog/fairness/carbon.md)). The three clients replayed identical fixtures and a local video under matched prewarm policies.
There are 108 recorded attempts per machine. The Fly suite completed all 108. The laptop suite
completed 106, with two GPUI first-frame timeouts at `NT_PREWARM=off`. The same three binary
hashes were used on both machines.

## Follow-up on this laptop, on battery

A second isolated run used the same Intel GPU, 60 Hz private Sway output, binaries, and fixtures.
All 36 attempts completed across six rotated rounds. The laptop used the
`power` energy preference on battery throughout. The highest one-minute host load
at slice start was 2.95. The open and memory tests used full prewarm.
The 27-notch wheel sequence used prewarm off. The table reports medians and observed ranges
over the five warm repeats per app, with the first round held out.

| Metric | iced | GPUI | Rune |
|---|---:|---:|---:|
| Click to first visible frame (ms) | 160.5 (131.5–176.0) | 154.6 (139.6–161.7) | 141.3 (118.6–149.9) |
| Home idle PSS (MiB) | 132.5 (131.5–132.7) | 226.5 (225.1–226.9) | 144.1 (143.8–145.0) |
| Playback scenario peak PSS (MiB) | 276.9 (276.5–278.0) | 321.1 (318.1–322.2) | 237.5 (235.9–240.7) |
| Wheel-phase app draw rate (fps) | 58.0 (57.7–58.8) | 58.8 (58.0–58.8) | 60.3 (59.9–60.3) |
| Wheel-phase p95 frame interval (ms) | 17.5 (17.4–17.6) | 18.6 (18.1–19.7) | 19.3 (19.1–19.5) |
| Wheel CPU (cores) | 0.248 (0.215–0.296) | 0.373 (0.348–0.421) | 0.207 (0.196–0.233) |

The first-frame ranges overlap again. iced retains the lowest idle memory and Rune the
lowest scenario peak. GPUI spends more wheel CPU than either other app. Draw rate stayed
near 60 fps across the three clients. The separate touchpad footage illustrates one gesture.

Raw results are in `.cache/iso-test/out/fair-jbUJioGu`. The summary is
`.cache/media/out/fair/laptop-confirmation.json`.

## What changed in the rankings

- **First video frame.** With full prewarm on the laptop, all three ranges overlap (iced 108 ms,
  GPUI 105 ms and Rune 96 ms medians). At shipped defaults, iced and GPUI still overlap, while
  Rune starts slower with its default of no prewarm. Full prewarm on Fly gives iced a clear lead.
- **Playback.** On the laptop, the GPU path retires about 16–19× fewer user-space CPU instructions
  per rendered video frame than forced software playback at the shipped defaults. On Fly, the
  default EGL route uses about 3.27 cores and renders roughly 150–154 frames in six seconds.
  Forced software uses about 1.20–2.11 cores and renders roughly 180 frames in six seconds.
- **Scrolling.** With prewarm off on the laptop, iced and Rune overlap on wheel CPU time. Rune
  has the lowest instructions per drawn frame. On Fly, iced leads on both metrics, and GPUI and
  Rune overlap in wheel CPU time. The viewports still mount different numbers of cards.
- **Memory.** With full prewarm, iced has the lowest Home idle PSS on both machines. Rune has the
  lowest peak PSS in the full-prewarm Home-to-playback scenario on both machines. The peak is
  for this short scenario. The earlier scripted tour followed a longer path.

### GPUI first-open timeouts

Laptop rounds 3 and 6 for GPUI with prewarm off reached player Ready and attached the Wayland
surface, then rendered zero video frames in the playback phase. The second open succeeded in
both runs. The first open had a 12 s deadline, and those runs are excluded from successful-run
medians. The remaining three warm runs are descriptive. The cause of the missing video
frames is still unconfirmed.

A separate six-run laptop diagnostic with GPU initialization tracing produced a first frame
on every run. Host load was above the benchmark threshold, so those runs were kept out of the
comparison. The diagnostic log is `.cache/media/out/fair/gpui-diagnostic.log`.

Ten further isolated battery runs with GPU and libmpv tracing also reached a first frame.
Those diagnostic runs used a higher load ceiling and disabled instruction counting. Their
load, outcomes, and per-run log locations are in
`.cache/media/out/fair/gpui-closeout/driver.log`. The original timeouts remain unexplained.

The raw laptop JSON is in the directory named by `.cache/media/out/fair/laptop-run-dir`.
Fly JSON and the shutdown receipt are in `.cache/media/out/fair/fly/` and
`.cache/media/out/fair/fly-logs/cleanup.log`. The generated summary reports the failed attempts
and sample sizes per metric.

```sh
fair=.cache/media/out/fair
python tools/media/fair_summary.py \
  --env "Laptop GPU=$(cat "$fair/laptop-run-dir")" \
  --env "Fly no GPU=$fair/fly" \
  --out "$fair/summary.json" --markdown "$fair/tables.md"
```

## What changed in the apps (equal work)

| # | Audit finding | Change | Files |
|---|---|---|---|
| 1 | GPUI and Rune load Watch comments on every open; iced only after a scroll | One rule in core, `policy::comments_due`: the first page of comments is requested when the comments header is within one viewport height below the viewport's bottom edge, checked after every layout and every scroll. iced now also checks after the frame that lays out the header (before, only on scroll). GPUI checks against the laid-out page with its info, not the skeleton. The Retry row is unchanged. At 1280×800 the header is within reach at offset 0, so **all three now load page 1 of comments on open** | `core/src/policy.rs`, `apps/iced/src/{app,view}.rs`, `apps/gpui/src/app.rs`, `apps/rune/src/app.rs` |
| 2 | Rune decoded Home thumbnails twice (8.1–9.1 MiB vs 3.95) | Rune asks for no image before the window's first layout. Before that layout, the view still had the default scale, so a 2× output got every Home thumbnail at 1× and then at 2×. Reproduced in the container at scale 2 (21.9 MiB, after the fix 16.9 MiB). The remaining Home gap at scale 1 (4.95 vs 3.97 MiB) is Rune's larger mount band (2.5 vs 2.0 viewports, 15 vs 12 cards), a design difference kept and disclosed | `apps/rune/src/{images,view}.rs` |
| 3 | iced's autopilot `scroll` rebuilt the view 27 times | Autopilot wheel commands (`scroll`, `scroll_pixels`, `fling`) no longer become messages. A task queues them for the page's scroller, which applies them with their arrival time on the next frame, as it does real wheel events. The task then requests a redraw with `window::Action::RedrawAll` | `apps/iced/src/{ext,scroller,app}.rs` |
| 4 | Prewarm policies differed | `NT_PREWARM=off|player|full` (`NT_NO_PREWARM=1` still means off) in core's `policy::Prewarm`. Every app starts its prewarm `PREWARM_AFTER_HOME` = 250 ms after the frame that carries `home_loaded`. Shipped defaults: iced `full`, GPUI `full` (was 600 ms after start, now 250 ms after Home), Rune `off` (as at HEAD since 657b90d). Each app logs `NT_PREWARM policy/player/surface` with times | `core/src/policy.rs`, `apps/iced/src/{ext,app}.rs`, `apps/gpui/src/{app,player}.rs`, `apps/rune/src/{video,app}.rs` |
| 5 | Marks fired before submit in iced and GPUI, after present in Rune | All frame marks, `playback_started` included, are now recorded after the frame's submit and present call returns. iced records them from `Ext::Presented`, a message its event loop handles only after the redraw that queued it. GPUI uses `Window::defer`, which runs after `Window::present` when the frame's update ends. Rune keeps `on_frame_encoded` / `on_frame_drawn`. The GPU-path condition is one function, `policy::gpu_picture_ready`: status Ready/Ended, the surface presenting, and `presented_frames() > 0` for this load. Software: the frame that draws the uploaded picture | `core/src/policy.rs`, `apps/iced/src/{scroller,surface,app}.rs`, `apps/gpui/src/{app,player}.rs`, `apps/rune/src/wayland_video.rs` |
| 6 | Antialiasing | Disclosed only, see below | — |

Side effect of item 5 in iced: one extra update and view build per frame that carried a mark.
This happens after the mark, a handful of times per run.

### Antialiasing and MSAA (disclosed, unchanged)

| Framework | Setting |
|---|---|
| iced 0.14 | `.antialiasing(false)`: no MSAA for triangle meshes (the canvas spinner). Quads and rounded rectangles get analytic AA in the shader. Text is grayscale (cryoglyph). |
| GPUI (gpui-pre 0.3.12) | Paths use 4× MSAA (falls back to 2× or 1×). Quads and shadows get SDF AA. Text is subpixel when the adapter supports dual-source blending, else grayscale. |
| Rune (4fbd2d6) | No MSAA (sample count 1). Shapes get analytic coverage in the shader. |

No app renders visibly worse at 1× on the Home and Watch screenshots from the isolated runs.
iced's MSAA-off setting only affects mesh primitives, which it barely draws, so nothing changed.

## How it was measured

- **Driver:** [`tools/iso-test/fair.py`](../../tools/iso-test/fair.py), the same script on both machines.
  Each run starts a fresh profile (autoplay off; Rune theatre and guide off; the others default),
  with fixture replay, the local 1280×720 H.264 clip, a muted player (`ao=null`) and no MPRIS.
  The app is stopped before `exec` so `perf stat` attaches first.
- **Order:** six rounds. In each round the app order rotates (iced, GPUI, Rune, then GPUI, Rune,
  iced, …), and within a round each slot (suite × condition) runs all three apps back to back.
  Round 1 is reported separately as a cold-run proxy using the machine's cache state at suite
  start. Warm statistics use the median and min–max of valid rounds 2–6. Each process starts
  with a fresh profile.
- **Conditions:** `NT_PREWARM=off`, `NT_PREWARM=full`, and the shipped default with no variable
  (iced full, GPUI full, Rune off). Each run's log confirms the default (`NT_PREWARM policy …`).
- **Suites:**
  - *latency* (each condition): Home, then a 4 s Home idle, open related card 1 to the unified
    first frame, 6 s playback, and a switch to card 0 to its first frame.
  - *wheel* (off, full): 2 s idle on Home, then 15 notches down and 12 up, 0.1 s apart (2.7 s).
  - *software* (default): forced software video, 6 s playback.
- **Metrics:**
  - Window map: Sway `window:new` IPC event minus the resume of the stopped process.
  - Home: `exec_to_init` plus `home_loaded`.
  - Open and switch: autopilot command to `playback_started`.
  - Home idle PSS: process-tree PSS 2.0–3.8 s after Home, with libmpv and EGL/Mesa mappings
    checked in `/proc/<pid>/maps` across the same window.
  - Peak PSS: the whole latency scenario.
  - Cores: in-app CPU seconds per phase divided by phase seconds.
  - Instructions: user-space `instructions:u` summed over hybrid PMUs, unscaled, in windows
    enabled and disabled at autopilot snapshot boundaries through a FIFO handshake. Home runs
    from exec to the first `home_loaded`.
  - Thumbnail cache: core's decoded image bytes at the end of the Home idle phase.
- **Laptop:** rootless Podman container ([tools/iso-test](../../tools/iso-test/README.md)), its own
  headless Sway on one 1280×800 output at scale 1, only the Intel render node, `--cpus 4`, no
  network. A copy of the default seccomp profile also allows `perf_event_open`, with no added
  capability, so `perf_event_paranoid=2` still applies. AC power. Before each slice, the host
  mirrors the slice into `.cache/orchestrate/native-tube/measure.lock`, so other agents start no
  renders, and a slice starts only at a host 1-minute load average ≤ 3. It waits up to 10 min,
  then refuses. Load, governor and EPP are recorded per slice.
- **Fly:** a temporary `performance-4x` in `pronsh-sandbox` (sjc), with Sway on Pixman, llvmpipe EGL
  and lavapipe Vulkan. The same binaries, fixtures and clip. The app was destroyed afterwards.
- **Verdicts:** a difference is a **win** only when the winner's warm max is below every other
  app's warm min. Overlapping leaders are **tied within noise**. A ranking requires at least five valid warm
  samples per app. These ranges describe repeatability on these two setups.
- **YouTube web** is not re-measured here. The existing Fly rerun, with ads separated, stands. The
  n = 2 laptop Helium comparison should be dropped from the article.

## Results

### Laptop GPU: NT_PREWARM=off

Warm median (min–max) over valid rounds 2–6. Cold values come from round 1.

| Metric | Unit | iced | GPUI | Rune | Verdict |
|---|---|---|---|---|---|
| Window map | ms | 114 (96.8–147); cold 300 | 164 (152–175); cold 376; warm n=3 | 122 (110–129); cold 367 | Fewer than five valid warm samples for at least one app |
| Home loaded | ms | 183 (160–216); cold 380 | 281 (254–324); cold 479; warm n=3 | 171 (158–177); cold 407 | Fewer than five valid warm samples for at least one app |
| Open → first frame | ms | 202 (171–231); cold 648 | 192 (168–193); cold 199; warm n=3 | 242 (196–259); cold 340 | Fewer than five valid warm samples for at least one app |
| Warm switch → first frame | ms | 44.3 (31.7–57.0); cold 47.2 | 50.3 (47.7–52.6); cold 68.3; warm n=3 | 43.2 (39.5–49.1); cold 42.5 | Fewer than five valid warm samples for at least one app |
| Home idle PSS | MiB | 66.0 (65.5–66.7); cold 71.8 | 177 (177–178); cold 183; warm n=3 | 79.7 (78.9–79.8); cold 94.8 | Fewer than five valid warm samples for at least one app |
| Peak PSS (Home, open, 6 s playback, switch) | MiB | 273 (271–281); cold 285 | 322 (321–323); cold 328; warm n=3 | 241 (237–262); cold 274 | Fewer than five valid warm samples for at least one app |
| Home thumbnail cache | MiB | 3.97 (3.97–3.97); cold 3.97 | 3.97 (3.97–3.97); cold 3.97; warm n=3 | 4.95 (4.95–4.95); cold 4.95 | descriptive |
| Default-path playback | cores | 0.095 (0.087–0.105); cold 0.093 | 0.097 (0.090–0.107); cold 0.103; warm n=3 | 0.085 (0.085–0.088); cold 0.102 | Fewer than five valid warm samples for at least one app |
| UI frames during 6 s playback | frames | 6.00 (6.00–6.00); cold 6.00 | 20.0 (20.0–20.0); cold 20.0; warm n=3 | 3.00 (3.00–3.00); cold 3.00 | descriptive |
| Default-path playback instructions | M/video frame | 1.18 (1.18–1.19); cold 1.18 | 1.43 (1.43–1.43); cold 1.42; warm n=3 | 1.16 (1.15–1.16); cold 1.16 | Fewer than five valid warm samples for at least one app |
| Instructions exec → Home | M | 667 (617–829); cold 1,049 | 1,345 (1,343–1,374); cold 2,239; warm n=3 | 517 (496–555); cold 2,103 | Fewer than five valid warm samples for at least one app |
| Instructions per open | M | 1,453 (1,448–1,480); cold 1,590 | 1,203 (1,190–1,242); cold 1,282; warm n=3 | 1,469 (1,446–1,550); cold 1,528 | Fewer than five valid warm samples for at least one app |
| Instructions per switch | M | 435 (420–487); cold 441 | 300 (295–373); cold 377; warm n=3 | 464 (431–519); cold 487 | Fewer than five valid warm samples for at least one app |
| Wheel scroll (27 notches, 2.7 s) | cores | 0.203 (0.185–0.233); cold 0.192 | 0.340 (0.337–0.359); cold 0.355 | 0.192 (0.181–0.233); cold 0.207 | iced = Rune tied within noise; ahead of GPUI |
| Frames drawn during wheel | frames | 160 (160–161); cold 160 | 160 (159–162); cold 161 | 163 (162–163); cold 160 | descriptive |
| Wheel instructions | M/drawn frame | 8.56 (8.55–8.61); cold 8.58 | 17.5 (17.4–17.6); cold 17.5 | 8.25 (8.22–8.28); cold 8.33 | Rune wins |
| Thumbnail bytes decoded during wheel | MiB | 4.93 (4.93–4.93); cold 4.93 | 5.93 (5.93–5.93); cold 5.93 | 4.95 (4.95–4.95); cold 4.95 | descriptive |

### Laptop GPU: NT_PREWARM=full

Warm median (min–max) over valid rounds 2–6. Cold values come from round 1.

| Metric | Unit | iced | GPUI | Rune | Verdict |
|---|---|---|---|---|---|
| Window map | ms | 100 (99.0–108); cold 86.7 | 176 (156–203); cold 155 | 126 (90.6–132); cold 103 | iced = Rune tied within noise; ahead of GPUI |
| Home loaded | ms | 167 (160–173); cold 136 | 318 (267–329); cold 303 | 180 (127–187); cold 149 | iced = Rune tied within noise; ahead of GPUI |
| Open → first frame | ms | 108 (84.8–132); cold 101 | 105 (91.9–146); cold 126 | 95.8 (81.8–109); cold 89.6 | iced = GPUI = Rune tied within noise |
| Warm switch → first frame | ms | 49.0 (42.2–57.8); cold 45.7 | 53.3 (44.1–67.8); cold 40.7 | 44.4 (38.2–64.7); cold 57.3 | iced = GPUI = Rune tied within noise |
| Home idle PSS | MiB | 132 (132–133); cold 132 | 227 (226–228); cold 227 | 144 (144–145); cold 143 | iced wins |
| Peak PSS (Home, open, 6 s playback, switch) | MiB | 276 (275–279); cold 278 | 321 (321–323); cold 322 | 238 (236–239); cold 237 | Rune wins |
| Home thumbnail cache | MiB | 3.97 (3.97–3.97); cold 3.97 | 3.97 (3.97–3.97); cold 3.97 | 4.95 (4.95–4.95); cold 4.95 | descriptive |
| Default-path playback | cores | 0.095 (0.090–0.098); cold 0.093 | 0.097 (0.078–0.103); cold 0.098 | 0.088 (0.082–0.098); cold 0.087 | iced = GPUI = Rune tied within noise |
| UI frames during 6 s playback | frames | 6.00 (6.00–6.00); cold 6.00 | 20.0 (19.0–20.0); cold 20.0 | 3.00 (3.00–3.00); cold 3.00 | descriptive |
| Default-path playback instructions | M/video frame | 1.18 (1.18–1.19); cold 1.19 | 1.42 (1.40–1.42); cold 1.41 | 1.15 (1.15–1.16); cold 1.16 | Rune wins |
| Instructions exec → Home | M | 770 (500–815); cold 664 | 1,352 (1,304–1,460); cold 1,373 | 516 (507–571); cold 490 | iced = Rune tied within noise; ahead of GPUI |
| Instructions per open | M | 1,033 (984–1,079); cold 1,004 | 807 (793–851); cold 824 | 965 (919–999); cold 970 | GPUI wins |
| Instructions per switch | M | 414 (398–417); cold 510 | 392 (290–453); cold 306 | 502 (482–588); cold 536 | iced = GPUI tied within noise; ahead of Rune |
| Wheel scroll (27 notches, 2.7 s) | cores | 0.214 (0.178–0.384); cold 0.211 | 0.388 (0.322–0.414); cold 0.377 | 0.211 (0.200–0.222); cold 0.203 | iced = Rune tied within noise |
| Frames drawn during wheel | frames | 160 (50.0–161); cold 158 | 159 (158–160); cold 159 | 163 (161–163); cold 162 | descriptive |
| Wheel instructions | M/drawn frame | 8.56 (8.55–24.7); cold 8.66 | 17.4 (17.4–17.5); cold 17.4 | 8.25 (8.23–8.30); cold 8.26 | Rune wins |
| Thumbnail bytes decoded during wheel | MiB | 4.93 (4.93–4.93); cold 4.93 | 5.93 (5.93–5.93); cold 5.93 | 4.95 (4.95–4.95); cold 4.95 | descriptive |

### Laptop GPU: shipped default (iced full, GPUI full, Rune off)

Warm median (min–max) over valid rounds 2–6. Cold values come from round 1.

| Metric | Unit | iced | GPUI | Rune | Verdict |
|---|---|---|---|---|---|
| Window map | ms | 95.4 (75.0–105); cold 113 | 167 (158–209); cold 155 | 137 (121–141); cold 98.1 | iced wins |
| Home loaded | ms | 157 (137–173); cold 168 | 336 (280–346); cold 275 | 182 (175–255); cold 135 | iced wins |
| Open → first frame | ms | 104 (97.3–110); cold 81.4 | 101 (90.7–106); cold 101 | 224 (222–279); cold 245 | iced = GPUI tied within noise; ahead of Rune |
| Warm switch → first frame | ms | 49.7 (42.3–60.4); cold 66.2 | 50.6 (44.3–62.6); cold 41.1 | 45.7 (40.4–57.8); cold 45.2 | iced = GPUI = Rune tied within noise |
| Home idle PSS | MiB | 132 (132–133); cold 132 | 226 (226–227); cold 229 | 79.8 (79.2–80.4); cold 80.0 | Rune wins |
| Peak PSS (Home, open, 6 s playback, switch) | MiB | 278 (275–279); cold 275 | 321 (321–323); cold 324 | 240 (238–265); cold 237 | Rune wins |
| Home thumbnail cache | MiB | 3.97 (3.97–3.97); cold 3.97 | 3.97 (3.97–3.97); cold 3.97 | 4.95 (4.95–4.95); cold 4.95 | descriptive |
| Default-path playback | cores | 0.100 (0.093–0.103); cold 0.100 | 0.100 (0.092–0.103); cold 0.108 | 0.087 (0.083–0.090); cold 0.095 | Rune wins |
| UI frames during 6 s playback | frames | 6.00 (6.00–6.00); cold 6.00 | 20.0 (19.0–20.0); cold 20.0 | 3.00 (3.00–3.00); cold 3.00 | descriptive |
| Default-path playback instructions | M/video frame | 1.18 (1.18–1.19); cold 1.19 | 1.41 (1.40–1.42); cold 1.42 | 1.16 (1.16–1.16); cold 1.16 | Rune wins |
| Instructions exec → Home | M | 695 (659–889); cold 615 | 1,497 (1,483–1,697); cold 1,349 | 498 (493–545); cold 477 | Rune wins |
| Instructions per open | M | 1,011 (992–1,119); cold 976 | 818 (758–925); cold 783 | 1,463 (1,422–1,479); cold 1,447 | GPUI wins |
| Instructions per switch | M | 404 (396–454); cold 443 | 368 (299–416); cold 303 | 492 (441–502); cold 421 | iced = GPUI tied within noise |
| Forced-software playback | cores | 0.418 (0.408–0.455); cold 0.415 | 0.422 (0.403–0.442); cold 0.452 | 0.418 (0.405–0.440); cold 0.407 | iced = GPUI = Rune tied within noise |
| Forced-software playback instructions | M/video frame | 22.1 (22.1–22.1); cold 22.1 | 22.4 (22.4–22.6); cold 22.5 | 22.3 (22.2–22.4); cold 22.2 | iced wins |

### Fly no GPU: NT_PREWARM=off

Warm median (min–max) over valid rounds 2–6. Cold values come from round 1.

| Metric | Unit | iced | GPUI | Rune | Verdict |
|---|---|---|---|---|---|
| Window map | ms | 194 (180–207); cold 2,974 | 315 (315–318); cold 911 | 319 (314–324); cold 1,349 | iced wins |
| Home loaded | ms | 267 (244–286); cold 3,089 | 499 (480–508); cold 1,074 | 359 (329–382); cold 1,393 | iced wins |
| Open → first frame | ms | 426 (386–730); cold 1,703 | 478 (472–568); cold 966 | 709 (625–1,071); cold 1,282 | iced = GPUI tied within noise |
| Warm switch → first frame | ms | 228 (207–244); cold 357 | 251 (233–326); cold 277 | 270 (234–296); cold 174 | iced = GPUI = Rune tied within noise |
| Home idle PSS | MiB | 171 (168–172); cold 182 | 265 (265–266); cold 277 | 183 (181–185); cold 208 | iced wins |
| Peak PSS (Home, open, 6 s playback, switch) | MiB | 422 (415–446); cold 457 | 476 (473–480); cold 488 | 415 (407–418); cold 435 | iced = Rune tied within noise; ahead of GPUI |
| Home thumbnail cache | MiB | 3.97 (3.97–3.97); cold 3.97 | 3.97 (3.97–3.97); cold 3.97 | 4.95 (4.95–4.95); cold 4.95 | descriptive |
| Default-path playback | cores | 3.253 (3.245–3.265); cold 3.280 | 3.271 (3.243–3.292); cold 3.277 | 3.263 (3.248–3.268); cold 3.267 | iced = GPUI = Rune tied within noise |
| UI frames during 6 s playback | frames | 6.00 (6.00–6.00); cold 6.00 | 7.00 (7.00–7.00); cold 7.00 | 0.00 (0.00–0.00); cold 0.00 | descriptive |
| Default-path playback instructions | M/video frame | 848 (845–852); cold 850 | 860 (859–865); cold 859 | 843 (842–847); cold 844 | iced = Rune tied within noise; ahead of GPUI |
| Instructions exec → Home | M | 1,398 (1,397–1,726); cold 2,986 | 2,837 (2,832–2,837); cold 5,742 | 1,674 (1,658–1,822); cold 7,279 | iced = Rune tied within noise; ahead of GPUI |
| Instructions per open | M | 5,297 (4,978–5,625); cold 8,415 | 6,185 (5,665–8,242); cold 13,963 | 10,259 (8,443–18,448); cold 15,083 | iced wins |
| Instructions per switch | M | 4,610 (4,407–5,547); cold 7,024 | 5,934 (5,013–7,094); cold 6,515 | 5,846 (4,668–6,236); cold 3,551 | iced = GPUI = Rune tied within noise |
| Wheel scroll (27 notches, 2.7 s) | cores | 1.598 (1.586–1.626); cold 1.590 | 3.056 (3.010–3.135); cold 3.085 | 3.023 (2.992–3.047); cold 2.997 | iced wins |
| Frames drawn during wheel | frames | 165 (165–166); cold 165 | 148 (145–151); cold 149 | 124 (123–125); cold 123 | descriptive |
| Wheel instructions | M/drawn frame | 175 (174–175); cold 175 | 481 (480–483); cold 481 | 442 (439–444); cold 444 | iced wins |
| Thumbnail bytes decoded during wheel | MiB | 4.93 (4.93–4.93); cold 4.93 | 5.93 (5.93–5.93); cold 5.93 | 4.95 (4.95–4.95); cold 4.95 | descriptive |

### Fly no GPU: NT_PREWARM=full

Warm median (min–max) over valid rounds 2–6. Cold values come from round 1.

| Metric | Unit | iced | GPUI | Rune | Verdict |
|---|---|---|---|---|---|
| Window map | ms | 187 (183–200); cold 196 | 319 (316–348); cold 322 | 312 (306–321); cold 307 | iced wins |
| Home loaded | ms | 265 (246–270); cold 285 | 504 (482–538); cold 521 | 355 (347–365); cold 318 | iced wins |
| Open → first frame | ms | 240 (210–267); cold 212 | 363 (341–383); cold 344 | 393 (360–399); cold 406 | iced wins |
| Warm switch → first frame | ms | 200 (187–224); cold 241 | 263 (235–310); cold 281 | 331 (264–512); cold 273 | iced wins |
| Home idle PSS | MiB | 229 (229–233); cold 231 | 309 (307–316); cold 307 | 242 (239–244); cold 238 | iced wins |
| Peak PSS (Home, open, 6 s playback, switch) | MiB | 419 (417–423); cold 422 | 478 (475–479); cold 474 | 409 (404–416); cold 413 | Rune wins |
| Home thumbnail cache | MiB | 3.97 (3.97–3.97); cold 3.97 | 3.97 (3.97–3.97); cold 3.97 | 4.95 (4.95–4.95); cold 4.95 | descriptive |
| Default-path playback | cores | 3.257 (3.252–3.278); cold 3.280 | 3.258 (3.250–3.275); cold 3.277 | 3.262 (3.251–3.293); cold 3.308 | iced = GPUI = Rune tied within noise |
| UI frames during 6 s playback | frames | 6.00 (6.00–6.00); cold 6.00 | 7.00 (7.00–7.00); cold 7.00 | 0.00 (0.00–0.00); cold 0.00 | descriptive |
| Default-path playback instructions | M/video frame | 850 (849–853); cold 849 | 862 (860–864); cold 862 | 843 (839–847); cold 845 | Rune wins |
| Instructions exec → Home | M | 1,398 (1,398–1,567); cold 1,398 | 2,837 (2,836–2,838); cold 2,832 | 1,672 (1,662–1,714); cold 1,668 | iced wins |
| Instructions per open | M | 4,105 (3,583–4,532); cold 3,704 | 6,444 (5,831–6,560); cold 5,856 | 6,656 (5,777–7,015); cold 6,650 | iced wins |
| Instructions per switch | M | 4,092 (3,808–4,961); cold 4,634 | 5,626 (5,108–7,561); cold 6,308 | 7,080 (5,889–11,477); cold 5,432 | iced wins |
| Wheel scroll (27 notches, 2.7 s) | cores | 1.633 (1.602–1.649); cold 1.742 | 2.976 (2.945–3.025); cold 3.035 | 2.987 (2.886–3.019); cold 3.002 | iced wins |
| Frames drawn during wheel | frames | 165 (165–166); cold 164 | 149 (143–151); cold 152 | 124 (108–126); cold 125 | descriptive |
| Wheel instructions | M/drawn frame | 175 (175–175); cold 175 | 467 (465–468); cold 467 | 443 (440–448); cold 442 | iced wins |
| Thumbnail bytes decoded during wheel | MiB | 4.93 (4.93–4.93); cold 4.93 | 5.93 (5.93–5.93); cold 5.93 | 4.95 (4.95–4.95); cold 4.95 | descriptive |

### Fly no GPU: shipped default (iced full, GPUI full, Rune off)

Warm median (min–max) over valid rounds 2–6. Cold values come from round 1.

| Metric | Unit | iced | GPUI | Rune | Verdict |
|---|---|---|---|---|---|
| Window map | ms | 203 (179–215); cold 202 | 322 (311–325); cold 322 | 318 (309–330); cold 322 | iced wins |
| Home loaded | ms | 298 (257–314); cold 288 | 499 (464–513); cold 498 | 365 (327–371); cold 351 | iced wins |
| Open → first frame | ms | 242 (221–259); cold 244 | 372 (333–388); cold 350 | 707 (660–1,085); cold 657 | iced wins |
| Warm switch → first frame | ms | 218 (186–231); cold 232 | 260 (240–298); cold 252 | 229 (214–332); cold 305 | iced = Rune tied within noise |
| Home idle PSS | MiB | 229 (228–231); cold 232 | 310 (308–313); cold 309 | 182 (180–184); cold 183 | Rune wins |
| Peak PSS (Home, open, 6 s playback, switch) | MiB | 423 (417–425); cold 423 | 477 (475–478); cold 478 | 425 (407–428); cold 405 | iced = Rune tied within noise; ahead of GPUI |
| Home thumbnail cache | MiB | 3.97 (3.97–3.97); cold 3.97 | 3.97 (3.97–3.97); cold 3.97 | 4.95 (4.95–4.95); cold 4.95 | descriptive |
| Default-path playback | cores | 3.272 (3.263–3.293); cold 3.288 | 3.268 (3.258–3.295); cold 3.258 | 3.268 (3.258–3.292); cold 3.303 | iced = GPUI = Rune tied within noise |
| UI frames during 6 s playback | frames | 6.00 (6.00–6.00); cold 6.00 | 7.00 (7.00–7.00); cold 7.00 | 0.00 (0.00–0.00); cold 0.00 | descriptive |
| Default-path playback instructions | M/video frame | 851 (848–853); cold 851 | 861 (859–863); cold 857 | 846 (844–849); cold 844 | iced = Rune tied within noise; ahead of GPUI |
| Instructions exec → Home | M | 1,560 (1,396–1,716); cold 1,716 | 2,837 (2,836–2,837); cold 2,837 | 1,716 (1,675–1,765); cold 1,666 | iced = Rune tied within noise; ahead of GPUI |
| Instructions per open | M | 4,173 (3,781–4,481); cold 4,261 | 6,425 (5,709–7,005); cold 6,563 | 10,562 (9,526–17,599); cold 9,181 | iced wins |
| Instructions per switch | M | 4,471 (3,812–4,623); cold 4,354 | 5,805 (4,939–6,669); cold 5,700 | 4,501 (4,086–7,357); cold 6,673 | iced = Rune tied within noise |
| Forced-software playback | cores | 1.198 (1.182–1.243); cold 1.223 | 1.968 (1.942–2.005); cold 1.957 | 2.105 (2.050–2.135); cold 2.078 | iced wins |
| Forced-software playback instructions | M/video frame | 254 (253–255); cold 255 | 521 (519–521); cold 519 | 456 (454–457); cold 456 | iced wins |
