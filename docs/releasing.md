# NativeTube releases

Each prerelease has six `.tar.gz` downloads, one for each combination of iced,
GPUI, or Rune with Linux x86_64 or macOS arm64. Every archive comes with a
`.tar.gz.sha256` file. Grab the pair from the same GitHub Release and run the
checksum command in your downloads directory.

```sh
# Linux example
sha256sum -c native-tube-iced-linux-x86_64.tar.gz.sha256
tar -xzf native-tube-iced-linux-x86_64.tar.gz
cd native-tube-iced-linux-x86_64
./nt-iced
```

```sh
# Apple silicon example
shasum -a 256 -c native-tube-iced-macos-arm64.tar.gz.sha256
tar -xzf native-tube-iced-macos-arm64.tar.gz
cd native-tube-iced-macos-arm64
./nt-iced
```

## Runtime requirements

- **Linux x86_64.** Start with an Ubuntu 24.04 desktop or a compatible newer
  distribution, a Wayland session, and a working GPU driver. Install the
  system graphics libraries and `libmpv.so.2` for playback. The Ubuntu
  package list is in the archive's `RUN.txt`.
- **macOS arm64.** Use an Apple silicon Mac with macOS 15 or newer. Run
  `brew install mpv` with native Apple silicon Homebrew for playback. Run
  the command-line executable from Terminal. The binaries ship without
  Developer ID signing or notarization, so macOS may ask you to approve
  the launch in Privacy & Security.

The archives rely on system libraries. `BUILD.json` records the libraries
linked at build time and the executable's glibc symbol requirement or
Mach-O deployment target. `RUN.txt` covers libraries loaded at runtime
that may not be in the linker report, including libmpv.

## Build and stage

1. Land the desired source and the release workflow on the default branch.
2. In GitHub Actions, run **Release packages** manually on that branch.
   This builds six downloadable workflow artifacts with read-only repository
   access. Unzip the Actions artifact to get the archive and checksum inside.
   Check the checksums and test each frontend on the matching platform.
3. Push a version tag such as `v0.1.0-rc.1` or `v0.1.0` pointing at the
   tested commit. Keep the tag fixed while the release is staged and reviewed.
   A tag push builds all six archives from scratch with the locked Cargo
   dependency graphs and the Rust pin in `rust-toolchain.toml`.
4. The release job checks that all six archive/checksum pairs are present,
   that their hashes and embedded provenance match the run, and that the
   remote tag resolves to the built commit. It creates a **draft prerelease**
   only after those checks. For an existing draft the job stops, so a rerun
   cannot silently replace assets. If an upload failed midway, review the
   incomplete draft before discarding it and rerunning the failed job.
5. In **Releases**, review the draft's tag, all twelve assets, notes, and
   attribution before publishing. Check that the tag still points at the
   source revision in `BUILD.json`. Leave **Set as a pre-release** selected.

Linux builds run on Ubuntu 24.04. AWS-LC uses a native C build, GPUI's
font path includes `fontconfig` and FreeType, GPUI links `xkbcommon`, and
the Wayland backend can use the system Wayland client libraries. The workflow
installs the corresponding Ubuntu development packages. macOS builds use
the Xcode SDK on a native macOS 15 arm64 runner. GPUI's build script calls
Xcode's Metal tools.

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
nice -n 19 ionice -c3 python3 tools/release/package.py package iced linux-x86_64 \
    /tmp/native-tube-release-candidate --candidate apps/iced/target/release/nt-iced
python3 -B -m unittest discover -s tools/release
```

Candidate mode uses the local Cargo cache for attribution. A fresh build
requires a clean checkout and a matching native host.
