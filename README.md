# NativeTube

The same native YouTube client for Linux, built three times with three Rust GUI frameworks,
to compare them: **iced**, **GPUI** and **Rune**.

- [Framework comparison](docs/blog/x-article.md)
- [Matched benchmark results](docs/media/fair.md) for the Intel GPU laptop on AC and battery
  and a CPU-rendered Fly VM
- [Review findings](docs/review/) and [known benchmark limitations](docs/media/fair.md)

## Release downloads

Check [GitHub Releases](https://github.com/Priyansh4444/native-tube/releases) for published test builds.
Choose iced, GPUI, or Rune, and download the zip and its SHA256 file for Apple silicon
macOS. Verify the checksum, unzip, and open the app. Playback libraries are inside it.
The apps are ad hoc signed and are not notarized, so the first open may need a
Control-click. See [release instructions](docs/releasing.md) for the checksum command.

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

## Lint gate

Run `./check.sh` for rustfmt, Clippy, and Hawk. The first run downloads the pinned
[Hawk 0.1.17 release](https://github.com/astral-sh/hawk/releases/tag/0.1.17) from
GitHub, checks its SHA-256 digest, and caches it in `.cache/hawk/0.1.17/`.
Hawk uses Rust 1.99.0, which the gate installs through rustup if needed. Subsequent
runs reuse the cached copy. For an offline run without the Hawk cache, use
`NT_SKIP_HAWK=1 ./check.sh` to run rustfmt and Clippy.

## How it talks to YouTube

NativeTube is an independent, signed-out client. It uses YouTube's private web API for catalog
data and streams video with libmpv. Requests are rate-limited and back off when YouTube pushes
back. The app stores settings and library state locally.
