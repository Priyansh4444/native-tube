# Coordinator notes for the article

Raw input for the article from the agent that ran the project. These are the facts and the
lessons as observed. Numbers are measured; where something is an estimate, it says so.
Re-measure everything marked "re-run" before publishing.

## The setup

- One shared Rust backend, nt-core: Innertube client, polite traffic gate, fixtures,
  image cache, Chromium scroll physics port, libmpv player, MPRIS. Three frontends on top of it:
  - iced 0.14
  - GPUI: gpui-pre 0.3.12 + gpui-component 0.9.1
  - Rune: anomalyco/rune git rev 4fbd2d6
- Rules: pure Rust plus native libraries at runtime. No JS in the loop (no webview, no yt-dlp,
  no Lua). Logged out, polite, muted tests. Off-screen headless Hyprland outputs, with real
  input through a wlr virtual-pointer tool (tools/nt-input).
- Every frontend was built by an Opus agent from the same written spec (docs/frontend-spec.md,
  then docs/frontend-spec-v2.md after a review council). Later rounds were done by
  GPT 6.2 Sol and Gemini 4 Neon; see the git history.

## Where the differences come from

**Playback is shared, so differences there come from how each framework gets pixels on
screen.**
- Core decodes with libmpv's software render API into RGBA frames. Each app uploads them:
  - iced: a custom wgpu primitive, with no atlas entry and no new image per frame
  - GPUI: an image element rebuilt every frame; there's no paint_surface on Linux
  - Rune: Pixels live values
- Core alone, measured muted, median:

  | Player | Cold first frame | Warm first frame | CPU | PSS |
  |---|---|---|---|---|
  | NativeTube core | 451 ms | 218 ms | 31.8% | 141 MB |
  | mpv, same URLs | 1541 ms | 557 ms | 16.4% | 393 MB |
  | mpv + yt-dlp | 5701 ms | – (4 of 6 runs fail with 403) | 30.4% | 365 MB |
  | YouTube in Helium | 4170 ms | 3885 ms | 63.8% | 987 MB |

- Why core starts faster than mpv:
  - YouTube's visionOS client returns plain stream URLs, with no JS challenge and no PO token.
  - googlevideo trickles an open-ended `Range: bytes=0-`, so bounded requests are used
    (request_size 10 MB, initial 1 MB). First frame on a 14 h video went from 16.6 s to
    0.7 s.
  - VAAPI copy instead of NVDEC: about 210 ms against about 950 ms cold.
  - All of mpv's scripts are off. load-scripts=no alone still ran 6 Lua scripts, so each
    load-* option is turned off too.
  - `video-timing-offset=0`: without it the render call blocked until each frame's display
    time.
- Why core uses more CPU than mpv: software rendering plus a frame upload, against mpv's GPU
  path. A GPU render path is the known next step.

**Scroll physics are shared too** (a port of Chromium's ScrollAnimator). Differences in feel
come from how each framework delivers wheel and touchpad events, and from the redraw cost:
- GPUI's Wayland backend dropped `axis_stop`, so touchpad flings never started. We vendored a
  patch.
- CPU while wheel-scrolling, in the scenario before the fix round (re-run):
  - iced 12.7%
  - GPUI 30.0%
  - Rune 11.8%
- GPUI rebuilds the whole tree every frame. That makes features cheap to write but costs CPU
  per frame. Rune's live values update without re-running views.

**Idle cost, first numbers (re-run on a quiet machine):**
- Idle PSS: iced 97 MB, GPUI 163 MB, Rune 135 MB.
- Threads: 11, 38, 17.

**Raw shells** (one window, one label, bundled Roboto, no nt-core; 8 warm runs on a busy
machine, re-run):

| Shell | Window | PSS | Wakeups/s | Threads | Stripped binary | Crates |
|---|---|---|---|---|---|---|
| iced, default | 460 ms | 113 MB | 107 | 28 | 12.5 MB | 319 |
| iced, low power | 175 ms | 50 MB | 2 | 20 | 12.5 MB | 319 |
| GPUI | 253 ms | 96 MB | 5 | 32 | 27.7 MB | 640 |
| Rune | 259 ms | 78 MB | 3 | 11 | 34.1 MB | 434 |

The iced default is slow and heavy because wgpu picks the high-performance GPU on a hybrid
laptop. Setting `WGPU_POWER_PREF=low` and `WGPU_BACKEND=vulkan` fixes it, and nothing in
iced's docs tells you to. Start-up is measured from exec to Hyprland's `openwindow` event,
which is the same signal for every framework.

**Agent build cost (first full build of each app):**

| | iced | GPUI | Rune |
|---|---|---|---|
| Cost | $23.54 | $28.41 | $27.99 |
| Time | 49 min | 59 min | 55 min |

Harness prices: output $20, cache write $8, cache read $0.20 per Mtok. Later rounds cost far
more than the first build: the review findings, stability fixes and YouTube parity were most
of the spend.

**Estimated effort to full YouTube parity** (docs/review/youtube-parity.md):

| | GPUI | iced | Rune |
|---|---|---|---|
| UI work | ~28 h | ~44 h | ~40 h |
| Shared core work, once | | ~36 h | |

GPUI is cheapest for anything floating or layered:
- `anchored()` / `deferred()`, `.hover()`, shadows with blur and `with_animation` are built in.
- A second video surface is one more element.

iced pays for every layer in boilerplate:
- every interaction is a Message
- popups need a stack or a hand-written Overlay
- no ellipsis: we wrote `Clamp`, which binary-searches the longest prefix that fits
- focus can't be cleared through the API
- no blur

Rune's live values are great for video and scroll, but its kit is small and young:
- we re-themed every button and field
- some fixes need `[patch.crates-io]` for winit and wgpu-hal
- `set_app_id` wants `&'static str`
- there is no Linux video Player

## Stability: what actually broke, and why (the "dirty details")

- **GPUI panic:** "cannot update PlayerView while it is already being updated". The cursor
  auto-hide ran inside `cx.defer_in`, which runs within the view's update. It sent a fake
  keystroke, because GPUI has no public hide-cursor call. GPUI redraws before handling a key,
  so it re-rendered PlayerView mid-update. Fixed with `Window::defer`, which holds no entity.
  The lesson: in GPUI any synchronous path from an update back into the same entity is a
  runtime panic, not a compile error.
- **GPUI fonts:**
  - Variable-font weights were lost, so we bundled static Roboto Regular, Medium and Bold.
  - On macOS GPUI showed no text at all, and iced showed text as dots.
  - Rune was usable on macOS.
  - Cmd+Q didn't quit in any of the three until we wired it up.
- **iced search box:** it lost focus right after gaining it. A bisect found the trigger:
  changing the width of the icon slot on focus. With the width fixed, focus sticks, and
  changing the padding is harmless. The likely mechanism is that the widget tree changes shape
  and the input's state is lost; confirm it against the iced NOTES before printing it. The fix
  overlays the icon and changes only the padding.
- **iced focus:** you can't unfocus a text input through the API.
- **Rune:** clicking a related video in the sidebar lagged or broke. This was the owner's
  report; see the Rune NOTES for the root cause.
- **Fixtures:** GPUI's build of serde_json had `preserve_order` turned on by some dependency,
  so its fixture keys differed from the other two apps. We switched to canonical JSON keys and
  re-recorded 1145 fixtures, after which all three replay identically. This is a classic
  cross-framework trap: one crate's features are unified across the dependency graph.
- **Autopilot:** the scenario runner couldn't parse `{"do": …}` steps. Agents found it while
  benchmarking.
- **Test windows hid the owner's own launches.** Test runs now get their own app_id
  (`nttest-<fw>`) and a window rule that sends them to off-screen outputs. One agent once
  moved the owner's browser window onto a test workspace by mistake; it asked first, and the
  window was moved back.
- **The desktop shell swallowed clicks** near the top and left edges of the headless outputs,
  so the agents floated the windows or used the keyboard to test the top bar.

## Playback quality and the viewing experience (review findings)

- At first:
  - capped at 720p with soft upscaling
  - no frame timestamps, so judder at 165 Hz
  - about 26 ms A/V lag at 60 Hz
  - expired URLs or dropped connections showed as "Ended"; now recovered in core, up to 3
    times per video
  - the screen locked mid-video, fixed by MPRIS: Caelestia only holds off idle while an
    MPRIS player is playing
  - a live stream took 16.5 s to start
- Now 1080p by default, and quality is selectable.
- YouTube offers up to 2160p on the test video. Above 1080p we need VP9 or AV1 and the GPU
  path.

## How the agents were run (process lessons)

- A written spec plus a review council beat iterating on vibes. Spec v2 came from a council
  of reviewers (UI/UX, metrics, core, frontends, viewing) comparing against YouTube.
- What made the agents' results trustworthy:
  - real input through a virtual pointer
  - off-screen outputs
  - fixtures, so runs are repeatable and stay off YouTube
  - a local test clip
  - screenshots the agent must look at itself
- Usage limits ended several rounds mid-way: Claude's 5-hour and weekly limits. A shared
  checkout, explicit scopes per agent and a baseline commit made handing work across models
  (Opus → GPT 6.2 Sol and Gemini 4 Neon) possible without losing anything.
- Property-based testing:
  - Bombadil only drives browsers and terminals, so it fuzzes a terminal REPL over core.
  - The native apps expose a state probe (docs/testing/state-probe.md), so a Rust explorer
    runs Bombadil-style temporal properties against them.
  - See docs/testing/.

## YouTube itself, as the baseline

- YouTube in Helium: 4.2 s to the first frame cold, 64% CPU, about 1 GB PSS.
- NativeTube core: 0.45 s, 32% CPU, 141 MB.
- These are the headline numbers, but they compare a whole web app with a player. The fair
  comparison of the apps is the full scenario (re-run).
- What YouTube still has and we don't:
  - captions
  - chapters
  - scrub previews
  - dubbed audio tracks
  - miniplayer
  - playlists
  - search filters
  - channel tabs
  - hover previews
- These are being added; check docs/review/youtube-parity.md for the final state.

## Things not to put in the article

- Anything about the owner beyond the GitHub handle (Priyansh4444): no email, no real name,
  no account details. The reference screenshots of youtube.com came from a signed-in browser
  and must not be published.
- Don't claim the app is ToS-compliant. The README says plainly that it is outside YouTube's
  terms.

## Prior art: Akshar Patel's yt-music-gpui

See docs/review/yt-music-gpui-notes.md for the full notes. Key points for the article:

- A mature GPUI-only YouTube Music client (about 66k lines, v0.2.24). Its stream fast path is the
  same cookie-free visionOS /player lookup our core uses. It falls back to yt-dlp + Deno, which
  we refuse.
- Its biggest lesson for framework choice: video should not go through the UI framework at all.
  mpv renders with OpenGL into a desync Wayland subsurface below a transparent GPUI window, and
  the page paints around the hole. Our software path (RGBA upload per frame) is why our playback
  CPU is 2× mpv's, and why GPUI's per-frame rebuild shows up during playback. Whether each
  framework can host a transparent window, expose raw Wayland handles and paint around a hole
  is a real differentiator ⟨record per framework once the app agents report⟩.
- Other ideas adopted:
  - YouTube's own loudness rule as a per-file mpv volume-gain
  - prepare_next (he measures 54 ms Next)
  - continuations with lead time and retry, from the owner's issue #82 on his repo
  - crash-proof Clippy lints
  - a performance budget gate
