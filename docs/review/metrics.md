# Review: measurement validity and coverage

Reviewer lens: are the numbers any good, and what isn't measured. Scope: core/src/metrics.rs,
core/src/autopilot.rs, bench/*, tools/headless.sh, bench/results/*, apps/*/shots/report.json,
each app's frame hook, and the three builder transcripts.

Legend: **[ran]** = verified by running something; **[read]** = concluded from reading code or data.
I did not run the GUI apps or touch the NT-* outputs. For a few facts I read `/proc/<pid>` of the
three app instances another reviewer had running (fdinfo, smaps, status). That is read-only, and it
is marked **[proc]** below.

---

## 0. Summary

Most current numbers can't support a framework comparison. The steady-state CPU per phase
reproduces (two runs per app agree within about 1–4 points). Nearly everything else is invalid,
confounded or missing:

- GPUI replays a different set of recorded YouTube answers from iced and Rune, so it shows
  different videos and its misses go to live YouTube (P0-1).
- Start-up is measured at three different points in the pipeline, at 10 ms resolution, from one
  run, and it moved 2.8× between two runs (P0-2, P0-3).
- Frame statistics mix motion with scripted sleeps and call every 30 fps video frame "long" (P0-4).
- No metric measures input latency, presented frames, GPU memory or time, energy, video
  drops, binary size or build cost. No raw-shell baseline exists.

---

## 1. Validity of every number produced today

### P0-1 GPUI runs on different data from iced and Rune, and can go live mid-benchmark  [ran]
- `core/src/client.rs:141` builds the fixture key as `format!("POST {endpoint} {body}")` from a
  `serde_json::Value`. In GPUI's dependency graph serde_json is built with `preserve_order`:
  `apps/gpui/Cargo.lock` lists `indexmap` as a serde_json dependency, and iced's and Rune's locks
  don't. So in GPUI the body serializes as `{"query":…,"params":…}`, while in iced and Rune it is
  `{"params":…,"query":…}`. These hash to different fixture files. Example for the home "music"
  topic: `21ffe9b5c138b924.bin` (GPUI order, recorded 01:15) versus `ab8de184c8e00eb8.bin`
  (sorted order, recorded 01:17). I computed the FNV hashes for both orderings, and both files
  exist.
- Visible result: `bench/results/gpui/01-home.png` opens with "Future ft. Drake…", while iced and
  Rune open with "Quédate Tonight…", and shared videos show different view counts (55K vs 56K).
  `ntctl home` in replay mode reproduces the iced/Rune feed.
- `tools/headless.sh` uses `NT_FIXTURE_MODE=auto`, so any miss is fetched from YouTube and
  recorded. Fixture files carry mtimes during the agents' runs (06:20–06:29) and during this review
  (12:03). Runs are not network-free, and one app's run can change the next app's data.
- Why it matters: home, search, watch, comments and channel content differ between apps. That
  changes the number of thumbnails, text shaping and comment heights, so every per-phase CPU,
  memory and frame number for GPUI is confounded. It also breaks the "small traffic" rule silently.
- Fix:
  - Canonicalize the key: serialize with sorted keys explicitly, e.g. through a `BTreeMap`, or
    hash `endpoint` plus a canonical JSON writer.
  - Re-record the fixtures once.
  - Benchmarks must use `NT_FIXTURE_MODE=replay` (fail on a miss).
  - Add a `fixture_misses` counter to the report and fail the run if it is non-zero.
  - Write a hash of the home feed's video ids into the report, so identical input is checkable.

### P0-2 `exec_to_init_ms` (110 ms in all three) is real but coarse, and is not exec-to-init  [ran]
- How it is computed: `/proc/uptime` (10 ms resolution) minus `starttime` from `/proc/self/stat`
  (clock ticks, `CLK_TCK=100`). The result is quantized to 10 ms and can only be 100, 110 or
  120 ms. Identical values are expected, not a bug. I checked that the two clocks agree: a bash
  process reads a difference of 0–10 ms.
- `starttime` is the fork time of the background subshell in `headless.sh` (`setsid "$@" &`), not
  the exec.
- What dominates it [ran]: `ntctl --help` links the same libmpv tree (204 shared objects, 46,656
  relocations) and costs **70 ms of user CPU and 73–120 ms wall before doing anything**
  (`perf stat -r 5`, task-clock 70.3 ms ±2%). `perf record -e cycles:u` puts **83% of it in
  `ld-linux-x86-64.so.2`** (symbol lookup and relocation). The CPU ran at about 1.3 GHz under the
  powersave governor with EPP `balance_performance`.
- So roughly 70–100 ms of every app's start-up is the libmpv dependency tree. That cost is the
  same in all three and has nothing to do with the UI framework.
- Fix (measurement):
  - The harness records `t0 = CLOCK_MONOTONIC` immediately before `execve`. For example the
    launcher passes `NT_T0_NS`, or the external probe in §2 spawns the app itself.
  - `metrics.rs` reports every mark as absolute `CLOCK_MONOTONIC` ns, so in-app marks and external
    observations share one clock.
- Fix (product, P1): `dlopen` libmpv on first playback instead of linking it. That stays within
  "native libraries only". Expect −60 to −100 ms on every cold and warm start.

### P0-3 `first_frame` / `home_loaded` mean different things per app, and one run is noise  [read + data]
- **Where the hook fires** (NOTES.md "Frame hook" sections and code):
  - iced `scroller.rs:654`: inside `FrameHook::draw`, while the CPU builds primitives, *before*
    wgpu encodes, submits and presents.
  - GPUI `app.rs:1003`: in a canvas paint callback, during paint, *before* `present()`.
  - Rune `main.rs:40`: in `on_frame_encoded` with outcome `Drawn`, *after* encode, submit and
    present.
- On first frame, pipeline and shader compilation and swapchain creation happen at submit and
  present. iced and GPUI therefore under-report first pixels relative to Rune by however long that
  takes. On a cold Mesa shader cache this can be tens to hundreds of ms.
- **What is marked:**
  - GPUI marks `first_frame` on the first paint (chrome only, 336 ms) and `home_loaded` 60 ms later.
  - Rune emits both in the same frame (226.40 vs 226.39 ms; `home_loaded` is even timestamped
    first, because marks are flushed in push order).
  - iced: 234 ms and 261 ms.
  - With zero-latency fixtures, `home_loaded` measures "feed JSON parsed and laid out". It does not
    measure the network or the thumbnails. No metric says "thumbnails on screen".
- **Variance:** the agent's own run (`apps/*/shots/report.json`) against the later
  `bench/results` run gives:

  | App | first_frame, agent run | first_frame, bench run |
  |---|---|---|
  | iced | 645 ms | 234 ms (2.8×) |
  | GPUI | 465 ms | 336 ms |
  | Rune | 255 ms | 226 ms |

  Cache state (page cache, shader cache, freshly built binary) is uncontrolled. A single start-up
  number is meaningless.
- **`comments_loaded`** also differs:
  - GPUI and Rune mark it when the first comments page lands during `open_video` (about 38.3 s).
  - iced marks it only after `load_more_comments` (48.4 s).
  - `marks_ms` keeps only the first occurrence, so the column compares different events.
- Fix:
  - Measure first pixels *outside* the app (§2, external probe): the time of the first compositor
    frame that contains the window's content.
  - Keep in-app marks only for "data in the frame". Stamp them at present time where the
    framework exposes it: Rune's `FrameEncoded::presents_at()` and `started()` exist and are
    unused.
  - Add a `thumbs_visible` mark: the first frame in which every thumbnail inside the viewport is
    decoded and drawn.
  - Store *all* marks with phase and sequence, not just the first of each name.

### P0-4 Frame statistics: "long frames", percentiles and fps are not smoothness metrics  [read + data]
- `metrics.rs:241-243` computes gaps between all consecutive frames in a phase, including the
  scripted `sleep`s:
  - `frame_ms_p99` and `max` are idle gaps. Examples: search phase p99 887 ms (iced) and 885 ms
    (GPUI); `wheel_scroll_home` max 910 ms.
  - `fps = frames / phase duration` includes idle time, so "44 fps" during a wheel scroll means
    nothing.
- "Long frame" means 25 ms < gap < 250 ms. During 30 fps playback every gap is about 33 ms, so
  playback shows 302, 301 and 280 "long frames", and `scroll_while_playing` shows 55, 53 and 27.
  These are video cadence, not jank. Elsewhere a long frame can also be the gap between two
  discrete animations.
- 60 Hz is hard-coded. The real panel runs at 165 Hz.
- Fix: count only *motion frames*, that is frames where the scroll offset changed or an animation
  was active. Per frame, record `(present_ts, offset, animating, video_pts)`. Then:
  - Missed vsyncs per frame interval: `round(Δt / refresh_period) − 1`.
  - Report: % of motion intervals equal to exactly one refresh period; dropped-frame count; p50,
    p95 and p99 of the *motion* intervals; the longest motion stall.
  - Scroll *position error*: displayed offset against the core animator evaluated at the present
    time. This is what makes Chrome feel smooth even at the same fps.
  - Take `refresh_period` from the output (`wl_output` mode or `wp_presentation`), not a constant.

### P0-5 `metrics::frame()` doesn't mean the same thing in the three frameworks  [read]
- iced: CPU draw. GPUI: CPU paint. Rune: after encode and present. Whether a frame was actually
  presented (FIFO vs mailbox) and when the compositor showed it is unknown in all three.
- Make it comparable from outside the process, and keep the in-app hook only as a cross-check:
  - **(a) Recommended:** an external probe binds `zwlr_screencopy_manager_v1` or
    `ext-image-copy-capture-v1` on the app's NT-* output and calls `copy_with_damage` in a loop.
    Each `ready` event carries the compositor's timestamp for the presented output frame, plus the
    damage. This gives presented-frame times, identical in meaning for all three apps *and for
    Chromium*.
  - **(b) Cheap fallback:** `WAYLAND_DEBUG=1`. libwayland-client (iced via winit's dlopen) and
    wayland-backend's Rust backend (GPUI) both print every request with a µs timestamp. Count
    `wl_surface@<toplevel>.commit` and `wl_callback.done`. The text output perturbs CPU, so use it
    only for the frame-count cross-check.

### P1-1 CPU% for short phases (`open_video` ≈ 240% in all three) is quantization plus a real burst  [read]
- `cpu_seconds()` reads `utime + stime` in 10 ms ticks. `open_video` lasts about 0.1 s, so each
  endpoint carries ±10 ms, which is up to ±20 points of error. mpv start-up legitimately uses
  several threads (decoder, VA-API, demux), so >100% is plausible. The figure is not comparable at
  this precision.
- Fix:
  - Use `clock_gettime(CLOCK_PROCESS_CPUTIME_ID)` (ns) for process CPU.
  - Use `/proc/self/task/*/stat` deltas for per-thread attribution: UI, render, mpv, tokio.
  - Report CPU in *CPU-ms per phase*, not % of wall time, for phases under 2 s.

### P1-2 Phase CPU% includes the scripted sleeps  [read]
- The "startup" phase is 2.4 s, of which 2.0 s is `sleep`. `wheel_scroll_home` includes 2 s of
  trailing sleeps.
- Fix: split each phase into `active` (from the first command to animation end) and `tail`.
  Report CPU-ms per active phase, and CPU per idle second separately.

### P1-3 Memory: RSS overstates, GPU memory is invisible, and peak misses the end value  [proc + data]
- From smaps_rollup of the instances that were running:

  | App | RSS | PSS | Pss_Anon | Pss_File | i915 GPU buffers (`drm-total-system0`) |
  |---|---|---|---|---|---|
  | iced | 146 MB | 85 MB | 57 MB | 28 MB | 88 MB |
  | GPUI | 236 MB | 142 MB | 75 MB | 66 MB | 123 + 6 MB |
  | Rune | 186 MB | 118 MB | 75 MB | 44 MB | 74 MB |

  RSS counts shared libraries fully. GPU buffers are about as large as the whole PSS and are
  counted nowhere today.
- All three map the NVIDIA userspace driver (11–22 MB of PSS in `libnvidia-*`) and hold
  `/dev/nvidia0` open, but they render on the Intel iGPU (i915 fd with non-zero
  `drm-engine-render`). The Vulkan loader enumerates both ICDs. GPUI also maps `libLLVM` and
  `libgallium` (a Mesa GL/EGL path).
- `rss_mb_peak` comes from the 100 ms sampler only. For phases shorter than 100 ms, peak is less
  than end (`open_video`: peak 331, end 349 in iced). Include the end sample in the peak.
- The core image LRU sits at its 96 MB budget in all three (`image_cache_mb` 95.8–96.0). Decoded
  thumbnails are a large, framework-independent share of anonymous memory. Report it separately.
- Fix: report PSS (anon and file), GPU memory from `/proc/<pid>/fdinfo/*` (`drm-total-*`, deduped
  by `drm-client-id`), the core image cache, and the framework texture atlas where exposed. Report
  peak and end per phase.

### P1-4 Run conditions are uncontrolled and unrecorded  [ran + proc]
- Power state is unrecorded: governor `powersave`, EPP `balance_performance`, profile
  `balanced`; AC was online during this review.
- Arrow Lake hybrid CPU: P-cores at 5.1 GHz, E-cores at 4.4 GHz, LP-E cores at 2.5 GHz. Thread
  placement changes per-frame cost run to run.
- Desktop shell drawing on the test outputs:
  - Caelestia/quickshell (pid 1801) puts 1280×800 `caelestia-background` and `caelestia-drawers`
    layer surfaces, plus border-exclusion zones, on *every* NT-* output.
  - Hyprland composites them on each frame of the app. Shell animations add compositor work, and
    the exclusion zones can change the window's usable size.
- Window geometry and scale are not in the report. During this review the three test windows
  were floating at 640×700 (changed by someone else). A run in that state would silently compare
  a different layout.
- Other load: the other reviewers' apps, vesktop and the T3 Code window were running.
- Headless 60 Hz output: frame callbacks are driven by Hyprland's timer, not real vblank, and
  there is no 165 Hz case at all.
- `NT_DATA_DIR=$out/data` is reused across runs (`run-scenario.sh` deletes only the report and the
  PNGs). History accumulates.
- Caches are always warm: page cache for binaries, libraries and fixtures; Mesa shader cache in
  `~/.cache/mesa_shader_cache` (112 MB); NVIDIA cache.
- Fix: the checklist in §5. Every report should embed: governor, EPP, profile, AC/battery,
  per-core max frequency, kernel, Mesa and NVIDIA versions, git SHA, binary sha256, window
  geometry and scale, output refresh, and fixture-miss count.

### P1-5 One run per app; no spread  [data]
- Steady phases reproduce: two runs per app agree to within 1–4 CPU points (e.g. GPUI
  `infinite_scroll` 40% / 40%; Rune `playback` 34% / 33%).
- Start-up does not reproduce (P0-3), and `bench/results/playback.json` has 2 runs whose web
  values differ 3×: cold 2.0 s vs 6.3 s, warm 1.7–2.3 s vs 5.4–5.5 s. Medians of two values are
  not medians.
- Fix: §5 (N ≥ 10 for start-up, N ≥ 5 for sessions, interleaved order, median and IQR).

### P1-6 `bench/playback.py` compares unlike things  [read]
- "first_frame" means four different things:
  - core: picture in RAM, no window;
  - mpv: "first video frame after restart shown";
  - web: `currentTime > 0`, polled every 20 ms;
  - mpv cold includes process launch, while web cold excludes browser launch (the clock starts in
    the page).
- Core CPU (25–63%) includes software rendering to RGBA (`player.rs:401` `"sw"` render API after
  `vaapi-copy`), but the apps then add a copy and a GPU upload. mpv uses zero-copy `gpu-next`.
- Fine as a smoke test. Not a reference column for the apps. Replace it with the same external
  probe (§2) on all players.

### P2-1 `video_frames_rendered` is mpv render calls, not frames shown  [read]
- Rune's playback phase draws 327 UI frames for 303 video frames. Nobody checks that each video
  frame was displayed exactly once (or twice at 60 Hz).
- See §2 video: the frame-index barcode clip.

### P2-2 Report gaps  [read]
- `more_loaded` count per phase is missing (only the first occurrence is kept).
- Failed waits are listed, but there is no `ok` flag per phase.
- `samples` are at 100 ms, but there are no thread counts or context switches.

### Fine
- Idle phases really are idle: 0 frames and 0.0% in all three.
- The autopilot drives all three through the same command sequence.
- `run-scenario.sh` isolates one app per run.

---

## 2. Coverage: the metric set this comparison needs

Rule for every metric: same input data (replay mode, zero misses), same window (1280×800 at
scale 1 unless stated), muted, external measurement where possible. Each metric is reported as the
median and IQR over N runs. **P0** = needed for a credible verdict, **P1** = needed for a complete
one, **P2** = nice to have.

### Tooling to build (test tooling, so outside the native-only rule)
- **`nt-probe`** (Rust, extends `tools/nt-input`).
  - On one NT-* output it:
    - spawns the app itself (records `t0` = CLOCK_MONOTONIC just before `execve`);
    - runs a screencopy-with-damage loop (each `ready` gives the presented-frame timestamp and the
      damage);
    - injects input through the existing wlr virtual pointer and keyboard;
    - samples `/proc/<pid>` and `/proc/<pid>/task/*` every 50 ms (CPU ns, PSS via smaps_rollup
      every 500 ms, threads, context switches, `fdinfo` GPU memory and `drm-engine-render` ns);
    - samples Hyprland's CPU and DRM engine time too.
  - It writes one JSON with the app's own report merged in, aligned on CLOCK_MONOTONIC.
  - The same probe drives Chromium (window `--class=nttest-bench`), so the web column is measured
    identically.
- **`metrics.rs` changes:** absolute monotonic timestamps; per-frame records `(t, presents_at if
  known, offset, animating, video_pts)`; all marks; CPU via `CLOCK_PROCESS_CPUTIME_ID`;
  `fixture_misses`; environment block.
- **Autopilot additions:** a `seek {seconds}` command plus a `seek_done` mark; `set_size {w,h}`;
  `loop {n, steps}` for soak tests; a `mark_frame` hook that tags the next presented frame.
- **Clips:** generate locally with ffmpeg. Each frame carries its index as a 16-bit binary barcode
  of 32×32 px black/white blocks in a corner, so the probe can read *which* video frame each
  presented output frame shows. Variants, all H.264 with AAC:
  - 720p30 (existing content plus barcode);
  - 1080p60;
  - 1440p60 or 2160p30;
  - VP9 1080p60 and AV1 1080p60 versions, because YouTube serves those.

### Start-up (P0)

| Metric | Definition | Capture |
|---|---|---|
| `t_first_pixels` | `t0` (exec) → compositor timestamp of the first screencopy frame where the window region is not uniform background | nt-probe |
| `t_interactive` | `t0` → first presented frame after which a click on the search box shows the caret / focus ring | nt-probe: inject a click at `t_first_pixels`+k×16 ms until a response; report the first k that works |
| `t_home` | `t0` → presented frame containing the home feed's first row of titles | in-app `home_loaded` at present time, cross-checked by damage in the grid region |
| `t_thumbs` | `t0` → presented frame where every thumbnail in the viewport is drawn | new in-app `thumbs_visible` mark, at present time |
| breakdown | exec→main (dynamic linking), main→window mapped, →first commit, →home, →thumbs | `NT_T0_NS` + marks; `LD_DEBUG=statistics`; `perf record -e cycles:u -g` on one run |

- **Cold run:** evict the binary, every `ldd` library and the fixtures from page cache without root
  (`dd if=<f> iflag=nocache count=0`; works for own files [ran], `fincore` verifies). Use an empty
  shader cache (`MESA_SHADER_CACHE_DIR=$(mktemp -d)`, `__GL_SHADER_DISK_CACHE_PATH=$(mktemp -d)`)
  and a fresh `NT_DATA_DIR`.
- **Warm run:** discard one warm-up launch, then measure.
- Also record the variant with the NVIDIA ICD hidden
  (`VK_LOADER_DRIVERS_SELECT='*intel*'`) to size the dGPU driver's cost.

### Input latency (P0)
Input event (virtual pointer `frame` sent, CLOCK_MONOTONIC) → compositor timestamp of the first
presented frame whose damage covers the expected region. Inject at random phase relative to
vsync, 50 samples per kind. Report p50 and p95.
- wheel notch on the home grid (content moves);
- touchpad 1-px pixel scroll;
- hover on a card (hover state appears);
- click on a chip (selection changes);
- a typed key in search (glyph appears);
- click on a video (watch page chrome appears).

### Scroll smoothness (P0)
Only *motion intervals* count: frames where the scroll offset changed or an animation was active.

| Metric | Definition |
|---|---|
| on-time % | share of motion intervals equal to 1 refresh period (±25%) |
| dropped frames | Σ (round(Δt/period) − 1) over motion intervals, and per second of motion |
| p50 / p95 / p99 / max motion interval | |
| position error | rms px between the presented offset and the core animator's value at present time |
| first-frame response | wheel event → first moved frame (from the input latency set) |

- Scripts: wheel 60 notches, fling 6000 px/s, touchpad drag plus fling, infinite scroll across
  3 page loads, and scroll while a video plays.
- Run on headless@60 and headless@165 (`hl.monitor({… mode = "1280x800@165" …})`).
- A real-panel 165 Hz run only by the owner, manually, since test windows may not open on the
  real screen.
- Source: screencopy timestamps (external) plus app offsets per frame.

### CPU and energy (P0 for CPU, P1 for energy)
- CPU-ms per active phase and per presented frame, split by thread (UI, render, mpv decode, tokio
  workers, others), plus Hyprland CPU-ms in the same window (the compositor cost of the app's
  damage).
- Idle CPU over 60 s in three states:
  - visible and unfocused (current);
  - hidden: workspace moved off the output, `hyprctl dispatch movetoworkspacesilent`;
  - focused: requires focusing the headless window, which steals the owner's keyboard focus, so
    run it only in an unattended session.
- Energy: RAPL `energy_uj` is root-only here [ran]. Either the owner runs once per boot
  `sudo chmod a+r /sys/class/powercap/intel-rapl:*/energy_uj` (package, cores, psys), or measure
  on battery from `BAT0/power_now` over ≥120 s windows minus a desktop-idle baseline. Report
  joules per phase and watts at idle and during playback.

### Memory (P0)
- PSS (anon and file) at phase ends.
- GPU memory per process (i915 `drm-total-system0` / `drm-resident-system0` from fdinfo; NVIDIA
  via `nvidia-smi --query-compute-apps` if ever used).
- Core image cache bytes. Framework texture atlas, where exposed.
- **Soak:** loop home → search → watch 10 s → channel → back×3 for 20 iterations in one process.
  - Report PSS and GPU memory at home after each iteration.
  - Leak = slope over iterations 3–20 (MB/iteration), with a pass threshold (e.g. < 0.5 MB/iteration).
  - "After returning home" = PSS at home in iteration 20 minus iteration 1.
- Heap by source without heaptrack: optional counting `#[global_allocator]` wrapper behind an
  `NT_ALLOC_STATS` feature (live bytes and allocations per phase).

### GPU (P1)
- Which device renders: the fd with non-zero `drm-engine-render`. [proc] All three render on i915
  but open `/dev/nvidia0`.
- GPU busy ns per phase (`drm-engine-render` delta) for the app and for Hyprland.
- GPU memory as above. Report whether the dGPU is woken. Here Hyprland itself holds `/dev/nvidia0`
  because outputs hang off it, so runtime D3 is unavailable anyway; say so in the report.

### Video (P0 for 720p30 and 1080p60; P1 for the rest)
- Time to first video frame: click → presented frame showing barcode frame 0 or 1.
- Presented-frame accuracy from barcodes: % of source frames shown, repeats, skips, judder
  (stddev of on-screen duration per frame in vsync units: at 60 Hz a 30 fps clip should be exactly
  2, 2, 2…).
- mpv properties per phase: `frame-drop-count`, `decoder-frame-drop-count`,
  `vo-delayed-frame-count`, `avsync` (audio is muted but the audio clock still runs; check that the
  AO is active with mute).
- Seek latency: `seek` command → first presented frame with the target barcode.
- CPU-ms and GPU-ns per decoded frame.
- Clips: 720p30, 1080p60, 1440p60/2160p30, VP9 and AV1.

### Size, links, threads, wakeups (P1)
- Stripped binary size (`strip -o` a copy), plus `.text` and `.rodata`.
- `NEEDED` and transitive `ldd` count.
- Threads at idle (`/proc/<pid>/status`).
- Wakeups per second at idle: Σ voluntary + nonvoluntary context switches over
  `/proc/<pid>/task/*/status` across 30 s.

Values today [ran / proc]:

| | iced | GPUI | Rune |
|---|---|---|---|
| stripped binary | 20.6 MB | 48.7 MB | 40.4 MB |
| `.text` | 13.6 MB | 32.4 MB | 25.4 MB |
| raw binary | 27.7 MB | 66.6 MB | 51.7 MB |
| shared objects loaded (all via libmpv) | 204 | 204 | 204 |
| threads, early in session | 7 | 34 | 13 |

`ntctl` (core plus CLI) strips to 8.1 MB.

### Build cost (P1)
- Measure in a separate `CARGO_TARGET_DIR` after `cargo fetch` (so downloads are excluded), with
  sccache off, the same toolchain and fixed `-j`, on AC with the performance profile:
  - clean release build, with `--timings`;
  - clean dev build;
  - incremental rebuild after touching a leaf app file (e.g. view.rs);
  - incremental rebuild after touching `core/src/model.rs`.
- Target dir size.
- Dependency count: `cargo tree -e normal,build --target x86_64-unknown-linux-gnu --prefix none`,
  unique name and version pairs, minus core's 126.

  | | iced | GPUI | Rune |
  |---|---|---|---|
  | Linux dependencies, normal + build [ran] | 279 | 549 | 341 |
  | Cargo.lock packages (all platforms; not comparable) | 431 | 811 | 500 |
  | target dir | 1.1 GB | 2.2 GB | 1.7 GB |

- Lines of app code (cloc, code lines only) [ran]:

  | | code | files | note |
  |---|---|---|---|
  | iced | 3,375 | 10 | |
  | GPUI | 2,736 | 7 | plus 14,948 vendored lines in `vendor/gpui-pre-linux`, of which about 30 are the patch |
  | Rune | 2,208 | 8 | |
  | core | 4,256 | | shared |

### Chromium reference column (P1)
- Helium/Chromium on NT-BENCH, driven by CDP, but measured by the *same* nt-probe:
  - first pixels and home-with-thumbnails (external);
  - wheel and fling smoothness from screencopy;
  - process-tree PSS and CPU;
  - GPU process memory and time;
  - `getVideoPlaybackQuality()` for drops.
- Live network is unavoidable. Mark it as "reference, live data", use one session per run, and
  keep N = 5. Helium's bundled uBlock means no ads: note it.

---

## 3. The raw-shell baseline (P0: it doesn't exist)

Create three crates under `shells/<fw>/`, each with its own `Cargo.lock`. Each pins exactly the
framework versions, revisions and feature flags the app uses:
- iced 0.14 with `wgpu, wayland, advanced, image-without-codecs, crisp, tokio`;
- `gpui-pre = 0.3.12` and `gpui-pre-platform` with `wayland`;
- Rune git rev `4fbd2d6`.

Each crate uses the same `[profile.release]` (`debug = false`, `codegen-units = 16`, no LTO) and
the same toolchain (`rust-toolchain.toml`).

Content, identical in all three:
- One window, 1280×800, title "NativeTube shell", app_id `nttest-shell-<fw>` (`nt-shell-<fw>`
  when launched normally).
- Background #0f0f0f, not decorated beyond what the framework does by default.
- One label, "NativeTube", 14 px, colour #f1f1f1, centred, in the *same font family and loading
  method* the app uses (bundled vs system).
- No images, no animation, no timers, no async runtime unless the framework requires one.
- **No nt-core and no libmpv.** This is essential: libmpv alone costs about 70 ms of start-up and
  ~200 shared objects (P0-2).

Instrumentation:
- Move `metrics.rs` (and the bits of autopilot it needs) into a tiny `nt-metrics` crate with only
  `libc` and `serde_json`, used by both apps and shells.
- Each shell calls `init` first thing, `frame()` at the same hook its app uses, and `first_frame`
  on its first drawn frame.
- With `NT_SHELL_EXIT=<s>` it writes its report and exits after s seconds.

Variants:
- `shell-min`: framework only.
- `shell-kit`: adds the component kit the app uses (gpui-component plus gpui-kit-assets; rune-kit;
  iced has none, so its kit variant equals min). This separates framework cost from kit cost.
- Optional `shell-list`: 1,000 text rows scrolled by the core scroll physics, to show the
  framework's per-frame cost without app logic.

Measure: the start-up set, idle CPU and wakeups (visible and hidden), PSS and GPU memory, threads,
stripped size, `NEEDED`, clean and incremental build time, and dependency count. Then report
**app minus shell** as the "cost of the app on that framework".

---

## 4. Agent build cost [ran: read-only script over the transcripts]

Transcripts (identified by the first prompt, the cwd and which app paths they touch):

| App | Transcript |
|---|---|
| iced | `<transcripts>/00045ead-e5cb-4953-b5e9-56d42a3164dc.jsonl` |
| GPUI | `…/18cc1abb-bd0c-4cf4-a303-c8dc5b484592.jsonl` |
| Rune | `…/455f0010-7e76-49ab-8a3e-7ea05ce01ae6.jsonl` |

Shared, not attributable to an app:
- `0c9be2c5…` (scroll port): harness cost $6.16.
- `682e2420…` (stream resolver).
- `e3c6a833…`: the orchestrator, which made the mid-task core changes.

Script: `bench/agent-cost.py`
(prices are arguments). Method:
- Dedupe assistant lines by `message.id`: one API response spans several JSONL lines with the
  same usage.
- Sum `usage.{input_tokens, output_tokens, cache_creation.ephemeral_5m/1h, cache_read}`.
- Skip `<synthetic>` (rate-limit) messages.
- Tool calls = `tool_use` blocks.
- Cargo calls = Bash commands matching `cargo (build|check|clippy|run|test)`. A call counts as
  failed if its `tool_result` contains `error[E…]`, `error:` or `could not compile`.
- Wall time = first to last user/assistant timestamp, minus each gap that follows a rate-limit
  message.
- The `cost-state` lines in each transcript carry the harness's own cumulative cost.

| | iced | GPUI | Rune |
|---|---|---|---|
| API responses (turns) | 181 | 244 | 224 |
| user prompts (start plus resumes) | 4 | 5 | 4 |
| input tokens (uncached) | 366 | 494 | 450 |
| output tokens (of which thinking) | 204,127 (90,996) | 191,942 (77,901) | 172,356 (66,216) |
| cache write, 1 h TTL | 1,047,224 | 1,087,983 | 1,197,566 |
| cache read | 55,381,558 | 79,318,825 | 74,780,845 |
| **cost, harness `cost-state`** | **$23.54** | **$28.41** | **$27.99** |
| cost at a hypothetical $5 in / $25 out, 1.25× and 2× cache write, 0.1× read | $43.27 | $55.34 | $53.68 |
| wall span | 9.86 h | 10.01 h | 9.92 h |
| active time (span minus 3 rate-limit pauses) | **49 min** | **59 min** | **55 min** |
| model API time (harness) | 35.3 min | 36.2 min | 33.5 min |
| tool calls (Bash / Read / Write) | 193 (150/30/8) | 252 (205/40/4) | 226 (180/36/10) |
| cargo calls / with compile errors | 26 / 2 | 34 / 3 | 27 / 1–2 |
| rustc error diagnostics seen | ~2 | ~19 (10× E0599) | ~11 |
| wall time inside cargo calls (incl. `cargo run` sessions) | 4.6 min | 6.3 min | 13.1 min |
| final app code (cloc) | 3,375 | 2,736 (+14,948 vendored) | 2,208 |
| harness "lines added" (Write tool only) | 3,731 | 2,714 | 2,687 / −100 |

- **Price assumptions are parameters, not facts.** A least-squares fit to the 12 `cost-state`
  snapshots reproduces the harness's dollar figures to within $0.0002 with these per-MTok prices
  for `claude-opus-5-5`: output $20, 1-hour cache write $8, cache read $0.20. (Uncached input is
  too small to fit.)
- **Cache reads dominate:** for GPUI, 79.3 M × $0.20 = $15.9 of $28.4.
- Pass other prices to `cost.py --in --out --cw1h --cr`.

Approximate:
- Failed builds and error counts depend on the agents' output filtering (`| tail`, `| grep`).
- Lines changed via `sed` or heredocs are not in the "lines added" figure. Use final cloc.
- `cargo run` wall time includes app sessions.

Confounders:
1. **Rate limits.** All three shared one account and stopped at the same moments: the 5-hour
   limit at 04:48 UTC after only about 15 min, the weekly limit at about 09:20 UTC, and the 5-hour
   limit again at 10:07 UTC. Pauses of 262, 28 and 252 min. Each resume after more than 1 h
   re-created the prompt cache (1-hour TTL), which inflates cache-write cost. Active minutes
   exclude the pauses; dollar cost doesn't and can't.
2. **Mid-task core changes.**
   - Commits `78fa687` (autopilot accepts `{"do":…}`, 14:25 UTC) and `1497d8b` (`Player::stop`
     keeps mute, 14:29 UTC) landed during the agents' final segments.
   - Before that, GPUI wrote its own scenario-rewriting workaround (`apps/gpui/src/main.rs:61-92`),
     and iced has a similar check (`apps/iced/src/main.rs:41`). That is extra work caused by a
     core bug.
3. **GPUI got one extra nudge** ("You ended your turn while your benchmark run was still going…").
4. **Vendored code.** GPUI vendors 14,948 lines to apply a ~30-line patch (axis_stop). That
   inflates its tree and build but not its authored code.
5. **Data.** GPUI's agent saw different fixture data (P0-1).
6. **Shared prerequisites** (core, scroll port, resolver, spec) were built once, before the agents
   started, and are excluded from all three.

---

## 5. Deliverable: the final harness

### Scripts
- `bench/prepare.sh`:
  - checks AC is online;
  - `powerprofilesctl set performance` (restored on exit);
  - records governor and EPP;
  - fails if other `nt-*`, `mpv` or browser processes, or the other reviewers' apps, are running;
  - sets Hyprland window rules so test windows tile full-size;
  - asks the owner to hide Caelestia layers on NT-* outputs, or records that they're present;
  - waits until the coretemp package is under 50 °C;
  - sets up the outputs at 60 and 165 Hz;
  - verifies `fixture_misses = 0` with a dry replay through `ntctl`.
- `bench/startup.sh <app|shell> <cold|warm> N`: nt-probe launch, the cold-cache eviction recipe
  from §2, a fresh data dir.
- `bench/session.sh <app> N <hz>`: the scenario, with nt-probe external sampling and the in-app
  report merged.
- `bench/latency.sh <app> N`: the input-latency set.
- `bench/video.sh <app> <clip>`: barcode clips.
- `bench/soak.sh <app>`: 20 iterations.
- `bench/build.sh <app|shell>`: clean, dev, incremental, with `--timings`, in a scratch target dir.
- `bench/web.sh`: the same via CDP plus nt-probe.
- `bench/report.py`: aggregates into JSON plus a Markdown table. Python is fine for test tooling.

### Runs, order, statistics
- Start-up: N = 10 cold and 10 warm per app and per shell (warm-up launch discarded).
- Session: N = 5 at 60 Hz and N = 5 at 165 Hz. Latency: 50 samples per kind. Video: N = 3 per clip.
  Soak: 1 long run, repeated twice. Build: clean ×3, incremental ×5.
- **Interleave**: round-robin with rotated order (iced, GPUI, Rune / GPUI, Rune, iced / …), so
  drift and thermals spread evenly.
- Report the median, IQR (p25–p75) and min.
- For each headline comparison, give a bootstrap 95% CI of the median difference. Call a
  difference real only if the CI excludes 0 *and* it exceeds 5%.
- Frame metrics: compute per run, then take the median across runs (don't pool).

### Final report table (one column per: iced, GPUI, Rune, iced-shell, GPUI-shell, Rune-shell, Chromium/YouTube)
- **Start-up:**
  - exec→first pixels, cold and warm;
  - →interactive;
  - →home;
  - →thumbnails;
  - breakdown (link / init / first commit).
- **Input latency:** wheel, click, key (p50/p95).
- **Scroll at 60 and 165 Hz:** on-time %, dropped per s of motion, p95 and max motion interval,
  rms position error.
- **CPU:**
  - CPU-ms for the wheel, fling, infinite-scroll and scroll-while-playing phases;
  - CPU-ms per motion frame;
  - Hyprland CPU-ms per motion frame;
  - idle % visible and hidden;
  - wakeups/s idle.
- **Energy:** J per scenario, W idle, W during 1080p60 playback (if RAPL is readable).
- **Memory:**
  - PSS at home after start;
  - PSS peak in session;
  - GPU memory;
  - PSS at home after 20 soak iterations;
  - leak slope.
- **GPU:** device used, GPU-ms per phase.
- **Video**, per clip: time to first frame, % frames shown exactly once, judder, mpv drops,
  seek latency, CPU-ms per frame.
- **Footprint:** stripped size, `.text`, shared objects, threads.
- **Build:** clean release (min), incremental leaf (s), incremental core (s), target size,
  dependencies.
- **Agent:** tokens by type, $ (with the stated price table), active minutes, turns, tool calls,
  cargo calls, failed builds, app LOC.
- Every cell gives the median with the IQR in small type, and the environment block is printed
  under the table.

### P2
Archive `bench/results/<date>-<sha>/` instead of overwriting, so regressions can be tracked.
