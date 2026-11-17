# NativeTube on macOS

NativeTube has three separate desktop frontends: GPUI, iced, and Rune. Each can be built as a
native macOS executable. The macOS build uses the platform's native window/GPU backend (Metal
where supported); the iced frontend only enables Wayland on Linux.

## Requirements

- Xcode Command Line Tools: `xcode-select --install`
- Rust toolchain from `rust-toolchain.toml`
- Homebrew libmpv, used by the shared video player:

  ```sh
  brew install mpv
  ```

## Build and run

From the repository root, build any frontend:

```sh
cargo build --locked --release --manifest-path apps/gpui/Cargo.toml
cargo build --locked --release --manifest-path apps/iced/Cargo.toml
cargo build --locked --release --manifest-path apps/rune/Cargo.toml
```

Run the one you want, for example:

```sh
./apps/gpui/target/release/nt-gpui
```

The other executable names are `nt-iced` and `nt-rune`. Keep Homebrew's `mpv` installed when
running the apps so macOS can load `libmpv` for video playback.

The macOS CI workflow builds all three frontends and publishes the executables as workflow
artifacts. These are native executables, not signed or notarized `.app` bundles; distributing a
double-clickable app bundle requires a separate signing/notarization and dependency-bundling
setup.
