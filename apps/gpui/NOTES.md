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

## What changed in the parity round (2026-11-19, Opus + GPT 6.2 Sol in turns)

**Fixes from the brief**
- Posters and the up-next thumbnail are requested at the drawn width × scale factor (no fixed
  320 px); every `images::image` call passes physical pixels, and GPUI samples linearly.
- A watch page never shows black at 0:00 while its video starts: the player shows that video's
  own last picture (Back) or its thumbnail at the player's width under the spinner, and starts
  with `load_at` at the remembered position.
- Escape in fullscreen: closes an open player menu first, then leaves fullscreen, whatever had
  focus (verified: settings open → Esc → menu closed, still fullscreen → Esc → windowed).
- Seek-bar hover when the pointer lands on the bar in the same motion that wakes the controls:
  the bar didn't exist in that frame, so its own `on_mouse_move` missed it and no tooltip or
  storyboard appeared until the next motion. The player's root move handler now tracks the
  hover from the bar's last bounds.

**Parity** (all in this app, data from core where noted)
- Card / row / related menus: Play, Save to / Remove from Watch later, Add to queue, Share
  (copies the link, toast), Go to channel, Not interested, Don't recommend channel (B13).
  Hidden videos leave the list at once.
- Card hover: a #272727 plate, radius 12, bleeding 8 px, plus "⋮".
- Guide: ☰ opens the 240 px guide (icons, labels, dividers, "You", subscriptions); it pushes
  the page on wide non-Watch pages (remembered in `gpui-prefs.json`) and is a drawer with a
  scrim on Watch and narrow windows. `?` opens the shortcut sheet; Esc or a click outside
  closes it.
- Keyboard: Tab / Shift+Tab move through tab groups top bar (1) → rail/guide (2) → content
  (3); `focus_visible` draws a 2 px #f1f1f1 ring at a 2 px offset (two box shadows: ring, then
  a page-coloured gap), keyboard only. Enter/Space activate (GPUI's keyboard click).
- Player: right-click menu (Copy video URL, … at current time `&t=Ns`, Loop, Stats for
  nerds with live numbers and ×); settings: Subtitles/CC with auto-translate, Audio track,
  Stable volume and Loop switches, Sleep timer (Off, 10–60 min, End of video; pauses when it
  fires), speed, quality. C toggles captions. Chapters: segmented bar with 2 px gaps, "• Intro ›"
  in the bar, chapter rows in the description. Heatmap over the bar on hover. Storyboard
  preview + time + chapter above the pointer (A3).
- Watch page: comment replies ("N replies"), Sort by (Top/Newest), pinned / hearted / verified
  marks, related chips (none in logged-out answers), playlist and mix panel (B12), local queue
  panel (B13), prepare_next for the up-next / queue / playlist successor (C2).
- Search: notices in YouTube's wording (age gate, "Showing results for", "Did you mean"), the
  "Matches in video transcripts · search.pronsh.dev" section with "▶ 9:52" chips and bold
  highlights (a click plays from that time), filter chips and the Filters dialog (B9), mixed
  results: playlists, channels, shelves (B10). "pronsh" shows "Confirm your age" + 7 matches.
- Channel tabs (Home, Videos, Live, Playlists), Videos sort, links and About (B11). Home feed
  chips from the server when it sends them, otherwise the fixed topics (B14). Badges: ✓, LIVE,
  Upcoming, 4K/CC/New (A6). Muted hover preview on thumbnails (B15). "Continue watching" card
  on Home from `last_played` (C5).
- Lists never dead-end: next pages are fetched a screen and a half early, and a failed page
  shows "Couldn't load more · Retry" in place, keeping what's loaded (C3).
- GPU video (C6): the window is transparent, the page paints around the video rect
  (`hardware::paint_around`, after Akshar Patel's yt-music-gpui `player_page.rs`), and core's
  Wayland subsurface presents the video; the rect follows layout, scroll, theatre and
  fullscreen. `NT_SOFTWARE_VIDEO=1`, a failed attach or a lease error fall back to the
  RenderImage path.
- State probe (`probe.rs`, `NT_PROBE=1`): one `NT_STATE` line per change (≤ 10/s) plus a 1 s
  heartbeat. Targets are recorded in prepaint by zero-size canvases tagged with a layer, so the
  rects are exactly what was laid out; a cached view keeps its last list until it renders
  again; popups drop or trim what they cover.
- Lints: `unwrap_used`, `expect_used`, `panic`, `string_slice`, `todo`, `unimplemented`,
  `unreachable`, `unchecked_time_subtraction` are denied in Cargo.toml; clippy is clean.
- New dependency: `raw-window-handle` 0.6 (already in GPUI's tree), for the window handles
  core's `VideoSurface::attach` takes.

**Captions placement and test hooks (C7)**
- `PlayerView::place_captions` sends mpv `sub-pos` (core `set_caption_position`) like iced: 98
  at rest (~8% above the picture's bottom), lifted above the 76 px of scrubber + bar row while
  the controls show, eased over 200 ms (cubic ease-out, `request_animation_frame` while it
  moves). The lift is computed from the player box and the video's aspect, so theatre,
  fullscreen, resize and letterbox bars need less rise. Only changes ≥ 0.05 are sent.
- Input-free test switches: `NT_TEST_CC=1`, `NT_TEST_CYCLE=controls` (3.2 s on/off),
  `NT_TEST_FULLSCREEN=1`, `NT_TEST_THEATRE=1`, `NT_TEST_SCRUB=<0..1>` (holds the seek-bar
  hover: tooltip, chapter, heatmap). Evidence: shots/parity/captions-montage.png,
  heatmap-0y6RKiIk6cs.png. Dates come from core (`published_label`); the local ISO parser is gone.

**Measurements** (NT-GPUI 1280×800, muted)
- Playback CPU over 10 s on the 720p test clip: GPU surface 0.67 s, software path 3.89 s
  (`NT_SOFTWARE_VIDEO=1`); RSS 310 MB vs 318 MB.
- Stress, three rounds on one process (fixtures): each round 20 related clicks 0.7 s apart and
  35 rapid Back/Forward (alt+Left ×2, alt+Right ×1, 150 ms apart), ~38 s; plus 10 cycles of
  scrub drag, settings menu, context menu, theatre ×2, fullscreen + Esc. No panic, no stale
  video (the probe logs any player/page id mismatch: 0), menus closed at the end. RSS 312 MB at
  start → 644 → 606 → 650 MB after rounds 1–3, flat (606 → 606 MB) across the toggle cycles.
- Start of the next video, live (probe, ±100 ms): Shift+N on the prepared successor 1.5 s,
  a click on an unprepared related video 1.8–2.0 s (both include the watch-page request).

**Performance follow-up**

- After 20 scripted related opens on the same local clip, PSS settled at 404 MiB and RSS at
  439 MiB. The previous build reached 599–612 MiB PSS. The updated run kept all 22 navigation
  pages. In a second run, 35 scripted Back/Forward operations 75 ms apart ended at 267 MiB
  PSS, with zero failed waits, panics or player/page mismatches.
- Most of the increase was anonymous memory. Turning off grid images still left 546 MiB PSS.
  One diagnostic `malloc_trim(0)` call cut PSS from 599 to 348 MiB with the same images, GPU
  surface and pages alive. Running with mpv's `cache=auto` instead of core's forced read-ahead
  reached 366 MiB PSS. GPUI now batches glibc reclamation on a background worker after every six
  playback starts. The three calls took 22–32 ms each on that worker.
- Watch pages only request row images within 144 px of the viewport. They reuse cached card
  thumbnails during navigation, defer the full-size poster upgrade and drop it on the first
  picture. The up-next thumbnail loads when its countdown opens. Core's decoded image cache
  held 18 MiB after the run, down from 34 MiB. GPUI had 11 MiB of cached BGRA images and continued
  evicting atlas tiles. Shared memory stayed near 35 MiB.
- Playback starts ahead of the Watch metadata request. `NT_TRACE=1` records card activation,
  navigation, core load submission, observed stream metadata, player readiness, first frame and
  first paint. A diagnostic build also traced resolve and `loadfile` in core.

  | Live action | Action → navigate → load | Resolve | `loadfile` → GPU frame | GPU frame → paint | Total |
  | --- | ---: | ---: | ---: | ---: | ---: |
  | Related row (`Sntj4HmuykI`) | 0.1 + 0.1 ms | 98 ms | 544 ms | 8 ms | 651 ms |
  | Next from that video (`wzzh7Not8XE`) | 0.1 + 0.3 ms | Prepared hit | 509 ms | 10 ms | 519 ms |

  A baseline trace saw 817 ms for a related open and 366 ms for prepared Next, with different
  videos returned by the live feed. Muted `ntctl play` measured 840 and 950 ms for the two
  videos in the table, including resolution and software startup. The Next action after the
  related open was exercised through the player callback. The physical Shift+N chord and
  opening the shortcut sheet with `?` need input verification.

**What was easy / hard in GPUI this round**
- Easy: every popup (context menu, stats, sleep timer, guide drawer, shortcut sheet,
  storyboard tooltip) is one more absolutely positioned or `deferred` element; `focus_visible`,
  `tab_group`/`tab_index` and keyboard clicks are built in; `PathBuilder` drew the heatmap in
  20 lines; the transparent window + painting around a hole is ~50 lines.
- Hard: the probe. GPUI has no query for "where was this element laid out", so each target
  carries a canvas child; cached views skip prepaint, so per-layer lists are kept between
  frames. Hover that depends on an element which appears on the same event (the seek bar)
  needs manual tracking. Every new callback had to be checked for re-entrancy.
- Upstream write-ups: docs/upstream/gpui-axis-stop.md, gpui-reentrant-keystroke.md,
  gpui-hide-cursor.md, with repros in shells/gpui/repros (each run on NT-GPUI).

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

- **Miniplayer and the transcript panel:** not built (no core API for the transcript panel;
  the miniplayer wasn't in this round's brief).
- **Like and dislike aren't clickable:** the app never signs in. No dislike counts.
- **Quality above 1080p:** core caps it.
- **The spinner is the kit's (Lucide arc), not YouTube's red-and-grey spinner.**
- **Subpixel (RGB) text antialiasing:** GPUI's renderer; YouTube in Chrome is grayscale on
  Linux.
- **"Search instead for" re-runs the query:** core has no "don't correct" flag, so YouTube may
  correct it again.
- **Frame-accurate stepping:** "," and "." are ±1/30 s seeks; core has no frame-step command.

## Not portable / unverified on macOS

Nothing hard-codes a font or a path. The vendored crate and the portal Inhibit are Linux-only
through `cfg`, and so is the Wayland feature. Not built or run on macOS here: the app menu, Cmd+Q
and Cmd+W, and fullscreen there are unverified.

## Core requests

1. A frame-step command (`frame-step` / `frame-back-step`) for `,` and `.`.
2. Persistent watch positions and the autoplay choice in the library, so all three apps share
   them (this app keeps them in `gpui-prefs.json`).
3. A "don't auto-correct" search flag for "Search instead for".
