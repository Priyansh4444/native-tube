# A zero-width sibling can reset a text input's focus in iced 0.14

A focused input should keep its focus when a sibling grows. In this case, a preceding
`Space` grows from 0 to 40 logical pixels after the first click. The bar moves, the
input loses focus, and keyboard input goes nowhere. The pinned release uses
`iced_widget 0.14.2` with `iced_core 0.14.0`.

## Reproduction

Build the [standalone example](../../shells/iced/repros/focus-sibling-width):

```sh
CARGO_BUILD_JOBS=4 nice -n 19 ionice -c3 cargo build --release \
  --manifest-path shells/iced/repros/focus-sibling-width/Cargo.toml
MODE=width tools/headless.sh run iced nice -n 19 ionice -c3 \
  shells/iced/repros/focus-sibling-width/target/release/iced-focus-sibling-width-repro
```

With a tiled 1280 × 800 test window, click at (250, 215), then type `iced`.
Use `tools/nt-input` for both actions. With the test window floated at (72, 48)
relative to NT-ICED, add the origin to those coordinates. End the run with
`tools/headless.sh kill iced`.

The field moved 40 px, the placeholder remained, and stderr contained no
`REPRO_INPUT` messages on NT-ICED. `MODE=insert` showed the same behavior.
`MODE=stable` passes the children through `Row::from_vec` to retain the zero-width
child. Use that mode as the control.

## Cause and proposed change

In iced_widget 0.14.2, `src/row.rs:65` builds `row!` via `extend` and `push`.
`src/row.rs:138` skips any child whose hint is void. In iced_core 0.14.0,
`src/size.rs:81` considers either fixed dimension of zero void. When the width
grows, the old input state is at index 0 and the new input is at index 1.
`iced_core/src/widget/tree.rs:57` and `:96` reconcile by tag and position, so
the new input gets fresh state. The same zero-size filtering is present in
[upstream master at 98bd48fe](https://github.com/iced-rs/iced/blob/98bd48fe011efc0ec5819fee0fa7673a7169ff11/widget/src/row.rs#L124).

Changing the default would also introduce gaps around void children in existing
layouts with spacing. An opt-in constructor can keep width/height inference
and preserve identity. Proposed against iced_widget 0.14.2:

```diff
--- a/widget/src/row.rs
+++ b/widget/src/row.rs
@@ -143,6 +143,18 @@
         self
     }
 
+    /// Adds a child even when a fixed dimension is zero, preserving its tree slot.
+    /// The child participates in the row's configured inter-item spacing.
+    pub fn push_retaining_void(
+        mut self,
+        child: impl Into<Element<'a, Message, Theme, Renderer>>,
+    ) -> Self {
+        let child = child.into();
+        let size = child.as_widget().size_hint();
+        self.width = self.width.enclose(size.width);
+        self.height = self.height.enclose(size.height);
+        self.children.push(child);
+        self
+    }
+
     /// Extends the [`Row`] with the given children.
```

NativeTube layers the focus glyph above the input and changes the input padding.
The input has a stable slot across focus changes.
