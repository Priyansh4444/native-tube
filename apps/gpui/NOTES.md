# NativeTube: GPUI frontend notes

## Versions

- `gpui-pre` 0.3.12 (Zed's gpui, zed@d2a4fb6), `gpui-pre-platform` 0.3.12 (`wayland` feature on
  Linux only), `gpui-component` 0.9.1 (search `Input`, `Spinner`), `gpui-kit-assets` 0.9.1 (only
  for the kit's own internal icons).
- `gpui-pre-linux` 0.3.12 is vendored with a ~30-line patch (`vendor/gpui-pre-linux`): it reports
  the touchpad finger lift (`wl_pointer.axis_stop`) as a `TouchPhase::Ended` scroll, which the
  Chromium fling model needs. Upstream GPUI drops that event.
- Fonts: the shared Roboto Regular/Medium/Bold, embedded with `include_bytes!` and registered with
  `text_system().add_fonts`; the root and the kit theme use "Roboto". Icons: the shared Material
  Symbols SVGs, embedded and served by our `AssetSource` (`icons.rs`), tinted with `text_color`.

## What changed in the v2 round

**Stability**
- **The "PlayerView is already being updated" panic.**
  - What happened: when the player controls auto-hide under the pointer, `PlayerView::render`
    scheduled the cursor hide with `cx.defer_in(window, …)`. A deferred closure runs *inside*
    `update_entity(PlayerView)`. The hide works by dispatching a synthetic keystroke, and
    GPUI's `dispatch_key_event` first draws the window when it is dirty. That draw rendered
    PlayerView again while it was still leased, which panics.
  - The fix: the hide now goes through `Window::defer`, which holds only the window and no
    entity.
  - Audit: every `defer_in`, `dispatch_*` and self-handle update was checked. Nothing else
    dispatches input or draws from inside an update. Each other update goes from one entity
    into a different one, and emits are deferred by GPUI.
  - Checked with three back-to-back stress sessions of about 2 minutes each (below): no panic.
- Panic hook: logs message and location to stderr, then runs the default hook.
- Quit: Ctrl+Q and Cmd+Q (`Quit` action, also handled at the root to save watch positions),
  Cmd+W, and closing the window. On macOS `set_menus` gives the app menu a "Quit NativeTube"
  item; About and Hide aren't added (not verified on macOS).
- Every async answer is checked against a token before it is applied: pages (`req`), "more"
  pages (`req`), comments (`comments_req`), suggestions (`seq`, plus a focus check), images (keyed
  by URL and width; answers for evicted entries are dropped), and the up-next thumbnail
  (`next_req`).
- Playback: a failed `Player::new` (libmpv missing) or a failed video shows "This video can't be
  played" with Retry. The start is retried on the next load, never disabled for the session. A new
  video never shows the previous video's picture: `load` drops it, and the upload compares frame
  generations.
- Bounded:
  - images: 160 ready entries, 48 MB, and 600 entries in total, evicted least recently drawn,
    with atlas tiles freed;
  - navigation stack: 30 pages (the oldest one above the root is dropped);
  - Forward stack: 20 pages;
  - watch positions: 400 entries.
- Error states show a plain line ("Couldn't reach YouTube", "This video is unavailable") with
  Retry. Retry reruns only what failed: page data, a "more" page, or comments. Retrying page
  data never restarts the video. Raw errors go to stderr.

**Structure (review G2)**
- The window is now five views. `TopBar` and `Rail` are cached views (`Entity::cached`), reused
  unless notified.
- `PageView` (chips, blocks, scrollbar) is cached and notified explicitly (`content_changed`)
  whenever page data, the scroll offset or images change. It renders through `NtApp`, so every
  listener still acts on the app.
- `PlayerView` is a sibling of the page, positioned over the page's player block and clipped to
  the page area. A video frame notifies only it (and the cheap root), never the page.
- When the player is scrolled out of view it isn't drawn at all, so its frames cost nothing (G5).
- The scroll animation ticks in a `Window::on_next_frame` callback before each draw, so the page
  and the player move in the same frame.

**Look and feel**
- All of spec v2's tokens, type scale (Roboto with real 500/700 weights), Material Symbols icons,
  top bar (collapsed search below 656 px, focused search icon, suggestions with the typed prefix
  at 400 and the completion at 500), rail (74 px entries, 10 px labels, filled icon on the page on
  screen only, hidden below 792 px and on Watch).
- Home grid columns follow spec v2 (1/2/3/4/5), with cards filling the row. Title heights are
  measured, so a one-line title doesn't reserve a second line. The chip bar is sticky and scrolls
  horizontally (wheel and arrow buttons, with a fade).
- Cards have no hover background; a "⋮" button shows on hover. Thumbnails darken while pressed,
  and watched videos get the red progress bar.
- Search rows are centered (max 1096) with flexible thumbnails, and the snippet bolds the query
  terms.
- Watch has no rail. It switches to two columns at 1017 px; the primary column is capped at
  `(h−169)×16/9` and centered. It has white Subscribe (Subscribed shows a bell), a like | dislike
  pill, Share (copies the link) and Watch later. The description has links, #tags and
  timestamps that seek, plus an inline "...more". Comments are clamped to 4 lines with Read more.
- Channel content is max 1284 wide, with a /6.2 banner, a 160 px avatar, a 36 px name, the
  description on one line with an inline "...more", and a Videos tab. The grid has no avatars.
- Empty and error states: a 120 px icon, a title, a line of text and an action.

**Right-click / "⋮" menu (owner feedback)**
- Right-click on any card, search row or related row (or its "⋮") opens a menu at the pointer.
  The menu flips left or up to stay on screen.
- Items: Play, Save to / Remove from Watch later (with a toast), Copy link (toast "Link copied"),
  Go to channel.
- It closes on Escape (even with the search box focused), on a click outside with either button
  (the click does nothing else), and on a wheel (which also scrolls the page). A full-window
  catcher occludes the page, so nothing underneath reacts.
- Right-click never opens the video: GPUI's `on_click` is primary-button only.

**State on Back/Forward (owner feedback)**
- Each page keeps its data, scroll animator, text measurements and layout in the stack, so going
  back restores them without a refetch or a flash.
- The search box shows the query of the page on screen: a Search page's own query, empty on root
  pages, unchanged on Watch and Channel.
- Watch: leaving stops the player and stores the position. Returning resumes there and shows that
  same video's last picture (parked) under the spinner until the new frames arrive (seen at
  150 ms, `shots/back-to-watch-150ms.png`).
- Positions are also kept across sessions (`gpui-prefs.json` in the data dir): resume when more
  than 30 s in and more than 30 s from the end, as YouTube does.
- Forward (Alt+Right, mouse forward) re-pushes popped pages.

**Viewing**
- Fullscreen: F, double-click, or the button; Escape leaves. The video is letterboxed on black.
- Theatre mode: T or the button; the player is full width on black, height
  `min(56.25% of width, h−169)`.
- Control bar: play/pause/replay, next, mute with a 52 px slider that slides out on hover, time
  (or a LIVE pill), autoplay toggle (remembered), settings (speed 0.25–2, hidden for live;
  quality Auto (current)/heights), theatre, fullscreen.
- Scrubber: 3 → 5 px on hover, buffered and played bars, a knob on hover only, a hover time
  tooltip. Dragging shows the target and seeks once, on release.
- Controls and cursor auto-hide after 3 s while playing; they stay while paused or while a menu
  is open.
- A center play/pause icon fades out over 500 ms.
- Keys: Space/K, J/L, ←/→, ↑/↓, M, F, T, Shift+</> speed, ,/. frame step while paused, 0–9,
  Home/End, Shift+N, Escape.
- `set_target_size` gets the painted size in physical pixels on every size change.
- End of video: an "Up next" card with a 5 s countdown, Cancel and Play now. With autoplay off,
  the play button shows replay.
- Screen blanking: core's MPRIS covers desktops whose idle manager watches MPRIS. `inhibit.rs`
  additionally asks the xdg-desktop-portal Inhibit interface while playing, for desktops that
  ignore MPRIS (GNOME). It logs once and stops if the portal lacks Inhibit, as on this Hyprland
  machine. Linux only (`cfg`).

## Scrolling

- **Wheel and touchpad.** Raw `ScrollEvent`s on the scroller feed `nt_core::scroll`:
  - Wheel notches arrive as `Lines` = value120/120 × 3 × −1, converted to `wheel_ticks`.
  - Finger deltas arrive as `Pixels` = axis × 3 × −1; × −4 gives Chrome's ×12, fed to
    `wheel_pixels`.
  - The patched finger lift calls `fling_start`, only after precise deltas.
- **No framework scrolling.** No GPUI scroll handle or overflow scrolling is used.
- **Wheel elsewhere.** Over the top bar, rail or chips the wheel scrolls the page (root
  fallback); a horizontal wheel over the chips scrolls the chips.
- **Keys and scrollbar.** Keys use `scroll_by` with the module's steps. The scrollbar thumb
  drags with `jump_to`; a track click pages.
- **Present.** `tick(now)` runs in `on_next_frame` before each draw, and frames are requested
  only while `needs_frame()` is true. Idle draws nothing.
- **Virtualization.** Blocks with known heights are binary-searched. The view builds the
  viewport plus half a viewport above and below, and requests images a full viewport beyond.
  Spinners animate only inside the visible band.
- **Measuring.** Text is measured at bucketed widths (8 px for titles, 16 px for comments), so a
  live resize re-measures rarely.

## Video path

mpv renders BGRA into a CPU buffer on the core's render thread and runs our waker, which does
`try_send` on a bounded(1) channel. A task on `PlayerView` calls `poll`, which notifies the
player view only when the frame generation or a displayed part of the state changed. On the next
draw the newest frame is copied once into a new `RenderImage`; GPUI uploads it to its texture
atlas when painted, and the previous tile is freed with `Window::drop_image`. GPUI has no in-place
texture update on Linux (`paint_surface` is macOS-only), so one copy and one upload per frame is
the framework's floor.

## Frame hook

`metrics::frame()` runs in the paint callback of a zero-size `canvas`, the root view's last child.
It runs once per drawn frame, during paint, right before present. Page marks are moved into that
closure. `playback_started` is emitted from the player view's own paint hook in the frame that
first shows a picture.

## What was hard, and workarounds

- **View caching.** A cached view whose render reads another entity is *not* invalidated when
  that entity notifies: only the notified view and its ancestors are marked dirty. Every page
  change therefore notifies the page view explicitly.
- **Notifying a view during render doesn't count for that frame.** So the scroll tick runs in
  `on_next_frame`, and triggers (infinite scroll, comments) run in `defer_in` after the root's
  render.
- **Hiding the cursor.** GPUI has no public way to hide it. A synthetic inert keystroke
  (`Window::dispatch_keystroke`) trips GPUI's own hide-while-typing path
  (`CursorHideMode::OnTypingAndAction`), and the root handler swallows the key. It must run from
  `Window::defer` (see the panic above).
- **Kit custom button colours are drawn at 20% strength**, so the app draws its own pill buttons.
- **Test environment.** Clicks near the output edges on this machine are eaten by the desktop
  shell's (caelestia) full-output layer. Top-bar and rail clicks were tested with the window
  floated away from the edges, and by keyboard.

## Remaining differences from YouTube, and why

- **No captions, chapters, storyboard previews, comment replies, dislike counts or verified
  marks:** the core doesn't provide them; spec v2 says to leave them out.
- **Like and dislike aren't clickable:** the app never signs in.
- **No inline hover previews on cards:** not specified, and would cost a second player.
- **Rail labels:** "Subscriptions" at 10/14 fits; there's no hamburger or guide drawer.
- **The spinner is the kit's (Lucide arc), not YouTube's red-and-grey spinner.**
- **Subpixel (RGB) text antialiasing:** that's GPUI's renderer; YouTube in Chrome is grayscale
  on Linux.
- **No keyboard focus rings or Tab traversal across cards:** not done in this round. GPUI
  supports `tab_index` and `focus_visible`; it needs a focus handle per interactive element.
- **Frame-accurate stepping:** "," and "." are exact ±1/30 s seeks; the core has no frame-step
  command.

## Not portable / unverified on macOS

Nothing hard-codes a font or a path. The vendored crate and the portal Inhibit are Linux-only
through `cfg`, and so is the Wayland feature. Not built or run on macOS here: the app menu, Cmd+Q
and Cmd+W, and fullscreen there are unverified.

## Core requests

1. A frame-step command (`frame-step` / `frame-back-step`) for `,` and `.`.
2. Persistent watch positions and the autoplay choice in the library, so all three apps share
   them (this app keeps them in `gpui-prefs.json`).
3. Comment replies, captions and chapters, which spec v2 lists as "not available from the core
   yet".
