# iced, GPUI or Rune? I had an AI agent build the same YouTube client in all three

*Draft. Numbers marked "preliminary" are from single runs and will be replaced when the
benchmark harness described at the end is finished.*

If you want a native GUI in Rust in late 2026, three GPU-rendered frameworks are worth a serious
look: **iced** (the Elm-style veteran), **GPUI** (the engine under the Zed editor) and **Rune**
(Anomaly's three-week-old newcomer). Feature tables don't tell you which one to pick. So I ran an
experiment: one spec, one shared backend, three frontends, each built from scratch by one Claude
Opus 5.5 agent working alone, then reviewed line by line by a second set of agents.

The app is a real YouTube client: home feed with topic chips, search with suggestions, a watch
page with a video player, comments and related videos, channel pages, local subscriptions,
history and Watch Later, infinite scroll everywhere. Everything that isn't UI lives in one shared
crate (YouTube data, image cache, libmpv playback, a port of Chromium's scroll physics), so the
only variable is the framework.

Versions: iced 0.14.0, gpui-pre 0.3.12 with gpui-component 0.9.1, Rune at `4fbd2d6`. Linux,
Wayland (Hyprland), Intel Arc iGPU, Rust 1.99.1.

## The short answer

| If you want | Pick |
|---|---|
| The smallest binary, the fewest dependencies, a stable release on crates.io, and you don't mind writing widgets | **iced** |
| A big ready-made component kit and the smoothest frame pacing, and you can afford 2–3× the CPU while things move | **GPUI** |
| Animation, scrolling and video that cost almost nothing per frame, the least code, and you can live on a pinned git revision of a pre-release | **Rune** |

The rest of this post is why.

## What it cost to build

Each agent got the same spec and the same backend and worked until its app passed the same
scripted end-to-end session.

| | iced | GPUI | Rune |
|---|---|---|---|
| Agent cost (as billed by the harness) | $23.54 | $28.41 | $27.99 |
| Active time | 49 min | 59 min | 55 min |
| API turns | 181 | 244 | 224 |
| Cargo runs / runs with compile errors | 26 / 2 | 34 / 3 | 27 / 1–2 |
| App code (lines, excluding vendored code) | 3,375 | 2,736 | 2,208 |
| Crates in the lockfile | 431 | 811 | 500 |
| Stripped release binary | 20.6 MB | 48.7 MB | 40.4 MB |

Two things stand out.

**Not knowing the framework barely mattered.** Rune was created after the model's training
cutoff, so the agent knew nothing about it and worked entirely from the repository's README and
examples. It cost the same as GPUI and produced the least code. Rune's docs are written for
agents (its README tells you to hand the repo to one), and it shows.

**iced was cheapest to build and needed the most code.** The model knows iced well, so it made
fewer wrong turns. But iced gives you less, so more had to be written by hand.

Caveat: all three runs were interrupted by the same API rate limits and resumed, and the shared
backend got two fixes mid-build. The ordering is trustworthy; the third significant figure is not.

## The dirty details

### iced: you will write widgets

- **There is no text ellipsis.** iced 0.14 cannot clamp text to N lines with "…". The app shapes
  each title, binary-searches the longest prefix that fits, and caches the result per text and
  width.
- **There is no "unfocus" operation.** To drop focus from the search field after submitting, the
  app downcasts the text input's internal state during the widget tree diff.
- **Draw order inside a layer is fixed:** quads, then meshes, then custom primitives, then images,
  then text. A duration badge drawn as a quad lands *under* its thumbnail. The workaround is to
  draw badge backgrounds as stretched 1×1 images.
- **Any message rebuilds the whole view.** That is the Elm architecture working as designed, and
  it is fine for clicks. It is not fine at 60 messages a second, so smooth scrolling cannot go
  through `update`. The app has a 680-line custom `Scroller` widget that takes raw wheel events,
  moves already-laid-out children by a translation, and only asks for a rebuild every half
  viewport of travel.
- **Video needs a custom shader widget** that writes each frame into one reused wgpu texture.
  It works well and copies nothing in app code, but you are writing a wgpu pipeline.
- **By default it wakes the discrete GPU.** iced asks wgpu for the high-performance adapter and
  probes every backend. On a hybrid laptop that loaded NVIDIA and GL drivers: 254 MB at start-up.
  Setting `WGPU_POWER_PREF=low` and `WGPU_BACKEND=vulkan` brought it to about 150 MB.
- **cosmic-text maps every system font** (about 34 MB resident here).
- The good part: the redraw model is exactly right. A widget gets `RedrawRequested` with a
  timestamp, can request precisely the next frame, and an idle app draws nothing.

### GPUI: a lot in the box, and a per-frame bill

- **The crate named `gpui` on crates.io is a year old.** Current code targets weekly `gpui-pre`
  snapshots of Zed's main branch, pinned with `=`, which is also what the component kit builds on.
- **The kit is the reason to choose it.** gpui-component gave the app its text input, buttons,
  slider, spinner and skeletons with no custom widget code. GPUI on its own has no text input.
- **Every frame rebuilds and lays out the element tree.** While scrolling, building elements took
  0.4–0.7 ms of app code, then Taffy layout 2–3 ms and paint about 1 ms, with no hotspot to
  optimise. That is where the 2–3× CPU in the table below comes from. The upside is the steadiest
  frame pacing of the three.
- **The Wayland backend throws away the touchpad finger-lift event** (`wl_pointer.axis_stop`).
  Without it no app can start a fling the way Chrome does. The app vendors the whole Linux
  platform crate for a 30-line patch, which must be rebased on every GPUI upgrade.
- **Variable-font weights are silently dropped.** Every bold and medium weight rendered as
  Regular with the system font. The fix is to ship static font files.
- **Text uses RGB subpixel anti-aliasing**, which shows colour fringes on OLED, rotated or
  fractionally scaled outputs.
- **No fast path for video on Linux.** `paint_surface` is macOS only, so each frame is a copy
  plus a new atlas image, and the old one must be dropped by hand or the atlas grows.
- **It is heavy at rest:** 38 threads at idle (iced 11, Rune 17), the biggest binary, twice the
  dependency count, 14 MB of fonts, and it initialises every GPU on the machine at start-up.

### Rune: the right primitives, on a moving target

- **It is pre-release and three weeks old.** You depend on a git revision, and your app must
  carry Rune's `[patch.crates-io]` entries for its patched winit and wgpu-hal.
- **Live values are the killer feature.** A `Live` offset or a `Pixels` image can change from any
  thread and the window repaints *without re-running your view function*. Scrolling and video
  playback cost no view runs at all; the view re-runs only when the scroll crosses half a
  viewport.
- **Its built-in scrolling owns its own physics**, so if you want your own (we wanted Chrome's),
  you reimplement windowing yourself on absolutely positioned blocks.
- **Its video `Player` does not work on Linux yet.** We supplied our own decoder and pushed
  frames into `Pixels`, which costs one extra copy per frame.
- **The `poll` hook runs before that frame's input handlers,** which adds a frame of latency
  unless you re-evaluate your animation inside the handler too.
- **The component kit is smaller.** No volume slider; buttons needed re-theming.
- **It is built for agents.** Every app can render headless to a PNG, dump its element tree and
  replay scripted input. That is why an agent with no prior knowledge could build with it.
- It looked the most like a browser out of the box: bundled Inter with real weights, grayscale
  anti-aliasing, proper outline icons.

## What it costs to run (preliminary)

Same scripted session, each app alone on the machine, 1280×800 at 60 Hz.

| | iced | GPUI | Rune |
|---|---|---|---|
| Memory at idle on the home feed (PSS) | 97 MB | 163 MB | 135 MB |
| CPU at idle | 0.0% | 0.4% | 0.2% |
| CPU while wheel-scrolling the feed | 12.7% | 30.0% | 11.8% |
| CPU during a touchpad fling | 9.0% | 26.2% | 7.4% |
| CPU playing 720p30 video | 31.5% | 43.2% | 33.1% |
| CPU scrolling while a video plays | 39.6% | 65.6% | 38.0% |
| Frames later than 25 ms in the wheel-scroll phase | 5 | 0 | 7 |

All three draw nothing when idle. About 25–30 points of the playback figure is the shared
decoder, identical in each. Known problems with these numbers: one run each, the frame hook
means slightly different things in each framework, and a bug in the test data layer gave GPUI a
different (same-sized) feed. Start-up time, input latency, GPU memory and clean build times are
not measured yet.

For scale, the same videos on youtube.com in a Chromium browser: about 4 seconds to the first
frame against about 0.45 s for the shared native player, roughly 64% CPU while playing, and
close to 1 GB of memory across the browser's processes.

## How I'd choose

- **Shipping something small and long-lived, and you like owning your widgets:** iced. It is the
  only one of the three with a stable release on crates.io, it is the lightest, and its
  architecture is the easiest to reason about. Budget time for the widgets it doesn't have.
- **Building a tool-like app (editor, IDE, dashboard) with lots of standard controls:** GPUI with
  gpui-component. You get the most UI per line of code. Accept the per-frame cost, pin your
  snapshot, and expect to read framework source.
- **Building something motion-heavy (feeds, media, animation), or building mostly with agents:**
  Rune, if a pre-release pinned by git revision is acceptable. It had the cheapest moving frames
  and the least code, and it is the only one designed to be driven headless.

## What is still missing from this comparison

- A bare "empty window" baseline per framework, to separate framework cost from app cost.
- Start-up to first pixels and input latency, measured from outside the app.
- Scroll smoothness counted only while content is moving, at 60 and 165 Hz.
- Ten runs per measurement with medians and spread, on AC power.
- None of the three apps looks enough like YouTube yet. That turned out to be the spec's fault
  more than any framework's: the review found no visual gap that a framework blocked.

The code, the spec, each agent's build notes and the full review findings are in this
repository: `apps/`, `docs/frontend-spec.md`, `apps/*/NOTES.md`, `docs/review/`.
