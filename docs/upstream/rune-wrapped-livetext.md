# Wrapped LiveText aborts a Rune window during commit

Rune 4fbd2d6 allows `text(...).wrap().live_text(&value)` to be constructed. Its first
scene commit panics, including in release builds. A live binding has a fixed layout box.
A wrapped element could fall back to single-line fade and preserve that fixed box. Full
live reflow would be a separate feature.

The standalone reproduction is in
[shells/rune/repros/wrapped-livetext](../../shells/rune/repros/wrapped-livetext).
Run it on the fixture display:

```sh
CARGO_BUILD_JOBS=4 nice -n 19 ionice -c3 cargo build --release \
  --manifest-path shells/rune/repros/wrapped-livetext/Cargo.toml --target-dir shells/rune/target
nice -n 19 ionice -c3 tools/headless.sh run rune shells/rune/target/release/rune-wrapped-livetext-repro
tools/headless.sh log rune
```

`crates/rune/src/el/text.rs:609–617` documents plain single-line text and accepts the
binding on an already wrapped element. `crates/rune/src/runtime/commit/flatten.rs:258–261`
asserts against `t.wrap` on flatten. The upstream default branch at
`8fbc395190dbff430db9d9fc515bb6ac8cc7d13d` still has the same check.
[Rune at the pinned revision](https://github.com/anomalyco/rune/blob/4fbd2d6ac4dd12595dd8ed0b7849b28a309c7c00/crates/rune/src/runtime/commit/flatten.rs#L258)
[Rune at the checked upstream HEAD](https://github.com/anomalyco/rune/blob/8fbc395190dbff430db9d9fc515bb6ac8cc7d13d/crates/rune/src/runtime/commit/flatten.rs#L259)

The same assert (`flatten.rs:258`) also fires for `.truncate()` (its `!t.truncate` term)
and for rich spans (`rich(..)`, non-empty `t.spans`): `text(..).truncate().live_text(&v)`
aborts at the same line, although an ellipsis is what a single-line label usually wants.
Only `truncate_fade` is accepted for overflow. An earlier NativeTube draft used
`.truncate()` on the live chapter title, which would hit it the same way; the shipped code
uses `truncate_fade` with an explicit size.

NativeTube uses one `LiveText` per Stats row, each with an explicit layout box. It uses
`truncate_fade` on live chapter titles. A blank placeholder alone reserves too little
width to show the later value.

Proposed fallback against the pinned revision. Resolve the incompatible wrap option at
flatten so the result is independent of builder call order.

```diff
diff --git a/crates/rune/src/runtime/commit/flatten.rs b/crates/rune/src/runtime/commit/flatten.rs
--- a/crates/rune/src/runtime/commit/flatten.rs
+++ b/crates/rune/src/runtime/commit/flatten.rs
@@ -258,5 +258,10 @@ fn prepare_text(
     if let Some(t) = text && t.cold.live.is_some() {
+        if t.wrap {
+            t.wrap = false;
+            t.truncate_fade = true;
+            t.shape = None;
+        }
         assert!(input.is_none() && !t.redacted && !t.wrap && t.spans.is_empty() && t.reveal < 0.0 && !t.truncate,
             "live_text requires plain single-line text; use truncate_fade for overflow");
     }
     if input.is_none()
```

The standalone release binary reproduced the panic on NT-RUNE at `flatten.rs:259:9`.
The app's off-screen Stats capture shows all nine rows after the workaround.
[Reproduction log](/tmp/native-tube-rune-round/wrapped-livetext-runtime.log)
[Stats capture](/tmp/native-tube-rune-round/final-stats.png)
