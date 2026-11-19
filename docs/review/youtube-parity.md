# YouTube parity: gaps and effort per framework

Compared on 2026-11-18. Reference: youtube.com in Helium at 1280×800, driven with the `browse`
tool (OpenCode's Browse MCP). The apps were captured on the same screens with
`tools/headless.sh` and real input from `tools/nt-input`. The reference screenshots come from
the owner's signed-in browser, so they stay out of the repo, in the session scratchpad.

**Status:** ✓ done · ◐ partial · ✗ missing · ? not verified.

**Effort** is Opus agent time for one app, on top of any core work:

| Size | Agent time | Cost |
|---|---|---|
| S | ≤ 30 min | ~$15 |
| M | 1–2 h | ~$40 |
| L | 3–5 h | ~$110 |
| XL | a day or more | ~$250 |

These costs come from the measured first builds: $23.5–28.4 for 49–59 min, about $28 per
agent hour. The **Core** column is shared work in `nt-core`, done once for all three apps.

## What YouTube has that we don't (by screen)

### Home

| Feature | YouTube | GPUI | iced | Rune | Core | G | I | R |
|---|---|---|---|---|---|---|---|---|
| ⋮ menu items: Add to queue, Save to Watch later, Save to playlist, Share, Not interested, Don't recommend channel | 8 items | ◐ 4 | ◐ 4 | ◐ 4 | queue, local playlists, hide lists (M) | S | S | S |
| Hover: card gets a tinted background, ⋮ shows | yes | ? | ? | ? | – | S | S | S |
| Inline muted preview after about 1 s of hover (with a mute button and a progress line) | yes | ✗ | ✗ | ✗ | a second, preview-only `Player` (M) | M | M | L |
| Meta: verified ✓ after the channel name, `▷ 14k · 2 hr ago` | yes | ✗ | ✗ | ✗ | parse badges (S) | S | S | S |
| Badges: LIVE (red, plus a ring on the avatar), Upcoming + "Scheduled for…", 4K/CC/New | yes | ◐ LIVE | ◐ LIVE | ◐ LIVE | parse upcoming and badges (S) | S | S | S |
| Chips from the feed itself (logged out: what the feed returns), not a fixed topic list | yes | ◐ fixed | ◐ fixed | ◐ fixed | parse feed chips + their continuations (M) | S | S | S |
| Miniplayer: leaving Watch keeps playing in a corner box (expand, close, play/pause, progress) | yes | ✗ | ✗ | ✗ | – (the player already outlives the page) | M | L | L |

### Search

| Feature | YouTube | GPUI | iced | Rune | Core | G | I | R |
|---|---|---|---|---|---|---|---|---|
| Suggestions dropdown (14 rows, typed prefix normal weight, the rest bold) | yes | ✓ | ✗ in the latest build (focus lost; the agent is on it) | ✓ | – | – | S | – |
| Filter chips: All, Shorts, Videos, Unwatched, Watched, Recently uploaded, Live | yes | ✗ | ✗ | ✗ | search `params` per chip (M) | S | S | S |
| Filters dialog: upload date, type, duration, features, sort | yes | ✗ | ✗ | ✗ | the same `params` (M) | M | M | M |
| Non-video results: playlists ("44 videos" stack), channels, Shorts shelf, "People also watched" shelf | yes | ✗ | ✗ | ✗ | parse `lockupViewModel`, channel and shelf renderers (M) | M | M | M |
| Row extras: chapter strip ("5 chapters · Intro | History…") with an expander, Matching chapter, 4K badge | yes | ✗ | ✗ | ✗ | parse (S) | S | M | S |
| Watched progress on thumbnails | yes | ✓ | ✓ | ✓ | – | – | – | – |

### Watch: player

| Feature | YouTube | GPUI | iced | Rune | Core | G | I | R |
|---|---|---|---|---|---|---|---|---|
| Captions (CC button, language list, auto-translate into 156 languages) | yes | ✗ | ✗ | ✗ | timedtext tracks from `/player` (present in the visionOS answer); render through mpv `sub-add`, so subtitles land in the frame for free (M) | S | S | S |
| Chapters: segmented scrubber, chapter title in the bar ("Intro ›"), chapter list | yes | ✗ | ✗ | ✗ | parse `/next` macro markers (S) | M | M | M |
| Scrub preview: storyboard thumbnail + time + chapter above the pointer | yes | ✗ time only | ✗ time only | ✗ time only | storyboard spec from `/player` (present), sprite fetch and crop (M) | M | M | M |
| Most-replayed heatmap above the scrubber | yes | ✗ | ✗ | ✗ | parse `markersMap` from `/next` (S) | S | M | M |
| Settings: Audio track (original + 8 dubs on the test video) | yes | ✗ | ✗ | ✗ | expose the tracks, switch audio URL (M) | S | S | S |
| Settings: Sleep timer | yes | ✗ | ✗ | ✗ | – | S | S | S |
| Settings: Quality 1440p and 2160p | yes | ✗ (max 1080) | ✗ | ✗ | raise the cap; needs VP9/AV1 decode and the GPU path to stay smooth (L) | S | S | S |
| Settings: Stable volume (loudness normalisation) | yes | ✗ | ✗ | ✗ | mpv `af=dynaudnorm` toggle (S) | S | S | S |
| Ambient mode (soft glow of the picture behind the player) | yes | ✗ | ✗ | ✗ | a tiny downscaled frame each second (S) | S (shadow/gradient) | L (no blur: CPU blur into an image) | M |
| Right-click on video: Loop, Copy video URL, Copy URL at current time, Stats for nerds | yes | ✗ | ✗ | ✗ | `loop-file` (S) | S | M | M |
| Next button, autoplay toggle, theatre, fullscreen, speed, quality ≤ 1080 | yes | ✓ | ✓ | ✓ | – | – | – | – |

### Watch: page

| Feature | YouTube | GPUI | iced | Rune | Core | G | I | R |
|---|---|---|---|---|---|---|---|---|
| Related chips: All · From your search · From <channel> · Related | yes | ✗ | ✗ | ✗ | parse chips + continuations (S) | S | S | S |
| Comment replies: "13 replies ⌄" expands the thread | yes | ✗ | ✗ | ✗ | reply continuation (S) | S | M | M |
| Comment Sort by: Top / Newest | yes | ✗ | ✗ | ✗ | sort tokens from the header (S) | S | S | S |
| Comment badges: pinned, creator heart, channel-owner highlight | yes | ◐ | ◐ | ◐ | parse (S) | S | S | S |
| Description expanded: full text, chapters carousel, links, "Show less" | yes | ◐ | ◐ | ◐ | chapters (above) | S | M | S |
| "…" menu: Show transcript (a panel with timestamps that seek) | yes | ✗ | ✗ | ✗ | transcript from the caption track (S) | M | M | M |
| Playlists and mixes: the playlist panel on Watch, next/previous, shuffle and loop | yes | ✗ | ✗ | ✗ | `/next` with `playlistId` (M) | M | L | M |
| Queue ("Add to queue") | yes | ✗ | ✗ | ✗ | local queue (S) | M | M | M |

### Channel

| Feature | YouTube | GPUI | iced | Rune | Core | G | I | R |
|---|---|---|---|---|---|---|---|---|
| Tabs: Home (featured video + shelves), Videos, Shorts, Live, Playlists, Posts, plus search | yes | ◐ Videos | ◐ Videos | ◐ Videos | browse `params` per tab (M) | M | M | M |
| Videos sort: Latest / Popular / Oldest | yes | ✗ | ✗ | ✗ | sort tokens (S) | S | S | S |
| Links row ("fireship.dev and 2 more links"), "…more" opens About | yes | ✗ | ✗ | ✗ | parse (S) | S | M | S |

### Everywhere

| Feature | YouTube | GPUI | iced | Rune | Core | G | I | R |
|---|---|---|---|---|---|---|---|---|
| Guide: ☰ opens the full sidebar (Home, Shorts, Subscriptions, You, History, Watch later, playlists) | yes | ✗ rail only | ✗ | ✗ | – | S | M | M |
| Shorts player (vertical, swipe/wheel to the next) | yes | ✗ | ✗ | ✗ | reel feed (L) | L | XL | L |
| Live: chat beside the player, "N watching", DVR seek | yes | ✗ | ✗ | ✗ | live chat polling (L) | M | L | M |
| `?` keyboard shortcut sheet | yes | ✗ | ✗ | ✗ | – | S | S | S |
| Back/Forward keep scroll, query, video position | yes | ✓ | in progress | in progress | – | – | – | – |
| Keyboard focus rings and Tab order | yes | ✗ | ◐ | ◐ | – | M | M | M |

### Out of scope, on purpose

These need a signed-in account, an AI service or downloading, which the project rules exclude.
See the terms-of-service note in the [README](../../README.md):

- Ask YouTube
- voice search
- Create
- notifications
- the account menu and Premium
- Subscribe as a YouTube action (subscriptions stay local)
- posting likes, comments or replies
- Download
- Report
- Clip and Thanks

## Totals

These are sums of the table, counting S = 0.5 h, M = 1.5 h, L = 4 h and XL = 8 h, at ~$28
per agent hour. Treat them as ±50%.

| | GPUI | iced | Rune |
|---|---|---|---|
| UI work, excluding Shorts and live chat | ~28 h · ~$780 | ~44 h · ~$1,220 | ~40 h · ~$1,100 |
| With Shorts and live chat | ~34 h · ~$940 | ~56 h · ~$1,550 | ~45 h · ~$1,260 |
| Shared core work (once, for all three) | | ~36 h · ~$1,000 | |

GPUI needs about two thirds of iced's effort for the same result. Rune is close to iced.

## Why the same feature costs different amounts

**GPUI is cheapest for anything floating or layered.** That covers menus, the miniplayer, the
scrub preview, the ambient glow and tooltips:

- `anchored()` and `deferred()` place a popup at the pointer and keep it on screen.
- `.hover()` styles, shadows with blur, gradients and `with_animation` are built in.
- The whole window is rebuilt every frame, so an extra surface is one more element.
- A second video (miniplayer, inline preview) is one more image element fed from another
  `Player`.
- The catch is re-entrancy. An update that triggers a redraw of the same view panics (the
  `PlayerView` crash), so every new callback path needs `cx.defer` or `Window::defer`.

**iced pays for every layer and every interaction in boilerplate.**
- Everything is a `Message` through `update()`.
- Popups need a `stack` or a hand-written `Overlay` with manual positioning.
- Text has no ellipsis (we keep our own `Clamp`), and focus can't be cleared through the API.
- Animation needs a time subscription.
- There is no blur, so the ambient glow is a CPU-blurred image.
- Video is a custom wgpu primitive, so a second surface means a second primitive and its own
  frame plumbing.

In exchange, iced is the cheapest to run: low idle wake-ups, and the lowest CPU while
scrolling.

**Rune sits in between.**
- Its live values (`Live`, `LiveText`, `Pixels`) make video, scrolling and frequently
  changing numbers cheap. A second video is a second `Pixels`.
- The kit is small and young. Its buttons and fields need re-theming, and its menus and
  overlays are built by hand.
- Some fixes need patches to Rune itself or its pinned dependencies. That's why most layered
  features are M there, where GPUI is S.

**Core work is the same for all three and comes first.** It covers chapters, the heatmap,
storyboards, captions, audio tracks, replies, sorting, chips, filters, tabs, playlists, the
queue, badges and the preview player. Once nt-core has them, most rows above shrink to UI-only
work.

## Order of work

1. **Core, round A.** These are the biggest wins for "feels like YouTube":
   - captions via mpv
   - chapters, heatmap and storyboards
   - comment replies and sort
   - related chips
   - verified, upcoming and 4K badges
   - audio tracks
   - stable volume and loop
2. **UI, round A, in all three apps.** The player items above, then the player right-click
   menu, sleep timer, ⋮ menu parity, replies and sort.
3. **Core, round B.** Search filters, non-video results, channel tabs and sort, playlists,
   queue, the inline preview player, feed chips.
4. **UI, round B.** Miniplayer, inline preview, the guide, search filters, channel tabs,
   playlists, the transcript.
5. **Later:** Shorts, live chat, quality above 1080p (needs the GPU video path).
