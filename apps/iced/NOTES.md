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
  Each `Page` owns its body (Home, Search, Watch, Channel, Subs, History, Later) and its own
  `ScrollModel`, so a page under the top keeps its items and offset untouched. Back pops the top
  page with no refetch.
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

- There is one `Player` for the app's lifetime, created on the first video, with `PixelFormat::Rgba`.
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
- **Leaving and returning:** leaving a watch page stores the position and calls `stop()`.
  Back reloads the video and seeks to the stored position once the player reports `Ready`.
  Checked: 8 s of playback, then channel, then Back showed 0:10 when paused.

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
- **No unfocus operation in iced 0.14.** `SearchBox` unfocuses the input during `diff` when a
  "blur" token changes after a search is submitted.
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

## Core requests

1. **`autopilot::Step::Do(Command)` doesn't accept the scenario's `{ "do": { "cmd": … } }` form.**
   Serde reads the untagged newtype as the bare command object, so `bench/scenario.json` fails to
   parse at its first standalone `do` and no scenario runs. `main.rs::normalize_scenario`
   unwraps those steps into a temp copy and repoints `NT_AUTOPILOT` before
   `autopilot::start()`. It does nothing if the file already parses.
2. **`WatchInfo.channel` can come back empty.** In one live run it had no name, id or avatar.
   The app fills the missing fields from the card that opened the video. Not re-verified live,
   to keep YouTube traffic down.
3. **`Video::thumbnail` is documented as always 16:9**, but `subscription_feed` items carry
   4:3 `hqdefault` images with bars.
