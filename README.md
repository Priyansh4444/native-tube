# NativeTube

The same native YouTube client for Linux, built three times with three Rust GUI frameworks,
to compare them: **iced**, **GPUI** and **Rune**.

- Write-up (draft): [docs/blog/iced-vs-gpui-vs-rune.md](docs/blog/iced-vs-gpui-vs-rune.md)
- Status: **work in progress.** The three apps build and run, but a review found real defects
  (see [docs/review/](docs/review/)) and the benchmark numbers are preliminary.

## Layout

| Path | What |
|---|---|
| `core/` | `nt-core`, the shared backend: YouTube data, image cache, local library, libmpv playback, a port of Chromium's scroll physics, benchmark hooks |
| `apps/iced/`, `apps/gpui/`, `apps/rune/` | The three frontends, each its own Cargo workspace, each built by one AI agent from the same spec |
| `docs/frontend-spec.md` | The spec all three were built to |
| `docs/scroll.md` | How Chromium scrolls, and how the port is wired |
| `docs/review/` | Independent review findings |
| `bench/` | Scripted benchmark session and harness |
| `tools/` | Off-screen test display and real-input tooling (Hyprland) |

## Build and run

Needs Rust 1.99.1 (pinned by `rust-toolchain.toml`), libmpv, and Wayland development packages.

```sh
cd apps/iced   # or apps/gpui, apps/rune
cargo run --release
```

`./check.sh` runs the lint gate: `cargo fmt --check` and Clippy with warnings as errors.

## How it talks to YouTube

There is no sign-in. Data comes from the same private API the YouTube website uses, and video
streams are requested the way YouTube's own native apps request them; nothing in the app runs
JavaScript, Lua or Python. Requests are rate-limited and back off when YouTube pushes back.
Nothing is downloaded or saved.

This is not an official client and it is outside YouTube's Terms of Service, which do not permit
third-party clients. It is a framework comparison and a personal project; use it at your own risk.
