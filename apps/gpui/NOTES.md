# NativeTube: GPUI frontend notes

## Versions

- `gpui-pre` 0.3.12 (Zed's gpui, snapshot of zed@d2a4fb6), with `gpui-pre-platform` 0.3.12
  (`wayland` feature) for Linux. On macOS, the platform crate selects its native backend. The
  platform lives in its own crates now (`gpui-pre-platform` → `gpui-pre-linux` → `gpui-pre-wgpu`);
  `gpui-pre` alone has no backend.
- `gpui-component` 0.9.1 (GPUI Kit) and `gpui-kit-assets` 0.9.1 for the Lucide icons.
- `gpui-pre-linux` 0.3.12 is **vendored with a ~30-line patch** (`vendor/gpui-pre-linux`, see
  "Workarounds").
- About 3,000 lines of app code in `src/`.

## Architecture

- **One root view, `NtApp` (`app.rs`).** It holds the navigation stack (`Vec<Page>`), the search
  `InputState` and suggestion state, the image cache, the video state, the volume
  `SliderState`, and scrollbar and seek-drag state. gpui-component's `Root` wraps it, because
  the kit's inputs need it.
- **Pages (`pages.rs`).** Each `Page` has a `Kind` (Home, Search, Watch, Channel, Subscriptions,
  History, WatchLater), a load `status`, a `Feed`, its own `ScrollAnimator` and a cached
  `Layout`. Pages stay in the stack, so Back restores items and offset without a refetch.
  `req` counts (re)loads so stale answers are dropped, and `version` invalidates the layout.
- **Actions** are plain methods on `NtApp` (`push`, `back`, `replace_root`, `open_video`,
  `submit_search`, `load_more`, `load_comments`, …), called from element listeners
  (`cx.listener`) and from the autopilot. The autopilot is a `cx.spawn_in(window, …)` loop over
  `autopilot::next_command()`, and every command calls the same method a real input reaches.
- **Async results.** `nt_core` `Job`s are awaited in `cx.spawn` tasks on GPUI's foreground
  executor, which then `this.update(...)` the page if it is still there with the same `req`, and
  `cx.notify()`. Search suggestions are debounced with a 150 ms GPUI timer plus a sequence
  number, so stale answers are dropped. The image BGRA swizzle and `RenderImage` construction
  run on GPUI's background executor.
- **Views (`views.rs`)** are free functions that build elements for the top bar, rail, chips, the
  scroller and each block kind. From the kit: `Input` (search), `Button` (back, search, Retry,
  Subscribe, Watch Later, Clear history, play/pause, mute), `Slider` (volume), `Spinner`,
  `Skeleton` (first-load cards), `Icon`. Everything else is GPUI `div`s, `img` and `canvas`.

## Scrolling

- **Raw input.** The page scroller is a plain `div` with `.on_scroll(...)`. No GPUI
  `ScrollHandle`, `overflow_scroll` or kit scroll area is used, so GPUI's own scrolling never
  runs. GPUI's Wayland backend reports a wheel notch (`axis_value120`) as `ScrollDelta::Lines`
  of `value120/120 × 3 × −1`, so `ticks = −lines.y / 3` → `wheel_ticks`. Finger axis events come
  as `ScrollDelta::Pixels` of `value × 3 × −1`, so `px = −pixels.y × 4` reproduces Chrome's ×12
  → `wheel_pixels`. The finger lift arrives as a zero `Pixels` delta with `TouchPhase::Ended`
  (from the vendored patch) → `fling_start`. A mouse-down on the feed calls `cancel_fling`.
  Keys (Up/Down/PageUp/PageDown/Space/Shift+Space/Home/End, when the search box isn't
  focused and the watch-page keys don't apply) call `scroll_by` with the module's step
  helpers; any other key cancels a fling.
- **Scrollbar.** A thin thumb shows while scrolling and hides about 1.3 s after motion stops. It
  is draggable (root `on_mouse_move` → `jump_to`), and a track click pages toward the pointer.
- **Present.** `render()` calls `tick(now)` once per drawn frame and positions blocks at
  `y − offset`, with the offset snapped to physical pixels. It calls
  `window.request_animation_frame()` only while `needs_frame()` is true, so an idle app draws
  nothing (idle phases: 0 fps, 0.0–0.2% CPU). The clock is `nt_core::metrics::now()`, used for
  both events and frames. GPUI doesn't expose the Wayland frame-callback timestamp to views, so
  the frame time is the time `render()` runs, which is right after GPUI handles the frame
  callback.
- **Virtualization.** Each page's content is a list of absolutely positioned `Block`s with known
  heights (`pages.rs::build_layout`). Grid rows and list rows have fixed heights. Titles, the
  description and comments are measured once with `WindowTextSystem::shape_text` at the
  width they're drawn, and comment heights are cached per width. Blocks are sorted per column,
  so the visible ones come from a binary search. The scroller builds only blocks within
  half a viewport above and below the screen (one viewport of overscan in total). Images are
  requested for blocks up to a full viewport beyond the screen without building them. The
  layout is rebuilt only when the data or window geometry changes, and `set_max_offset` and
  `set_viewport` are updated then. Infinite scroll and the comments trigger compare
  `target()`, not `offset()`, against the end of the list.
- **Cost.** Per scroll frame on Home: building elements ≈ 0.4–0.7 ms, then GPUI's own
  layout (Taffy) ≈ 2–3 ms and prepaint plus paint ≈ 1 ms. Measured with temporary probes;
  profiled with `perf record -e cpu-clock:u`, the cost is spread across Taffy, style refinement
  and text layout, with no hotspot in app code. Halving the overscan cut scroll-phase CPU from
  about 33% to 24%.

## Video

- One `nt_core::Player`, created lazily, with `PixelFormat::Bgra`, because GPUI's `RenderImage`
  is BGRA. The waker does `try_send` on a bounded(1) `async_channel`, so bursts collapse. A
  foreground task drains it and calls `VideoState::poll`, which notifies the view only if
  `frame_generation()` changed or a displayed part of the state changed (status, paused,
  buffering, mute, volume, duration, position in quarter-seconds, buffered seconds). A paused
  video draws nothing (paused phase: 0.7 fps, 0.7–1.2% CPU, from the clock text).
- **How frames reach the screen.** On the next `render()`, if the generation changed, the newest
  `VideoFrame` is copied once (`pixels().to_vec()`, about 1.2 MB at 736×414, about 0.2 ms)
  into an `image::RgbaImage` (holding BGRA bytes) and wrapped in a new `Arc<RenderImage>`.
  The previous image's atlas tile is freed right away with `Window::drop_image`, then the new
  image is drawn with `img(...).object_fit(Contain)`. GPUI uploads it into its wgpu sprite
  atlas (`queue.write_texture`) when it is first painted. Exactly one video tile is alive at a
  time, and since the old tile is freed first, the new frame usually reuses its slot. GPUI has
  no API to update a texture in place (on Linux there is no `paint_surface`), so each frame
  costs one CPU copy and one upload.
- `set_target_size` is called with the player box in physical pixels whenever it changes.
- Leaving a Watch page (pushing a page over it, Back, or the rail) calls `Player::stop` and
  stores the position. Coming back with Back calls `load` again and seeks to the stored
  position once the status is `Ready`.
- When `NT_MUTE` is set, nothing in the UI can unmute: M, volume keys and the slider keep
  `mute` on.

## Frame hook

`metrics::frame()` runs in the paint callback of a zero-size `canvas` that is the root view's
last child. It runs once per frame GPUI actually draws, during the paint phase, right before
`window.present()`. Pending marks (set when data arrives, or when a new video picture is
uploaded for `playback_started`) are moved into that closure, so they're emitted in the paint
of the frame that first shows the content. `first_frame` is marked in the first painted frame,
which already has the top bar and rail.

## What was hard / easy, and every workaround

- **Easy:** declarative layout and styling, `cx.listener`, `cx.spawn` with `Job` futures
  (they're runtime-agnostic), `request_animation_frame`, and `canvas` for the frame hook. GPUI
  never draws when nothing is dirty, which makes the idle requirement free.
- **Finger lift is dropped by GPUI.** Upstream `gpui_linux` ignores `wl_pointer.axis_stop`, so
  there was no way to start a Chromium fling. `vendor/gpui-pre-linux` is the crates.io 0.3.12
  source plus a ~30-line patch (marked `nt-gpui patch`). It reports a finger or continuous
  `axis_stop` as a zero-delta precise `ScrollEvent` with `TouchPhase::Ended`, wired in with
  `[patch.crates-io]`.
- **Inactive windows are throttled to 30 fps** by GPUI's default `inactive_frame_interval`.
  The test window is never focused, so `inactive_frame_interval` and
  `low_power_frame_interval` are set to `None`.
- **Kit custom button colours.** gpui-component draws a `ButtonCustomVariant` background as the
  colour mixed 80% toward transparent, so buttons also get an explicit `.bg()` with the spec
  token.
- **Focus.** A `track_focus` on the root div took focus back from the search input on every
  click. The focus handle now lives on the scroller, and key handling stays on the root.
- **Shortcuts added:** `/` and Ctrl+L focus the search box. They're also how the search box was
  tested, since pointer clicks near the output edges don't reach the app (see "Testing").
- **Search dropdown and focus.** A window losing or regaining activation blurs or refocuses the
  input. Blur closes the dropdown only while the window is active, and focus doesn't reopen it.
  Suggestions come back on the next keystroke.
- **Fling after wheel samples.** Hyprland's virtual pointer (used by `nt-input touchpad`) sends
  the "touchpad" frames as wheel-source `value120` and only the final `axis_stop` as finger. A
  fling therefore starts only if the last scroll input was a precise (pixel) delta, which
  matches Chrome, where `axis_stop` only follows finger frames. With `nt-input touchpad` this
  path behaves as fractional wheel notches; the precise path is exercised by the autopilot's
  `ScrollPixels` and `Fling`.
- **Channel name.** Some watch responses (multi-artist videos) have an empty channel name, so
  the card's channel name is shown instead.
- The rail label "Subscriptions" at 12 px (the smallest spec size) is wider than the 72 px rail
  and overflows it slightly.

## Testing caveats

- The desktop shell (caelestia) has a full-output top-layer surface (`caelestia-drawers`) on
  every output, including NT-GPUI, and its input region covers bands near the output edges.
  `nt-input` clicks at the top bar (y < ~60), the rail (x < ~90) and the right edge never
  reached the app (no `wl_pointer` events at all). Those controls were checked with the window
  floated and resized to 760×600 in the middle of the output
  (`hyprctl dispatch 'hl.dsp.window.float/resize/move(...)'`, which also tested the narrow
  layout), and by keyboard. This is the environment, not the app.
- In screenshots the wallpaper shows faintly (~4%) through the window background, although
  Hyprland reports opacity 1 and GPUI configures an opaque surface. Not explained; not
  investigated further.

## Memory

Peak RSS in the benchmark is about 460 MB; about 236 MB is already there at startup. Of that,
about 90 MB is GPU driver libraries (`libnvidia-*` ≈ 58 MB, `libLLVM` 16 MB, Mesa
`libgallium` 8.5 MB, `libvulkan_intel` 5 MB). They're loaded because gpui-wgpu enumerates
adapters on all backends (`Backends::all()`), which initialises every installed driver
(Intel and NVIDIA, Vulkan and GL). The binary itself is about 32 MB resident. Images add the
core's decoded cache (≤ 96 MB) plus this app's `RenderImage`s, which keep their pixels on the
CPU; they're capped at 140 ready images (about 32 MB) and evicted least-recently-drawn first,
freeing their atlas tiles. Playback adds mpv's decoder and demuxer cache.

## Not done / deviations

- Related videos load more on scroll (`more_videos`), but in the wide layout the trigger is the
  related column's end, which is normally above the primary column's end.
- Frame timestamps are taken when `render()` runs, not from `wl_surface.frame` (GPUI doesn't
  expose it to views).

## Core requests

1. **`bench/scenario.json` doesn't parse with `autopilot::Step`.** `Step::Do(Command)` is an
   untagged newtype variant, so it accepts the command object itself (`{"cmd": "fling", …}`),
   but the scenario writes single commands as `{ "do": { "cmd": … } }`, which matches no
   variant. As shipped, `autopilot::start()` prints "data did not match any variant of untagged
   enum Step" and runs nothing. Workaround in `main.rs::fix_scenario`: only if the file fails to
   parse, write a copy to `$TMPDIR` with each `{ "do": X }` step replaced by `X` and point
   `NT_AUTOPILOT` at it. Suggested fix: `Do { #[serde(rename = "do")] command: Command }`.
2. It would help if `ScrollAnimator`'s Wayland note mentioned that GPUI (and possibly other
   toolkits) drops `axis_stop`.
