# NativeTube performance media

The default render builds the three short article videos, a cover, and eight PNG cards. The
full renders are in `.cache/viz/out/`, with single-frame JPEGs in [previews](previews).
The renderer works from local receipts and saved recordings. Archived explainers and tour
comparisons remain available by name through `--only`.

## Render

From the repository root, install the test-only environment. Cairo, Pango, pkg-config, DejaVu Sans, FFmpeg with libass and libx264, and ffprobe are required.

```sh
uv venv --python 3.13 .cache/viz/venv
nice -n 19 ionice -c3 uv pip install --python .cache/viz/venv/bin/python -r tools/viz/requirements.txt
nice -n 19 ionice -c3 .cache/viz/venv/bin/python -B tools/viz/render.py
```

The render command imports completed receipts from `.cache/fly-bench/out/` when `docs/media/fly-bench.md` is available. It chooses each comparable group as a unit and labels the measurement environment on screen. The instruction comparisons use the laptop PMU windows unless a matched VM instruction series is supplied. `--laptop` forces the laptop version.

The output includes `sources.json` and `verification.json`. Manim logs, decoded QA frames, and the tour alignment receipt are under `.cache/viz/`.

### Normalized CPU and warm labels

CPU shares and warm-switch times come from `.cache/media/out/normalized.json` (methods in `docs/media/normalized.md`). [normalized.py](normalized.py) maps each label to a key path. Until a key exists, the label reads "pending normalization". When the file lands, rerender the affected videos:

```sh
nice -n 19 ionice -c3 .cache/viz/venv/bin/python -B tools/viz/render.py --only FirstFrame ScrollCost SideBySide FirstSecondReplay
```

`--only` re-encodes the named outputs and leaves the others untouched. It still rewrites `sources.json` and `verification.json`. If the file uses different key names, edit `PATHS` in `normalized.py`.

| Label | Default key path |
|---|---|
| Fly wheel CPU | `fly.wheel.<app>.machine_share_percent`, `fly.wheel.<app>.cores_busy` |
| Laptop gate wheel CPU | `laptop.gate_wheel.<app>.machine_share_percent`, `laptop.gate_wheel.<app>.cores_busy` |
| YouTube warm, ads excluded | `fly.youtube.warm.content_s_ads_excluded` |

## Obsidian look and publication extras

Everything renders on pure black. Glass (#121212 with sheen and an 8 % rim) is limited to small elements: chips, badges and the source tag. The text is Roboto, with Google Sans for interface labels and CMU Serif for headings and numbers in `FrameworkUpdates.mp4`. The font files are bundled in `assets/fonts`. Accent colour is used only on data: iced #4FD1FF, GPUI #FFB547, Rune #B98CFF, YouTube web #FF4B4B, and core or player #4FE3B0. Each explainer is a run of one-idea beats, and opens on a 2 s question and closes on a 2.5 s takeaway card. [callouts.py](callouts.py) derives every difference badge and end-card number from the receipts and only compares values from one machine, so a re-measurement needs only a rerender.

The side-by-sides use archived footage. A 2 × 2 grid shows the recordings, cut per step and aligned on each tile's own command. Native tiles have the app name, a running timer for timed steps, and the step label. The timer stops and the tile flashes when the app reaches its mark. The open step stops at the first visible video frame. The live-network YouTube web tile plays alongside the native race with a step label.

The article uses four short exports. `Demo.mp4` has one shot per feature.
`FrameworkUpdates.mp4` uses Manim to show what the NativeTube clients reuse across scroll events.
`FirstFrameRealTime.mp4` shows one archived open per app at normal speed and labels the footage as predating the matched benchmark.
`ScrollRealTime.mp4` uses a continuous 60 fps recording from a private Sway output. Its source
and unretimed trim are in `.cache/viz/scroll-capture/`. The [matched benchmark](../../docs/media/fair.md)
provides the measured comparisons, including the separate wheel workload. The other Manim
explainers use the earlier measurement series and belong with its methods.

The renderer also writes `cover.png` (1600×900), `posters/<video>.png`,
`contact/<video>.png` (six frames) and `contact/<video>-1fps-NN.jpg` (one frame per second).

### QA gates

With `VIZ_AUDIT_TEXT=1`, which `render.py` sets, every 0.25 s sample of each explainer must pass these gates, and a failure aborts the render. (a) Everything visible stays within a 96 px safe frame after the camera transform. (b) Text is at least 22 px em for the footnote and 32 px for everything else. (c) At most 14 prose words are on screen at once; numbers, units, app names and the footnote don't count. (d) No glass container covers more than 35 % of the frame. (e) No text overlaps other text. Summaries are written to `.cache/viz/gates/`. `audit.py --gates` checks them, plus (f): background pixels outside the glass decode to pure #000 at every 0.25 s sample. `render.py` runs it last and fails if any gate fails.

For `FrameworkUpdates.mp4`, the export check also scans the full outer margin in 4 × 4 pixel cells at each 0.25 s sample. It allows a single cell for antialiasing at the safe boundary and records the time and cell coordinates if visible content enters the protected margin.

```sh
nice -n 19 ionice -c3 .cache/viz/venv/bin/python -B tools/viz/render.py --only Cover Demo FrameworkUpdates FirstFrameRealTime ScrollRealTime
nice -n 19 ionice -c3 .cache/viz/venv/bin/python -B tools/viz/audit.py --gates Demo FrameworkUpdates FirstFrameRealTime ScrollRealTime
```

## Measurement choices

- The laptop tours use fixture replay and a local fixture video. The timeline aligns the first scripted scroll command, with a short Home lead-in. The replay uses the first video-open command as zero and slows one source second to four seconds. The capture lacked a common timestamp with the app report. The last reported command and the final scripted wait provide an approximate translation from the report clock to the end of capture. The first-frame flash uses visible fixture color bars from the recording.
- The laptop memory panel uses the matched software-playback baseline to estimate player and residual shares. The rise from cold to warm Home measures player prewarm under the documented resource protocol.
- The first-frame explainer shows measured totals and illustrates the possible stages. The receipts do not quantify time within each stage.
- The EGL card shows the vendor-selection mechanism. The fact-check requires remeasurement before publishing the historical Rune renderer-init timings. The GPU card uses system-wide playback counter estimates and labels their scope.

## Reviewed inputs for a rerender

If needed, supply `.cache/fly-bench/out/viz-input.json` to override groups from the raw collector. Each provided group takes the full `iced`, `gpui`, and `rune` set from one VM, with an optional `youtube` row. Each row includes `sources`, a list of existing repo-relative receipt paths. Keep `docs/media/fly-bench.md` alongside the receipts.

| Group | Row keys |
|---|---|
| `timing` | `home_ms`, `open_ms` |
| `memory` | `cold`, `warm`, `playing`, `peak`, optional `core`, all MiB PSS |
| `wheel` | `rate`, userspace instructions per second |
| `tours` | Native rows use `video`, `report`, optional `offset_s` from capture zero to report time |

Measurement gaps may use null values. Supply a wheel group only when all included rates use the same wheel phase and counter protocol.

A browser recording needs a review for logged-out content and privacy before inclusion. Its `tours.youtube` row uses `video`, `wheel_s`, `open_s`, optional `visible_frame_s`, `stages` as ordered `[video_seconds, badge]` pairs, and optional `samples` as ordered `[video_seconds, cpu_percent, memory_mib]` triples. It can set `memory_label` and describe its clock in `clock`. A browser tile with no reviewed recording stays labelled as unavailable.

## Motion review

The runner checks visible text bounds during Manim animation. To review the exported timing and the FFmpeg overlays, extract a contact sheet of every quarter-second frame.

```sh
nice -n 19 ionice -c3 .cache/viz/venv/bin/python -B tools/viz/audit.py
```

The contact sheets are in `.cache/viz/quarter-second-audit/`. Timing markers, cards, and per-frame dots retain their source references in `sources.json`. The scroll dots illustrate average work per draw at about 60 fps.
