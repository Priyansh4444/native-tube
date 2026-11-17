# Core review: is the shared backend correct, robust and fast?

Reviewer lens: `core/src` (nt-core) line by line, `core/Cargo.toml`, `tools/nt-input/src/main.rs`,
`tools/headless.sh`. No GUI app was run.

**What I ran** (all muted, no live YouTube traffic):
- `cargo test --release` in core: 32 pass. `cargo clippy --all-targets`: no warnings.
- `ntctl play` on `.cache/test-clip-720p30.mp4` (NT_VIDEO_OVERRIDE, scratch NT_DATA_DIR).
- A scratch crate outside the repo linking nt-core by path, with small programs for images,
  library, scroll and autopilot. Their output is quoted below.
- Read `.cache/fixtures/index.txt` and the recorded bodies.

Everything else was concluded from reading the code. Each item says which applies.

---

## P0

### P0-1. Fixture keys depend on a Cargo feature, so GPUI is benchmarked on different YouTube data
`client.rs:142` builds the replay key as `format!("POST {endpoint} {body}")`. The printed key
order follows serde_json's Map. That Map is a BTreeMap (sorted keys) unless the
`preserve_order` feature is on, and then it keeps insertion order. GPUI's dependency graph turns
`preserve_order` on and iced's and Rune's do not (checked with
`cargo tree -e features -i serde_json` in each app). So the same search is keyed differently:

```
d72e0f9ce7327181 POST search {"query":"sports highlights","params":"EgQIAxAB"}   <- GPUI
28de34073403209d POST search {"params":"EgQIAxAB","query":"sports highlights"}   <- iced/Rune/ntctl
```

`tools/headless.sh:64` runs in `NT_FIXTURE_MODE=auto`. On a miss it went to live YouTube without
a word and recorded a second copy. The two recordings hold **different videos**:

```
sports highlights: GPUI first item ecqeLJ32ZhA (recorded 01:15:07), others 3wc9kxNDXTU (01:17:21)
music videos:      GPUI first item Sh4pJYJOT7M,                     others -hSuPihfZ24
```

Every continuation token, thumbnail and watch page that follows differs too. So every home,
search, scroll and infinite-scroll number for GPUI is measured on other content, and its first
run included live network latency. `rust programming` and every home topic exist in both forms
(8 vs 9 keys).

Fix:
- Build the key from a canonical form. Sort keys yourself, or key on endpoint plus the specific
  fields (`query`, `params`, `continuation`, `videoId`, `browseId`).
- Make benchmark runs use `replay`, so a miss fails loudly. Count misses in the report.
- Re-record once and delete the duplicates.
- Add a test that the key is the same with and without `preserve_order`. A simple check: the
  key never depends on `Value` printing.

---

## P1

### P1-1. A stale frame of the previous video can still be published after `load`/`stop`
`player.rs:790` checks `accept_frames`. Then it fills alpha (≈1 ms at 720p) and publishes at
`player.rs:807`. `load` (`player.rs:474-481`) and `stop` (`:520-523`) store `false` and clear
`frame` without any lock shared with the render thread.

Failure scenario (reading):
1. The render thread passes the check for video A.
2. The user clicks video B. `load` clears the frame.
3. The render thread then publishes A's picture with a new generation.
4. That picture stays until B's `START_FILE`, which comes after the resolve (0.3–1 s on the
   network).

iced uses "no frame" to show the spinner (`apps/iced/src/view.rs:621`), so the old video shows
with no spinner. At 30 fps the window is about 3% of the time. This is the bug 69dc24c meant to
close.

Fix:
- Capture `load_ticket` before rendering.
- Publish under the `frame` mutex only if the ticket is unchanged.
- In `load`/`stop`, bump the ticket and clear `frame` while holding the same mutex.

Test: load clip A (all red) with `NT_VIDEO_OVERRIDE`, then load B (all blue) in a loop. Assert
that no frame published after `load` returns is red.

### P1-2. An expired stream URL or a network drop shows as "Ended" in the middle of the video
- `resolve.rs` drops `streamingData.expiresInSeconds`, and the player never resolves again.
- `stream-lavf-o` (`player.rs:348`) sets only the request sizes. libmpv adds `reconnect=1`,
  `reconnect_delay_max=7` (the strings are in libmpv.so), but `reconnect_on_network_error` is off.
- With `request_size=10 MiB`, every chunk is a **new connection**. The cache holds at most 60 s
  or 64 MiB.

Scenario 1: the user pauses for more than about 6 h (the googlevideo `expire=` parameter), or the
laptop changes network (URLs are IP-bound). On resume the next chunk gets HTTP 403. Scenario 2:
the network is down at the moment a chunk starts.

Either way the demuxer reports EOF, `eof-reached` becomes true, and `apply_property`
(`player.rs:716-718`) sets `Status::Ended` at, say, 4:10 of 12:00. The user sees the replay
state, not an error or buffering. Concluded from reading; not reproduced, to stay off YouTube.

Fix:
- Keep `expires_at` from `/player`.
- On `Ended` with `position < duration - 2` for a non-live video, or on `END_FILE` with an error,
  resolve again and `loadfile ... start=<pos>`. Do the same before resuming once `expires_at` has
  passed.
- Add `reconnect_on_network_error=1,reconnect_streamed=1,reconnect_delay_max=30` to
  `stream-lavf-o`.

### P1-3. Viewing quality is capped at 720p, with no API to change it
`resolve.rs:89-91` defaults `NT_MAX_HEIGHT` to 720. The render cap starts at 1280×720
(`player.rs:397`). Fullscreen on a 1440p or 4K panel shows a 720p stream upscaled by the toolkit.
YouTube web picks 1080p or 1440p there. The only control is an environment variable, and there is
no `Player` API for quality, so no frontend can offer "Auto / 1080p / 720p".

Fix: pick the stream height from the size set with `set_target_size` × DPR (Auto), and add
`Player::set_max_height` / `available_qualities()`. When the target grows past the current
stream, switch with `loadfile` at the current position.

`resolve.rs:175-182` also picks codec first, then height. With a cap above 1080 it takes avc1
1080p over VP9 1440p. Rank height first, then the codec the GPU can decode.

### P1-4. The audio track is chosen by bitrate alone: dubs, auto-dubs and DRC variants
`resolve.rs:185-187` takes the highest-bitrate `audio/mp4`. Multi-language videos list one
format per language (`audioTrack.{id,displayName,audioIsDefault}`), and many videos now carry
auto-dubs. Stable-volume variants (`isDrc`) also appear. The pick is whichever happens to be
largest, so a viewer can get the Spanish dub of an English video.

Fix: filter to `audioTrack.audioIsDefault == true` (or the original). Choose DRC on or off on
purpose. Expose the track list for a picker.

This is from reading the response schema. The visionOS client's answer for a dubbed video was not
checked live. Record one `/player` response as a test fixture (with URLs stripped) and test
`pick` against it. `pick` is only tested on hand-written JSON today.

### P1-5. The image cache's byte count is wrong in both directions, so it ends up thrashing
`images.rs:336` calls `account` once for **every** job awaiting a slot, not once per decode.
`account` (`:349-361`) can evict an entry that is still in flight. Its bytes are then added when
it lands, but they can never be subtracted. Verified with the scratch program:

```
[double count] one 320x180 image = 230400 bytes; cached_bytes() after 5 concurrent requests = 1152000
[leak] cached_bytes() = 91.3 MB (budget 96 MB)
  re-request B5:     0.0 ms  cached_bytes now 91.3 MB
  re-request A:    49.8 ms  cached_bytes now 91.3 MB    <- A was evicted while in flight (re-fetched),
                                                           yet its 45.8 MB are still counted
```

Over a session the counter only rises. Once it passes 96 MB, every completed image evicts nearly
everything, and scrolling back re-downloads and re-decodes thumbnails. `image_cache_mb` in the
benchmark report is also wrong.

There is a second effect. `Err` (`:363-366`) removes whatever slot now sits under that key,
possibly a newer in-flight one.

Fix:
- Account inside the `get_or_init` closure, so it runs once.
- Never evict a slot whose `OnceCell` is empty.
- On `Err`, remove the slot only if it is `Arc::ptr_eq` to this one.
- Add a unit test like the one above.

### P1-6. Nothing can be cancelled or prioritised: thumbnails, suggestions and resolves queue behind stale work
- `Job::spawn` (`client.rs:29-38`) and `images::image` (`images.rs:322`) start the work
  regardless of whether anyone still holds the Job. Dropping the Job cancels nothing.
- Images queue FIFO behind an 8-permit gate (`images.rs:326`). After a fling past 300 cards, the
  cards on screen wait behind every off-screen thumbnail requested on the way. That works against
  "scrolling as good as Chrome".
- The polite gate (`polite.rs:433-452`) is FIFO with no priority. `subscription_feed` queues up
  to 40 requests 150 ms apart, so a search or the **resolve for playback** issued right after
  waits up to about 6 s.
- Stale suggestion requests also hold gate slots.

Fix:
- Have the spawned task `select!` on `tx.closed()`, and skip the fetch if every receiver is gone
  before the permit is granted.
- Serve image requests newest-first (LIFO), or let frontends pass a priority.
- Give the polite gate two classes: interactive (resolve, watch, search) ahead of background
  (feeds, suggestions, continuations).

### P1-7. Core does not enforce "muted in tests"; iced can unmute a test run
`NT_MUTE` is read only when mpv starts (`player.rs:342`). `Player::set_muted(false)`
(`player.rs:565`) always obeys. GPUI and Rune each wrote their own `mute_locked()`. iced did not:
`apps/iced/src/app.rs:847` (volume slider unmutes) and `:861` (ToggleMute, the `m` key) unmute a
headless run.

Fix: in `set_muted`, ignore `false` while `NT_MUTE` is set. Put this in core so no frontend can
break the rule.

### P1-8. The library can silently lose everything
- `library.rs:51-54`: if `library.json` fails to parse for any reason, the library starts empty.
  The next `save` then overwrites the user's subscriptions, history and Watch Later. Any later
  non-`Option` field added to `Video`, `ChannelRef` or `LibraryData` without
  `#[serde(default)]` makes every existing file fail to parse, which wipes everyone's library on
  upgrade.
- `library.rs:148-160`: every mutation spawns its own blocking write of a full snapshot to the
  **same** `library.json.tmp`. Up to 4 run at once (`max_blocking_threads(4)`). Two writers can
  interleave inside one tmp file, and an older snapshot's rename can land last (a lost update).
  Six runs of 500 `record_watch` + 401 `set_volume` calls ended consistent, so this one is from
  reading only.
- `set_volume` writes a pretty-printed snapshot (500 history items, about 200 KB) for every
  slider event (`player.rs:559-563`).
- All three apps share one file, and each overwrites it with its in-memory copy (last writer
  wins).

Fix:
- On a parse error, rename the file to `library.json.bad-<time>` and log it.
- Put `#[serde(default)]` on the persisted structs.
- Use a single writer thread with a dirty flag, coalescing writes (for example 250 ms). Write a
  unique tmp name, fsync, then rename.

### P1-9. The benchmark's frame metrics hide the worst stalls and penalise video
`metrics.rs:397` counts a long frame only when `25 ms < gap < 250 ms`:
- A 400 ms freeze during a fling is **excluded** from `long_frames`.
- During playback, a UI that redraws only on new video frames shows 33 ms gaps, and every one of
  them counts as "long". Rune's report has 280 long frames in `playback`.

`fps` divides by phase length including scripted sleeps. `marks_ms` keeps only the first mark of
each name (`:416-419`). The time each command was *sent* is never recorded, so click-to-result
latency (open_video → playback_started, search → search_loaded) can't be computed for the
second and later instances.

Fix:
- Record "frame requested" alongside "frame presented", and count a long frame as a gap above
  1.5 refresh intervals while a redraw was pending (or while the scroll animator reports
  `needs_frame`). Report the stalls over 250 ms separately.
- Have the autopilot emit a mark for every command it sends, and report latency per command.
- Compute fps over active frames only.

### P1-10. Start-up: the "All" home waits for six searches queued through the rate gate
`client.rs:215-237` fires six searches. The gate allows 3 in flight and starts requests 150 ms
apart (`polite.rs:390-392`). `home_raw` waits for **all** of them before returning, so
`home_loaded` on a live launch is at least 750 ms of spacing plus the slowest of six round trips.
In replay mode the gate is skipped, so the benchmark doesn't show this. There is also no on-disk
cache of the last home feed.

Fix:
- Return the first topic's page as soon as it lands and stream the rest in (or fetch fewer
  topics for the first paint).
- Cache the last home page in the data dir and show it at once (stale-while-revalidate).
- Add a live-mode start-up measurement to the bench.

### P1-11. The autopilot drops steps without a word
`autopilot.rs:52-80`: `Step` is `#[serde(untagged)]` with no `deny_unknown_fields`. Verified:

```
[{"do":{"cmd":"back"},"sleep":1.0}]         -> Sleep { sleep: 1.0 }          (the command is dropped)
[{"wait_mark":"home_loaded","timout":5}]    -> WaitMark { timeout: None }    (the typo becomes 30 s)
[{"phase":"x","do":{"cmd":"quit"}}]         -> Phase { phase: "x" }          (the command is dropped)
```

If parsing fails, `start()` (`:101-109`) prints a line and the app runs **without** a scenario.
`run-scenario.sh` then waits 240 s and reports "no report". That is how the `{ "do": … }` bug cost
all three builders a workaround.

Fix: add `#[serde(deny_unknown_fields)]` on every variant (or a tagged format), and
`exit(2)` on a parse error when `NT_AUTOPILOT` is set.

### P1-12. Parsers return empty pages instead of errors when YouTube changes shape, and none are tested on real responses
`collect_videos` (`innertube.rs:209-234`) and `parse_search` / `parse_video_continuation` return
`Ok` with zero items when no known renderer is found. `parse_watch` returns an empty
`ChannelRef` (`:295-311`); iced's NOTES saw this happen live. `home_raw` treats an empty page as
success. So a renderer rename shows up as an empty home with no error.

The repo has 1,850 recorded responses, but the parser tests are four tiny synthetic ones
(`rfc3339`, avatar, suggestions, counts).

Fix: add a test that runs every recorded `POST` fixture through its parser and asserts non-empty
items, titles, a channel and a continuation. Treat "parsed a 200 response and found zero videos
and no continuation" as `Error::Parse`. Log the unknown renderer keys.

---

## P2

**Player**
- Race between the resolve task and `load`/`stop` (`player.rs:494-505`). The ticket check is
  done under `worker_cmd`, but `load`/`stop` don't take it. A `stop` landing between the check
  and `loadfile` lets the old video start playing (sound, no UI). The window is narrow. Fix: take
  `worker_cmd` in `load`/`stop` around the ticket bump and the `stop` command.
- Events from the previous file are applied to the new one (`player.rs:656-668`). An old
  `END_FILE(ERROR)`, or old `duration`/`time-pos`, processed after `load(B)` shows "Couldn't play"
  or A's duration while B resolves. Fix: tag state by `playlist_entry_id` (present in
  `mpv_event_end_file`) and ignore other files' events.
- `Ready` is set at `FILE_LOADED` (`:646-654`), before any picture exists, although the doc says
  "a picture is up". Set `Ready` on the first published frame or on `PLAYBACK_RESTART`.
  `buffering` covers only `paused-for-cache`, not seeking or initial buffering. Expose `seeking`.
- A/V sync: `video-timing-offset=0` (`:359`) has mpv signal a frame when it is due. The frame
  then reaches the screen one UI frame, plus upload, plus compositor later (about 20–50 ms of
  video lag). render.h says this leaves "A/V sync slightly off". Fix: pass the frame's
  `target_time` (`MPV_RENDER_PARAM_NEXT_FRAME_INFO`) with each `VideoFrame`, and measure the
  offset in the bench. Compensate with a small `video-timing-offset` and present at the target,
  or with `audio-delay`.
- CPU per frame (measured, local 720p30 clip, no UI): `ntctl play` 8.3 s wall, 2.9 s user CPU
  (about 35% of a core) with `vaapi-copy`. Render averages 1.81 ms per frame (max 13.7);
  `hwdec=no` gives 3.35 ms. 1080p60 is about 4.5× the pixel rate. The alpha fill
  (`:795-797`) writes every pixel of every frame. Skip it where the toolkit can draw opaque.
- Is a capped software render defensible? It is defensible as the *comparison baseline*: every
  app gets identical frames. It is not defensible for the product, because CPU goes to readback
  plus YUV→RGB conversion plus scaling, and each toolkit then copies again. Faster designs within
  the constraints:
  - mpv's OpenGL render API on an offscreen EGL context on the render thread, rendering into a
    dma-buf-backed texture each toolkit imports (zero CPU per pixel);
  - or GPU render plus asynchronous PBO readback into the existing pool (conversion and scaling
    move to the GPU, the API is unchanged).
- `seek_by` (`:554`) is a relative seek, so it is keyframe-rounded under mpv's default
  `hr-seek`. ±5 s can land seconds off. Use `relative+exact` for small steps.
- Each 10 MiB chunk and each seek opens a new TLS connection to googlevideo. Add
  `multiple_requests=1` to `stream-lavf-o` (the option exists in this ffmpeg).
- Document that the waker must never block. It runs on the render thread, and render.h forbids
  any wait dependency from the render thread on a thread making libmpv calls.

**Resolve**
- `resolve.rs:63-87`: every playback depends on `sw.js_data` at index `[0][2][0][0][13]`. If that
  shape changes, every video fails with no fallback. Fall back to the `visitorData` from any
  Innertube `responseContext` (the client already gets these), or send no visitor id.
- Live without `hlsManifestUrl` (`:153`) falls through to adaptive formats and fails obscurely.
  HLS variant choice ignores `NT_MAX_HEIGHT`; mpv takes the top variant, possibly 4K on a
  software renderer. Set `hls-bitrate` or pick the variant.
- Resolve has its own `reqwest::Client` (`:51-60`), so the first play pays a fresh TLS handshake
  to www.youtube.com. `Cargo.toml` leaves out reqwest's `http2` feature, so every request is
  HTTP/1.1.
- A 403 from `/player` (which can also mean a stale client version) pauses **all** YouTube
  traffic for 5 min (`polite.rs:476-479`).

**Client and parsers**
- Duplicates across "load more" pages are not removed. `interleave` (`client.rs:338-357`) dedups
  within one page only, and continuation pages repeat items, so keyed lists get duplicate ids.
  Return a per-feed `seen` set with the cursor, or document that frontends must dedup.
- `more_multi` (`client.rs:206-210`) fails the whole page if one topic's continuation fails,
  unlike `home_raw`, so the "All" feed stops paging.
- `subscription_feed` (`client.rs:313`) silently uses only the first 40 subscriptions. If every
  feed fails it returns `Ok([])`, so the UI says "no videos" instead of showing an error.
- `fixtures.rs:559-566`: concurrent stores of one key share `<hash>.tmp`. The writes are
  synchronous `std::fs` calls on the 2 async worker threads.
- `parse_feed` gives an entry with no `published` the timestamp 0 ("56 years ago")
  (`innertube.rs:471`).

**Scroll** (the port matches docs/scroll.md and its 19 tests pass; these are traps)
- One NaN poisons the offset for good. Verified:
  `[NaN wheel_ticks] offsets after: NaN, after a real notch: NaN, after a touchpad delta: NaN`.
  The same happens with `wheel_pixels(NaN)` and `scroll_to(NaN)`. `f64::clamp` passes NaN through
  (`scroll.rs:976, 1023, 1112`). Drop non-finite deltas and offsets at the API boundary.
- `tick(+inf)` (or event times on another clock) freezes time. Verified: after `tick(inf)`, a
  notch at t=5 lands instantly with no animation. Ignore non-finite times, and
  `debug_assert` that times come from one clock.
- The first sample of a gesture gets `dt` = time since the previous gesture
  (`scroll.rs:1266`). A flick of 3 frames or fewer then gets almost no fling (verified: a 3-frame
  40 px flick at 8 ms produced a 1 px fling). Chrome counts every `wl_pointer.frame`. Clamp the
  first frame's `dt` to one frame interval.
- `viewport` defaults to `INFINITY` (`:834`). A frontend that forgets `set_viewport` gets no
  delta limit and no 3-viewport fling cap. Default to a finite size, or debug-assert it was set.
- Shrinking `max_offset` mid-curve keeps requesting frames while pinned (verified: 6 extra
  frames). Harmless.

**Tools**
- `tools/headless.sh run` doesn't set `NT_DATA_DIR`, so manual test runs write the owner's real
  `~/.local/share/native-tube/library.json`: history, volume, and with P1-8, possible loss. That
  file was last written today at 06:40 and has 9 history entries.
- `nt-input touchpad` (`main.rs:227-243`) uses an ease-out profile, so lift-off speed is about
  1/8 of the swipe's average. Its default "600" is in axis units, which is 7200 px after the ×12
  Chrome scale. Verified through the animator: the swipe moved 7200 px in 256 ms and the fling
  added 2336 px. Real flicks lift at peak speed. Use a constant or ease-in profile with px-sized
  defaults.
- `nt-input key/type` focuses the test window (`:259`), taking keyboard focus from the owner's
  window for about 160 ms.

**Metrics and autopilot**
- `exec_to_init` comes from `/proc` ticks (10 ms resolution), and marks are relative to `init`.
  Report `first_frame` and `home_loaded` from exec directly.
- `autopilot::run` ends with `process::exit(0)` 5 s after Quit, while other threads may still be
  tearing down mpv or the GPU. Harmless for reports.

**Fine (checked)**
- FFI struct layouts and the use of event data are correct. `Handle` Send+Sync is correct. The
  render context is created on the UI thread and then used only on the render thread, which is
  allowed since mpv 0.30.
- Shutdown order is correct: worker gate, render thread joined, context freed, `quit`, event
  thread joined, destroy.
- The frame pool never overwrites a frame someone holds: reuse requires `strong_count == 1`, it
  is guarded by `Arc::get_mut`, and `to_bytes` holds an `Arc`.
- The muted-flag reset by `stop` (1497d8b) is fixed.
- The scenario-parse regression test exists (`autopilot.rs:211`).
- The polite gate's cooldown arithmetic is correct.
- `format.rs` is correct.

---

## API: what pushed the frontends into awkward or slow code, and what is missing

The frontends had to work around:
- the scenario format (fixed in core);
- the mute rule (two apps re-implemented it; P1-7);
- RGBA-only images, so GPUI copies every thumbnail through `to_bgra`. Let `image()` take a
  `PixelFormat`;
- `Arc<VideoFrame>` when Rune needs an owned `Vec`, which costs an extra copy per frame;
- iced building a new image handle per frame, so it uploads a new texture each time.

Offer a frame API that writes into a buffer the caller owns (or recycles), and send each frame's
target time with it.

There is also a two-level image cache: the core keeps a 96 MB decoded LRU, and each app keeps
its own copies on top. Either drop the core LRU when a frontend caches, or make the core cache
the only one.

Missing for a first-class viewing experience:
- quality selection and Auto (P1-3);
- playback speed (`speed` property);
- captions: `captionTracks` from `/player`, loaded with `sub-add`; the software renderer
  composites them;
- chapters, from the description or `/next` engagement panels;
- an audio-track picker (P1-4);
- audio-only mode (`vid=no`; saves the decode and render);
- a resume position in history;
- seeking state, dropped-frame and A/V-offset counters;
- live: whether the stream is DVR and how far behind the live edge playback is.

## Tests that would have caught the known bugs, and what is untested

- The `{ "do": … }` scenario: `scenario_parses` now covers it. Also add a
  `deny_unknown_fields` round-trip over `bench/*.json` (P1-11).
- The stale frame after load: a two-clip colour test with `NT_VIDEO_OVERRIDE` (P1-1).
- The muted flag reset by stop: `Player::new` with `NT_MUTE=1`, then `stop()`, then assert
  `state().muted`; and `set_muted(false)` stays muted (P1-7).

Untested today:
- every Innertube parser against the recorded fixtures (P1-12);
- `pick` against a real `/player` response with multiple audio tracks (P1-4);
- image cache accounting under concurrent and evicting requests (P1-5);
- library load of a corrupt or old-schema file, and save ordering (P1-8);
- `interleave` and `more_multi` dedup and failures;
- the fixture key's independence from Cargo features (P0-1);
- the scroll API with non-finite input;
- the player's state machine (Ended → play, seek past the end, which I checked by hand: Ended at
  60.0/60.0 and paused; error END_FILE; a stop racing a resolve).
