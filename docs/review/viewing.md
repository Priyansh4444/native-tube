# Viewing review: is watching a video as good as YouTube's player and mpv?

Lens: picture quality, smoothness, A/V sync, start/seek/buffering, CPU and power, and the
viewer features. core.md already covers four of these from reading: the 720p cap (its P1-3),
"Ended" on expiry or network drop (P1-2), the A/V offset, and keyframe-rounded relative seeks.
Below they are measured or reproduced and referenced, not re-argued. ui-ux.md already lists the
missing player UI (fullscreen, theatre, settings, captions, chapters, autoplay); here only the
player-core side and a ranking are added.

## How this was checked

- **Not run:** none of the GUI apps; no off-screen outputs were used.
- **Pipeline, headless:**
  - `core/target/release/ntctl` (`play`, `resolve`, `search`).
  - Two scratch binaries linked against `nt-core`. `pace` timestamps every frame the `Player`
    publishes, at the moment an app's waker fires. `watch` prints every change of
    `PlaybackState` and can pause, resume and seek.
  - mpv (`--mute=yes`, `--vo=image` or `--vo=null`), ffmpeg (zscale, psnr, ssim) and ffprobe.
  - A local Range-capable HTTP server that answers 403, or drops connections, after N seconds.
  - Everything muted (`NT_MUTE=1`).
- **Local clips (ffmpeg, scratch directory):**
  - Lossless 4:2:0 BT.709 stills at 720p and 1080p: a chroma zone plate, a Mandelbrot tile,
    SMPTE HD bars and testsrc2 text.
  - 1080p60, 2160p30, HDR10 PQ, a 10-bit gradient, full range, SD BT.601, vertical,
    anamorphic, interlaced.
  - 24/25/30/60 fps 720p clips with audio.
- **Live YouTube traffic, kept small:**
  - Big Buck Bunny `aqz-KE-bpKQ`: one resolve with a 4320 cap, one play with seven seeks.
  - One age-restricted video.
  - One search and two live resolves.
- **Machine:** Core Ultra 7 255H (Arc 140T, VA-API) and an RTX 5070 Mobile. The owner's panel
  is eDP-1, 1920×1200 at **165 Hz**, scale 1.333.
- **Code:** everything in core/ and apps/ since 96df4ec is rustfmt only, so the video path is
  as delivered. Line numbers are for the current tree.
- **perf:** call graphs are blocked (`perf_event_paranoid=2`), so the profile is a flat
  `cpu-clock:u` profile.

## The pipeline as it stands

1. **Resolve.** `resolve.rs` picks avc1, then vp9, at height ≤ `NT_MAX_HEIGHT`, default
   **720** (`resolve.rs:96-100`, `:212`). Live streams hand mpv the HLS master playlist
   (`:181`).
2. **Decode.** libmpv decodes with `hwdec=vaapi-copy,auto-copy` (`player.rs:351`). The decoded
   NV12 surface is copied back to RAM.
3. **Convert and scale.** mpv's **software** renderer (render API `sw`, zimg) converts and
   scales into an RGB0 buffer.
   - Size: at most the target and never larger than the video (`render_size`,
     `player.rs:866-872`). The width is rounded *down to a multiple of 16*. The target defaults
     to 1280×720 (`:442`).
   - The loop then writes alpha = 255 into every pixel (`:957-960`) and publishes an
     `Arc<VideoFrame>`.
4. **Hand-off.** With `video-timing-offset=0` (`:395`), mpv hands over each frame when it is due.
   No timestamp travels with it.
5. **Display.** The app shows the frame on its next UI frame:
   - **iced** writes it into one persistent texture (`queue.write_texture`) and samples it
     bilinearly (`apps/iced/src/video.rs:121-122`; `Rgba8UnormSrgb` on an sRGB surface,
     `:161`).
   - **GPUI** copies it with `to_vec()` into a new `RenderImage` each frame
     (`apps/gpui/src/video.rs:111`). It frees the old atlas tile, and GPUI re-uploads the image.
   - **Rune** copies it with `to_vec()` into `rune::Pixels` on the player thread
     (`apps/rune/src/video.rs:43`). Rune then copies that into its image page and uploads the
     dirty rows.

## Measured facts

### 1. Picture quality

The mpv software path (`--vo=image` with the same options) is **bit-identical** to `ntctl play`
(PSNR ∞). It was used to render sizes `ntctl` cannot ask for. The table gives luma PSNR against
an ideal conversion of the same YUV (zimg spline36). Above about 45 dB a difference is
invisible; around 35 dB fine detail and text are visibly soft.

| Case | NativeTube | Alternative |
|---|---|---|
| 720p stream, 1280×720 box (1:1) | 46.1 dB | n/a |
| 1080p stream, 1280×720 box | 46.3 dB | 58.0 dB (lanczos from YUV, gpu-next-like) |
| **1920×1080 box (fullscreen-sized)** | **34.9 dB** (720p stream + the app's GPU bilinear) | 36.3 dB (720p + lanczos); **49.7 dB** (1080p stream, rendered 1:1) |
| Box 1226×690 (width not a multiple of 16) | **41.8 dB** (render at 1216×684, GPU-bilinear to 1226) | 61.9 dB (render at 1226 directly) |

What else was checked:

- **Colour is right.**
  - SD bars render as BT.601: green is 190, against 190 expected for 601 and 171 if 709 had
    been wrongly assumed.
  - HD is BT.709, and full-range flags are honoured; flat patches match within ±1–2.
  - 10-bit sources are dithered (zimg `random`), with no banding on a 10-bit gradient.
  - Chroma upsampling is bilinear (`zimg-scaler-chroma` default). That is slightly soft on
    saturated edges and invisible on normal content.
- **HDR10 (PQ) is hard-clipped, not tone-mapped.** The 0–100% ramp reaches white at about a third
  of its width. Today YouTube never sends PQ here: avc1 is preferred, and YouTube's avc1
  renditions of HDR uploads are SDR. It becomes real when VP9.2 or AV1 is picked above 1080p.
- **Shapes.** Anamorphic sources come out at the right display aspect. Vertical 1080×1920
  renders at 400×710 (within 0.2%), but the first one or two frames are 1280×720, because
  `dwidth`/`dheight` arrive after the first render. Interlaced sources are combed
  (`deinterlace=no`); YouTube serves only progressive, so this is a non-issue.
- **What YouTube offers.** For BBB at a 4320 cap, the best avc1 is `1080p60 avc1 6.9 Mb/s`. The
  default cap gets 720p. 1440p and 2160p exist only as VP9 and AV1.
- **The owner's own default watch page is already upscaled.** On a maximised 1920×1200 window at
  scale 1.333 (1440 logical wide), the primary column is about 980 logical, i.e. about 1300 px
  physical. That is above the 1280 px render cap, so even the default watch page draws a 720p
  render stretched about 2%. The width rounding to 16 adds a second resample. Concluded from
  layout widths in the other reviews; not drawn.

### 2. Smoothness

**The core's publishing is excellent.** On local clips, intervals between published frames have
a standard deviation of 0.15–0.21 ms. Lateness against each frame's own timestamp is 0.2–0.5 ms
at p50 and at most 1.7 ms. No frame was dropped or repeated at 24, 25, 30 or 60 fps, nor for
1080p60 rendered at 1080p. 2160p30 rendered at 1080p averaged 7.7 ms per frame (max 41 ms),
with lateness p99 3.3 ms and no drops.

**Presentation is the problem.** Each frame goes up on the first vsync after it is published.
Nothing aligns its due time with the display, and the app never learns that time.

How I simulated it from the measured publish times:

- 32 vsync phases, 1 ms of UI work, and optional extra UI jitter.
- For each frame, compare its on-screen duration with the regular cadence that a
  display-synced player gives the same content (mpv `video-sync=display-resample`, or Chrome,
  which schedules each frame for a chosen vsync).
- The table shows the share of frames held for the wrong number of vsyncs, the mean over all
  phases, and the worst phase in brackets.

| Content | 60 Hz | 60 Hz, +2 ms UI jitter | 165 Hz | 165 Hz, +2 ms UI jitter |
|---|---|---|---|---|
| 24 fps | 2.8% (33%) | 11.7% (56%) | 7.2% (16%) | 25.5% (32%) |
| 25 fps | 1.4% (17%) | 11.3% (27%) | 2.8% (17%) | 30.4% (41%) |
| 30 fps | 0.5% (9%) | 7.7% (53%) | 3.7% (40%) | 31.2% (71%) |
| 60 fps | 0.9% (17%); 0.45% never shown | 7.7% (52%); 3.8% never shown | 6.2% (23%) | 29.0% (38%) |

- **The worst-phase numbers are what a viewer sees, periodically.** mpv times video to the
  audio clock (`video-sync=audio`), which drifts against the display clock. The phase therefore
  sweeps through the bad zone every few minutes, and while it is there, a third to two thirds of
  frames are held wrong. At 60 fps on 60 Hz, frames are doubled and dropped in bursts.
- **165 Hz, the owner's panel, is worse, not better.** Its 6.06 ms vsync is about the size of
  the UI's own latency noise, so any jitter moves frames across vsync boundaries.
- **Caveat.** This is a simulation from real publish times plus an assumed UI jitter. GPUI's own
  notes put layout at 2–3 ms per frame, so +2 ms is realistic. The barcode method in
  metrics.md:372 would confirm it end to end.

### 3. A/V sync (estimated; the apps were not run)

mpv considers a frame displayed when it is due, and syncs audio to that. The picture reaches
the glass later:

- Waiting for the app's next frame: 0 to 1 vsync, mean ½.
- Upload and draw: about 0.5 ms in iced. Add about 1 ms in GPUI (`to_vec` plus atlas upload)
  and 1–2 ms in Rune (an extra copy into the image page).
- The compositor's next refresh: about 1 vsync.

That makes the **video late by about 26 ms on average (18–45 ms) at 60 Hz, and about 10 ms
(7–18 ms) at 165 Hz**. ITU-R BT.1359 puts the detectability threshold for sound leading the
picture at about 45 ms, so the 60 Hz tail touches it. That is not dramatic, but it is free to
fix (see the architecture section).

### 4. Start, seek, end and failures

**Measured on BBB (live YouTube) and local clips:**

| What | Result |
|---|---|
| First picture, BBB, cold player (resolve included) | **518 ms** |
| Bench, warm and cold (`bench/results/playback.json`) | core 218 / 451 ms; YouTube web 3.9–4.2 s; mpv + yt-dlp 5.7 s |
| Seek within the buffer (±5 s, +10 s) | 13–61 ms |
| Seek into unbuffered parts (300 s, 590 s of 635) | 336 / 348 ms |
| Seek back to 30 s | 176 ms |
| Seek past the end | `Ended`, last picture stays |
| **Relative seek on a 10 s GOP** (mpv, same options) | +5 s from 3 s lands at **10 s**; `relative+exact` lands at 8 s |
| **Live stream** (Lofi Girl) | **16.5 s** to the first picture; picked the top HLS variant (1080p, render 8.7 ms avg); `duration` 5 s, then 14.9 s and growing |
| Age-restricted | `Failed("Sign in to confirm your age")` |

**Failures, reproduced locally:**

- **Network drop** (connections dropped after 6 s): 7 s of `buffering=true`, then `Ended` at
  **14 s of a 20 s clip**.
- **Expiry** (HTTP 403 after 6 s): the next 10 MiB chunk fails, and the state goes straight to
  `Ended` at **17 s of 20**, with no buffering state at all.

Both are reported as the video finishing (core.md P1-2, now reproduced). Expiry bites at the
next chunk fetch, not at resume. One 10 MiB chunk covers 30–60 s of 720p, so a viewer resuming
after more than 6 hours gets up to a minute of playback and then a fake end.

### 5. CPU and power

10 s of playback, whole process, rusage:

| Run | CPU (of one core) |
|---|---|
| ntctl, 720p60 clip (1:1) | **63%** |
| same, `hwdec=no` | 72% |
| mpv `vaapi-copy`, `--vo=null` (decode + copy-back only) | 19.5% |
| ntctl, 1080p60 → 1280×720 | **112%** |
| ntctl, 1080p60 rendered at 1920×1080 | 119% |
| same, `hwdec=no` (1080p60 → 1280×720) | 137% |
| mpv `vaapi-copy`, `--vo=null`, 1080p60 | 29% |

Bench, same YouTube videos: mpv with `--hwdec=vaapi --vo=gpu-next` **including its window**
used 16.4%. Core used 31.8% with **no** presentation at all.

The flat profile (user time) shows where the time goes:

| Where | 720p60 | 1080p60 |
|---|---|---|
| zimg (conversion and scaling) | 52% | 59% |
| libmpv | 13% | 12% |
| libc (copies) | 13% | 11% |
| libigdgmm (the VA-API tiled copy-back) | 10% | 11% |
| `render_loop`, essentially the alpha fill | 6% | 3% |

So roughly two thirds of the player's CPU is the CPU conversion that a GPU would do for free.
The apps' upload and copies come on top of that. A 1080p or 4K quality option on this path
would cost a whole core or more.

## Findings

### P0

**P0-1. Any picture larger than 1280 px wide shows a 720p stream, bilinearly upscaled. That
already includes the owner's maximised watch page.**
`resolve.rs:96-100` (cap 720) and `player.rs:442` (target 1280×720) are not linked to the drawn
size, and nothing upgrades the stream when the box grows.

- Measured: a 1920×1080 picture is 34.9 dB luma against 49.7 dB for a 1080p stream. That is
  visibly soft text and detail, and YouTube serves 1080p (or 1440p) at that size.
- Fullscreen and theatre (ui-ux.md P0-2) are pointless until this is fixed.

Fix: "Auto" quality from `set_target_size`. Choose the smallest stream with height ≥ the box's
physical height, avc1 up to 1080p. Above 1080p choose VP9, and only SDR itags until tone
mapping exists (P2-1). Switch with `loadfile <url> start=<pos>` (core.md P1-3 gives the API
shape). Above 1080p only after Stage 1 below, because the software path would burn more than a
core.

**P0-2. Frames are presented at "the next UI frame after they were due", so the cadence breaks
periodically: 25–30% of frames irregular at 165 Hz under realistic UI jitter.**
`player.rs:395` sets `video-timing-offset=0`, and `VideoFrame` carries no timestamp. The apps
therefore cannot choose the right vsync, and mpv cannot do display sync, because nothing calls
`mpv_render_context_report_swap`.

- Against mpv (display-resample) and Chrome, this is the one place the viewing is measurably
  worse on every video, including 24 fps films on the owner's 165 Hz panel.

Fix, in order:

1. Turn on `MPV_RENDER_PARAM_ADVANCED_CONTROL`.
2. Read `MPV_RENDER_PARAM_NEXT_FRAME_INFO.target_time` and set `video-timing-offset` to about
   2 vsyncs, so frames arrive early.
3. Keep a queue of 2–3 rendered frames, each stamped with its `target_time`.
4. In the app, at each frame, take the queued frame whose target time is closest to the predicted
   presentation time (last frame time + refresh period + compositor latency). Request redraws
   at display rate only while playing.

This is possible today in all three apps: each already has a per-frame hook (`render()`/
`prepare()`/the waker). It fixes P1-3 as well.

### P1

**P1-1. Width rounding to 16 resamples the picture twice for 15 of 16 box widths.**
`render_size` (`player.rs:866-872`) renders, for example, 1216×684 for a 1226×690 box. The app
then stretches that bilinearly to 1226 px. Measured: 41.8 dB against 61.9 dB rendered directly,
softer text on most window sizes.

Fix: render the exact width and pad only the stride to 64 bytes (`SW_STRIDE` allows it). Upload
with `bytes_per_row = stride`; wgpu's `write_texture` has no 256-byte rule.

**P1-2. The software conversion costs 2× mpv's whole player, before any UI.**
See §5: 63% of a core for 720p60 and 112% for 1080p60, about two thirds of it in zimg and the
copy-back. That is battery and fan on the laptop. It also makes P0-1's higher qualities
unaffordable on this path. The fix is Stage 1 below. Until then, skip the alpha fill
(`:957-960`) where the toolkit samples `.rgb` only: iced already ignores alpha
(`video.rs` shader), and GPUI/Rune could do the same.

**P1-3. The video lags the sound by about 26 ms on average, up to about 45 ms, at 60 Hz; it is
not compensated (§3).**
Fix: once P0-2 presents frames at their target time, the offset drops to about 0. Until then set
`audio-delay` to `1.5 × refresh period + upload time` for the window's output, and update it
when the window changes output.

**P1-4. Expiry and network drops end the video silently (reproduced; core.md P1-2).**
Measured outcomes: `Ended` at 14/20 s after a drop, and at 17/20 s after a 403. The viewer sees
the replay state mid-video.

Fix as core.md says: re-resolve at the position, add `reconnect_on_network_error`, and keep
`expiresInSeconds`. Show "Reconnecting…" over the last frame, not the end state.

**P1-5. Live streams take 16.5 s to start, ignore the quality cap, and the core cannot tell
the UI they are live.**

- The HLS master goes straight to libavformat, which opens the variants before playing; mpv then
  takes the top variant.
- `PlaybackState` has no `is_live`, and `duration` grows (5 s, then 14.9 s…). The apps' clocks
  and seek bars therefore show a growing fake duration.

Fix: parse the master playlist in Rust (it is plain text) and pass mpv the one variant chosen by
the P0-1 rule. Expose `is_live`, plus the DVR window and the distance from the live edge. Show
"LIVE" and a "jump to live" action, as YouTube does.

**P1-6. J/L/←/→ seeks snap to keyframes (confirmed with mpv; core.md P2).**
On a 10 s GOP, +5 s from 3 s lands at 10 s. YouTube's player seeks exactly.

Fix: `seek <s> relative+exact` in `seek_by` (`player.rs:649-650`). The cost is about 50–150 ms of
decoding, comparable to the 13–61 ms seen in-buffer.

**P1-7. Playback does not stop the screen from blanking.**
Nothing inhibits idle (grep finds no idle-inhibit or ScreenSaver code). A long video on the
owner's Hyprland will dim and lock mid-watch.

Fix: while `is_playing()`, create a `zwp_idle_inhibitor_v1` on the window's `wl_surface`; all
three frameworks expose raw window and display handles (see the table below). Alternatively,
call `org.freedesktop.portal.Inhibit` through zbus, which is pure Rust.

### P2

- **P2-1. HDR would be clipped** as soon as VP9.2 or AV1 is chosen. Filter HDR itags out in
  `pick` until the renderer tone-maps; Stage 1 does tone-map.
- **P2-2. GPU upscaling is bilinear** (`apps/iced/src/video.rs:121-122`; GPUI and Rune use their
  image samplers). That costs 1.4 dB against lanczos at 1.5×. It matters only once a box exceeds
  the stream, which P0-1 makes rare.
- **P2-3. The first frames of a non-16:9 video render at the 1280×720 default**, before
  `dwidth`/`dheight` arrive, so the picture changes size once. Skip publishing until the size
  is known, or render the first frame after `FILE_LOADED`.
- **P2-4. No media keys or MPRIS.** Play/pause keys and the desktop's media widget do nothing.
  Add `org.mpris.MediaPlayer2` via zbus (pure Rust): title, channel, artwork URL, position,
  and play/pause/seek/next.
- **P2-5. Positions are not remembered across sessions**, only in-session Back
  (`library.rs` stores no position). YouTube resumes long videos and shows a red progress bar on
  watched thumbnails. Store `{id: position}` when leaving or every 10 s, resume if more than 30 s
  in and more than 30 s from the end, and draw the bar on cards.
- **P2-6. Unplayable videos give YouTube's bare reason**, for example "Sign in to confirm your
  age". Say that the video cannot be played without signing in. Offer "Open on YouTube" (a URL
  handed to the desktop), and skip it in autoplay.
- **P2-7. A trap in GPUI.** gpui-pre-wgpu on Linux already compiles an NV12 "surfaces" pipeline,
  but its batch is a no-op (`gpui-pre-wgpu-0.3.12/src/wgpu_renderer.rs:1639`) and its shader
  hard-codes a BT.601 full-range matrix (`shaders.wgsl:1325-1330`). Enabling it as-is would give
  wrong colours on YouTube's BT.709 limited-range video.

### Viewer features, ranked by how much they matter

1. Fullscreen (F, double-click, Esc), together with P0-1. ui-ux.md P0-2.
2. Auto quality plus a quality menu (P0-1).
3. Idle inhibition (P1-7).
4. Exact keyboard parity with YouTube:
   - already present: K/Space, J/L, arrows, M;
   - missing: F, T, C, 0–9 (jump to 0–90%), Home/End, `,`/`.` frame step while paused,
     `<`/`>` speed, Shift+N next.
5. Playback speed: the `speed` property, with mpv's scaletempo2 keeping the pitch.
6. Captions: `captionTracks` from `/player`, loaded with `sub-add`.
   - The software renderer burns them in at render size.
   - Better: the UI draws them from `sub-text`, so they stay sharp at any size.
   - Remember the on/off choice and the language.
7. Theatre mode.
8. Autoplay next and an up-next card. Today the video stops at the end (Ended, last frame).
9. Remembering positions (P2-5).
10. Chapters on the scrubber, and timestamps in the description that seek.
11. Seek-bar thumbnails: YouTube's storyboard sprite sheets (`storyboards` in `/player`) are
    plain JPEGs the image pipeline can already fetch.
12. Media keys and MPRIS (P2-4).
13. Mini player and picture-in-picture: a second small window works on Wayland; real PiP does
    not exist there.
14. Loop.
15. Volume normalisation: YouTube's `loudnessDb` from `/player`, applied as mpv `volume-gain`,
    or the DRC audio variant (core.md P1-4).

## Recommended video path

### What each option costs (checked in the sources)

| Option | Quality | CPU / power | iced 0.14 (wgpu 27) | GPUI 0.3.12 | Rune 4fbd2d6 (wgpu 30) | Risk |
|---|---|---|---|---|---|---|
| **Today**: sw render ≤720p → RGBA → upload | P0-1, P1-1 | 2× mpv | works | works, plus a copy per frame | works, plus two copies | n/a |
| **Render at the drawn size** (sw) | fixes the size problems; still no tone mapping | +10–50% more CPU at 1080p+ | trivial | trivial | trivial | low; CPU gets worse |
| **NV12/P010 planes plus a shader per framework** | as good as the shader | low | `shader` widget has device and queue | needs a patched gpui-pre-wgpu: Linux `PaintSurface` is a no-op | `Compositor` hook (`compose.rs`) has device, queue and effect slots | **Not available: libmpv exposes no decoded frames.** The sw render API outputs single-plane RGB only (`render.h:362-385`), so this means replacing mpv with libavcodec plus your own A/V sync. Reject. |
| **mpv OpenGL render API, offscreen EGL, PBO readback into the existing pool** (Stage 1) | mpv's GPU renderer: lanczos/EWA scaling, tone mapping, dithering, subtitles | GPU conversion and zero-copy hwdec inside mpv. Readback ≈ 8 MB a frame at 1080p, async. Estimated at a few % plus the upload; not measured. | unchanged app code | unchanged | unchanged | medium, all in core: an EGL context on the iGPU in the render thread. libEGL/libGL are native libraries, so allowed. |
| **The same, but DMA-BUF export into the framework's Vulkan device** | as above | lowest | `wgpu::Device::create_texture_from_hal` and wgpu-hal `texture_from_raw` exist, but the device must be created with `VK_EXT_external_memory_dma_buf` and image DRM-format modifiers, and every framework creates its own device | same, plus no Linux surface primitive | same | high: patch three frameworks; driver-specific modifiers |
| **Wayland subsurface below the window, mpv GL into a `wl_egl_window`** (Stage 2) | exactly mpv: display-sync (`report_swap`, `video-sync=display-resample`), and direct scanout in fullscreen | lowest; equals mpv | wl handles via `iced::window::run` (`iced_runtime-0.14.0/src/window.rs:463`) | `HasWindowHandle`/`HasDisplayHandle` (`gpui-pre-0.3.12/src/window.rs:7866-7872`); transparent background and opaque-region support (`gpui-pre-linux …/wayland/window.rs:2474`) | winit handle (`rune/src/window.rs:15`) | medium-high: the app must leave a transparent hole, set the subsurface position and viewporter crop in sync with scrolling (sync mode applies on the parent's commit), and draw controls in the parent above it. A one-frame lag during scroll is possible if the commits drift. |

### Staged plan (biggest gain first)

**Stage 0 (days, sw path kept):**

- P1-1: exact width.
- P0-1: auto quality up to 1080p avc1, plus a quality menu.
- P1-6: exact relative seeks.
- P1-4: re-resolve on expiry or drop.
- P1-5: pick the HLS variant in Rust, and expose `is_live`.
- P1-7: idle inhibitor.
- P1-3: `audio-delay` compensation.

That removes everything visible except cadence, with no change to the apps' video code.

**Stage 0.5: timestamps (P0-2).**

- Core: `ADVANCED_CONTROL` plus `NEXT_FRAME_INFO`, a positive `video-timing-offset`, and a
  2–3-frame queue with target times.
- Each app: choose the frame for the vsync it is drawing.

This is the cadence fix. It also zeroes the A/V offset, so the `audio-delay` stopgap goes.

**Stage 1: GPU rendering in core.**

- Replace the `sw` render context with mpv's OpenGL render API on a surfaceless EGL context
  (iGPU) in the existing render thread.
- Render at the drawn size into an FBO, then do an async PBO readback into the same
  `Arc<VideoFrame>` pool.
- The frontends stay unchanged, and every app still gets identical frames, which keeps the
  framework comparison fair.

Gains: mpv-grade scaling, HDR tone mapping, sharp subtitles, zero-copy VA-API decode, and
1440p/4K VP9 become affordable. Measure the CPU against the §5 table, and accept Stage 1 only if
it at least halves the 1080p60 number.

**Stage 2: zero-copy presentation, per app, optional.**

- A Wayland subsurface presenter for the watch page and fullscreen, using mpv's own timing
  (`report_swap`, `display-resample`).
- Keep the Stage 1 path as the fallback, and as the benchmark mode that measures the toolkit
  rather than mpv.
- Do it first in iced (simplest surface ownership), then GPUI, then Rune.

DMA-BUF import into wgpu is not worth it while the subsurface route exists: it needs patches to
all three frameworks' device creation for the same result.
