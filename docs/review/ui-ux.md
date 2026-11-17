# UI/UX review: does it look and feel like YouTube?

Reviewer lens: visual fidelity, interaction feel, text rendering and missing features in the three
apps (`nt-iced`, `nt-gpui`, `nt-rune`), compared with the real site, logged out, at the same
1280×800 window. Screenshots are in `docs/review/ui-ux-shots/`.

## How this was checked

- **What I ran.** I ran the release binaries built from the current tree (commit 66f442f) through
  `tools/headless.sh run` with a scratch `NT_DATA_DIR`, muted and replaying `.cache/fixtures`.
  I drove them with `nt-input`: clicks, wheel, keys and typing.
- **Pages covered.** Home, search suggestions, search results, watch (hover, paused and
  comments), channel, Back, Subscriptions, History and Watch Later. I also covered the empty and
  error states (`NT_FIXTURE_MODE=replay` plus a query with no fixture, so no network) and the
  Subscribe and Watch Later toggles.
- **Window widths.** I tested 640 px for all three, and 1000 px for GPUI only, by floating and
  resizing the windows. 1920 px is reasoned from the code only.
- **The YouTube reference.** I used Helium 0.21 (Chromium 157), logged out, on NT-BENCH. It
  loaded four pages: a search for "rust programming", the "Rust in 100 Seconds" watch page,
  `@Fireship` and `@Fireship/videos`. Browser chrome takes the top 66 px, so YouTube's viewport
  is 1280×734.
- **Clean-up.** The browser, all apps and the NT-BENCH output are gone.
- **Hover screenshots.** `nt-input` warps the cursor back after every action, so hover states
  were captured with a scratch helper. It moves the cursor onto the test output with
  `hyprctl dispatch hl.dsp.cursor.move`, takes a screenshot with `grim`, then moves the cursor
  back.
- **Measuring.** Sizes and colours are from pixel sampling (ImageMagick and numpy) on the
  screenshots. The scroll distance per notch was found by cross-correlating screenshots taken
  before and after the notch.
- **Labels used below.**
  - "Verified" means I saw it on screen.
  - "Code" means I concluded it from reading the source.
  - [spec] means the app followed `docs/frontend-spec.md` and the spec is what's wrong.
  - [app] means the app itself is at fault.
  - [fw] means a framework limit.
- **Edge zones.** Clicks near the output edges were swallowed for GPUI only: the rail at x=36 and
  the top bar at y=28. iced and Rune took the same clicks.
- **Source line numbers refer to commit 66f442f (HEAD).** At 16:20, after my runs, the working
  tree changed under `apps/`, `core/` and `tools/`: rustfmt plus about 4,500 inserted lines, not
  by me. Lines in the working copy have moved. Everything was observed on the binaries built at
  11:20 from HEAD.

## Verdict

None of the three reads as YouTube. The shared reasons, in order of how much they hurt:

1. **The watch page is a different design.**
   - The rail and a 96 px left margin stay on the watch page, so the player is **736×414**.
     YouTube's is **874×492** in the same window (`*-watch.png` vs `yt-watch.png`).
   - Subscribe is a red pill; YouTube's is white.
   - The player has none of YouTube's chrome beyond play, mute, volume, time and seek. There is
     no fullscreen, theatre mode, settings, captions, chapters or autoplay.
   - The controls never auto-hide while the pointer rests on the video.
2. **The typography is not YouTube's, and differs between the three apps.**
   - The spec says "the framework's default sans". That gave Noto Sans in iced and Adwaita Sans
     in GPUI (with every bold and medium weight silently lost). Rune bundles Inter. YouTube uses
     Roboto 400/500/700.
3. **The iconography is not YouTube's.**
   - iced uses Unicode and colour-emoji glyphs (🔍 👍 🔇 ⌂ ▤ ⟲ ◷).
   - GPUI uses Lucide, and Rune uses its own outline set.
   - None uses filled icons for the selected state, as YouTube does.
4. **The spec bakes in several non-YouTube choices**:
   - a grey "card highlight" on hover;
   - an accent of `#ff3b30` (Apple red; YouTube's red is `#ff0033`);
   - a red Subscribe;
   - 12 px rail labels, which clip "Subscriptions";
   - a 360 px search thumbnail at every width;
   - a 1000 px two-column threshold;
   - no sticky chip bar, no fullscreen and no theatre mode.
5. **Responsive behaviour is not YouTube's.** The rail never hides. At 640 px, iced and GPUI show
   one 420 px column with a dead gutter, and GPUI's search field collapses to about 150 px.

**Closest to YouTube today: Rune.**
- It has the strongest weight hierarchy (Inter at real 500/600/700 weights).
- Its rail labels are 10 px and fit, like YouTube's.
- Its empty states are well formed, and its narrow grid keeps two columns.
- It has proper outline icons.

**GPUI is a close second.**
- Its search box (`#121212` field with a `#303030` border) and its chip-bar position match
  YouTube.
- It is the only app whose chips stay pinned while scrolling, as on YouTube.
- But every bold weight renders as Regular, which flattens the whole page.

**iced is furthest.**
- Its icons are emoji and Unicode glyphs.
- Its empty states, and probably its loading spinners, are invisible.
- Noto Sans is wider than Roboto, so its 12–14 px metadata reads larger and looser.

None of the gaps below is a framework limit. All three frameworks have fullscreen, cursor
shapes, SVG or icon support, font loading and focus rings (see "App vs framework").

---

## Findings

### P0: broken or misleading

**P0-1 [iced][app] Empty states, and almost certainly loading spinners, are invisible.**
- **Verified.**
  - Subscriptions with no subscriptions renders a completely blank page
    (`iced-subs-empty.png`).
  - Watch Later with nothing saved shows only its "Watch Later" heading and no message
    (`iced-later-empty.png`).
  - Rune and GPUI show their messages (`compare-rune-history-later.png`,
    `compare-gpui-1000-library.png`).
- **Cause (code).** `empty()` is
  `container(..).width(Fill).height(200).center(Fill)` (`apps/iced/src/view.rs:315`).
  `center(Fill)` resets the height to `Fill` inside the scroller's unbounded lane, so the text
  is laid out outside the visible band.
- **Same construct elsewhere.** The first-load spinner (`view.rs:337`), the infinite-scroll
  spinner row (`:348`) and the comments spinner (`:547`) use the same pattern. I couldn't see a
  loading state because fixtures load instantly, but they should be assumed invisible too.
- **Why it matters.** A user sees a dead page. This includes the first launch with an empty
  library, and every slow network load.
- **Fix.** Use `.center_x(Fill).center_y(Length::Fixed(h))`, or `.align_x/align_y`, keeping
  the fixed height.

**P0-2 [all][spec+app] No fullscreen and no theatre mode.**
- **What's missing.**
  - No F key, double-click or button for fullscreen (`grep fullscreen apps/*/src` finds
    nothing).
  - No theatre mode either. The largest picture possible at 1280×800 is 736×414.
- **Why it matters.** This is the first thing a YouTube user reaches for, and the owner's bar is
  "viewing experience uncompromised".
- **Frameworks support it.** iced has `window::set_mode(Mode::Fullscreen)`, GPUI has
  `Window::toggle_fullscreen`, and Rune's own window chrome calls `set_fullscreen`
  (`rune/src/window/chrome.rs:91` in the pinned checkout). This is purely an omission.
- **A caveat once it exists.** The core caps streams at 720p (`core/src/resolve.rs:89`) and
  the render target at 1280×720 by default. A fullscreen 1920×1200 picture would then be
  upscaled 720p. The quality cap needs a UI ("Auto / 1080p / 720p …") or a size-driven default.

### P1: clearly below the bar

**P1-1 [all][spec] The watch page keeps the navigation rail, so the player is about 28% smaller
than YouTube's.**
- **Verified at 1280×800.**

  | | Player | Left edge | Related column |
  |---|---|---|---|
  | YouTube | 874×492 | x=20 | 910–1260 |
  | All three apps | 736×414 | x=96 | 856–1256 |

  (`compare-watch-at-300ms.png`, `yt-watch.png`)
- **Cause.** YouTube hides the guide on watch pages; it moves into a drawer behind the hamburger
  button. The spec gives every page the rail plus 24 px of padding.
- **Fix.** On Watch, drop the rail (keep Back) and use 24 px outer padding. Size the primary
  column to `window − 24 − 24 − 24 − secondary`, with the secondary column between 300 and
  402 px. Add theatre mode (see the spec section).

**P1-2 [all][spec] Subscribe is a red pill; YouTube's is white.**
- **Apps (verified):** red `#ff3b30` background with white text.
- **YouTube (sampled from `yt-watch.png` and `yt-channel.png`):**
  - Subscribe: `#f1f1f1` background, `#0f0f0f` text, 36 px tall, 18 px radius.
  - Subscribed: `rgba(255,255,255,.1)` with a bell and a chevron.
- **Why it matters.** The red Subscribe is the most visibly "not YouTube" element on the watch
  and channel pages (`compare-subscribed-saved.png`).
- **Fix.** Change the spec token.

**P1-3 [all][spec] Typography: wrong family, and different in each app.**
- **What YouTube uses.** Roboto at these sizes and weights:

  | Element | Size / line height | Weight |
  |---|---|---|
  | Grid title | 16/22 | 500 |
  | Search title | 18/26 | 400 |
  | Watch title | 20/28 | 700 |
  | Metadata | 14/20 or 12/18 | 400 |

- **What the apps resolve "default sans" to:**
  - **iced:** Noto Sans (and maps every system font, per its NOTES).
  - **GPUI:** Adwaita Sans, the desktop UI font.
  - **Rune:** its bundled Inter.
- **Visible effects (`compare-card-text-zoom.png`, `compare-search-row-text-zoom.png`).**
  - iced's Noto is about 6–8% wider than Roboto at the same size. "5.3M views • 4 days ago"
    runs longer, and titles wrap earlier.
  - iced and GPUI set the search-result title in Medium; YouTube uses 400.
- **Why it matters.** Typography is most of the "feel" at reading distance. It also breaks the
  owner's requirement that the three apps look the same.
- **Fix.** Bundle Roboto as static TTFs (Regular, Medium, Bold; Apache-2.0) in each app, and
  register them with the framework:
  - iced: `font::load`
  - GPUI: `text_system().add_fonts`
  - Rune: its font registration

  Then set every text style from the spec table below.

**P1-4 [gpui][app] Every Medium, Semibold and Bold weight renders as Regular.**
- **Verified.**
  - "History" (`FontWeight::BOLD`, `views.rs:560`) renders at the same stroke weight as body
    text. Rune's 700 is clearly bold (`gpui-history.png` vs `rune-history.png`).
  - The same holds for the watch title (`SEMIBOLD`, `views.rs:928`), the card titles, the chips
    and "NativeTube" (`gpui-watch.png`, `compare-subscribed-saved.png`).
- **Cause.**
  - The process maps only `AdwaitaSans-Regular.ttf`. It is a variable font: fc-query lists
    named instances, and there is no separate Bold file.
  - GPUI's Linux text stack picks that face for every weight and doesn't apply the `wght` axis.
    This is my conclusion from the rendering plus the font file.
- **Why it matters.** The whole hierarchy flattens. This is the main reason GPUI "looks
  cheaper" than Rune.
- **Fix.** The Roboto static files from P1-3 fix it.

**P1-5 [iced][app] Icons are Unicode and colour-emoji glyphs.**
- **Glyphs used:**
  - rail: `⌂ ▤ ⟲ ◷` (`view.rs:197`)
  - search button: 🔍 (`:153`)
  - like: 👍 (`:678`)
  - player: `▶ ❚❚` and 🔇 / 🔊 (`:629`, `:635`)
  - Watch Later: `+ / ✓` (`:685`)
- **Verified.** 🔍, 👍 and 🔇 render as Noto Color Emoji: blue-grey, beige and orange with a red
  slash (`iced-unicode-icons.png`).
- **Why it matters.** It looks broken next to a YouTube-like layout.
- **Fix.** iced has an `svg` widget (enable the `svg` feature). Use one shared SVG icon set across
  all three apps, defined in the spec, with filled variants for the selected rail entry.

**P1-6 [all][app+spec] The player controls never auto-hide, and the chrome is thin.**
- **Auto-hide (code).** The controls show whenever the pointer is over the player:
  - iced: `hover(base, controls)`, `view.rs:654`
  - Rune: `player_hover`, `view.rs:557`
  - GPUI: hover

  There's no idle timer, so a pointer resting on the video covers the bottom 80 px for the whole
  video. YouTube hides the controls and the cursor after about 3 s without movement while
  playing.
- **What's missing compared with YouTube (`compare-player-controls.png`):**
  - the settings menu (speed, quality)
  - captions
  - chapter segments on the scrubber
  - the hover time tooltip (and storyboard preview)
  - the scrubber growing from 3 to 5 px on hover, with a 13 px knob that appears on hover only
  - the volume slider expanding only on hover
  - the center play/pause bezel on click
  - autoplay and up next: at the end, the player just stops at 1:00/1:00
  - theatre, fullscreen and miniplayer buttons
  - keyboard keys F, T, C, `<`/`>`, `,`/`.`, 0–9, Home/End and Shift+N
- **GPUI's volume slider** is gpui-component's default: a white knob with a black center. It
  isn't YouTube's, and it is always expanded.
- **Fix.** Spec the player bar (see the spec section). Adding auto-hide is about 20 lines per
  app.

**P1-7 [all][spec] The search results layout doesn't match YouTube at 1280.**

| | YouTube (`yt-search-2.png`) | Apps (`*-search.png`) |
|---|---|---|
| Thumbnail | 500×281 (flex, 240–500 px wide) | fixed 360×202 |
| Content area | centered, about 1150 px wide | 96–1256, flush left, the right third of every row empty |
| Title | 18/26 at 400 weight | Medium in iced and GPUI |
| Snippet | 1 line, query terms in bold | 2 lines, no bold |
| Verified tick | yes | no |
| "⋮" menu | yes | no |
| Badges ("New", "4K", "CC") | yes | no |
| Gap between rows | 16 | 16 (fine) |

**P1-8 [all][spec] Card hover paints a grey `#272727` box behind the whole card.**
- **Verified** (`compare-card-hover.png`). The spec calls for it ("Hover highlights the card").
- **What YouTube does.** It paints no card background on the home or channel grid. Hover reveals
  the "⋮" menu and, after about 1 s, a muted inline preview. Compact and search rows don't get a
  background either.
- **Why it matters.** The grey box makes the grid feel like a desktop list view.
- **Fix.**
  - On hover, show the "⋮" button (and optionally a 4 px focus outline for keyboard focus only).
  - Press feedback: the card scales to 0.98 or darkens slightly for 100 ms.

**P1-9 [all][spec] The navigation rail.**
- **Label clipping (verified).** "Subscriptions" is clipped to "ubscriptions" in iced and GPUI.
  The spec forces 12 px, the smallest spec size, into a 72 px rail. Rune used 10 px, which is
  YouTube's size, and fits.
- **Selected state.** All three mark it with a 64×64 `#272727` box. YouTube's mini guide marks it
  with a filled icon and white label, and no background. The `rgba(255,255,255,.1)` background
  at radius 10 is for hover only.
- **Entry size.** YouTube's entries are 74 px tall (icon 24, label 10/14, 16 px top padding).
  The apps' are 64–68 px.
- **Rune also highlights "Home" on Search, Watch and Channel pages** (`rune-search.png`). The
  stack root isn't the current page; YouTube highlights nothing there.

**P1-10 [all][spec] Responsive layout.**
- **At 640 (verified, `compare-home-640.png`):**
  - The rail and the "NativeTube" title stay. YouTube hides the mini guide below 792 px and
    collapses search to an icon below 656 px.
  - iced and GPUI show one 420 px card column with about 100 px of dead space on the right.
    This is the spec's 300–420 card rule.
  - GPUI's search field shrinks to about 150 px with the query clipped ("rust progran ×").
  - The chip row is hard-clipped mid-word ("Scienc") with no fade and no "›" arrow.
- **At 1000 (GPUI verified, `compare-gpui-1000-library.png`; iced and Rune from code: they
  share the formula, `iced/src/view.rs:522`, `rune/src/pages.rs:331`):**
  - The two-column watch layout leaves a 456 px primary column.
  - The player is about 456×256, and the channel name is cut to "free…" and "12M su…".
  - YouTube switches to one column below about 1017 px and lets the secondary column shrink to
    300.
- **At 1920 (code):**
  - **Home:** 5 columns of about 347 px (GPUI `pages.rs:164`).
  - **Watch:** the primary column is about 1376 px wide, so comment and description lines run
    about 1376 px long.
  - **Player at 1080 px tall:** height is capped at 70% (756 px), so the 16:9 picture is
    pillarboxed inside a wider box.
  - **YouTube instead** caps the player at `calc((100vh − 169px) × 16/9)` and centers the
    two-column layout.

**P1-11 [gpui][app] The `/` shortcut sometimes types "/" into the search field.**
- **Verified.** Second run: `/`, then typing "rust programming", searched for
  "/rust programming" (`gpui-slash-leak.png`). The first run didn't leak.
- **Fix.** Consume the key event after focusing the field.

### P2: polish

- **[iced, GPUI][app] Titles always reserve two lines.**
  - iced `Clamp::reserve()` (`view.rs:418`); GPUI fixed row heights.
  - A one-line title leaves a blank line before the channel name ("Fall Out Boy - ACES",
    `gpui-suggest.png` run, `iced-home-640.png`). YouTube doesn't reserve the line. Rune is
    correct.
- **[iced, Rune][spec] The chip bar scrolls away.** YouTube pins it under the top bar, as GPUI
  does (`apps/*/shots/02-home-scrolled.png`).
- **[Rune][app] No hand cursor anywhere.** The cursor is an arrow over cards, links and buttons
  (`compare-card-hover.png`). Rune has `El::cursor(Cursor)`; iced and GPUI show the hand.
- **[GPUI][app] Hover sticks after the pointer leaves the window.**
  - The card highlight stays on the channel page (`gpui-channel.png`), the "Saved" button stays
    lit, and the player controls stay up (`compare-watch-640.png`).
  - iced and Rune clear hover on leave.
  - Seen under nt-input's cursor warp; a real pointer leaving the window should reproduce it.
- **[all][app] No visible keyboard focus.** Tab ×3 shows no focus ring in iced or GPUI; Rune only
  focuses the search field (`compare-focus-tab3.png`). YouTube draws a 2 px white outline on
  each focus stop.
- **[all][app] Error states show the raw request** ("Missing fixture: POST search
  {"params":…}", `compare-error-state.png`). Real network errors will show similar internals.
  YouTube shows a friendly line plus Retry.
- **[iced, Rune][app] Double ellipsis in the description.** The description ends "rust-i......"
  because YouTube's shortened link text already ends in "...", and the clamp adds another
  (`iced-watch.png`, `rune-watch.png`).
- **[all][app] Description links, hashtags and timestamps are plain text.** YouTube shows them in
  `#3ea6ff`, clickable; timestamps seek the player. "Show more" sits on its own line instead of an
  inline "…more".
- **[GPUI][app] The description box leaves about 24 px of empty space above "Show more"**
  (`gpui-watch.png`). The measured height doesn't match the drawn height.
- **[all][app] Comments.**
  - Long comments aren't clamped (GPUI shows a 12-line timestamp comment). YouTube clamps to
    4 lines with "Read more".
  - "N replies" is static text. In GPUI it's blue but does nothing. YouTube's is blue with a
    chevron and expands the thread.
  - Author handles are grey. YouTube's are 13 px/500 in `#f1f1f1`.
- **[all][spec] Accent red.** The accent is `#ff3b30`; YouTube's is `#ff0033`. The duration badge
  is black at 80% opacity; YouTube's is `rgba(0,0,0,.6)`.
- **[all][spec] Channel page (`compare` of `*-channel.png` with `yt-channel.png`).**

  | | YouTube | Apps |
  |---|---|---|
  | Avatar | 160 | 80 |
  | Name | 36 px bold | 20 px |
  | Content width | max 1284, centered | full width |
  | Banner | radius 16 | radius 12 (GPUI's is about 190 tall; spec says 160) |
  | Tabs (Home / Videos / Shorts / Playlists) | yes | no |

  The channel's own grid repeats the channel avatar and name on every card; YouTube drops both
  there. In Rune, Subscribe floats to the far right edge.
- **[all][app] Search suggestions.** YouTube bolds the completed part of each suggestion (the
  typed prefix stays regular), draws the dropdown on `#212121` with radius 12, and uses 32 px
  rows. iced draws the magnifier as an emoji.
- **[all][spec] Watch-page action row.** The apps have only Like (count) and Watch Later.
  YouTube has a segmented like/dislike pill, Share and Save. A dislike button, even without a
  count, and Share (copy link) are cheap.
- **[Rune][app] History rows have no duration badge or snippet** (`rune-history.png`). iced has
  both.
- **[GPUI][app] The watch channel avatar is a grey circle** when `WatchInfo.channel` is empty
  (`compare-watch-640.png`). The card's avatar could fill it, as iced does for the name.

### Fine as is (verified)

- **Colours:** background `#0f0f0f`, text `#f1f1f1` / `#aaaaaa`, chip `#272727`, selected chip
  `#f1f1f1` all match YouTube exactly.
- **Home grid at 1280:** three columns of 376 px, 16 px gap, avatar 36 — matches.
- **Wheel scrolling:** one notch moves exactly 120 px in all three apps, as in Chrome. The
  overlay scrollbar (about 6 px, rounded, shown while moving) is reasonable
  (`compare-scrollbar.png`).
- **Back** restores the watch page and resumes the video within 200 ms in all three
  (`compare-back-to-watch.png`).
- **Page transitions** from replayed data render within 150 ms; the first video frame within
  300 ms.
- **The suggestions dropdown** works with typing in iced, Rune and GPUI (`*-suggest.png`).
- **Subscribe and Watch Later** toggle correctly.

---

## Page-by-page measurements (1280 wide; YouTube viewport 1280×734)

| Element | YouTube (measured) | iced | GPUI | Rune |
|---|---|---|---|---|
| Top-bar search field | 40 h, bg `#121212`, 1 px `#303030`, button 64 w `#222222`, mic 40 `#272727` | 38 h, bg `#272727`, border `#3e3e3e`, button `#222` | 40 h, bg `#121212` (match), button `#272727` | 40 h, bg `#272727`, no border, detached circular button |
| Chip row y (window) | 12 px below the top bar (68–100), sticky | 80–111, scrolls away | 68–99, sticky | 80–111, scrolls away |
| Chip | 32 h, radius 8, 14/20 500 | 32 h, Noto 500 | 32 h, weight renders 400 | 32 h, Inter 500 |
| Rail entry | 74 h, 10 px label, filled icon when selected, no background | 68 h, 12 px label clipped, glyph icons, grey box | 76 h, 12 px label clipped, Lucide, grey box | 72 h, 10 px label, own SVG, grey box |
| Card title | Roboto 16/22 500 | Noto 16/22 500 | Adwaita 16/22, renders 400 | Inter 16/22 500 |
| Row gap (text bottom → next thumbnail) | about 40 | about 34 (+22 when the title is 1 line) | about 34 (+22) | about 34 |
| Search thumbnail | 500×281 | 360×202 | 360×202 | 360×202 |
| Watch player | 874×492, x=20, no rail | 736×414, x=96 | 736×414 | 736×414 |
| Watch title | 20/28 700 | 20/28 500 | 20/28, renders 400 | 20/28 600 |
| Subscribe | `#f1f1f1` / `#0f0f0f`, 36 h | red | red | red |
| Related thumbnail | 168×94 (classic) | 168×94 | 168×94 | 168×94 |
| Channel avatar / name | 160 / 36 px 700 | 80 / 20 | 80 / 20 | 80 / 20 |

## Interaction feel

- **Hover.** All three paint the spec's grey box (P1-8). GPUI's hover sticks (P2). YouTube's only
  hover cues are the "⋮" menu, the inline preview and the hand cursor.
- **Press feedback and transitions.** None of the three has press feedback or page transitions;
  pages swap instantly. Instant is right for YouTube page swaps, but buttons should darken on
  press. The Rune player controls fade (Rune transitions). The iced and GPUI controls pop in and
  out.
- **Loading.**
  - GPUI uses skeleton cards (`views.rs:482`), which is YouTube's style.
  - Rune uses a spinner.
  - iced's spinner is likely invisible (P0-1).
  - The spec allows "skeleton or spinner". It should require skeletons for grids and lists, as
    YouTube does.
- **Latency.** Everything replayed shows within 150–300 ms (above). Live latency is outside this
  review.
- **Scrolling.** 120 px per notch in all three (verified). The physics are the shared core port;
  I didn't compare the curves frame by frame. Wheel over the Rune top bar or rail scrolls the
  page (its NOTES); a browser wouldn't.
- **Keyboard.**
  - The watch keys (Space/K/J/L/M/arrows) work. K was verified in all three.
  - `/` focuses search in iced and GPUI (GPUI leaks it, P1-11).
  - There is no Tab traversal with visible focus.
  - Escape closes suggestions.
- **Back/forward.** Alt+Left verified in all three. There is no Forward (Alt+Right / mouse
  forward). YouTube in a browser has it; add a forward stack.

## Text rendering

- **iced** (cosmic-text, `crisp` feature): sharp, grayscale anti-aliasing, the most "hinted"
  look. Noto Sans is wider and taller than Roboto, so 12–14 px metadata looks large.
- **GPUI:** uses RGB subpixel anti-aliasing. Colour fringes (blue left, orange right) are visible
  on stems in a screenshot (`gpui-history.png`). On a real LCD in RGB order that's fine; on
  OLED, rotated or scaled outputs it fringes. The weight loss (P1-4) makes its text look thin
  and grey.
- **Rune:** Inter with real weights, grayscale anti-aliasing, crisp at 12–20 px. It looks the
  most like a browser.
- **Image scaling:** thumbnails are clean in all three (no visible aliasing at 376 px; the core
  resizes to the drawn physical width). Thumbnails are cover-cropped, so there are no black bars
  except where the source image itself has them.

## Features a YouTube user would miss, ranked

1. Fullscreen (F, double-click).
2. Theatre mode (T).
3. Controls and cursor that auto-hide while playing.
4. Playback speed.
5. Quality selection, and anything above 720p.
6. Captions (C).
7. Autoplay / up next at the end of a video.
8. Chapters: segments on the scrubber, and timestamps in the description and comments that seek
   when clicked.
9. Expandable comment replies.
10. A hover preview on the seek bar (time tooltip at minimum).
11. Clickable links and hashtags in descriptions.
12. Shorts, playlists and channel tabs.
13. Forward navigation.
14. Resume position on thumbnails (the red progress bar along the bottom of watched thumbnails).
    The library already records history.
15. "Not interested", Share and "Save to playlist" menus (⋮).

Items 1–3 are table stakes for "viewing experience uncompromised". 4–8 are what make it feel
like YouTube rather than a feed viewer.

## App vs framework

Nothing in this list is blocked by a framework.

| Need | iced 0.14 | GPUI (gpui-pre 0.3.12) | Rune (4fbd2d6) |
|---|---|---|---|
| Fullscreen | `window::set_mode` | `Window::toggle_fullscreen` | `set_fullscreen` (used by its chrome) |
| Cursor shape | `mouse::Interaction` (used) | `cursor_pointer` (used) | `El::cursor` (unused by the app) |
| Custom fonts | `font::load` | `text_system().add_fonts` | font registration |
| SVG icons | `svg` widget (feature) | `svg()` + kit icons | `icon()` (used) |
| Focus ring | widget style / custom | `focus` styles | `El::focus_ring` |
| Line clamp + ellipsis | none built in; app wrote `Clamp` | app's `clamp_lines` | `max_lines` |

The framework-caused issues are two:

- **iced's lack of built-in ellipsis:** solved in the app.
- **GPUI's variable-font weight handling:** solved by shipping static weights.

Both have easy workarounds. iced's layer order (text above quads) forced the badge trick, but
the badge looks right.

---

## Proposed spec changes (adoptable verbatim)

Replace "Design tokens", the Top bar, Navigation rail, Home, Search results, Watch and Channel
sections with the following. Everything else in the spec stays.

> ### Design tokens
>
> | Token | Value |
> |---|---|
> | background | `#0f0f0f` |
> | surface (dropdowns, menus) | `#212121`; menus `#282828` |
> | fill (chips, pills, inputs, description box) | `rgba(255,255,255,0.10)` (= `#272727` on background) |
> | fill hover | `rgba(255,255,255,0.20)` (= `#3f3f3f`) |
> | search field | bg `#121212`, border 1 px `#303030`; focused border `#1c62b9`; button bg `#222222` with the same border |
> | selected chip | bg `#f1f1f1`, text `#0f0f0f` |
> | primary button (Subscribe) | bg `#f1f1f1`, text `#0f0f0f`; Subscribed uses fill + bell icon |
> | text / secondary | `#f1f1f1` / `#aaaaaa` |
> | link / call to action | `#3ea6ff` |
> | divider | `rgba(255,255,255,0.20)` |
> | brand red (progress, LIVE, unread dot) | `#ff0033` |
> | badge | bg `rgba(0,0,0,0.6)`, radius 4, padding 1×4, text 12/18 500, inset 8 |
> | radii | thumbnails 12 (compact 8), chips 8, pills 18, menus 12, avatars round |
> | font | **Roboto, bundled** (Regular 400, Medium 500, Bold 700 as static TTFs); never the system default |
> | text styles | 10/14 400 (rail), 12/18 400 (meta S), 13/18 500 (comment author), 14/20 400 (meta, body), 14/20 500 (chips, buttons, compact titles), 16/22 500 (card titles), 18/26 400 (search titles), 20/28 700 (headings, watch title), 36/44 700 (channel name) |
> | icons | one shared 24 px SVG set (Material Symbols Outlined; Filled for the selected rail entry); no emoji or Unicode glyphs |
> | spacing | 4, 8, 12, 16, 24, 40 |
>
> ### Top bar
>
> - **Height and padding.** 56 tall; 16 px side padding.
> - **Left:** Back, then the "NativeTube" wordmark (20/28 700).
> - **Center:** the search field with its button, 640 px max in total (40 tall; field radius
>   20 on the left; button 64 wide, radius 20 on the right). A focused field shows a 20 px
>   search icon inside on the left.
> - **Below 656 px:** search collapses to an icon button that expands over the bar.
> - **Suggestions:**
>   - Dropdown on `#212121`, radius 12, 16 px vertical padding.
>   - Rows 32 tall, 16/22 text with the typed prefix at 400 and the completion at 500, and a
>     20 px search icon.
>   - Hover and selected rows use fill hover.
> - **Shortcuts:** `/` focuses search without inserting the character.
>
> ### Navigation rail
>
> - **Entries:** Home, Subscriptions, History, Watch Later.
> - **Size and type:** 72 wide; entries 74 tall; 24 px icon, 16 px from the top; 10/14 label,
>   6 px below the icon.
> - **States:**
>   - Selected: filled icon + `#f1f1f1` label, no background.
>   - Hover: fill, radius 10.
>   - Only the page actually shown is selected (none on Search, Watch, Channel).
> - **Where the rail shows:**
>   - Shown on Home, Subscriptions, History, Watch Later, Search and Channel when the window
>     is ≥ 792 wide.
>   - Hidden below 792, and always hidden on Watch.
>
> ### Home
>
> - **Chip bar:** 56 tall and sticky under the top bar. Chips are 32 tall with 12 px padding and
>   a 12 px gap. When the bar overflows: horizontal wheel/drag scroll, a 50 px fade at the
>   cut-off edge, and a 40 px circular "›" / "‹" button.
> - **Grid:**
>   - Columns = 1 below 600, 2 below 900, 3 below 1600, 4 below 2300, otherwise 5.
>   - Cards fill the row exactly (no dead gutter).
>   - Column gap 16, row gap 40, content padding 24 (16 below 600).
> - **Card:**
>   - 16:9 thumbnail, radius 12.
>   - Then, 12 px below: avatar 36 with a 12 px gap.
>   - Title: 16/22 500, up to 2 lines with ellipsis; no reserved second line.
>   - Channel: 14/20 secondary.
>   - Meta line: 14/20 secondary.
> - **Hover:** no card background. Show a 40 px "⋮" button at the top right of the text column,
>   and the hand cursor over the whole card.
> - **Press:** darken the thumbnail 10% for 100 ms.
> - **Watched videos** show a 4 px brand-red progress bar along the bottom of the thumbnail.
> - **Loading:** skeleton cards (thumbnail + avatar + two lines in fill), never a lone spinner.
>
> ### Search results
>
> - **Content area:** centered, max 1096 wide.
> - **Rows:**
>   - Thumbnail width = clamp(240, 40% of the content width, 500), 16:9, radius 12.
>   - 16 px gap to the text.
>   - Rows 16 apart.
> - **Text column:**
>   - Title: 18/26 400, 2 lines.
>   - Meta: 12/18 secondary.
>   - Channel row: 12 px vertical padding, avatar 24, 8 px gap, 12/18 name with a verified mark
>     when known.
>   - Snippet: 12/18 secondary, 1 line, with query terms at 500 in `#f1f1f1`.
>   - "⋮" appears on hover.
>
> ### Watch
>
> - **Layout:**
>   - No rail.
>   - Two columns when the window is ≥ 1017 wide: outer padding 24, gap 24, secondary column
>     between 300 and 402.
>   - The primary column is at most `(window_h − 169) × 16/9` wide, and the two columns are
>     centered as a block.
>   - Below 1017: one column, related videos after the description and before comments.
> - **Player:**
>   - 16:9, radius 12 in the default view, black. Click toggles pause and shows a 52 px center
>     icon that fades in 500 ms.
>   - Double-click toggles fullscreen.
>   - Controls appear on pointer movement and hide after 3 s without movement while playing,
>     taking the cursor with them; they stay while paused or while a menu is open.
> - **Control bar:**
>   - 48 tall over a bottom gradient (transparent → `rgba(0,0,0,0.6)`).
>   - **Left:** play/pause, then mute with a 52 px volume slider that slides out on hover, then
>     time `0:00 / 0:00` (13 px).
>   - **Right:** settings (speed 0.25–2, quality Auto/1080/720/480), captions when available,
>     theatre, fullscreen.
>   - All icons 24 px in 40 px hit targets.
> - **Scrubber:**
>   - 3 px tall, 5 px on hover.
>   - Track `rgba(255,255,255,0.2)`, buffered `rgba(255,255,255,0.4)`, played brand red.
>   - 13 px red knob on hover only.
>   - Hover tooltip with the time.
>   - Chapter gaps of 2 px when chapters exist.
> - **Keys:** Space/K, J/L, ←/→, ↑/↓, M, F (fullscreen), T (theatre), C (captions),
>   Shift+`<`/`>` (speed), `,`/`.` (frame step while paused), 0–9 (seek to 0–90%), Home/End,
>   Escape (leave fullscreen).
> - **Theatre mode:** the player spans the full window width on black, height
>   min(56.25% of the width, window_h − 169). The columns continue below.
> - **End of video:** show a "Up next" card with a 5 s countdown to the first related video
>   (cancelable). Autoplay can be turned off and the choice is remembered.
> - **Title and owner row:**
>   - Title: 20/28 700, 12 px below the player.
>   - Owner row, 12 px below: avatar 40, name 16/22 500, subscribers 12/18 secondary, then
>     Subscribe (primary button, 36 tall, 16 px padding).
>   - Right side: a like | dislike segmented pill (36 tall, fill, 1 px divider), Share (copies
>     the link) and Watch Later.
> - **Description:**
>   - Fill background, radius 12, padding 12.
>   - First line: 14/20 500.
>   - Body: 14/20, 3 lines, with an inline "…more" at 500.
>   - URLs, #tags and timestamps in link colour; timestamps seek.
> - **Comments:**
>   - Header "N Comments" 20/28 700.
>   - Each comment: avatar 40; "@author" 13/18 500 primary with the time 12/18 secondary; text
>     14/20 clamped to 4 lines with "Read more".
>   - Like count 12/18; "N replies" 14/20 500 in link colour with a chevron, which expands the
>     replies.
> - **Related:**
>   - Compact rows: thumbnail 168×94 radius 8, 8 px gap.
>   - Title 14/20 500, 2 lines; channel and meta 12/18 secondary.
>   - Rows 8 apart.
>
> ### Channel
>
> - **Content area:** centered, max 1284 wide.
> - **Banner:** radius 16, height = width / 6.2.
> - **Header:**
>   - Avatar 160 (72 below 600).
>   - Name 36/44 700.
>   - "@handle • subscribers • videos" 14/20 secondary.
>   - Description 14/20 secondary, 1 line + "…more".
>   - Subscribe (primary button) under the text.
> - **Tabs:** "Videos" only for now, 48 tall, 16/22 500, with a 2 px `#f1f1f1` underline under
>   the active tab.
> - **Grid:** cards drop the avatar and channel name.
>
> ### States
>
> - **Empty states:** centered within the page: a 120 px icon, a 20/28 700 title, 14/20
>   secondary text, and an optional action button.
> - **Errors:** "Something went wrong" + a one-line plain explanation ("Couldn't reach YouTube",
>   "This video is unavailable") + Retry. Raw error text goes to the log only.
> - **Keyboard focus:** every interactive element shows a 2 px `#f1f1f1` outline (offset 2) on
>   keyboard focus only. Tab order: top bar → rail → content.
> - **Navigation:** Forward (Alt+Right, mouse forward) re-pushes the last page popped by Back.

## Top ten changes, in priority order

1. **Fullscreen and theatre** in all three (P0-2), plus a quality setting.
2. **Fix iced's invisible empty and loading states** (P0-1).
3. **Bundle Roboto 400/500/700 and apply the text-style table** in all three (P1-3). This also
   fixes GPUI's lost weights (P1-4).
4. **Remove the rail on Watch and resize the columns to YouTube's formula** (P1-1). Raise the
   two-column threshold to 1017 (P1-10).
5. **Auto-hide controls and cursor after 3 s; rebuild the control bar** to the spec (P1-6):
   scrubber hover growth, tooltip, slide-out volume, settings menu with speed.
6. **Subscribe becomes the white primary button**, and the accent becomes `#ff0033` (P1-2).
7. **One shared SVG icon set** (outline, plus filled for selected) replacing iced's emoji and
   glyphs and unifying GPUI and Rune (P1-5). The rail gets 10 px labels, a filled selected icon
   and no box (P1-9).
8. **Card hover**: no grey box; "⋮" on hover, hand cursor (Rune), press darken. Don't reserve an
   empty second title line (P1-8, P2).
9. **Search results**: centered 1096 max width, thumbnails 240–500 wide, title at 400, 1-line
   snippet with bold query terms (P1-7). **Responsive**: hide the rail below 792, fill rows, make
   the chip bar sticky with fade and arrows, collapse search below 656 (P1-10).
10. **Watch page details**:
    - an Up next / autoplay countdown;
    - "…more" inline in the description, with clickable links and timestamps;
    - comments clamped to 4 lines, with expandable replies;
    - like/dislike pill and Share.

    Also fix GPUI's `/` leak and sticky hover.

## Screenshot index (`docs/review/ui-ux-shots/`)

- **YouTube reference:** `yt-search.png`, `yt-search-2.png`, `yt-watch.png` (paused, controls
  visible), `yt-watch-2.png` (description and comments), `yt-channel.png`,
  `yt-channel-videos.png`, `yt-channel-videos-2.png` (grid).
- **Per app** (`<fw>` = iced, gpui, rune):
  - `<fw>-home.png`, `<fw>-home-hover.png`, `<fw>-suggest.png`, `<fw>-search.png`,
    `<fw>-search-150ms.png`
  - `<fw>-watch-300ms.png`, `<fw>-watch.png`, `<fw>-watch-hover.png`,
    `<fw>-watch-paused.png`, `<fw>-watch-comments.png`
  - `<fw>-channel.png`, `<fw>-back-watch.png`, `<fw>-history.png`, `<fw>-later-empty.png`,
    `<fw>-subs-empty.png`
  - `<fw>-home-640.png`, `<fw>-watch-640.png`
  - GPUI's library shots and `gpui-watch-1000.png` are 1000×700 crops of a floated window.
- **Side-by-side:**
  - `compare-card-text-zoom.png`: YouTube, iced, GPUI, Rune.
  - `compare-search-row-text-zoom.png`: same order.
  - `compare-player-controls.png`: same order.
  - `compare-card-hover.png`, `compare-subscribed-saved.png`, `compare-home-640.png`,
    `compare-watch-640.png`, `compare-gpui-1000-library.png`, `compare-error-state.png`,
    `compare-focus-tab3.png`, `compare-scrollbar.png`, `compare-back-to-watch.png`,
    `compare-watch-at-300ms.png`, `compare-search-at-150ms.png`, `compare-subs-click.png`,
    `compare-rune-history-later.png`.
- **Specific defects:** `iced-unicode-icons.png`, `gpui-slash-leak.png`.
