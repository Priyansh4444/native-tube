# iced 0.14 always asks wgpu for the high-performance GPU

**What happens.** On a hybrid laptop (Intel iGPU + NVIDIA dGPU, Hyprland, Wayland), a bare
iced 0.14 window wakes the discrete GPU, probes every wgpu backend and loads the GL and NVIDIA
drivers. The window maps later, idles with ~100 wake-ups per second and takes more than twice
the memory.

**What should happen.** A UI toolkit should take the adapter that scans out the display
(wgpu's `PowerPreference::None` or `LowPower`), or at least let the application choose,
without environment variables.

## Reproduction

No separate project is needed: the repo's bare shell, `shells/iced` (one window, one label,
`iced = "0.14"` with `wgpu`, `crisp`, `thread-pool`, `wayland`), is the reproduction.
`bench/shells.py` runs it as is (`shell-iced`) and with wgpu's own knobs
`WGPU_POWER_PREF=low WGPU_BACKEND=vulkan` (`shell-iced-lowpower`), on the off-screen output
NT-BENCH, with no input:

```sh
CARGO_BUILD_JOBS=4 nice -n 19 ionice -c3 cargo build --release --manifest-path shells/iced/Cargo.toml
python3 bench/shells.py --targets shell-iced,shell-iced-lowpower --runs 3
```

Result on 2026-11-19 (median of 3 warm runs; the script also writes
`bench/results/shells.json`):

| target | exec → window mapped | idle wake-ups/s | PSS | threads |
|---|---|---|---|---|
| shell-iced (default) | 339 ms | 106 | 120 MB | 28 |
| shell-iced-lowpower | 199 ms | 3 | 50 MB | 20 |

## Root cause

- `iced_wgpu-0.14.0/src/window/compositor.rs:85-86`:
  `power_preference: wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance)`.
- `iced_wgpu-0.14.0/src/settings.rs:36`: `backends: wgpu::Backends::all()`, so GL is probed
  as well as Vulkan.
- `iced_wgpu-0.14.0/src/lib.rs:931` (headless renderer) hard-codes `HighPerformance` too.
- Neither `iced::Settings` nor `iced::application` exposes the preference in 0.14.

## Upstream status

Fixed on iced master (checked read-only at
[98bd48fe](https://github.com/iced-rs/iced/tree/98bd48fe011efc0ec5819fee0fa7673a7169ff11),
2026-11-19): `src/application.rs:373` adds `Application::power_preference`, the setting's
default is `backend::PowerPreference::None` (`wgpu/src/window/compositor.rs:374-375`), and the
compositor maps it at lines 86-92 (the environment variable still wins). Nothing to report
for master; a 0.14 point release would need the backport below.

## Proposed patch (backport to iced_wgpu 0.14.0)

```diff
--- a/src/window/compositor.rs
+++ b/src/window/compositor.rs
@@ -83,7 +83,7 @@
         let adapter_options = wgpu::RequestAdapterOptions {
             power_preference: wgpu::PowerPreference::from_env()
-                .unwrap_or(wgpu::PowerPreference::HighPerformance),
+                .unwrap_or(wgpu::PowerPreference::None),
             compatible_surface: compatible_surface.as_ref(),
             force_fallback_adapter: false,
         };
--- a/src/settings.rs
+++ b/src/settings.rs
@@ -33,7 +33,7 @@
         Settings {
             present_mode: wgpu::PresentMode::AutoVsync,
-            backends: wgpu::Backends::all(),
+            backends: wgpu::Backends::PRIMARY,
             default_font: Font::default(),
```

## Our workaround

`apps/iced/src/main.rs` (`prefer_integrated_gpu`) sets `WGPU_POWER_PREF=low` and
`WGPU_BACKEND=vulkan` at the top of `main`, only when the user hasn't set them, before iced or
any thread reads the environment. The same `shells.py` run measures it.

## Related: no text ellipsis

iced 0.14 has no ellipsis or line clamp for text (we keep `Clamp` in `apps/iced/src/widgets.rs`,
which binary-searches the cut). Master has it now (`core/src/text/paragraph.rs:44`,
`Paragraph::ellipsis`, `text::Ellipsis`), so it's a missing feature in 0.14, not a bug to
report.
