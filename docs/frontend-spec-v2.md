# NativeTube frontend spec, version 2

This replaces the look-and-feel parts of `docs/frontend-spec.md` and adds stability, platform and
viewing requirements. Where the two disagree, this file wins. Everything in the first spec that
is not mentioned here still holds (ground rules, pages and their behaviour, images, scrolling
through `nt_core::scroll`, benchmark hooks, verification, NOTES.md).

The goal: each app is **stable**, and looks and behaves as close to YouTube's dark desktop site
as its framework allows. An independent review of the first version is in `docs/review/`
(`ui-ux.md` with side-by-side screenshots in `ui-ux-shots/`, `frontends.md` for code findings per
app, `viewing.md`, `core.md`). Fix every P0 and P1 that names your app, and the cross-app ones.

## Shared assets (use these, do not substitute)

- `assets/fonts/Roboto-{Regular,Medium,Bold}.ttf`: embed them in the binary (`include_bytes!`)
  and use Roboto for all text. Never rely on a system font: on a machine without the font the
  first version asked for, text rendered as boxes or not at all (seen on macOS in two of the
  three apps).
- `assets/icons/*.svg`: Material Symbols Outlined, 24 px, with `*_fill1.svg` filled variants.
  Embed the ones you use. They are black paths: tint them with the text colour. No emoji and no
  Unicode symbols as icons anywhere.

## Stability (all of this is required)

- **Quit.** Ctrl+Q everywhere and Cmd+Q on macOS quit the app; closing the window quits. On
  macOS provide the standard application menu (About, Hide, Quit) if the framework supports
  native menus; if it does not, say so in NOTES.md and still handle Cmd+Q and Cmd+W yourself.
- **No panics.** No `unwrap`, `expect`, unchecked indexing or unchecked arithmetic on anything
  that depends on data, window size or timing. Install a panic hook that logs the message and
  location to stderr before the default behaviour.
- **Every state is visible.** Loading, empty, error and offline states are all visible and
  readable (the review found invisible empty states and spinners). Every error has a Retry that
  retries only what failed; retrying page data must not restart a playing video.
- **Stale answers.** Every asynchronous answer is checked against a request token for the page
  and query that asked for it before it is applied (topic switches, searches, "more" pages,
  comments, suggestions, images).
- **Playback failures** show a plain message and a Retry; a failed player start is retried on the
  next video, never disabled for the session. Opening a new video never shows the previous
  video's picture.
- **Long sessions.** Texture, image and layout caches are bounded in entries and bytes. Nothing
  grows with time or with the number of pages visited. Per-frame work during scrolling and
  playback must not grow with the number of loaded items (the review names the places).
- **Idle is idle.** Zero frames and no periodic wake-ups when nothing changes, including while
  paused, after controls hide, and when the window is unfocused or hidden.
- **Resizing** from 480x360 to 3840x2160 never panics, never produces overlapping or clipped
  text, and never refetches images for a few pixels of change (bucket the requested widths).
- **Platforms.** Linux (Wayland and X11) is what you can test. Keep the code free of
  Linux-only assumptions so it also builds and runs on macOS: no hard-coded font names or
  paths, GPU/backend environment tweaks only under `cfg(target_os = "linux")`, no
  Linux-only dependencies outside `cfg`. List anything you could not make portable.
- **Seek bar.** Dragging shows the target time and picture position, and seeks once on release
  (or at most one seek in flight), not once per pointer move.

## Viewing (new core API; rebuild `nt-core` to get it)

- `Player::set_target_size(w, h)`: call it with the drawn size in physical pixels whenever it
  changes, including on entering fullscreen and theatre mode. Auto quality is chosen from it.
- `Player::set_quality(None | Some(height))`, `Player::quality()`, and
  `PlaybackState::{height, heights}`: the settings menu lists `heights` plus "Auto" and shows
  the current one. Changing quality resumes at the same position (the core does that).
- `Player::set_speed(f64)` and `PlaybackState::speed`: the settings menu offers 0.25, 0.5,
  0.75, Normal, 1.25, 1.5, 1.75, 2.
- `PlaybackState::is_live`: show a red "LIVE" pill instead of the duration and hide the speed
  menu.
- Seeks are exact now; an expired link or a dropped connection is recovered by the core (the
  state goes to `Loading` with `buffering` and comes back), so show the spinner over the last
  picture instead of treating it as the end.
- Fullscreen: real window fullscreen through the framework, video letterboxed on black,
  controls and cursor auto-hide, F / double-click / Escape as in the Watch section.
- Not available from the core yet, so leave them out without placeholders: captions, chapters,
  seek-bar preview thumbnails, comment replies, dislike counts (show the dislike button without
  a number), verified marks.

## Verifying (in addition to the first spec's list)

- Before and after: screenshot every page at 1280x800 and compare with the YouTube reference
  screenshots in `docs/review/ui-ux-shots/yt-*.png`. Put your final screenshots in
  `apps/<fw>/shots/` and list remaining visual differences in NOTES.md with the reason for each.
- Stability run: drive the app with real input (`tools/nt-input`) for at least five minutes of
  mixed use (scroll fast, open and leave videos repeatedly, switch topics quickly, search while
  results load, resize, fullscreen in and out, change quality and speed mid-video), then check
  the log for errors and that resident memory has stopped growing.
- `./check.sh` must pass for your app (rustfmt, and Clippy with warnings as errors).
- The benchmark scenario must still finish with `"failed_waits": []`, run as
  `bench/run-scenario.sh <fw>` (it replays recorded data only).

## Look and feel (adopted from the review)

### Design tokens

| Token | Value |
|---|---|
| background | `#0f0f0f` |
| surface (dropdowns, menus) | `#212121`; menus `#282828` |
| fill (chips, pills, inputs, description box) | `rgba(255,255,255,0.10)` (= `#272727` on background) |
| fill hover | `rgba(255,255,255,0.20)` (= `#3f3f3f`) |
| search field | bg `#121212`, border 1 px `#303030`; focused border `#1c62b9`; button bg `#222222` with the same border |
| selected chip | bg `#f1f1f1`, text `#0f0f0f` |
| primary button (Subscribe) | bg `#f1f1f1`, text `#0f0f0f`; Subscribed uses fill + bell icon |
| text / secondary | `#f1f1f1` / `#aaaaaa` |
| link / call to action | `#3ea6ff` |
| divider | `rgba(255,255,255,0.20)` |
| brand red (progress, LIVE, unread dot) | `#ff0033` |
| badge | bg `rgba(0,0,0,0.6)`, radius 4, padding 1×4, text 12/18 500, inset 8 |
| radii | thumbnails 12 (compact 8), chips 8, pills 18, menus 12, avatars round |
| font | **Roboto, bundled** (Regular 400, Medium 500, Bold 700 as static TTFs); never the system default |
| text styles | 10/14 400 (rail), 12/18 400 (meta S), 13/18 500 (comment author), 14/20 400 (meta, body), 14/20 500 (chips, buttons, compact titles), 16/22 500 (card titles), 18/26 400 (search titles), 20/28 700 (headings, watch title), 36/44 700 (channel name) |
| icons | one shared 24 px SVG set (Material Symbols Outlined; Filled for the selected rail entry); no emoji or Unicode glyphs |
| spacing | 4, 8, 12, 16, 24, 40 |

### Top bar

- **Height and padding.** 56 tall; 16 px side padding.
- **Left:** Back, then the "NativeTube" wordmark (20/28 700).
- **Center:** the search field with its button, 640 px max in total (40 tall; field radius
  20 on the left; button 64 wide, radius 20 on the right). A focused field shows a 20 px
  search icon inside on the left.
- **Below 656 px:** search collapses to an icon button that expands over the bar.
- **Suggestions:**
  - Dropdown on `#212121`, radius 12, 16 px vertical padding.
  - Rows 32 tall, 16/22 text with the typed prefix at 400 and the completion at 500, and a
    20 px search icon.
  - Hover and selected rows use fill hover.
- **Shortcuts:** `/` focuses search without inserting the character.

### Navigation rail

- **Entries:** Home, Subscriptions, History, Watch Later.
- **Size and type:** 72 wide; entries 74 tall; 24 px icon, 16 px from the top; 10/14 label,
  6 px below the icon.
- **States:**
  - Selected: filled icon + `#f1f1f1` label, no background.
  - Hover: fill, radius 10.
  - Only the page actually shown is selected (none on Search, Watch, Channel).
- **Where the rail shows:**
  - Shown on Home, Subscriptions, History, Watch Later, Search and Channel when the window
    is ≥ 792 wide.
  - Hidden below 792, and always hidden on Watch.

### Home

- **Chip bar:** 56 tall and sticky under the top bar. Chips are 32 tall with 12 px padding and
  a 12 px gap. When the bar overflows: horizontal wheel/drag scroll, a 50 px fade at the
  cut-off edge, and a 40 px circular "›" / "‹" button.
- **Grid:**
  - Columns = 1 below 600, 2 below 900, 3 below 1600, 4 below 2300, otherwise 5.
  - Cards fill the row exactly (no dead gutter).
  - Column gap 16, row gap 40, content padding 24 (16 below 600).
- **Card:**
  - 16:9 thumbnail, radius 12.
  - Then, 12 px below: avatar 36 with a 12 px gap.
  - Title: 16/22 500, up to 2 lines with ellipsis; no reserved second line.
  - Channel: 14/20 secondary.
  - Meta line: 14/20 secondary.
- **Hover:** no card background. Show a 40 px "⋮" button at the top right of the text column,
  and the hand cursor over the whole card.
- **Press:** darken the thumbnail 10% for 100 ms.
- **Watched videos** show a 4 px brand-red progress bar along the bottom of the thumbnail.
- **Loading:** skeleton cards (thumbnail + avatar + two lines in fill), never a lone spinner.

### Search results

- **Content area:** centered, max 1096 wide.
- **Rows:**
  - Thumbnail width = clamp(240, 40% of the content width, 500), 16:9, radius 12.
  - 16 px gap to the text.
  - Rows 16 apart.
- **Text column:**
  - Title: 18/26 400, 2 lines.
  - Meta: 12/18 secondary.
  - Channel row: 12 px vertical padding, avatar 24, 8 px gap, 12/18 name with a verified mark
    when known.
  - Snippet: 12/18 secondary, 1 line, with query terms at 500 in `#f1f1f1`.
  - "⋮" appears on hover.

### Watch

- **Layout:**
  - No rail.
  - Two columns when the window is ≥ 1017 wide: outer padding 24, gap 24, secondary column
    between 300 and 402.
  - The primary column is at most `(window_h − 169) × 16/9` wide, and the two columns are
    centered as a block.
  - Below 1017: one column, related videos after the description and before comments.
- **Player:**
  - 16:9, radius 12 in the default view, black. Click toggles pause and shows a 52 px center
    icon that fades in 500 ms.
  - Double-click toggles fullscreen.
  - Controls appear on pointer movement and hide after 3 s without movement while playing,
    taking the cursor with them; they stay while paused or while a menu is open.
- **Control bar:**
  - 48 tall over a bottom gradient (transparent → `rgba(0,0,0,0.6)`).
  - **Left:** play/pause, then mute with a 52 px volume slider that slides out on hover, then
    time `0:00 / 0:00` (13 px).
  - **Right:** settings (speed 0.25–2, quality Auto/1080/720/480), captions when available,
    theatre, fullscreen.
  - All icons 24 px in 40 px hit targets.
- **Scrubber:**
  - 3 px tall, 5 px on hover.
  - Track `rgba(255,255,255,0.2)`, buffered `rgba(255,255,255,0.4)`, played brand red.
  - 13 px red knob on hover only.
  - Hover tooltip with the time.
  - Chapter gaps of 2 px when chapters exist.
- **Keys:** Space/K, J/L, ←/→, ↑/↓, M, F (fullscreen), T (theatre), C (captions),
  Shift+`<`/`>` (speed), `,`/`.` (frame step while paused), 0–9 (seek to 0–90%), Home/End,
  Escape (leave fullscreen).
- **Theatre mode:** the player spans the full window width on black, height
  min(56.25% of the width, window_h − 169). The columns continue below.
- **End of video:** show a "Up next" card with a 5 s countdown to the first related video
  (cancelable). Autoplay can be turned off and the choice is remembered.
- **Title and owner row:**
  - Title: 20/28 700, 12 px below the player.
  - Owner row, 12 px below: avatar 40, name 16/22 500, subscribers 12/18 secondary, then
    Subscribe (primary button, 36 tall, 16 px padding).
  - Right side: a like | dislike segmented pill (36 tall, fill, 1 px divider), Share (copies
    the link) and Watch Later.
- **Description:**
  - Fill background, radius 12, padding 12.
  - First line: 14/20 500.
  - Body: 14/20, 3 lines, with an inline "…more" at 500.
  - URLs, #tags and timestamps in link colour; timestamps seek.
- **Comments:**
  - Header "N Comments" 20/28 700.
  - Each comment: avatar 40; "@author" 13/18 500 primary with the time 12/18 secondary; text
    14/20 clamped to 4 lines with "Read more".
  - Like count 12/18; "N replies" 14/20 500 in link colour with a chevron, which expands the
    replies.
- **Related:**
  - Compact rows: thumbnail 168×94 radius 8, 8 px gap.
  - Title 14/20 500, 2 lines; channel and meta 12/18 secondary.
  - Rows 8 apart.

### Channel

- **Content area:** centered, max 1284 wide.
- **Banner:** radius 16, height = width / 6.2.
- **Header:**
  - Avatar 160 (72 below 600).
  - Name 36/44 700.
  - "@handle • subscribers • videos" 14/20 secondary.
  - Description 14/20 secondary, 1 line + "…more".
  - Subscribe (primary button) under the text.
- **Tabs:** "Videos" only for now, 48 tall, 16/22 500, with a 2 px `#f1f1f1` underline under
  the active tab.
- **Grid:** cards drop the avatar and channel name.

### States

- **Empty states:** centered within the page: a 120 px icon, a 20/28 700 title, 14/20
  secondary text, and an optional action button.
- **Errors:** "Something went wrong" + a one-line plain explanation ("Couldn't reach YouTube",
  "This video is unavailable") + Retry. Raw error text goes to the log only.
- **Keyboard focus:** every interactive element shows a 2 px `#f1f1f1` outline (offset 2) on
  keyboard focus only. Tab order: top bar → rail → content.
- **Navigation:** Forward (Alt+Right, mouse forward) re-pushes the last page popped by Back.
