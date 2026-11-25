# NativeTube on Fly

Measured November 24, 2026 (Pacific) on one private Fly `performance-4x` in `sjc`. Four AMD EPYC vCPUs, 8 GiB and Linux 6.12.118-fly. Sway 1.12 used Pixman and one 1280×800, 60 Hz headless output at scale 1. Mesa 26.3.1 supplied llvmpipe EGL and lavapipe Vulkan. GBM initialization failed. No public services or volumes.

The local Arch build used the [laptop NOW binaries](README.md). Fresh profiles replayed fixtures and the same synthetic 1280×720 H.264 clip with autoplay off, muted playback and no MPRIS. Borderless tiling replaced the forced-fullscreen pass, which hid GPUI’s Home and Search.

## Normalized results

All three apps use **73.0–73.5% of the four vCPUs** (2.92–2.94 cores busy) for CPU-rendered EGL playback. Forced software uses **22.7–39.7%** (0.91–1.59 cores) versus **3.66%** (0.147 cores) for `ntctl play`. Wheel scrolling uses **33.7–70.0%** (1.35–2.80 cores). The raw PMU counters below span whole scenarios. [Normalized tables, controls and warm audit](normalized.md)

## Native medians

Three runs per workload. Map uses Sway IPC matched by PID and geometry. Home and first-frame times use app markers. Switch is an autopilot related open on an already-running player with the local clip override, which bypasses stream preparation. Idle PSS covers 2.0–3.8 s after Home. Libmpv stayed unmapped in cold runs. Default runs mapped it and attached an EGL surface. Peak PSS covers the default scenario. CPU excludes Sway. One vCPU is 100%.

| App | Map ms | Home ms | Open ms | Switch ms | Cold PSS MiB | Warm PSS MiB | Peak PSS MiB | EGL CPU % | Software CPU % | Wheel CPU % | Stripped MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| iced | 142 | 158 | 191 | 161 | 175 | 235 | 427 | 292.5 | 90.7 | 134.7 | 29.6 |
| GPUI | 229 | 329 | 189 | 194 | 273 | 315 | 485 | 294.0 | 114.3 | 196.1 | 52.7 |
| Rune | 264 | 272 | 300 | 182 | 182 | 220 | 422 | 291.8 | 159.0 | 280.0 | 42.2 |
| ntctl play | — | — | — | — | — | — | 124 | — | 14.7 | — | 11.0 |

`ntctl play` decoded the same clip for six seconds. Firecracker exposed user-mode instructions and cycles. `perf stat` measured each complete scenario. Instructions are in billions.

| App | EGL | Software | Cold Home | Wheel |
|---|---:|---:|---:|---:|
| iced | 171.46 | 57.64 | 3.18 | 31.04 |
| GPUI | 172.89 | 61.60 | 3.84 | 34.50 |
| Rune | 176.42 | 103.11 | 4.31 | 65.19 |
| ntctl play | — | 7.15 | — | — |

Laptop open medians of 95/118/189 ms rose to 191/189/300 ms on Fly for iced/GPUI/Rune. Warm switches rose from 36/43/50 ms. Default peak PSS rose from 248/281/295 MiB. Hardware-surface playback used 9–10% CPU on the laptop and 292–294% with Fly’s CPU-rendered EGL. Forced software rose from 44–45% to 91–159%. Fly renders on the CPU and holds graphics buffers in system memory.

## Historical live YouTube

Chromium 158 was signed out and muted. It played `aqz-KE-bpKQ` then `5C_HPTJg5ek` from the saved live `ntctl` run. No bot check appeared. CPU and PSS include the browser process tree.

| Workload | First content frame proxy | CPU % | Settled PSS MiB | Peak PSS MiB |
|---|---:|---:|---:|---:|
| Cold, 3/3 at 854×480 | 1,857 ms | 123.8 | 865 | 1,014 |
| Warm, original deadline | >60 s in 3/3 | — | — | — |
| Warm, extended deadline | 96.7 s | 78.4 | 896 | 903 |

Extended results were 69.3 s, 96.7 s and a 240 s timeout. Warm CPU and PSS use the two completed 1280×720 runs. Fly's original cold playback used **30.94%** of the VM (1.238 cores busy), and extended warm playback used **19.59%** (0.784 cores). The differences from the local-clip `ntctl play` reference are +27.28 and +15.93 percentage points, respectively. Laptop Helium with ad blocking had a 4,171 ms cold median, 0.638 cores busy (3.99% of the laptop) and 978 MiB PSS across two runs. Live tests include network effects and different media from fixture playback.

### Frame timing rerun

On a new private `performance-4x` in `sjc`, Chromium recorded three cold clicks and three in-tab related clicks using [the CDP frame probe](../../tools/fly-bench/web-frames.mjs). The probe recorded first-any and first-content frame callbacks plus ad-state transitions. All three cold opens showed content in 2.234–3.297 s. Warm first-any frames took 0.613–0.735 s. The ad-free warm run reached content in 0.717 s. One warm run reached content in 16.064 s, with 15.371 s in ad state. The other showed an ad and timed out at 130 s. [Per-run timings and definitions](normalized.md#fly-youtube-rerun)

The initial probe lost events after navigation. The retry kept frame receipts in the active page and recorded trusted click timestamps across document loads. Logs are in `.cache/fly-bench/out/normalize-web/`. Fly showed 6m 33s from machine start to confirmed app teardown. `cleanup.log` records the app's destruction and subsequent lookup failure. The rerun collected YouTube only.

## Artifacts and teardown

Run [stage.sh](../../tools/fly-bench/stage.sh) and use `.cache/fly-bench/context` as the build context for the [Dockerfile](../../tools/fly-bench/Dockerfile).

Raw JSON, PMU CSVs, logs and tours are in `.cache/fly-bench/out/`. Four silent H.264 tours cover 1280×800 at 30 fps. The corner scan found zero flags across 3,327 frames. Contact sheets showed no account content or desktop notifications. Only the browser tour used the visible Skip button.

Started time was **34m 25s**. Two launches failed when Fly parsed `off` as the command. The third used `--autostop=off`. All temporary apps were destroyed and verified absent.
