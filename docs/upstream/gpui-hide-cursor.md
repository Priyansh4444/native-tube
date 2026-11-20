# GPUI has no public way to hide the pointer (video players need one)

gpui-pre 0.3.12 (zed@d2a4fb6).

**What happens.** A video player hides the pointer with the controls after a few seconds
without motion. GPUI can hide it, but only internally: the platform method
`hide_cursor_until_mouse_moves` is called from key dispatch when `CursorHideMode` says so.
`CursorStyle` has no "none" variant, and the `Platform` trait is not reachable from apps.

**What should happen.** A public `Window::hide_cursor_until_mouse_moves(cx)` (or
`CursorStyle::None`) for apps that hide the pointer themselves.

## Reproduction

[shells/gpui/repros/cursor-hide](../../shells/gpui/repros/cursor-hide) (43 lines). On a click
it dispatches an inert synthetic keystroke from `Window::defer` with
`CursorHideMode::OnTypingAndAction` set, and prints `App::is_cursor_visible()` before and
after: the only way an app can hide the pointer today.

```sh
cd shells/gpui/repros/cursor-hide
CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=../target nice -n 19 ionice -c3 cargo build --release
cd ../../../..
tools/headless.sh run gpui shells/gpui/repros/target/release/gpui-cursor-hide
tools/nt-input/target/release/nt-input gpui click 640 400
tools/headless.sh log gpui
```

Seen on NT-GPUI (2026-11-19): `visible before: true`, `visible after synthetic input: false`.

## Root cause

`src/platform.rs:580` declares `fn hide_cursor_until_mouse_moves(&self)` on the crate-private
platform trait. Its only callers are key dispatch (`src/window.rs:6336-6343`, when the
keystroke has a `key_char` and the mode is `OnTyping`/`OnTypingAndAction`) and actions
(`src/window.rs:6774-6777`). `App::set_cursor_hide_mode` (`src/app.rs:1390`) and
`App::is_cursor_visible` (`src/app.rs:1399`) are public, the hide itself is not. Zed `main`
(d5615304, `crates/gpui/src/window.rs:6542`, `6977`) is the same.

## Proposed patch (against gpui-pre 0.3.12; not applied or tested here)

```diff
--- a/src/window.rs
+++ b/src/window.rs
@@ -5824,2 +5824,11 @@
+    /// Hides the mouse cursor until the mouse next moves, as typing does with
+    /// `CursorHideMode::OnTyping`. For media players and presentations.
+    pub fn hide_cursor_until_mouse_moves(&self, cx: &App) {
+        cx.platform.hide_cursor_until_mouse_moves();
+    }
+
     pub fn dispatch_keystroke(&mut self, keystroke: Keystroke, cx: &mut App) -> bool {
```

## How NativeTube works around it

`nt-gpui` relies on the default `CursorHideMode::OnTypingAndAction`; when the player controls auto-hide under
the pointer it dispatches an inert keystroke `nt-hide-cursor` with an empty `key_char`
(`hide_cursor` in apps/gpui/src/player.rs), from `Window::defer` (the re-entrancy bug in
[gpui-reentrant-keystroke.md](gpui-reentrant-keystroke.md)). The root key handler swallows
it. The workaround depends on private dispatch behaviour and would break if GPUI stopped
hiding on synthetic keys.
