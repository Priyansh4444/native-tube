# Raw shells

The bare cost of each framework: one 1280x800 window, background `#0f0f0f`, one centred label
"NativeTube" at 14 px in `#f1f1f1`, set in the same bundled Roboto the apps use. No nt-core, no
libmpv, no images, no timers. Each crate pins the same framework versions and release profile as
its app, so **app minus shell** is the cost of the app on that framework.

The window's app_id is `nt-shell-<fw>`, or `$NT_APP_ID` when set (the test tooling sets it).
Start-up and memory are measured from outside the process by `bench/shells.py`, the same way for
all three, so the shells carry no instrumentation.
