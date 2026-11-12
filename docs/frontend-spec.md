# NativeTube frontend spec

Three apps implement this same spec, one per UI framework: **iced**, **GPUI**
and **Rune**. They are compared on how hard they were to build, how fast and
lean they run, and how good they feel, so they must be functionally identical
and visually as close as each framework allows. Everything that isn't UI
(YouTube data, images, the local library, video decoding, scroll physics,
benchmark hooks) lives in the shared crate `core/` (`nt-core`) and must be used,
not reimplemented.

## Layout of the repository

```
core/                 nt-core: the shared backend (read its public API: core/src/lib.rs and modules)
docs/scroll.md        how Chromium scrolls, and how a frontend must wire nt_core::scroll
docs/frontend-spec.md this file
bench/scenario.json   the scripted session the benchmark runs in every app
tools/headless.sh     run/screenshot/kill an app on its own off-screen display
tools/nt-input/       real mouse/touchpad/keyboard input into that display
apps/<fw>/            your app: its own Cargo workspace, binary `nt-<fw>`
```

`<fw>` is `iced`, `gpui` or `rune`.

## Ground rules

- Work only inside `apps/<fw>/`. Do not edit `core/`, `docs/`, `bench/`, `tools/`
  or another app. If `nt-core` has a bug or lacks something you need, work
  around it in your app and list it under "Core requests" in your NOTES.md.
- `apps/<fw>/Cargo.toml` is its own workspace (`[workspace]`), with its own
  `Cargo.lock` and `target/`, depending on `nt-core = { path = "../../core" }`.
  The binary is `nt-<fw>`. The toolchain is pinned by the repository's
  `rust-toolchain.toml` (1.99.1).
- Use the framework's own widgets and its standard component kit where it has
  one (see "Framework" in your task). Don't pull in a second UI framework or a
  webview.
- No `unwrap()`/`expect()` on anything that can fail at run time (network,
  decoding, playback). Errors show in the UI with a Retry.
- Never open a window on the user's real screen. Always launch through
  `tools/headless.sh run <fw> ...`, which places the window on the off-screen
  output `NT-<FW>` without taking focus. Kill it when you're done
  (`tools/headless.sh kill <fw>`).

## Window

- Title "NativeTube", initial size 1280x800, minimum 640x480, resizable.
- Wayland app_id **`nt-<fw>`** (the headless tooling finds the window by it).
- Dark theme only.

## Design tokens (use these exact values)

| Token | Value |
|---|---|
| background | `#0f0f0f` |
| surface (top bar, sidebar) | `#0f0f0f` |
| raised (chips, inputs, buttons) | `#272727` |
| hover | `#3f3f3f` |
| selected chip | bg `#f1f1f1`, text `#0f0f0f` |
| text | `#f1f1f1` |
| text secondary | `#aaaaaa` |
| divider | `#3f3f3f` |
| accent (progress, live badge, subscribe) | `#ff3b30` |
| badge background | `#000000` at 80% opacity |
| radii | thumbnails 12, chips 8, buttons 18 (pill), avatars round |
| fonts | the framework's default sans; sizes 12, 14, 16, 18, 20 |
| spacing | 4, 8, 12, 16, 24 |

Top bar height 56. Left navigation rail 72 wide (icon + small label per
entry). Content padding 24.

## Pages

A navigation stack. Every page keeps its data and scroll offset when another
page is pushed on top, so Back restores it exactly (same items, same offset,
no refetch). Back is: the back button in the top bar, Alt+Left, and the mouse
back button.

### Top bar (always visible)
- Back button (disabled at the root), app name "NativeTube" (text, no logo
  imitating YouTube's), a centered search field (max 640 wide) with a search
  button.
- Typing shows up to 10 suggestions in a dropdown under the field
  (`client().suggestions`, debounced 150 ms; stale answers are dropped).
  Up/Down moves through them, Enter or click searches, Escape closes.
- Submitting pushes a Search page.

### Navigation rail
Home, Subscriptions, History, Watch Later. Selecting one replaces the stack
with that root page.

### Home
- A row of topic chips from `nt_core::TOPICS` (the first is selected). Choosing
  one reloads the feed for that topic.
- A responsive grid of video cards: as many columns as fit with a card width
  between 300 and 420, column gap 16, row gap 32.
- Video card: 16:9 thumbnail (radius 12) with the duration badge bottom-right
  (or a red "LIVE" badge), then a row with the channel avatar (36, round, when
  known) and a text column: title (16, medium weight, at most 2 lines with
  ellipsis), channel name (14, secondary), "views • published" (14, secondary).
- Hover highlights the card; click opens the Watch page; clicking the avatar
  or channel name opens the Channel page.
- Infinite scroll: when the offset is within 1.5 viewport heights of the end
  and the page has a continuation, load the next page (`client().more_videos`)
  and append it. Only one request in flight. Show a spinner row while loading.
- Loading state on first load (skeleton cards or a spinner), and an error state
  with a Retry button.

### Search results
- A list: thumbnail 360x202 on the left, then title (18, 2 lines), "views •
  published" (12, secondary), channel avatar 24 + name (12, secondary), snippet
  (12, secondary, 2 lines). Infinite scroll as on Home.

### Watch
- Player (16:9, black, as wide as the primary column, never taller than 70% of
  the window) with controls on hover and while paused: play/pause, a seek bar
  (click and drag to seek; shows buffered-ahead), elapsed / total time, mute
  button and a volume slider. Click on the video toggles pause. A spinner while
  loading or buffering, and the error message if playback fails.
- Keys while the watch page is up and no text field is focused: Space or K
  play/pause, J/L seek -10/+10 s, Left/Right seek -5/+5 s, M mute, Up/Down
  volume ±5.
- Below the player: title (20), then a row with the channel avatar (40), name,
  subscriber count, a Subscribe / Subscribed toggle
  (`library().toggle_subscription`), and on the right the like count and a
  Watch Later / Saved toggle (`library().toggle_watch_later`).
- A description box (raised, radius 12): views and date in bold, then the
  description clamped to 3 lines with "Show more" / "Show less".
- Comments: "N Comments" header, then comments (avatar 40, "@author •
  published", text, likes and reply count). The first page loads when the user
  scrolls near the comments; later pages load by infinite scroll.
- Related videos: a column 400 wide to the right of the primary column when the
  window is at least 1000 wide; below the comments otherwise. Compact rows:
  thumbnail 168x94, title (14, 2 lines), channel and views (12, secondary).
  Clicking one replaces the current Watch page's video (pushes a new page).
- Opening a video records it in history (`library().record_watch`). Leaving the
  Watch page (Back or navigation) stops playback (`Player::stop`); returning to
  it with Back resumes the video from where it was.
- The whole page is one scroller (the player scrolls away), as in a browser.

### Channel
- Banner (full width, height 160, cropped to fill) when there is one, then
  avatar (80), name (20), handle, subscribers, video count, description (2
  lines), and a Subscribe toggle. Then a grid of uploads like Home with
  infinite scroll.

### Subscriptions
- If there are none: an empty state saying so.
- Otherwise a horizontal strip of subscribed channels (avatar + name, click to
  open the channel), then a grid of their newest uploads
  (`client().subscription_feed(library().subscriptions())`).

### History, Watch Later
- Lists of videos from the library (search-result style rows), newest first,
  with an empty state. History has a "Clear history" button.

## Images

- Load every thumbnail and avatar with `nt_core::images::image(url, width)`,
  passing the width it is drawn at in physical pixels. It returns straight
  RGBA (`RgbaImage::to_bgra` for toolkits that want BGRA).
- Only request images for items that are on screen or within one viewport of it.
- Show a raised-colour placeholder until the image arrives. Don't decode or
  resize images on the UI thread.
- Keep your own cache of uploaded textures/handles bounded (drop ones far
  off screen); the core already caches decoded pixels.

## Scrolling (the most important part)

Scrolling must feel like Chrome. Read `docs/scroll.md` and
`core/src/scroll.rs` first. In short:

- Every page's main scroller is driven by one `nt_core::scroll::ScrollAnimator`.
  The framework's own scroll physics must not run on top of it: take raw wheel
  and touchpad events, feed them to the animator, and lay out the content at
  the offset `tick(now)` returns, once per presented frame.
- Mouse wheel notches → `wheel_ticks`; precise touchpad deltas →
  `wheel_pixels`; the touchpad's finger-lift / axis-stop → `fling_start`;
  PageUp/PageDown/Space/Home/End/Up/Down (when no text field has focus and the
  watch-page keys don't apply) → `scroll_by` / `scroll_to` with the module's
  step helpers; a scrollbar (thin, appears while scrolling, draggable) →
  `jump_to`.
- Request frames only while `needs_frame()` is true. An idle app draws nothing
  and uses no CPU.
- Virtualize: build and lay out only rows within the viewport plus one
  viewport of overscan. Nothing per frame should scale with the number of
  loaded items.
- Clamp `set_max_offset` whenever content height or viewport changes.

## Video

- One `nt_core::Player` for the app's lifetime (create it lazily on the first
  video). Use `PixelFormat::Bgra` if your toolkit's images are BGRA, otherwise
  `Rgba`. The player's waker must make the UI redraw; redraw for video only
  when `frame_generation()` changes.
- Upload frames as efficiently as the framework allows (reuse one texture and
  update it in place if you can; if the framework only takes new images, drop
  the previous one so memory doesn't grow). Call `set_target_size` with the
  drawn size in physical pixels.
- Write down in NOTES.md exactly how frames get to the screen.

## Benchmark hooks (required)

- First line of `main`: `nt_core::metrics::init("<fw>")`, then
  `nt_core::autopilot::start()`.
- Call `nt_core::metrics::frame()` exactly once per frame the framework
  actually draws, as close to presentation as the framework lets you. Say in
  NOTES.md which hook you used.
- Emit `nt_core::metrics::mark` with the names in `nt_core::metrics::marks`
  when the frame that first shows that content is drawn (set a pending mark
  when the data arrives, emit it from your frame hook). `first_frame` is the
  first frame with real UI.
- Obey every `nt_core::autopilot::Command` (take them with
  `autopilot::next_command()` as an async stream, or `try_next()` plus
  `set_waker`). They must go through the same code paths as real input:
  `Scroll` → the same handler a wheel notch reaches, `ScrollPixels` → the
  touchpad path, `Fling` → `ScrollAnimator::fling`, `Search` → the same
  submit as pressing Enter, `OpenVideo { index }` → the same as clicking the
  index-th item of the main list on screen (the related list on a Watch page),
  `OpenChannel` → the channel of the video being watched, `Back` → back,
  `ScrollToTop` → `jump_to(0)`, `Home` → the Home root, `LoadMoreComments` →
  load the next comments page (the first, if none are loaded), `TogglePause`,
  `Quit` → exit cleanly.

## Verifying your work (required before you finish)

1. `cargo build --release` with no warnings.
2. Start the app on its off-screen display and screenshot it:
   `tools/headless.sh run <fw> apps/<fw>/target/release/nt-<fw>` then
   `tools/headless.sh shot <fw> /path/out.png` and look at the PNG.
   `tools/headless.sh log <fw>` shows its output.
3. Drive it with real input: `tools/nt-input/target/release/nt-input <fw> ...`
   (click, wheel, touchpad, drag, key, type; coordinates are logical pixels in
   the 1280x800 window). Check every page, the search dropdown, the player
   controls, subscribe, Watch Later, back navigation and resizing behaviour
   (narrow layout) against this spec, by screenshot.
4. Run the benchmark scenario end to end and look at every snapshot:
   ```
   NT_AUTOPILOT=bench/scenario.json NT_REPORT=apps/<fw>/shots/report.json \
   NT_SNAPSHOT_OUTPUT=NT-<FW> NT_SNAPSHOT_DIR=apps/<fw>/shots \
   tools/headless.sh run <fw> apps/<fw>/target/release/nt-<fw>
   ```
   It must finish by itself with `"failed_waits": []` in the report, and every
   snapshot must show the right page with real content. Check that idle phases
   show near-zero CPU and frames, and that scroll phases run at the display's
   60 fps.
5. Leave the final snapshots in `apps/<fw>/shots/`.

## NOTES.md (required)

`apps/<fw>/NOTES.md`, kept factual and short:
- Framework and component-kit versions used.
- Architecture: state, messages/actions, how async results come back, how the
  views are split.
- Scrolling: how raw events were captured, how the framework's own scrolling
  was bypassed, how virtualization works.
- Video: how frames reach the screen and what that costs.
- The frame hook used for `metrics::frame()`.
- What was hard, what the framework made easy, every workaround, and anything
  in the spec you could not do (and why).
- Core requests.
