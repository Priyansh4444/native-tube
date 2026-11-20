# NativeTube: iced frontend (`nt-iced`)

## Versions

- iced 0.14.0 (iced_core 0.14.0, iced_widget 0.14.2, iced_winit 0.14.1, iced_wgpu 0.14.0), with wgpu 27.0.1,
  winit 0.30.13 (Wayland through smithay-client-toolkit 0.19.2), and cosmic-text 0.15 for text.
- Features: `wgpu`, `advanced`, `image-without-codecs`, `crisp`, `tokio`, with `wayland` enabled only on Linux.
  Default features are off, so there is no tiny-skia fallback, no X11, and no image codecs (the core decodes).
  macOS uses winit's native backend and wgpu's native Metal backend.
- The component kit is iced's own widgets (button, text, text_input, slider, container, row/column,
  stack, hover, mouse_area, shader, image). No third-party widget crates.
- About 3,800 lines of Rust in `src/`.

## Architecture

- `app.rs` holds the state, `Message`, and `update`. The state is a navigation stack, `Vec<Page>`.
  Each `Page` owns its body, query and `ScrollModel`. Back moves it to the Forward stack.
  Page-tagged jobs can deliver results to either stack, and returning preserves the loaded rows
  and position.
- Async work: each `nt_core::client()` call returns a `Job`, which is a `Future`, wrapped in
  `Task::perform`. Results come back as messages tagged with the page id (and a request number
  for topic switches), so stale answers are dropped. iced's executor is replaced by a tiny
  `CoreExecutor` that spawns onto `nt_core::runtime()`, so there is one tokio pool and not two.
  The `tokio` feature is enabled only because iced refuses to compile without an executor feature.
- Off-thread events (`ext.rs`) travel on one futures channel read by a `Subscription::run`:
  - decoded images: `Job::on_done` sends them back;
  - player wake-ups: the waker sets an atomic "pending" flag and sends at most one message until
    `update` takes it, so a burst of frames becomes one message.
  The autopilot is a second subscription built on `autopilot::next_command()`.
- Search suggestions are debounced with a 150 ms `tokio::time::sleep` task and tagged with a
  sequence number, so stale answers are dropped.
- Views: `view.rs` (pages, cards, rows, watch page, top bar, rail, dropdown) is split from the
  custom widgets:
  - `widgets.rs`: `Clamp` (N-line text with an ellipsis), `Thumb` (image + badge), `Spinner`,
    `SeekBar`, and `SearchBox`, a wrapper that reports the text field's focus and maps
    Up/Down/Escape to the suggestion list;
  - `scroller.rs`: the page scroller, plus the `FrameHook` root;
  - `video.rs`: the video primitive;
  - `imgcache.rs`: image handles.

## Scrolling

- Every page is a single custom widget, `Scroller`. iced's `scrollable` is never used.
- Raw events come from `Widget::update`, as described in docs/scroll.md:
  - **Mouse wheel:** winit on Wayland binds `wl_seat` at version 7 or lower, so a wheel notch
    arrives as `axis_discrete`, which becomes `ScrollDelta::Lines`. It goes to `wheel_ticks(-y)`.
  - **Touchpad:** finger scrolling arrives as `ScrollDelta::Pixels`, which winit fills with
    `-axis * scale`. That is undone and the result goes to
    `wheel_pixels(wayland_axis_to_px(raw))` (Chrome's default ×12).
  - **Finger lift:** `wl_pointer.axis_stop` reaches iced as an all-zero `Pixels` delta
    (winit's `TouchPhase::Ended` is dropped by iced's conversion), so a zero delta is
    `fling_start`.
  - **Clicks:** a press on the content calls `cancel_fling`.
  - **Keys:** PageUp/PageDown/Space/Shift+Space/Home/End/Up/Down call `scroll_by` with the
    module's step helpers. Keys are ignored while the search field is focused, and on the watch
    page the player keys are tried first.
  - **Scrollbar:** a thin thumb shows while the page moves (and for 0.9 s after) or on hover.
    Dragging it calls `jump_to`.
- **Ticking:** the scroller ticks the animator on `window::Event::RedrawRequested(Instant)`.
  That timestamp is taken right before iced draws and presents the frame, and the scroller
  calls `shell.request_redraw()` only while `needs_frame()` is true.
- **What a scroll frame costs:** only a translation. The children are laid out once in content
  coordinates and drawn inside `with_translation(0, -offset)`, which is the same thing iced's
  scrollable does internally, so a scroll frame runs no message, no `view`, and no layout.
- **Virtualization:**
  - `view` builds only the items intersecting [offset − 1 viewport, offset + 2 viewports].
  - Content is laid out in "lanes": one lane on most pages, two on the wide watch page.
  - Grid and list rows have fixed heights, so locating the visible rows is O(1) whatever the
    item count.
  - Variable items (watch header, description, comments) carry an estimated height until the
    scroller measures them during layout and stores the real height in the page's cache. Later
    builds then place them exactly, and `set_max_offset` is clamped in every layout.
  - When the offset drifts more than half a viewport from where the band was built, or the
    page comes within 1.5 viewports of the end, the scroller publishes one message and `view`
    rebuilds the band.
  - Children are diffed by key, so rows that stay in the band keep their shaped text.
  - Infinite scroll keeps at most one request in flight per list.
- **Drawing:** only rows that intersect the clip are drawn.

## Video

- The main `Player` warms on a worker after the first window frame. The software path uses `PixelFormat::Rgba`.
  iced's image API is straight RGBA (`image::Handle::from_rgba`).
- **Getting a frame:** the player's waker sends a coalesced message. If `frame_generation()`
  changed, `update` keeps the new `Arc<VideoFrame>` without copying it, and the rebuilt watch
  page hands it to a `shader` widget.
- **Uploading it:** the custom `shader::Primitive` (`video.rs`) owns one wgpu texture
  (`Rgba8UnormSrgb` when the surface is sRGB). In `prepare` it calls `queue.write_texture`
  with the frame's pixels, and only when the generation differs from the last one uploaded.
  The texture is recreated only when the frame size changes.
- **Drawing it:** `draw` is one 4-vertex strip inside iced's own render pass.
- **Per-frame cost:** one ≈3.7 MB copy into the wgpu staging belt at 720p, plus one `view`
  rebuild and draw of the watch page (iced can't redraw from outside without a message).
  Nothing grows: there is no atlas entry and no new image per frame.
- **Sizing:** `set_target_size` gets the fitted video rectangle in physical pixels, recomputed
  in `view`.
- **Leaving and returning.** The decoder is parked while visiting a non-Watch page. Back to
  that Watch page resumes the same decoder and preserves the pause state. Switching between
  different videos uses the single decoder and core's `load_at` from the stored position.
  A bounded four-frame cache and cached posters cover the transition. Fullscreen uses a separate
  scroll model, so it cannot clamp the saved Watch offset.
- **GPU path.** On Wayland the window is transparent and `surface.rs` paints the opaque page
  around a hole for core's EGL child. Each root draw commits the child's position, including
  scroller-only draws. A child whose full rectangle would cross the parent window edge is
  hidden until it fits, since a Wayland subsurface is not clipped by its parent. The player
  shows its poster in that case. `NT_SOFTWARE_VIDEO=1` forces the software path. A separate,
  muted preview player and shader pipeline are reused across card hovers.

## Frame hook

- `metrics::frame()` is called from `FrameHook::draw`, the root widget.
- iced_winit calls `UserInterface::draw` exactly once per `RedrawRequested`, immediately before
  `compositor.present`. There is no later hook short of patching iced.
- Pending marks are set when their data lands in `update`, and emitted from the same `draw`,
  i.e. in the first frame that shows the data.

## What was hard / easy, workarounds

**Easy:**
- iced's redraw loop is a good fit. `RedrawRequested` reaches widgets with a timestamp before
  draw, a widget can request exactly the next frame, and an idle app draws nothing.
- A custom widget that translates its children was straightforward once the scrollable source
  was read.
- `shader` made in-place video upload possible.

**Hard, and the workarounds:**
- **No ellipsis in iced text.** `Clamp` shapes the text once and binary-searches the longest
  prefix that fits N lines with "…", caching the result per (text, width).
- **Layer order in iced_wgpu.** Each layer draws quads, then meshes, then primitives, then
  images, then text, so a badge quad would end up under its thumbnail. Badge backgrounds are
  stretched 1×1 images instead, which avoids one layer per card. The player controls sit in a
  `stack` layer above the video primitive.
- **Focus operations.** The basic widget facade omits `unfocus`, but the public
  `advanced::widget::operate` can run the core operation. `row!` drops a zero-width child
  before diffing, so growing a leading slot from zero moves the text input to a new tree index.
  The icon now sits above the field, with padding reserved only while focused. See the reports
  in `docs/upstream`.
- **Every external message rebuilds the whole `view`.** That is why the scroller avoids
  messages per frame and the player waker is coalesced.
- **GPU choice.** iced picks the high-performance adapter and probes every backend. That woke
  the NVIDIA dGPU and loaded GL/Mesa: about 254 MB RSS at startup. `main` now defaults
  `WGPU_POWER_PREF=low` and `WGPU_BACKEND=vulkan`, which are wgpu's own knobs and only set when
  unset. That gives the Intel iGPU and about 150 MB RSS at startup. The NVIDIA Vulkan ICD is
  still mapped by the Vulkan loader.
- **Fonts.** cosmic-text maps all system fonts (about 34 MB RSS here).
- **Thumbnails that aren't 16:9.** Subscription-feed thumbnails are 4:3 with bars, so all
  thumbnails are drawn cover-cropped.
- **`/` focuses the search field.** This is an addition to the spec (YouTube has the same
  shortcut). I needed it because the desktop shell's drawer layer swallows pointer input along
  the top and left edges of the headless output, so real clicks can't reach the top bar or the
  rail when the window is flush with those edges. The rail and the top-bar buttons were tested
  with the window floated away from the edges, and Back was also tested with Alt+Left and the
  mouse back button.

**Spec deviations:**
- At narrow widths (lane < 760 px) list thumbnails shrink below 360×202, to at least 160 px or
  45% of the lane, so the text column stays readable.
- The subscriptions "strip" wraps onto more lines instead of scrolling sideways, so it never
  steals the page's wheel.
- The rail label "Subscriptions" at 12 px is slightly wider than the 72 px rail.

## Historical core requests

1. **`autopilot::Step::Do(Command)` originally did not accept the scenario's `{ "do": { "cmd": … } }` form.**
   This round uses the existing core autopilot entry point.
2. **`WatchInfo.channel` can come back empty.** In one live run it had no name, id or avatar.
   The app fills the missing fields from the card that opened the video. Not re-verified live,
   to keep YouTube traffic down.
3. **`Video::thumbnail` is documented as always 16:9**, but `subscription_feed` items carry
   4:3 `hqdefault` images with bars.

## Stability and parity round

- A focused text field keeps the same tree slot. Tab traversal uses actual widget operations
  for pointer-only iced buttons, with an external ring on keyboard focus. The same control
  wrapper registers hit regions with the optional state probe. Draw-time tracing sees clipping
  and scroll transforms that a model-only probe would miss.
- Context menus clamp to window edges and consume dismissal events. Guide, settings, shortcuts,
  stats, and card tint use ordinary iced widgets and small wrappers. Separate layered draws are
  important because iced draws shader primitives after quads in the same layer.
- Search shows YouTube notices and mixed results, and transcript matches can start playback at
  their indexed timestamp. Channel tabs and playlists, comments and replies, scrub previews,
  caption and audio menus, local queue and hide lists use core's typed data and continuation jobs.
- Video preparation resolves on a card's press or dwell without starting the media, and prepares
  the first available queue, playlist, or related successor once the current playback is ready.
  Scrolling past a stationary pointer no longer queues repeated speculative loads.
- The probe writes on stderr only under `NT_PROBE=1`. It coalesces state every 100 ms and
  emits a one-second idle heartbeat. A watch frame counts when iced draws the shader or the
  Wayland child has presented and the parent paints the hole.
- Image requests use logical draw width times window scale, rounded up in 64-pixel buckets.
  Thumbnails are linearly sampled. The core chooses the actual source and owns decode limits.

### Measurements and real input

Five warm startup runs had median exec → window and exec → Home times of 379 and 393 ms
before the changes and 208 and 268 ms afterward. The new ranges were 191–239 and 240–306 ms.
See `/tmp/nt-iced-round/startup-summary.log`. Host load and scheduling priority differed.

Baseline live release → first draw was 801, 559, and 306 ms for `7QN49z7R8v0`, `H9-nB32_anM`,
and `svpyPbAdx7g`. Standalone `ntctl play` took 1290, 590, and 1660 ms for those IDs. The
first follow-up attempt on that feed failed playback. A later live feed offered `KePvp0-hcn0`,
`914ejh6CJ7s`, and `iqSX9pjTozc`. The in-app runner measured open → draw in 678, 187,
and 353 ms in one process. Fresh standalone `ntctl play` processes took 580, 600, and 730 ms
for the same three IDs. Different videos, render paths, and prewarm conditions limit the
comparison. The replayed autoplay check switched from `7QN49z7R8v0` to `Y3q-hkfB7Ok` and
rendered its first frame 347 ms after the countdown expired. The local clip makes this a
transition measurement. Live resolver savings from preparing the successor are unmeasured.

The same muted local 720p clip used 37.8% CPU in the software path and 15.5% in the Wayland GPU
path over six seconds each. PSS was 268.1 and 298.1 MiB, respectively. This is one sample per path
with the probe enabled. The figures measure the app process, including libmpv in that process.
The Wayland child can extend outside the parent. When scrolling would expose those pixels
outside iced's window, the app hides the child and draws the poster until the video fits again.
The software path supports the full scroll clip.

Real input for `rust programming` produced 18 results, appended to 30 at scroll offset 3840,
and returned from Watch to the same 30 results at offset 3840. The `pronsh` capture showed
YouTube's “Confirm your age” notice and seven transcript matches. See the run artifacts under
`/tmp/nt-iced-round`. A later interactive attempt did not acquire search focus. The latest
passive replay displayed the notice and seven matches. The Channel replay showed three sharp
top-row posters after the two missing image fixtures were populated. Core decoded those two
posters at 384 × 216 for 376 logical pixels of display width at 1×.

Before GUI input was disabled, the stress run observed 20 related opens across two fixture
videos and 35 delivered Back/Forward presses. Two clicks needed retry. Typical related
opens were observed in 275–345 ms, with one 4164 ms outlier. PSS rose from 274.1 to
532.8 MiB over the first six opens and ranged from 514.0 to 560.9 MiB afterward. This
run shows navigation under repeated transitions, with distinct-video coverage and flat
memory still unverified. A separate run scrubbed the paused player to 33.678 s and toggled
theatre and fullscreen with the GPU surface attached. The sleep settings screenshot covered
all choices. An End of video attempt through the menu still displayed Up next. The end path
itself is correct: with the test hook `NT_SLEEP_TIMER=end` (arms it at start) an autopilot
run reached `playback_ended` and `sleep_timer_fired` in the same millisecond, paused at
0:59, and showed no Up next card (`.../scratchpad/sleeptest/after-7s.png`). The failed attempt
most likely armed a different row: before the 36 px rows the Sleep page needed scrolling and
its last row, End of video, sat below the fold. Arming it from the menu is on the
needs-input list.

The released 0.14.2 row builder filters zero-width children before reconciling by position.
The search field therefore used to move into a fresh widget tree slot. The focus icon now
overlays its stable field. The public advanced focus operation clears focus on submit and
Escape. The independent repros and proposed upstream changes are in `shells/iced/repros` and
`docs/upstream`. No new direct dependencies were needed.

### Input-free checks (autopilot, probe and trace only)

- **Related opens, 25 live opens per run** (`NT_TRACE` events `open_video`, `player_ready`,
  `playback_started`): open → player Ready medians 616, 681 and 601 ms, with outliers of 5329
  and 2457 ms. Those fall between the open and core's Ready, which covers resolve, loadfile and
  demuxing, so they are core or network time. Ready → first presented frame, the part iced adds
  to drawing, had a median of 21–25 ms on the GPU path and 14 ms in software. The p90 was
  241–416 ms for both: mpv decodes its first picture after Ready. The GPU path had one real
  app-side gap. The waker filter only let through state or frame changes, so the moment the
  Wayland surface started presenting didn't wake the UI, and the hole was cut at the next
  unrelated redraw. `ext::wake` now also wakes on a presenting change, read with `try_lock` so
  the waker never waits for the UI thread.
- **Memory per feature** (`NT_TEST_CYCLE=theatre|fullscreen|stats|scrub|menus`, a test-only
  400 ms toggle loop on the watch page, one live video, 50 s, PSS each second): none 259→269,
  theatre 218→229, fullscreen 181→191, stats 215→225, scrub with storyboard previews and
  seeks every 1.6 s 196→207, menus 200→211 MiB. Every run drifts the same ~0.25 MiB/s, which is
  playback buffering. No feature leaks. The earlier interactive 515–560 MiB plateau came from
  20+ distinct watch pages in the stack (24 kept: comments, related, saved frames), the image
  cache (64 MiB of decoded pixels plus their GPU copies) and the hover-preview player. Over two
  further rounds of 20 related opens and 20 Back/Forward it stayed flat (RSS 610 → 615 MiB).
- **Rapid Back/Forward pause bug, fixed.** A page left before its new video had played
  recorded the previous video's pause, so it came back paused at 0:00. `leave_top` now
  records a pause only once this video was seen playing (`seen_playing`).
- **Hover tint, fixed.** The card tint only showed on the next unrelated redraw, because iced
  doesn't redraw on pointer motion unless a widget asks. `Control` now asks when the pointer
  enters or leaves.
- **Probe:** `rail_selected` now comes from the rail entries drawn in that frame. At startup
  the window can resize between `view` and `draw`, which gave the explorer's startup
  rail-selection mismatch.
- **Upstream:** `docs/upstream/iced-power-preference.md`. The bare shell maps its window in
  339 ms with 106 idle wake-ups/s and 120 MB PSS by default, against 199 ms, 3/s and 50 MB
  with the low-power adapter. iced master already defaults to `PowerPreference::None`.

- **Captions move with the controls.** mpv draws the captions into the picture, on both the
  software and the GPU path, so they are positioned with mpv's `sub-pos` through a new core
  setter, `Player::set_caption_position`. `AutoHide` reports when the controls show or hide.
  The app then eases the lift over 200 ms on `window::frames()`, sending only changed values.
  Hidden, the block rests about 8% above the picture's bottom. Shown, it clears the progress and
  control bars (76 px of the player box), less any letterbox under the picture in theatre and
  fullscreen. Checked live with autopilot snapshots on the GPU path in the default, theatre and
  fullscreen layouts, using the test hooks `NT_TEST_CC=1`, `NT_TEST_CYCLE=controls` and
  `NT_TEST_FULLSCREEN=1`.
- **Idle memory on Home (perf gate).** The GPU surface was attached as soon as the player
  warmed up, which loads EGL with both GL vendors (NVIDIA eglcore/glcore, Mesa gallium and
  LLVM). That came to about 38 MiB PSS before any video played. It now attaches on the first
  sign of playback: a hover dwell or press on a card (with the preload), or the open itself.
  In that last case the page opens once the attach is done (about 60 ms), and the streams
  resolve meanwhile through `prepare_next`. A/B on NT-ICED against 2caf0fa (fixtures, 2 s
  after Home, 3 runs each): idle PSS 136/136/138 → 152/151/152 MiB, down from 189. Click →
  first frame 92/95/85 → 148/176/142 ms when nothing was hovered first. Warm switch 44/37/47 →
  28/34/18 ms. What remains above the baseline is about 14 MiB of anonymous memory: posters
  decoded at the drawn width (384 px instead of mqdefault's 320) and the continue-watching
  bar. The libmpv warm-up stays (NT_NO_PREWARM=1 saves another 35 MiB but makes the first
  click pay for loading libmpv). Home → content rose from about 22 ms after the first frame to
  about 88 ms: core's `home_feed` asks for FEwhat_to_watch, which is empty logged out, and
  only then fetches the topic mix.
- **Review fixes (2026-11-20).**
  - A first comments page that has a cursor but no parsed comments used to make the next
    request reuse the watch page's cursor and replay page 1. `comments_started` now records
    that page 1 arrived, and later requests always use the returned cursor.
  - The GPU child used to be hidden as soon as the player crossed the top bar. It is now
    cropped instead: a new core call, `VideoSurface::set_clip`, sets the wp_viewport source and
    destination, so the visible part keeps playing while the page scrolls. Autopilot snapshots
    at offsets 120 and 360 show it (`.../scratchpad/crop/montage.png`).
  - Quit calls `library().flush()` after `stop`, so the final resume checkpoint is written
    before the process ends. In three autopilot runs it was saved at 6.0 s, before the first
    10 s periodic checkpoint.
