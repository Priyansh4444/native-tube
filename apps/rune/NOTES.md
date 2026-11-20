# NativeTube on Rune: notes

## Versions

- Rune `rune` 0.0.1 and `rune-kit` 0.0.1, both by git from github.com/anomalyco/rune at rev
  `4fbd2d6ac4dd12595dd8ed0b7849b28a309c7c00`, with Rune's `[patch.crates-io]` applied as its
  README asks (`winit` 0.30.13 and `wgpu-hal` 30.0.1, the copies vendored in that repo at that rev).
- wgpu 30 on Vulkan (Intel Arc iGPU here), winit 0.30.13 (Wayland), toolchain 1.99.1. On macOS,
  Rune selects its native Metal backend.
- rune-kit pieces used: `TextInput` (search field), `Button` (Subscribe, Watch Later, Retry,
  Clear history), `Spinner`, `Kit::from_palette` (themed from the spec's tokens).
- About 2,500 lines of app code in `src/`.

## Architecture

- `main.rs`: `metrics::init`, `autopilot::start`, `rune::set_app_id("nt-rune")`, an `AppSpec`
  (title, 1280x800, min 640x480, `init`, `view`, `poll`, `on_key`) run through `rune::App`.
- State (`app.rs`): one `App` struct: the navigation stack `Vec<Page>`, the search `TextEdit`
  and dropdown state, the viewport size, the image cache and the video state. Rune's model is
  "handlers get `&mut App`, then the view re-runs"; there are no message enums.
- Pages (`pages.rs`): `Page { kind, feed, scroll, version, plan }`. `Kind` is Home(topic),
  Search(query), Watch(box), Channel, Subs, History, Later. Every page keeps its data and its
  `Scroller`, so Back pops to exactly the same items and offset, with no refetch.
- Async: core `Job`s finish on core's workers; `on_result` wraps `Job::on_done` and hands the
  result to `AppHandle::update` (Rune runs it on the UI thread, then commits). Results find
  their page by id, so an answer for a page that was popped is dropped. Suggestions carry a
  sequence number and stale answers are dropped. The debounce is a due time plus `Tx::wake_after`.
- Views (`view.rs`): top bar, rail, the scroller viewport, the dropdown, and one builder per
  block kind (card, list row, compact row, chips, status, banner, channel header, strip,
  player, watch info, description, comments). `theme.rs` holds the tokens, the text styles and
  the app's own SVG icons. `video.rs` handles playback.
- Autopilot: `autopilot::set_waker` calls `AppHandle::update_if(drain)`. Commands go through
  the same methods as real input (`wheel_ticks`, `wheel_pixels`, `submit_search`,
  `open_video`, `back`, ...). Scroll commands don't commit the view.
- Marks: data handlers call `mark_next(name)` when content lands. The frame hook emits pending
  marks after the next drawn frame, which is the frame that shows that content.

## Scrolling

- Rune has its own scroll views (`.scroll()`, with 1:1 wheel and decay fling). The app uses
  none of them for pages. The root element's `on_wheel` takes every wheel and touchpad event
  (it returns true), so Rune's scroll physics never sees them.
- Raw events: Rune's `WheelEvent` keeps winit's distinction. A notch is `LineDelta(±1)`,
  which Rune reports as `dy = ±40`, `precise = false`; the app calls `wheel_ticks(-dy / 40)`.
  A touchpad gives `PixelDelta`, which winit's Wayland backend fills with the raw axis value
  (logical, negated). The app calls `wheel_pixels(wayland_axis_to_px(-dy))`, which is Chrome's
  ×12; `NT_TOUCHPAD_FACTOR` multiplies on top. Finger lift is `GesturePhase::Ended`, which calls
  `fling_start`. Timestamps are `Tx::input_time()` (when the window took the event from winit)
  on the app's own monotonic clock.
- Keys go through `AppSpec::on_key` (only when the search field doesn't have focus):
  arrows, PageUp/PageDown, Space/Shift+Space, Home/End call `scroll_by` with the module's
  steps. A pointer press on the viewport calls `cancel_fling`. Scrollbar thumb drag calls
  `jump_to`. ScrollToTop calls `jump_to(0)`.
- Presenting: `poll` (Rune calls it at the start of every frame) ticks the animator once with
  the frame time and writes `[0, -offset]` into a `rune::Live<[f32; 2]>` bound to the content
  element with `live_offset`. Rune draws a live value without running the view, diffing or
  laying out. Input handlers feed the animator, then re-evaluate it at the same frame time,
  so the frame that handles the input already moves (P13). `Tx::live_only()` keeps those
  handlers from re-running the view. Frames are requested with `wake_after(0)` only while
  `needs_frame()`; idle draws nothing (0.0 fps, ~0% CPU in both idle phases).
- Virtualization: each page has a plan, which is every block of content as an absolute rect.
  Sizes come from the width, fixed row metrics, and `rune::text::measure` for wrapped text
  (watch title, description, comments; comment heights are cached per width). The plan is
  recomputed only when the width, height or data version changes. The view builds only the
  blocks in the mounted range: the viewport plus at least one viewport above and below,
  quantised to half-viewport "bands". The view re-runs only when the offset crosses a band.
  A commit then costs about 1–2 ms (5–13 ms the first time new titles are shaped), and finding
  the visible blocks is a binary search.
- Scrollbar: thumb is a `live_rect` and opacity a `live_opacity` with a spring. It shows while
  scrolling and on hover over its track, then fades. It is never fully transparent (0.4%),
  because in testing a drag on the track at opacity 0 never reached the handler.
- `set_max_offset` and `set_viewport` are called every poll from the plan's height and the
  viewport's laid-out size (`on_layout`).

## Video

- One `nt_core::Player` (`PixelFormat::Rgba`: `rune::Pixels` takes straight-alpha RGBA), made
  on the first video.
- How a frame reaches the screen: mpv renders into core's pooled buffer on the player's render
  thread, which calls the app's waker on that thread. If `frame_generation()` changed, the waker
  copies the frame into one `rune::Pixels` with `Pixels::set` (one 3.7 MB memcpy at 720p, off
  the UI thread). `set` wakes only windows whose scene shows that image. On the next frame Rune
  copies it into the image's standalone page (same size, so the same slot) and uploads it to
  the GPU texture. The video draws with `pixels_fit(.., SurfaceFit::Fit)` in a black box. No
  view commit happens per video frame. Cost measured: about +8 MB and ~9% of a core at 720p30
  for the copy/upload, compared with not uploading.
- Seek-bar fill, buffered-ahead and the clock are `Live`/`LiveText` values, set by a
  coalesced `update_if` the waker queues. The widths are rounded to whole points, so an
  unchanged width doesn't wake a frame. Only discrete changes (paused, loading, buffering,
  mute, volume, error, first picture) re-run the view.
- `set_target_size` gets the drawn box in physical pixels. Leaving a Watch page calls `stop()`
  and remembers the position; Back reloads and seeks there once the player is `Ready`.
- In test runs (`NT_MUTE=1`) the app can mute but never unmute: M and the volume controls
  can't make sound there.

## Frame hook

`App::on_frame_encoded`, counting frames whose `FrameOutcome` is `Drawn` (encoded, submitted
and presented). It runs on the UI thread after each window frame and is the closest
presentation hook Rune offers on Linux; it also emits pending marks.

## What was hard / easy / workarounds

- Easy: live values (`Live`, `LiveText`, `Pixels`) are the right shape for a scroller you
  drive yourself and for video. Scrolling and playback cost no view runs. `AppHandle::update`
  is a clean way back from core's threads. `rune::text::measure` gives exact wrapped heights
  for virtualization. rune-kit's TextInput (caret, selection, IME) was a drop-in.
- Hard: Rune's documented scroll and virtualization (`.scroll`, `virtual_rows`) own their own
  physics, so the app reimplements windowing on absolute blocks. The `poll` timing (before
  that frame's input handlers) needed the "re-evaluate the animator in the handler" trick to
  keep first-frame response.
- Workarounds:
  - Kit buttons are 24–36 pt with small radii; the kit is re-themed: `radius.xl = 18`,
    `bg_button_neutral = #272727`, no shadows, and a red-accent copy for Subscribe.
  - The search field uses a kit copy with `bg_base = #272727` and radius 20.
  - Card hover uses Rune's `.hover(color)` overlay (no view commit while scrolling under a
    still pointer).
  - The mouse back button uses `.pointer_buttons([Back])` and `on_press_tx` on the root.
  - The app tracks mute itself (see Core requests).
- Not done or unverified:
  - Card hover can't be checked in a screenshot: nt-input moves the pointer back after
    each action.
  - Wheel over the top bar or rail also scrolls the page (the handler is on the root),
    which differs from a browser.
  - A click immediately after nt-input warps the cursor in was occasionally hit-tested at the
    previous pointer position (seen twice). I believe this is a test-tooling artifact; not
    investigated further.
  - The touchpad gain (×12, as docs/scroll.md describes Chrome) was only exercised with
    nt-input's synthetic swipe, not a real touchpad.
  - `open_video` shows one slow frame (p95 33–138 ms across runs). Plans (<4 ms) and
    `Player::new` (~10–17 ms) are ruled out; the rest is unattributed (likely mpv/VA-API
    start-up competing for the GPU).
  - Pausing draws a sub-second burst of frames while Rune's transitions for the controls run,
    then the window goes idle.

## Benchmark (final run, flattened scenario; see core request 1)

`shots/report.json`, `failed_waits: []`, all eight snapshots in `shots/`. Scroll phases run at
the display's 60 Hz while moving. Phase averages include the scenario's sleeps, so they read
below 60.

| phase | fps | cpu % | frame p95 ms | long frames | RSS peak MB |
|---|---|---|---|---|---|
| startup | 2.1 | 16.7 | 16.9 | 0 | 181.7 |
| idle_home | 0.0 | 0.2 | 0.0 | 0 | 181.7 |
| wheel_scroll_home | 45.9 | 11.9 | 19.1 | 7 | 216.7 |
| fling_home | 45.8 | 9.4 | 17.9 | 2 | 216.9 |
| touchpad_scroll_home | 42.4 | 11.1 | 17.7 | 2 | 217.9 |
| infinite_scroll | 53.1 | 26.5 | 22.5 | 7 | 335.1 |
| search | 39.7 | 9.4 | 17.8 | 0 | 332.4 |
| open_video | 45.0 | 234.1 | 28.2 | 1 | 391.3 |
| playback | 32.2 | 34.0 | 34.4 | 280 | 406.9 |
| scroll_while_playing | 54.3 | 37.6 | 33.2 | 26 | 407.3 |
| paused | 11.5 | 3.0 | 17.2 | 0 | 407.3 |
| channel | 39.8 | 11.5 | 19.6 | 1 | 407.3 |
| back_navigation | 40.1 | 22.2 | 25.2 | 7 | 419.9 |
| idle_end | 0.0 | 0.2 | 0.0 | 0 | 385.9 |

Playback draws only when a new picture arrives (30 fps clip), so its gaps are 33 ms and the
report counts them as "long". Most of the RSS growth during deep scroll is core's 96 MB decoded
image budget; opening a video adds about 70–80 MB (mpv). The machine was shared with the other
two apps' builds and runs, so frame numbers vary by a few long frames between runs.

## Core requests

1. `bench/scenario.json` does not parse: `NT_AUTOPILOT: can't read bench/scenario.json: data
   did not match any variant of untagged enum Step at line 19 column 2`. Steps written
   `{ "do": { "cmd": ... } }` match no `Step` variant: `Do(Command)` is untagged, so it
   expects the command's fields at the top level (`{ "cmd": "fling", ... }`). Either give
   `Do` a `#[serde(rename = "do")]` field or flatten the file. Until then the benchmark was
   run with `shots/scenario-flat.json`: the same steps with `{"do": X}` replaced by `X`.
2. `Player::stop` resets `PlaybackState::muted` to false while mpv stays muted, so after the
   first video the state reports "unmuted" wrongly. A toggle based on it would unmute. The app
   keeps its own mute flag (initialised from `NT_MUTE`, as core initialises mpv's).
3. (Nice to have) `Player::frame()` gives an `Arc<VideoFrame>`; Rune's `Pixels::set` wants an
   owned `Vec<u8>`, so each frame costs one extra copy.

## November 19 parity and stability round

The current app keeps the page plan in an `Rc` snapshot. It releases the plan's
`RefCell` before computing or rendering. Core completions use Rune's queued
`AppHandle::update`, with a page/request token that follows the page into Forward.
Back/Forward restore its loaded lists, query and exact scroll offset. Watch restores
the saved clock and pause state through `load_at`. The three most recently left Watch
pages can show their own cached picture while the single main player reloads, with
the page's own poster as the fallback for older entries.

Recommendations now activate the resolved row directly. The old velocity guard
treated both touchpad flings and ordinary wheel easing as a reason to suppress a
press, but the operation it called only cancelled flings. Leaving the page already
freezes its scroll position. The Wayland input trace also recorded native misses
where the event hit the viewport or never completed a row press. On one failed click,
the pointer entered the correct row, then moved and left before button release.
Playback commands and pixel uploads carry a per-load serial, so a prior decoder
callback cannot overwrite a new page's picture.

Rune's default enter fades compounded across the control bar and its children.
The player commits hover changes immediately, refreshes its hide deadline on motion
and uses Rune's keyboard-only rings and modal focus for menus. The card tint and
overflow action use a hover group. The delayed hover preview reuses one silent
software player with a separate `Pixels` buffer per hover. Captions, chapter title
and segments, replay heatmap, storyboard seek preview, audio options, Stable volume,
Loop and Sleep are wired to core's state. Storyboard jobs are keyed by frame and drop
results after the pointer leaves. To display Stats and chapter labels, reserve
explicit layout boxes for `LiveText`. Wrapped live text panics in Rune's release
commit path.
[Wrapped LiveText repro](../../docs/upstream/rune-wrapped-livetext.md)

Mixed search results retain their typed order and continuation. Search masks transcript
highlights with UTF-8-safe ranges and opens a match at its core timestamp. Channel tabs,
sort and About use their own cursors. Playlist cards lead to a Watch page with a panel;
adjacent rows and the local queue feed the same Next and preparation path. Home uses
server chips when supplied and fixed topics otherwise. The lists prefetch ahead of
the viewport and keep loaded rows alongside Retry. Each Retry button targets its own
list, including when related and comments fail simultaneously. A window close saves
the playback checkpoint before the command thread can exit. Home offers a Continue watching card
with the title and saved time. One background writer saves Guide, theatre and autoplay
preferences in order when toggles arrive close together.

### Wayland video

Rune exposes swapchain transparency through `WindowOptions::transparent` and borrows
the raw `wl_surface` through `WindowHandle::with_raw_handle`. At the pinned revision
there is no public raw-display callback. The frontend uses the existing `libloading`
crate to find Wayland 1.23's `wl_proxy_get_display` at runtime and get the matching
display from that same surface. Older Wayland clients, X11, macOS, explicit
`NT_RUNE_SOFTWARE=1` and native attach failures use `Pixels`. The window keeps the
lease on its event thread and detaches in the close and quit hooks. Four live opaque
rectangles frame the clipped hole. The child uses the same crop through `set_clip`
and hides when it scrolls fully out of view. Wayland commits the child's position with
the parent, while EGL presents the video independently. Rune's ordinary controls still
draw above it. The GPU hole is rectangular.

Switching videos on the GPU path (fixed 2026-11-19, `switch5` harness, live, muted):

- **Stall.** The hole only opened from `poll`, i.e. on a Rune frame. The GPU picture never
  wakes Rune, and core's "first frame presented" wake reached only `player_changed`, which
  didn't look at the surface. When no other change followed, the poster stayed up and
  `playback_started` never fired (2 of 5 and 1 of 5 waits timed out in two baseline runs).
  `player_changed` now runs `NativeVideo::update` too.
- **Black hole.** `is_presenting` stays true from the previous video until the worker
  unmaps, and core maps the child with one render before mpv has decoded anything. Traced:
  Ready at position 0.00 with that single black render held for 2.6 s. The hole now opens
  only once the requested load has reached the player and `Player::presented_frames()`
  counts a decoded frame presented after mpv's playback restart. The count resets
  on each load and stop and covers paused loads too. Until then the poster shows with
  the spinner (`App::gpu_pending`).
- Result: 6 of 6 consecutive runs without a failed wait, plus a 7th. The pure-black
  (max channel 0) +0.15 s snapshots that remain are the videos' own openings: `ntctl play`
  renders the same ids black at 0.3 s (`R_8UPhJNrZ4`) and 2 s (`kF6DLUiANMk`).
- `NT_GPU_TRACE=1` logs time, frames, position and status on every update.

Eight-second muted 720p clip, same 1280×800 off-screen window and same release build:

| path | CPU seconds | one CPU core | end RSS |
|---|---:|---:|---:|
| Wayland child | 0.74 | 9.2% | 347 MiB |
| Software Pixels | 2.71 | 33.9% | 283 MiB |

The probe uses Rune's after-draw inspection for clipped target rectangles and the
first-frame marker. It samples the playhead even while a native child advances without
repainting its parent. It emits stable page IDs as rail selections and stays off by
default. Static images request the drawn width in physical pixels. Rune's image shader
blends four neighboring texels with atlas-edge clamping.

### Reproduced and measured

- The local fixture run opened 20 distinct recommendations and sent 35 Back/Forward
  inputs in 1.29 s. Median click-to-frame was 580 ms and one click needed a retry.
  The strict retest confirmed five correct hits from ten attempts before a wheel
  input missed. Real input was disabled after the owner reported interference.
- `/proc/<pid>/smaps_rollup` sampled every 0.5 s in the strict retest. A warmed Watch
  page was 347.4–347.7 MiB RSS and 262.6–262.9 MiB PSS. Across switches, peaks were
  485.1 MiB RSS and 446.5 MiB PSS. The run ended before a post-switch steady phase,
  leaving the navigation memory plateau unverified.
- On the final release build, an off-screen Home launch with a seeded checkpoint
  showed the remembered Guide and a Continue watching card at 0:23. From seconds 11
  through 20, the idle process held at 153.5 MiB RSS and 143.4 MiB PSS. This sample
  covers idle Home.
- Real-input search for `rust programming` filled 16 items, paged to 27 and restored all
  27 at precisely 4,320 logical pixels after Back. Search for `pronsh` displayed the
  native age notice and seven transcript matches.
- The release smoke test opened Stats with all nine rows, closed it, selected Sleep
  until End of video and observed the pause at 60.0 s. It also opened the shortcut
  sheet and Watch guide. The GPU path was captured in Watch, theatre and fullscreen.
- The pinned Rune workspace's winit patch removes startup COM taskbar work on Windows.
  Its wgpu-hal patch supports direct DXGI adapter selection and defers FXC loading.
  The app repeats the pins because Cargo does not inherit a dependency's patches.
  The Windows startup changes were not exercised on this Linux host.
- Rune's process-wide `set_app_id(&'static str)` takes a runtime test ID via a single
  leaked string at startup. A toolkit-owned `OnceLock<String>` could retain that ID
  without requiring the caller to leak it.

### Memory over 60 opens (live, muted, autopilot `open60.json`, PSS each second)

| run | PSS @30 s | @120 s | @180 s | peak | end |
|---|---:|---:|---:|---:|---:|
| GPU path | 289 | 406 | 449 | 474 | 453 MiB |
| `NT_RUNE_SOFTWARE=1` | 283 | 394 | 455 | 481 | 428 MiB |
| GPU, app image map 400/64 MiB → 160/24 MiB | 300 | 430 | 473 | 489 | 470 MiB |

It is not a leak and not the surface path: both paths grow alike and level off after about
40 opens (back stack at its 40-page cap, caches at their budgets). `NT_MEM_TRACE=1` prints
the census every 5 s. Of the growth, core's decoded image cache accounts for 95 MiB (it
fills to its fixed 96 MiB budget as new thumbnails appear, as in GPUI and iced), the app's
`Pixels` copies about 46 MiB at the old bound (18 MiB now), pages/plans and the heap the
rest (heap 28 → 43 MiB, anonymous 144 → 274 MiB). Rune's PSS also carries ~160 MiB of mapped
libraries, about 50 MiB of it NVIDIA's EGL/GL libraries loaded for the GPU video surface.
Shrinking the app's image map is real (measured by the census) but inside run-to-run noise
of ±20 MiB at the plateau.
