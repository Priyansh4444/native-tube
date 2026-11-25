# Isolated real-input checks

Use rootless Podman on Linux. The image starts Sway on one 1280×800 headless output and uses
a private Wayland socket and seat. Sway gets no host input devices. An idle virtual pointer keeps its pointer capability attached,
and wtype uses the private seat for keyboard input. Its render node is
`/dev/dri/renderD128` only when that node reports Intel vendor `0x8086`. The software mode
uses pixman in Sway and llvmpipe in the applications.

The runner binds the release binaries, replay fixtures and 60-second local clip read-only.
Its sole writable host bind is `.cache/iso-test/out`. The root filesystem is read-only,
the runtime and profile are in container tmpfs, and the run uses keep-id, a 4-CPU quota,
a 6 GiB memory limit, dropped capabilities and no-new-privileges. Replay runs have no network.
Live mode uses a new container with pasta and gateway mapping disabled. All players are
muted and MPRIS is disabled.

## Local run

From the repository root, build the runtime stage of the Fly benchmark image or load its
prior local archive. The runtime stage needs no source or fixtures in its build context.

```bash
context=$(mktemp -d)
nice -n 19 ionice -c3 podman build --target runtime -t nt-fly-bench:local \
    -f "$PWD/tools/fly-bench/Dockerfile" "$context"
rmdir "$context"
tools/iso-test/build.sh
```

`build.sh` applies [sway.patch](/home/pronsh/Coding/native-tube/tools/iso-test/sway.patch) to
copies of `nt-input` and `nt-explore` inside the image. The shared tool sources stay intact. The build checks rustfmt, Clippy with warnings denied and unit tests on
the patched copies using Rust 1.99.1.

```bash
export NT_ISO_SAMPLE_HOST=1  # optional read-only Hyprland evidence
tools/iso-test/run.sh smoke
tools/iso-test/run.sh batch
tools/iso-test/run.sh explore
tools/iso-test/run.sh live  # only for features that need current YouTube media
```

The default release binaries are under `.cache/media/target/NOW/<app>/release/nt-<app>`.
Set `NT_ISO_BIN_ICED`, `NT_ISO_BIN_GPUI` or `NT_ISO_BIN_RUNE` to use an alternate build.
Set `NT_ISO_FIXTURES` or `NT_ISO_CLIP` to override the default local inputs. For a replay
without a working Intel render node, set `NT_ISO_RENDER=software`.

Pass one or more app names to limit a run. For example, `tools/iso-test/run.sh batch rune:rune-20-35`
runs the related-and-history stress case only. The driver logs the actual input coordinates,
wall-clock timestamps, probe states and screenshot names in `events.jsonl`. Inspect these and
the per-case logs and PNGs in the printed output directory. Visually assessed checks retain the
raw screenshots and get their verdicts in the results table.

For the fixture performance comparison, run `tools/iso-test/run.sh perf`. It uses the existing
performance budgets and workload against release baseline binaries from `2caf0fa`, expected at
`.cache/perf/target/<full-sha>/<app>/release/nt-<app>`. Set `NT_ISO_BASELINE_DIR` to use another
directory containing that build. Append `--smoke` for one repeat. The Sway `window:new` event
provides the compositor map timestamp. Results measure the private Sway renderer and a focused test window.

## GitHub Actions equivalent

On a Linux runner with rootless Podman and user namespaces enabled, use the same image build.
Prepare the three release binaries, fixtures and local clip in the runner workspace. Point
the `NT_ISO_BIN_*` variables at those binaries, choose `NT_ISO_RENDER=software` if the runner
has no Intel render node, and run `smoke`, `batch` and `explore` serially. Leave host sampling
disabled. Archive `.cache/iso-test/out` as a CI artifact. Evaluate the individual checks in
`events.jsonl` and explorer `status.txt`. The process exit code covers the runner itself.
Run the performance comparison on a runner with a fixed renderer and baseline artifacts.

## Recorded isolation

The initial compositor ran against Intel ARL GLES2, with a GBM swapchain. The offline logs show
only loopback, an empty hardware input-device list, a private Wayland socket and the sole
Intel render node. Sampled host window IDs are hashed and window titles are omitted.

- [Private seat and output evidence](/home/pronsh/Coding/native-tube/.cache/iso-test/out/batch-YNPcEK9p/isolation.txt)
- [Host cursor, focus and pause samples](/home/pronsh/Coding/native-tube/.cache/iso-test/out/batch-YNPcEK9p/host.jsonl)
- [Probe and input trace for Rune's 20/35 run at (990,125)](/home/pronsh/Coding/native-tube/.cache/iso-test/out/batch-EVnn8ZuZ/rune/events.jsonl)

The host sampler reads Hyprland cursor, monitor, active window and client state at about
one-second intervals. Across the replay, explorer, and live checks the focused monitor was
`eDP-1`. Host cursor and window focus changed during normal desktop use. No sampled host
cursor change landed on an injected point in the same sample interval. There were zero
test windows and zero test outputs in the host samples, and both pause files were present
before, during and after. The container's socket and mount configuration is the isolation
guarantee. The host samples corroborate it.

- [Live host samples](/home/pronsh/Coding/native-tube/.cache/iso-test/out/live-KtHVoLns/host.jsonl)
- [Container quota, network and mount settings](/home/pronsh/Coding/native-tube/.cache/iso-test/out/perf-j6hEVlrm/container-settings.txt)
- [Containerized full check.sh and Hawk output](/home/pronsh/Coding/native-tube/.cache/iso-test/check-final.log)
- [Staged Rust tool test output](/home/pronsh/Coding/native-tube/.cache/iso-test/tool-tests.log)

The full three-repeat performance comparison finished 72 runs. Its 1,140 host samples covered
before, during and after. The host had zero test clients and outputs in all of them, and both
pause files remained present.

- [Performance comparison](/home/pronsh/Coding/native-tube/.cache/iso-test/out/perf-j6hEVlrm/summary.md)
- [Performance host samples](/home/pronsh/Coding/native-tube/.cache/iso-test/out/perf-j6hEVlrm/host.jsonl)

All three apps loaded Home in an offline software smoke run with no render node.

- [Software smoke output](/home/pronsh/Coding/native-tube/.cache/iso-test/out/smoke-q0aiXkUY/driver.log)
- [Software smoke device and socket evidence](/home/pronsh/Coding/native-tube/.cache/iso-test/out/smoke-q0aiXkUY/isolation.txt)
