# iced 0.14's basic widget operations omit unfocus

The core operation already works. The convenience API in iced_runtime 0.14.0
exposes `focus`, `focus_next`, `focus_previous` and `is_focused` in
`src/widget/operation.rs:49–67`, but exposes no `unfocus`. The operation is
implemented in iced_core 0.14.0 at `src/widget/operation/focusable.rs:54`.
[Upstream master at 98bd48fe](https://github.com/iced-rs/iced/blob/98bd48fe011efc0ec5819fee0fa7673a7169ff11/runtime/src/widget/operation.rs#L9)
also lacks the convenience wrapper.

The [standalone example](../../shells/iced/repros/unfocus-helper) uses
`iced::advanced::widget::operate` to call the core operation. On NT-ICED,
clicking “Focus the field,” typing, and pressing Enter logged
`REPRO_FOCUSED true` followed by `REPRO_FOCUSED false`. NativeTube now uses
the same public operation on search submission and dismissal.

```sh
CARGO_BUILD_JOBS=4 nice -n 19 ionice -c3 cargo build --release \
  --manifest-path shells/iced/repros/unfocus-helper/Cargo.toml
tools/headless.sh run iced nice -n 19 ionice -c3 \
  shells/iced/repros/unfocus-helper/target/release/iced-unfocus-repro
```

Proposed convenience wrapper against iced_runtime 0.14.0:

```diff
--- a/runtime/src/widget/operation.rs
+++ b/runtime/src/widget/operation.rs
@@ -66,6 +66,11 @@
     task::effect(Action::widget(operation::focusable::focus(id.into())))
 }
 
+/// Clears focus from the currently focused widget.
+pub fn unfocus<T>() -> Task<T> {
+    task::effect(Action::widget(operation::focusable::unfocus()))
+}
+
 /// Moves the cursor of the widget with the given [`Id`] to the end.
```
