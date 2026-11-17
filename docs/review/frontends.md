# Frontends review: correctness and framework idiom

Reviewer lens: are the three apps' code correct, and is each a fair, idiomatic use of its framework.
Scope: `apps/iced/src`, `apps/gpui/src` and `apps/gpui/vendor/gpui-pre-linux`, `apps/rune/src`,
each app's `Cargo.toml` and `NOTES.md`.

**How I checked.** I read every line of the three apps. I checked framework behaviour in the
sources under `~/.cargo/registry/src` (iced_winit 0.14.1, iced_wgpu 0.14.0, iced_widget 0.14.2,
winit 0.30.13, gpui-pre 0.3.12, gpui-pre-linux 0.3.12, gpui-pre-wgpu 0.3.12) and
`~/.cargo/git/checkouts/rune-*/4fbd2d6`. I diffed the vendored GPUI crate against the crates.io
copy. I read the existing `apps/*/shots/report.json` files only to tie code to measured numbers.

I ran nothing: no app, no off-screen output, no cargo. I skipped `cargo clippy`. Three cold
builds of wgpu-based trees in a scratch target would take a long time, and the panic surface is
small enough to check by grep: there is no `unwrap`/`expect`/`panic!` in any app, and the only
`unsafe` is `std::env::set_var` at the start of `main` in iced and GPUI. Every finding below
comes from reading code unless it says otherwise.

---

## iced (`apps/iced`)

### P0
None.

### P1

**I1. Each video frame rebuilds and re-lays out the whole window.**
- Where: `ext.rs:153-158` (waker), `app.rs:551-554`, `app.rs:464-487` (`on_player`),
  `view.rs:521-576`.
- What happens: each player wake becomes a `Message`. iced then runs `view()` for the whole
  window: top bar, rail, and every built item of the watch page (player, info, description, all
  built comments and related rows). Then it runs diff, layout and draw.
  - This happens for every wake, including state-only wakes where nothing visible changed.
    `on_player` doesn't filter them (GPUI's `video.rs:277-288` and Rune's `video.rs:241-252`
    do).
  - It also happens when the player is scrolled out of the band and isn't even built.
- Failure scenario: a 60 fps video, the user reading comments far below the player. The app
  still does 60 full view+layout+draw passes per second for a picture nobody sees.
- Measured: playback 31 % CPU, scroll_while_playing 37 %. That's competitive only because the
  watch page is small.
- Fix, in this order:
  1. Wrap the non-video parts of the page in `iced::widget::lazy` keyed on their data, so a
     video message skips their diff and layout. This is iced's own answer to "every message
     rebuilds view".
  2. Drop wakes where neither `frame_generation()` nor a displayed field changed.
  3. Don't take the frame (or redraw for it) when the player item isn't in the built band.

**I2. Per-frame work on the watch page grows with the number of loaded comments.**
- Where: `view.rs:542-545`.
- What happens: every `view()` walks all loaded comments. For each one it computes an estimate
  (`c.text.chars().count()`, `c.text.matches('\n').count()`), does a `HashMap` lookup and
  advances `y`.
- Because of I1, `view()` runs once per video frame. So at 2,000 loaded comments that is about
  400k characters scanned per video frame. This breaks the spec's "nothing per frame scales
  with the number of loaded items".
- Fix: cache per-comment heights in a prefix-sum `Vec<f32>` per width (as GPUI and Rune do),
  then binary-search the first built comment.

**I3. Thumbnails are uploaded synchronously in the frame they scroll into view, and evicted
again as soon as they leave it.**
- Where: `widgets.rs:256` (`draw_image`), `imgcache.rs:257`.
- How iced's raster cache behaves (`iced_wgpu-0.14.0/src/image/raster.rs:78-121`): in every
  frame where a new image landed, it drops every entry that wasn't drawn since the previous
  trim.
- The scroller draws only rows that intersect the viewport (`scroller.rs:517-521`). So the
  overscan rows' images are never drawn, are dropped from the atlas, and are re-uploaded on
  the UI thread (`image/cache.rs:236-256`, sync path for < 2 MB) in the exact frame they
  appear.
- Failure scenario: scroll down two rows, then back up one row. Every thumbnail of the
  returning row is re-uploaded inside that frame. During a fling, each frame that reveals a new
  row pays 3–4 uploads. Rasterisation never runs ahead of the viewport (scroll.md P16).
- Fix: keep thumbnails resident for the built band. Hold an `image::Allocation` per band image
  from `iced::widget::image::allocate(handle)` (`iced_runtime-0.14.0/src/image.rs:20`). Live
  allocations are exempt from trimming (`raster.rs:91-97`) and upload on iced's worker. Drop
  them when the image leaves the band.

**I4. The scroll autopilot is costlier than a real wheel, which skews iced's scroll numbers.**
- Where: `app.rs:926-941`.
- What happens: a real wheel notch is handled inside `Scroller::update` with no message
  (`scroller.rs:411-435`). An autopilot `Scroll`, `ScrollPixels` or `Fling` command arrives as
  a `Message`, and every message makes iced rebuild `view()` and re-lay out the window.
- The infinite_scroll phase sends 160 commands at 20 per second, so iced's scroll-phase CPU
  includes about 20 full rebuilds per second that a real user never causes. GPUI and Rune go
  through paths no costlier than real input.
- Fix: a framework constraint (no external input without a message), so either:
  - make the rebuild cheap (I1's `lazy`), or
  - deliver pilot scroll commands through a shared queue the `Scroller` drains on
    `RedrawRequested`, with the subscription message only waking the loop.
  - At minimum, note it next to the iced numbers.

**I5. The memory comparison mixes an app-level GPU knob with the other two frameworks'
defaults.**
- Where: `main.rs:65-72`. It sets `WGPU_POWER_PREF=low` and `WGPU_BACKEND=vulkan`.
- What it does: iced loads only the Vulkan iGPU driver. NOTES puts that at about 150 MB RSS at
  startup instead of about 254 MB.
- GPUI cannot do the same: gpui-pre-wgpu hard-codes `Backends::all()`
  (`wgpu_context.rs:216, 525`). Rune can, with `RUNE_GPU_BACKEND`, but its builder didn't.
- The reported startup RSS (iced 142, Rune 182, GPUI 237 MB) is therefore partly an
  app-configuration difference, not only a framework difference.
- Fix: run all three with the equivalent knob where one exists (Rune: `RUNE_GPU_BACKEND`),
  report GPUI's driver overhead separately, or drop the knob from iced.

### P2
- **I6. Shaping on the UI thread during scroll.** `Clamp::fit` (`widgets.rs:120-155`)
  binary-searches the ellipsis cut point by reshaping the whole prefix about 8–10 times per
  clamped text. This runs in `layout` for every newly built row, which is inside a scroll
  frame (iced runs the band-rebuild `update`/`view`/layout inside the redraw,
  `iced_winit lib.rs:825-935`). A rebuild that brings in a new row of 4 cards shapes about 100
  paragraphs. Fix: shape once, find the line break for line N with the paragraph's hit-test or
  line API, then shape only the cut line. Or cache by (text, width) across trees.
- **I7. `Feed::append` is O(n·m).** It dedupes each new item against every loaded item
  (`app.rs:111-119`): about 150k string compares per page at 5,000 items, on the UI thread.
  Fix: a `HashSet` of ids.
- **I8. `Command::Home { topic }` with a non-default topic sends two home requests**
  (`app.rs:908-915`): `replace_root` loads topic 0, then `Topic(idx)` loads again. Only extra
  traffic, since `req` drops the first answer.
- **I9. The volume slider unmutes even under `NT_MUTE`.** `SetVolume` calls `set_muted(false)`
  (`app.rs:843-851`). GPUI and Rune refuse to unmute when `NT_MUTE` is set, so a manual test
  drag could make sound in iced.
- **I10. The offset isn't snapped to physical pixels** (`scroller.rs:312, 516`), so text
  shimmers at fractional offsets during slow scrolls. GPUI snaps it (`app.rs:970-971`).
- **I11. Wheel over the top bar or rail doesn't scroll the page** (`scroller.rs:411`
  `cursor.is_over(bounds)`). Chrome and YouTube scroll the page there; Rune does too.
- **I12. Dead workaround.** `normalize_scenario` (`main.rs:34-59`, with its `unsafe set_var`)
  is dead: core accepts `{"do": …}` since commit 78fa687. Remove it.
- **I13. Stale-row clicks are possible.** The `/` shortcut is fine, but `Open(list, i)` indexes
  the list at click time. If the band rebuilt between press and release, a click can land on a
  different row. This is theoretical, because rows don't move under a still pointer.

Fine: back-stack restore (data, offset, resume via `resume_pending`); stale answers dropped by
page id and `req` (`app.rs:593-600`); one "more" request in flight (`More::Loading`); search
debounce, stale suggestions and keyboard handling (`widgets.rs` `SearchBox`, `app.rs:725-786`);
watch keys disabled while typing (`view.rs:305`); spinner redraws only when visible
(`widgets.rs:312-323`); `metrics::frame()` from the root widget's `draw`, which iced calls once
per `RedrawRequested` right before `present` (`iced_winit lib.rs:938-981`); tick on
`RedrawRequested(Instant)`, frames only while `needs_frame()`; wheel units (`LineDelta` →
`wheel_ticks(-y)`; `PixelDelta` is physical in winit 0.30.13, `pointer/mod.rs:206-209`, and the
`/scale` undoes it); `axis_stop` as a zero `Pixels` delta → `fling_start`; video primitive
clipped to the scroller layer (`iced_wgpu lib.rs:466-560`); texture reused, upload only on a
new generation.

---

## GPUI (`apps/gpui`)

### P0
None.

### P1

**G1. A new watch page shows the previous video's last picture until its own first frame
arrives.**
- Where: `video.rs:344-365`, `views.rs:797-801`.
- Why: after `Player::load` or `stop`, the core clears its frame but `frame_generation()` stays
  the same. So `upload()` hits `generation == shown_gen && image.is_some()` and keeps the old
  `RenderImage`, and `player()` draws it under the spinner.
- Failure scenario: Watch A, click a related video B. B's page shows A's picture for the
  0.5–2 s B takes to start. The same happens returning with Back.
- Fix: in `VideoState::load`/`stop`, `take()` the image and `drop_image` it. Or, in `upload`,
  drop it whenever `p.frame()` is `None`. Rune clears the picture in `play()`
  (`video.rs:132`), and iced sets `frame = None`.

**G2. The whole window is one view, so every scroll and video frame re-renders and Taffy-lays
out everything.**
- Where: `app.rs:953-1058`.
- What it costs: `NtApp::render` builds the top bar (with the kit `Input`), the rail, the chips
  and every built block on each frame. NOTES measures 0.4–0.7 ms of element building plus
  2–3 ms of Taffy per scroll frame.
- How the cost splits:
  - Much of that is GPUI's immediate-mode model: block positions change each frame, and
    `list`/`uniform_list` would rebuild visible items too.
  - The top bar, rail and (on the watch page) everything around the player are rebuilt only
    because they live in the same view.
- Measured: GPUI's scroll-phase CPU is 2–3× the others' (wheel 28.7 % vs 12.5/11.9; fling
  29.4 % vs 7.8/9.4). Playback is 41 % vs 31/34, and scroll_while_playing 62 % vs 37/38.
- Fix (GPUI's own tool):
  - Split into child entities (`TopBar`, `Rail`, `Page`, `Player`) and embed the static ones
    with `AnyView::cached(style)` / `Entity::cached` (`gpui-pre/src/view.rs:46, 258`). Cached
    views reuse last frame's layout and paint unless notified.
  - Have the player waker notify only the `Player` entity.
  - This is the idiomatic GPUI structure, and the comparison currently charges its absence to
    the framework.

**G3. A playback failure has no Retry, and a failed `Player::new` is permanent.**
- Where: `views.rs:808-822` shows "Playback failed" and the message, with no button.
  `video.rs:292-313`: once `Player::new` fails, `ensure()` never tries again.
- The spec says errors show in the UI with a Retry; iced has `RetryPlayback`.
- Fix: a Retry button that calls `video.load(id)` from the current position, and let `ensure`
  retry after an error.

### P2
- **G4. Per-frame allocation in the frame path.** Each frame clones:
  - every built `Video` (`views.rs:475` `to_vec()`, plus `open = v.clone()` per card and
    row);
  - the whole `WatchInfo` twice (`views.rs:909-913`, `981`), including its related list and
    description;
  - the `ChannelInfo` with its first page of videos (`views.rs:709-710`);
  - each `Comment` (`views.rs:1020`).
  On top of that, `ImageCache::get` allocates a `String` key per image lookup
  (`images.rs:186`). That is hundreds of small allocations per frame. Fix: borrow from the
  page (split borrows, or `Rc`/`SharedString` fields), and key the cache by `SharedString`.
- **G5. The player wake re-renders even when the player is off-screen**
  (`app.rs:510-524`). On a watch page scrolled down to the comments, every video frame still
  re-renders and Taffy-lays out the window. Gate the notify on the Player block being in the
  built range, or fix it through G2.
- **G6. The kit `Spinner` and `Skeleton` animate while in the overscan band but off-screen.**
  The Spinner is used by `views.rs:519-521`, `804-806`. Each requests animation frames, so a
  "loading more" spinner half a viewport below the screen keeps the window at 60 fps of full
  renders.
- **G7. Wheel input over the chips bar, top bar or rail doesn't scroll.** `.on_scroll` is only
  on the scroller div (`views.rs:333`); on Home the chips bar is a separate overlay above it.
- **G8. Channel links open on mouse-down** (`views.rs:110-116`) rather than on click, unlike
  every other link in the app.
- **G9. Suggestions can open on an unfocused field.** A suggestions answer that lands after the
  field lost focus (without a submit) opens the dropdown anyway (`app.rs:741-755`): there's no
  focus check.
- **G10. Thumbnails are held twice.** Each `RenderImage` keeps a CPU copy next to its atlas
  tile (`images.rs:134-136`, cap 140 ≈ 32 MB), and is BGRA-swizzled from the core's RGBA
  (`to_bgra`, one extra copy, off-thread). That is the framework's API; it's noted here
  because it shows up in RSS.
- **G11. Resizing re-shapes every loaded comment per resize frame.** `build_layout` re-shapes
  all comments whenever the width changes (`pages.rs:323-330`), on the UI thread, in every
  frame of a live resize. Debounce it, or measure lazily as comments come into range.

### Vendored patch (`vendor/gpui-pre-linux`)
- What it changes: about 30 lines in `src/linux/wayland/client.rs` (2711-2740 plus two
  fields). It records a finger or continuous `wl_pointer.axis_stop` (vertical) and, in the
  next `wl_pointer.frame` that carries no other scroll event, emits a zero `Pixels`
  `ScrollEvent` with `TouchPhase::Ended`.
- Upstream drops `axis_stop` entirely, so without the patch no app on GPUI can start a Chromium
  fling from a touchpad.
- **Sound**:
  - The state borrow is dropped before `handle_input`, as in the neighbouring arms.
  - The early `return` skips only the scroll-delta flush, which is empty by construction.
  - The source check matches Chromium, which only flings after finger frames.
- Edge case (P2): if a compositor sends the final `axis` and `axis_stop` in the same frame,
  `std::mem::take` clears the flag and the stop is lost. wlroots, Mutter and KWin send the stop
  in a frame of its own, so it's unlikely.
- The cost is maintenance: a forked platform crate pinned by `[patch.crates-io]` to `=0.3.12`.
  Every GPUI upgrade needs the patch rebased. Worth upstreaming.

Fine: navigation and Back (`app.rs:188-208`); stale answers dropped by page id and `req`
(`app.rs:274-286`); one "more" request in flight; topic switch (bumps `req`); triggers use
`target()`; debounce and stale suggestions (`app.rs:722-777`); keyboard keys disabled while
typing (`app.rs:631-634`); wheel units (`Lines` = value120/120 × 3 × −1 → `−y/3`; `Pixels` =
axis × 3 × −1 → `×−4` = Chrome's ×12, checked against `client.rs:2626-2700`); tick once per
`render()` with `request_animation_frame()` only while `needs_frame()`; offset snapped to
physical pixels; `metrics::frame()` in a root `canvas` paint closure, once per drawn frame;
marks moved into that closure; video old tile freed with `drop_image` before the new one
(`video.rs:361-363`), so the atlas doesn't grow; GPUI has no Linux `paint_surface`
(`window.rs:5404`, macOS only), so a copy plus a new image per frame is the framework's floor.

---

## Rune (`apps/rune`)

### P0
None.

### P1

**R1. Switching topic can show the old topic's feed, or append old-topic videos to the new
one.**
- Where: `app.rs:408-416`, `285-302`, `326-352`.
- Why: answers are matched by page id only. `set_topic` reuses the same page, so it has no
  request token like `req`.
- Failure scenarios:
  - Click "Music", then quickly "Gaming". If Music's answer arrives last, the Gaming chip is
    selected over Music's videos.
  - Or: "more" is in flight when the chip is clicked. `Feed::default()` resets `more` to `Idle`,
    the stale answer extends the new topic's list with the old topic's page, and sets `next` to
    the old topic's continuation.
- Fix: a `req` counter on `Page` (or on `Feed`), bumped in `load_top`/`set_topic`, captured by
  each closure and compared on arrival, as GPUI does.

**R2. Thumbnail widths aren't bucketed, so a live resize refetches every visible thumbnail
over the network at each new pixel width.**
- Where: `view.rs:233-235`, `247`, `240`.
- Why: `px(w, scale) = ceil(w*scale)` follows the card width exactly. The core caches decoded
  images per exact `(url, width)` and keeps no byte cache (`core/src/client.rs:115-125`), so
  each new width is a new HTTP fetch plus decode.
- Failure scenario: dragging the window edge across 200 px refetches the 20–40 visible
  thumbnails up to 200 times. The image CDN isn't rate-limited by `polite`, but this is
  exactly the traffic the owner wants kept small.
- Fix: round up to 32-px buckets, as iced (`imgcache.rs:218-221`) and GPUI (`images.rs:158-161`)
  do.

**R3. Retry on a failed watch-info load restarts the playing video from 0.**
- Where: `app.rs:390-397` → `load_top()` → `app.rs:252-255` `self.video.play(&vid, None)`.
- Failure scenario: the watch request fails (network) but playback works. The user presses
  Retry under the player and the video jumps back to 0:00.
- Fix: start playback in `open_video`/`back`, not in `load_top`; or skip `play` when the video
  is already active.

**R4. A playback failure has no Retry, and a failed `Player::new` is permanent.**
- Where: `view.rs:550-553` shows the error text only. `video.rs:19, 60-65`: `PLAYER` is a
  `OnceLock<Result<…>>`, so one failed init disables video for the whole session.
- Fix: a Retry that calls `play(id, position)`, and a re-creatable player slot.

**R5. The release profile differs from the other two apps.**
- Where: `Cargo.toml` `[profile.release] incremental = true`.
- Incremental compilation in release splits code generation further and loses cross-unit
  inlining. Rune's binary is therefore built less optimised than iced's (default profile) and
  GPUI's (`codegen-units = 16`, which is the default anyway).
- Every CPU and frame-time number for Rune carries this handicap.
- Fix: use the same release profile in all three (default, or all with `lto = "thin"`).

### P2
- **R6. Scroll commands from input handlers can lose a band change.** `wheel_ticks`,
  `wheel_pixels` and `fling_start` call `Scroller::present` from the input handler
  (`app.rs:494-515`), which can move `band` but whose `true` return is ignored. The handler
  also sets `tx.live_only()`, so the view doesn't re-run, and the next `poll` sees the band as
  already current.
  - One lost crossing is hidden by the 1-viewport overscan.
  - Three in a row would show unbuilt rows. That needs a fling started from an event older
    than the frame, so it's rare.
  - Fix: store a `band_dirty` flag in `present` and let `poll` commit on it, or update the band
    only in `poll`.
- **R7. The description is re-measured on every view run** (`view.rs:685`, full
  `rune::text::measure`), although the plan already measured it (`pages.rs:347`). Pass
  `more` through the block.
- **R8. The image map thrashes on large windows, and failures are never retried**
  (`images.rs:184-238`).
  - With 6 columns the mounted range holds more than `KEEP = 96` images, so every band change
    drops everything not currently mounted. Scrolling back half a viewport re-requests from the
    core's cache, with one more memcpy per image and a re-upload.
  - A failed image stays an empty `Pixels` until evicted, with no retry.
  - Fix: size `KEEP` from the mounted range, and record failures with a retry time.
- **R9. Suggestions edge cases** (`app.rs:420-446`):
  - An answer for an older prefix that arrives after a newer keystroke (but before the newer
    fetch fires) is shown, because `search_changed` doesn't bump `sugg_seq`.
  - Losing focus doesn't close the dropdown.
- **R10. History is recorded only when the watch info loads** (`app.rs:260`), and from
  `info.as_video()`. Opening a video whose info fails isn't recorded; the other two record on
  open.
- **R11. `playback_started` can be marked one frame early.** The mark is queued from mpv's
  render thread (`video.rs:45-47`), so a frame already being encoded on the UI thread can
  flush it before the picture is in a frame.
- **R12. Extra frames while nothing changes.** The scrollbar fade spring
  (`view.rs:178` `bar_alpha.smooth`) and the controls transitions draw frames during
  "nothing changes" phases: paused 46 frames and 3.0 % CPU vs 3 frames and 0.2–0.7 %;
  back_navigation 146 frames vs 35–36. These come from the implementation, not the
  framework.
- **R13. A press on a card during a fling doesn't stop the fling.** Rune presses only the
  innermost handler (`runtime/input/gesture.rs:176`), so the viewport's `cancel_fling`
  (`view.rs:206`) never runs; the card opens instead.
- **R14. Possible cheaper video path (unverified).** Rune ships a latest-wins video surface,
  `NativeSurface::push_bytes` (`crates/rune/src/native_surface/mod.rs:1-3, 237`, BGRA or NV12,
  with its own pipeline). It is likely a cheaper path than `Pixels` (atlas-page copy plus
  upload); I did not verify its Linux upload path.
- **R15. Dead mute workaround.** The app's own mute flag (`video.rs:91-94`) works around a core
  bug fixed in 1497d8b. It is harmless, but now dead weight.

Fine: Back restores data and offset with no refetch; one "more" request in flight; wheel units
(`LineDelta` × 40 → `−dy/40`; `PixelDelta` / scale → raw axis → `wayland_axis_to_px`, checked
against `crates/rune/src/runtime/scroll/x.rs:212-213`); `fling_start` on `GesturePhase::Ended`;
input timestamps from `Tx::input_time` (the best of the three); scroll frames are `live_offset`
only, with the view re-run once per half-viewport band; idle is 0 fps; no view run per video
frame; video picture cleared on `play`; `metrics::frame()` in `on_frame_encoded` on `Drawn`
(encoded, submitted, presented); the autopilot goes through the same methods as input and,
like real wheel input, doesn't commit the view.

---

## Across all three

- **X1 (P2, core request).** No app cancels or deprioritises image requests for rows that have
  scrolled away. The core queues fetches FIFO behind an 8-wide semaphore
  (`core/src/images.rs:15, 37-75`), so after a fast fling through a long feed the visible
  thumbnails wait behind dozens of passed-over ones. Fix in core: cancel on `Job` drop, or
  LIFO/priority by last request.
- **X2 (P2). Seek-bar drags send one absolute seek per pointer move**: iced
  `widgets.rs:420-431`, GPUI `app.rs:1038-1043`, Rune `view.rs:573-577`. Seek on release and
  show the target while dragging, or throttle to one in flight.
- **X3 (P2). Four of the ten topic chips are unreachable at the 640-px minimum width.** The
  chips are clipped, not scrollable, in all three (iced `view.rs:367`, GPUI `views.rs:368`,
  Rune `view.rs:359`).
- **X4 (P2). The layouts differ, which changes the measured scroll distances.**
  - Chips: inside the scroller in iced and Rune, a fixed bar outside it in GPUI.
  - Search list width: capped at 1100 in Rune only (`pages.rs:307`).
  - Related list: GPUI loads more related videos; the others don't.
  - So "scroll N notches" covers different content in each app; Rune's search phase draws 187
    frames vs 109–111.

## Side by side

| | iced | GPUI | Rune |
|---|---|---|---|
| Scroll frame | Custom `Scroller` widget, translation only. view + layout runs in-frame every 0.5 vh of travel. | Full element rebuild + Taffy every frame (framework model), plus the monolithic root (G2). | `live_offset`, no view run. View re-runs every 0.5 vh band. |
| Virtualization | Fixed rows O(visible); comments O(n) per view (I2). | Block list + binary search, layout only on data change. | Block plan + binary search, plan only on data change. |
| Images | Zero-copy `Handle`; atlas evicts overscan, sync re-upload on view (I3). | `RenderImage` built off-thread; CPU + GPU copies; cap 140. | `Pixels::set` off-thread; cap 96 (R8). |
| Video | No app copy, `write_texture` into one texture, but full view + layout per frame (I1). | 1 copy + new atlas image per frame + full render (G2). | 1 copy on mpv's thread + framework copy + upload; no view run. |
| Measured CPU, wheel / fling / playback / scroll while playing | 12.5 / 7.8 / 31.0 / 37.3 % | 28.7 / 29.4 / 41.3 / 62.2 % | 11.9 / 9.4 / 34.0 / 37.6 % |

What the numbers come from:
- **Framework:** GPUI's per-frame layout cost during scroll. Rune's ability to move scroll and
  video without a view run. iced's need for a message (a full rebuild) for any external event.
- **Implementation:**
  - GPUI's single view (G2) and per-frame clones (G4).
  - iced's unfiltered player wakes and missing `lazy` (I1), and its pilot path (I4, which
    flatters real-input iced).
  - Rune's springs (R12) and its incremental release build (R5, unflattering).
  - iced's GPU env knob (I5, flattering on RSS).
- **Idiom:** iced and Rune are close to how a competent user would build this. GPUI fights the
  framework in one place, by not using view caching; the vendored patch is justified.

## Complexity

| | iced | GPUI | Rune |
|---|---|---|---|
| App lines (all / code) | 3,762 / 3,375 in 10 files | 3,045 / 2,736 in 7 files, + ~30-line vendored patch | 2,508 / 2,208 in 8 files |
| Largest files | app 980, view 730, scroller 681, widgets 679 | app 1,082, views 1,050, pages 489 | view 739, app 722, pages 404 |
| Crates in lockfile | 431 | 811 | 500 |
| Deps added | serde_json, tokio | gpui-component, gpui-kit-assets, async-channel, image, smallvec, serde_json | rune-kit only (Rune's own patched winit/wgpu-hal are required by Rune) |
| `unsafe` | 2× `set_var` | 1× `set_var` (+ the patch) | none |
| Framework-internal knowledge needed | Widget trait, tree diff by key, iced_wgpu layer order (badge hack), raw wgpu pipeline, downcasting `text_input::State` to unfocus | Wayland backend internals (patch), text shaper for heights | Live values, band scheme, `text::measure` |
| Hand-rolled vs kit | Ellipsis text, seek bar, spinner, search-box wrapper (iced has no line clamp) | Mostly kit (Input, Button, Slider, Spinner, Skeleton) | Volume slider (rune-kit has none), scroller (kit scroll owns its physics) |

Hardest parts, from the code:
- **iced:** the scroller (keyed diff across lanes, translated event routing) and the video
  shader.
- **GPUI:** the platform patch and manual measurement for block heights.
- **Rune:** the band/live-value split and the timing of `poll` versus input handlers (R6 is a
  residue of it).
