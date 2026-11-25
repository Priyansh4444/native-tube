# NativeTube performance media

Five Cairo Manim explainers, two FFmpeg tour cuts, and eight PNG cards. The full renders are in `.cache/viz/out/`. The JPEGs in [previews](previews) are single-frame previews. The runner uses the local receipts only and never launches NativeTube, Chromium, or a display server.

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
| Laptop tour CPU strip (cores busy of all CPUs) | `laptop.logical_cpus` |

The tour strips report sampled CPU as cores busy out of all logical CPUs: four vCPUs on Fly (from `docs/media/fly-bench.md`) and `laptop.logical_cpus` on the laptop. No reading goes above its scale.

## Measurement choices

- The laptop tours use fixture replay and a local fixture video. The timeline aligns the first scripted scroll command, with a short Home lead-in. The replay uses the first video-open command as zero and slows one source second to four seconds. The capture lacked a common timestamp with the app report. The last reported command and the final scripted wait provide an approximate translation from the report clock to the end of capture. The first-frame flash uses visible fixture color bars from the recording.
- The tour resource strip uses deltas of the native app's sampled CPU seconds (100% is one core), with a rolling window of about 400 ms, and sampled app RSS. The memory explainer uses process-tree PSS.
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
