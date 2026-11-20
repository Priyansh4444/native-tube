# What we learn from yt-music-gpui (Akshar Patel)

[AksharP5/yt-music-gpui](https://github.com/AksharP5/yt-music-gpui) is a native YouTube Music
client for Linux, built with Rust and GPUI 0.3.9 (MIT/Apache). It is about 66k lines of Rust, at
v0.2.24 on 2026-11-19. It solves many of the same problems as NativeTube, a year of releases
further along. These notes were read from the source (shallow clone, nothing built or run), and
from [issue #82](https://github.com/AksharP5/yt-music-gpui/issues/82), which the owner of this
repo filed about its queue.

## What he does, and why it works

### 1. Video that never passes through the UI framework

`native/video.c`, built by `build.rs` with `cc`, gives mpv its own surface:

- It creates a `wl_subsurface` of the GPUI window's `wl_surface`, set to **desync** and placed
  **below** the parent. It is sized with `wp_viewporter`.
- On a worker thread, it creates an EGL context and `mpv_render_context_create(...
  MPV_RENDER_API_TYPE_OPENGL ...)`.
- It renders into the default FBO with `eglSwapInterval(0)` and `MPV_RENDER_PARAM_ADVANCED_CONTROL`.
- X11 uses a child window instead.

On the GPUI side, the window is transparent where the video is. `player_page.rs` paints the page
*around* a "hole" (`paint_around(bounds, hole, ...)`), and the video shows through it. Controls are
ordinary GPUI elements drawn on top.

Why it matters:
- Decoding and scaling stay on the GPU (mpv's own `vo=gpu` path), with no copy to the CPU and no
  per-frame upload.
- Because the subsurface is desync, mpv presents frames at its own pace. The UI isn't redrawn per
  video frame at all.
- That is exactly what NativeTube lacks. Our core renders with libmpv's software API into RGBA
  and every app uploads each frame. That's why our playback CPU is about twice mpv's (31.8% vs
  16.4%), and why GPUI rebuilds its tree per video frame.

### 2. YouTube's own loudness levelling

He applies YouTube's per-video loudness with a file-local mpv option on `loadfile`:
`"volume-gain": gain_db`. It needs mpv 0.38 or newer. `normalization_gain` (src/player.rs:919)
reproduces the web player's rule (base.js):
- gain = `loudnessTargetLkfs − trackAbsoluteLoudnessLkfs`, otherwise `−loudnessDb`
- the selected format's values win over the response's `playerConfig.audioConfig`
- clamped to [−96, 0], so a loud track is turned down to the target and a quiet one is never
  boosted

Every song plays at the level the YouTube website would use, and the user's volume is
untouched. Our plan was mpv `dynaudnorm`, a compressor that reacts to the signal. His
approach is closer to YouTube's "Stable volume", and cheaper.

### 3. Preparing the next item before it is needed

`Player::prepare_next(video_id)` (src/player.rs:409) resolves only the natural successor of the
current song, respecting repeat and autoplay. It is re-run after every queue edit or quality
change, and a new candidate cancels the old work. mpv runs with `prefetch-playlist=yes` and
`gapless-audio=yes`, so at EOF it moves to the prepared entry without reopening a stream.

His measured results:
- 191 ms median from click to playback on a warm guest session
- 483 ms for first Play
- **54 ms for Next**

Our core already starts a video in about 220 ms warm, but we never prepare the next one, and
autoplay "Up next" waits for a fresh /player resolve.

### 4. A queue that should never dead-end (issue #82)

His design is the right shape:
- `Queue::incomplete` plus a source continuation (`src/queue_source.rs`)
- `prefetch_queue` appends the next page in the background

The bugs the owner found apply to us too:
- Prefetch started only 3 items before the end.
- A failed page stayed `Failed` unless a user action retried it, so a natural advance never
  retried.
- A cancelled source left `incomplete` behind.
- The UI told the user to "open the playlist again" instead of offering Retry.

Lessons for every continuation in NativeTube (home, search, related/Up next, comments, channel
videos, playlists):
- Fetch the next page with lead time.
- Retry a failed page two or three times with backoff before showing anything.
- Let natural progress retry.
- Show a Retry row in place, never a dead end.

### 5. Crash-proof lints and budgets

- **Lints.** `Cargo.toml` `[lints.clippy]` denies `unwrap_used`, `expect_used`, `panic`,
  `string_slice`, `todo`, `unimplemented`, `unreachable` and `unchecked_time_subtraction`.
  True invariants get a narrow `#[expect(..., reason = "...")]`.
- **Performance budget.** `bin/check-performance` builds two locked revisions and compares them
  against `bin/performance-budget.json`. The budgets are stripped binary size, first playback
  position (75 ms over the baseline) and warm switch (25 ms).
- **Native window tests.** `src/app/native_tests/*` run against a real window in a fresh private
  runtime: render, keyboard, navigation, playback, restart, resume and search.
- **Profiling method.** It is documented in CONTRIBUTING.md: the same muted track, alternating
  build order, three runs, PSS and CPU summed over the process tree.

### 6. Small things

- `audio-client-name` is set, so PipeWire and the desktop mixer show the app's name.
- mpv runs with every `load-*` Lua script switch off, as ours does.
- The last song resumes after a restart.
- MPRIS covers media keys and desktop controls, as ours now does.

## What we do differently, on purpose

- **No yt-dlp, Deno or Python.**
  - He uses yt-dlp (with Deno for YouTube's JS challenge) as a fallback and for signed-in
    playback, plus a Python helper (`src/ytdlp_account.py`).
  - His fast path (`src/player/native.rs`) is the same cookie-free **visionOS** /player lookup
    our core uses.
  - NativeTube stays JS-free at runtime by owner rule, so we take only the native path.
- **No sign-in.** He copies the browser's session cookies into a keyring. NativeTube stays
  logged out.
- **Three frameworks, one core.** He is GPUI only. Our core is shared, so anything we adopt
  goes into nt-core first.

## What we are adopting, and where

| Idea | Where | Notes |
|---|---|---|
| GPU video on a desync subsurface below a transparent window; UI paints around the hole | core (surface + mpv render_gl) and each app (transparent window, hole, raw window handle) | Software path kept as fallback (X11, macOS, or failure) |
| YouTube loudness via per-file `volume-gain` | core `Player`, on by default with a toggle (replaces the dynaudnorm plan) | Needs mpv ≥ 0.38; skip silently below |
| `prepare_next` + mpv `prefetch-playlist` | core API; apps call it with the next autoplay/queue item | Measure Next ms like he does |
| Continuations with lead time, retry with backoff, Retry rows | core `Client` retry; apps prefetch earlier and show Retry in place | Applies to all paginated lists |
| Crash-proof lints | core and apps `[lints.clippy]` | Core has about 36 unwraps to remove |
| Performance budget gate | tools/test-all.sh plus a budget JSON | First frame, warm switch, binary size, idle PSS |
| Resume the last video after restart | core library plus apps | |
| `audio-client-name` | core | |
