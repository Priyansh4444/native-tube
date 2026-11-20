# GPUI drops the touchpad's `axis_stop` on Wayland, so a fling can never start

gpui-pre 0.3.12 (zed@d2a4fb6), crate `gpui-pre-linux` 0.3.12, Wayland backend.

**What happens.** A two-finger touchpad swipe reaches the app as `ScrollEvent`s with
`TouchPhase::Moved`, but the finger lift never does. The compositor sends
`wl_pointer.axis_stop` for it and GPUI discards it. An app that wants Chromium-style momentum
(start a fling on lift, cancel it on touch) cannot tell when the fingers left the pad.

**What should happen.** The lift arrives as a scroll with `TouchPhase::Ended` (macOS GPUI
already reports phases from NSEvent), so the app can start its fling there.

## Reproduction

[shells/gpui/repros/axis-stop](../../shells/gpui/repros/axis-stop) (48 lines, same pins as
the app, upstream `gpui-pre-linux`, not our vendored copy). It logs every scroll event's phase.

```sh
cd shells/gpui/repros/axis-stop
CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=../target nice -n 19 ionice -c3 cargo build --release
cd ../../../..
tools/headless.sh run gpui shells/gpui/repros/target/release/gpui-axis-stop
tools/nt-input/target/release/nt-input gpui touchpad 640 400 300 200   # swipe, then lift
tools/headless.sh log gpui | awk '{print $1}' | sort | uniq -c
```

Seen on NT-GPUI (2026-11-19): `25 Moved`, no `Ended`. The same swipe in nt-gpui, which uses
the patched crate, ends with one `Ended` event and the page flings.

## Root cause

`gpui-pre-linux-0.3.12/src/linux/wayland/client.rs`: the `wl_pointer` dispatch handles
`AxisSource` (line 2618), `Axis`, `AxisValue120` and `Frame` (line 2711), but has no
`wl_pointer::Event::AxisStop` arm, so the event falls through to `_ => {}`. Zed's `main`
(d5615304, 2026-11-19, `crates/gpui_linux/src/linux/wayland/client.rs`) still has no
`AxisStop` arm.

## Proposed patch (against gpui-pre-linux 0.3.12)

This is the patch nt-gpui vendors in `apps/gpui/vendor/gpui-pre-linux`. It applies with
`patch -p1` in the crate root.

```diff
--- a/src/linux/wayland/client.rs
+++ b/src/linux/wayland/client.rs
@@ -401,6 +401,8 @@
     vertical_modifier: f32,
     horizontal_modifier: f32,
     scroll_event_received: bool,
+    // A finger/continuous `axis_stop` seen in this pointer frame.
+    axis_stop_received: bool,
     enter_token: Option<()>,
     button_pressed: Option<MouseButton>,
     mouse_focused_window: Option<WaylandWindowStatePtr>,
@@ -1118,6 +1120,7 @@
             },
             capslock: Capslock { on: false },
             scroll_event_received: false,
+            axis_stop_received: false,
             axis_source: AxisSource::Wheel,
             mouse_location: None,
             continuous_scroll_delta: None,
@@ -2708,7 +2711,33 @@
                     _ => unreachable!(),
                 }
             }
+            // Report the finger lift (`axis_stop`) as a zero-delta precise scroll with
+            // `TouchPhase::Ended`, so apps can start a fling.
+            wl_pointer::Event::AxisStop {
+                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
+                ..
+            } => {
+                if matches!(state.axis_source, AxisSource::Finger | AxisSource::Continuous) {
+                    state.axis_stop_received = true;
+                }
+            }
             wl_pointer::Event::Frame => {
+                if std::mem::take(&mut state.axis_stop_received)
+                    && !state.scroll_event_received
+                    && let Some(window) = state.mouse_focused_window.clone()
+                {
+                    let input = PlatformInput::Scroll(ScrollEvent {
+                        position: state.mouse_location.unwrap_or_default(),
+                        delta: ScrollDelta::Pixels(point(px(0.0), px(0.0))),
+                        modifiers: state.modifiers,
+                        touch_phase: TouchPhase::Ended,
+                        source: ScrollSource::Pointer,
+                    });
+                    drop(state);
+                    window.handle_input(input);
+                    return;
+                }
                 if state.scroll_event_received {
                     state.scroll_event_received = false;
                     let continuous = state.continuous_scroll_delta.take();
```

A fuller upstream change would also emit `TouchPhase::Started` on the first finger frame and
handle the horizontal axis; the app only needs the vertical lift.

## How NativeTube works around it

`apps/gpui/Cargo.toml` patches `gpui-pre-linux` with the vendored copy above.
`NtApp::on_scroll_event` (apps/gpui/src/app.rs) calls `nt_core::scroll` `fling_start` on the
`Ended` event, and only after precise (touchpad) deltas.
