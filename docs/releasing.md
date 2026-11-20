# NativeTube releases

Each prerelease has three `.zip` downloads, one app each for iced, GPUI, and
Rune on Apple silicon macOS. Every zip comes with a `.zip.sha256` file. Grab
the pair from the same GitHub Release and run the checksum command in your
downloads directory.

```sh
shasum -a 256 -c native-tube-iced-macos-arm64.zip.sha256
unzip native-tube-iced-macos-arm64.zip
open "NativeTube Iced.app"
```

## Runtime requirements

Use an Apple silicon Mac with macOS 15 or newer. Unzip the download and open
the app. libmpv and the libraries it needs are inside the bundle, so Homebrew
is not required. The apps are ad hoc signed and are not notarized. If
Gatekeeper blocks a downloaded copy, verify its SHA256, then Control-click
the app and choose Open.

`BUILD.json`, inside the app's Resources, records the system libraries linked
at build time and the Mach-O deployment target.

## Build and stage

1. Land the desired source and the release workflow on the default branch.
2. In GitHub Actions, run **Release packages** manually on that branch.
   This builds three downloadable workflow artifacts with read-only repository
   access. Unzip the Actions artifact to get the archive and checksum inside.
   Check the checksums and test each frontend on Apple silicon.
3. Push a version tag such as `v0.1.0-rc.1` or `v0.1.0` pointing at the
   tested commit. Keep the tag fixed while the release is staged and reviewed.
   A tag push builds all three archives from scratch with the locked Cargo
   dependency graphs and the Rust pin in `rust-toolchain.toml`.
4. The release job checks that all three archive/checksum pairs are present,
   that their hashes and embedded provenance match the run, and that the
   remote tag resolves to the built commit. It creates a **draft prerelease**
   only after those checks. For an existing draft the job stops, so a rerun
   cannot silently replace assets. If an upload failed midway, review the
   incomplete draft before discarding it and rerunning the failed job.
5. In **Releases**, review the draft's tag, all six assets, notes, and
   attribution before publishing. Check that the tag still points at the
   source revision in `BUILD.json`. Leave **Set as a pre-release** selected.

Builds run on a native macOS 15 arm64 runner and use the Xcode SDK. GPUI's
build script calls Xcode's Metal tools.

Archives include `RUN.txt`, `BUILD.json`, the frontend's `Cargo.lock`,
`DEPENDENCIES.md`, and available license and notice texts. Dependency metadata
comes from Cargo's target-filtered graph, including build dependencies. The
project has no repository-wide LICENSE file at present. The included asset
and vendored dependency notices apply to their respective files. Review
project licensing and upstream obligations before making a public release.

## Package an existing binary locally

This is useful for inspecting the archive layout with a binary that may
predate the checkout. The archive and its build metadata mark it as a
local candidate and leave the executable's source revision unknown.

```sh
python3 tools/release/package.py package iced macos-arm64 \
    /tmp/native-tube-release-candidate --candidate apps/iced/target/release/nt-iced
# writes NativeTube Iced.app inside a zip; requires brew install mpv on that Mac
python3 -B -m unittest discover -s tools/release
```

Candidate mode uses the local Cargo cache for attribution. A fresh build
requires a clean checkout and a matching native host.
