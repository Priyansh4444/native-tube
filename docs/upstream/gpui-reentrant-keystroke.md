# `Window::dispatch_keystroke` inside an entity update panics ("already being updated")

gpui-pre 0.3.12 (zed@d2a4fb6).

**What happens.** Calling `window.dispatch_keystroke(..)` from code that runs while a view
entity is leased (a `cx.defer_in` / `cx.listener` closure of that view) panics when the
window is dirty:

```
thread 'main' panicked at gpui-pre-0.3.12/src/app/entity_map.rs:192:36:
cannot update gpui_reentrant_keystroke::View while it is already being updated
```

nt-gpui hit this in production: the player hid the cursor (see
[gpui-hide-cursor.md](gpui-hide-cursor.md)) from a `defer_in` closure, and the owner's app
crashed with "cannot update nt_gpui::player::PlayerView while it is already being updated".

**What should happen.** A programmatic key dispatch either works from inside an update or
fails without a panic. The docs don't say it must not be called there.

## Reproduction

[shells/gpui/repros/reentrant-keystroke](../../shells/gpui/repros/reentrant-keystroke)
(52 lines). The view marks the window dirty (`window.refresh()`) and dispatches `F10` from
`cx.defer_in`. With `REPRO_WINDOW_DEFER=1` it does the same from `Window::defer`, which holds
no entity.

```sh
cd shells/gpui/repros/reentrant-keystroke
CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=../target nice -n 19 ionice -c3 cargo build --release
cd ../../../..
tools/headless.sh run gpui shells/gpui/repros/target/release/gpui-reentrant-keystroke
tools/headless.sh log gpui           # the panic above
REPRO_WINDOW_DEFER=1 tools/headless.sh run gpui shells/gpui/repros/target/release/gpui-reentrant-keystroke
tools/headless.sh log gpui           # keeps running, no panic
```

Seen on NT-GPUI (2026-11-19): the panic at `entity_map.rs:192:36` in the first run; the
second run stays up.

## Root cause

`src/window.rs:5824` `dispatch_keystroke` → `dispatch_event` → `dispatch_key_event`
(`src/window.rs:6293`), whose first lines are

```rust
if self.invalidator.is_dirty() {
    self.draw(cx).clear(cx);
}
```

The draw renders every dirty view, including the one whose update is still on the stack, and
`lease_erased` (`src/app/entity_map.rs:186-195`) panics on the double lease. The eager draw
exists so real key events see an up-to-date dispatch tree. Zed `main` (d5615304,
`crates/gpui/src/window.rs:6493-6496`) has the same code.

## Proposed patch (against gpui-pre 0.3.12; not applied or tested here)

Skip the eager draw for synthetic dispatches; they use the last rendered frame, which is what
the caller saw. Real platform input is unchanged.

```diff
--- a/src/window.rs
+++ b/src/window.rs
@@ -5824,6 +5824,9 @@
     pub fn dispatch_keystroke(&mut self, keystroke: Keystroke, cx: &mut App) -> bool {
         let keystroke = keystroke.with_simulated_ime();
+        // May run inside an entity update: don't draw (and re-render that entity) first.
+        let was = std::mem::replace(&mut self.synthetic_dispatch, true);
         let result = self.dispatch_event(
             PlatformInput::KeyDown(KeyDownEvent {
@@ -6293,5 +6296,5 @@
     fn dispatch_key_event(&mut self, event: &dyn Any, cx: &mut App) {
-        if self.invalidator.is_dirty() {
+        if self.invalidator.is_dirty() && !self.synthetic_dispatch {
             self.draw(cx).clear(cx);
         }
```

plus a `synthetic_dispatch: bool` field on `Window` (initialised `false`) restored to `was`
at the end of `dispatch_keystroke`. The alternative is documenting that
`dispatch_keystroke` must not be called while any view is being updated, and pointing to
`Window::defer`.

## How NativeTube works around it

`PlayerView::render` schedules the cursor hide with `window.defer(cx, hide_cursor)` instead
of `cx.defer_in` (apps/gpui/src/player.rs, the `cursor_hide` branch). `Window::defer` runs
after the update with only the window borrowed. Every other callback path was audited for
updating an entity from inside its own update (NOTES.md, "Stability").
