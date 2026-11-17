# Viewing review: is watching a video as good as YouTube's player and mpv?

Reviewer lens: picture quality, smoothness, A/V sync, start/seek/buffering, CPU and power,
and the features a viewer expects. Draft in progress; sections marked TODO are still being
measured.

## How this was checked

- I did not run the GUI apps. The off-screen outputs were not used.
- Everything ran headless: `core/target/release/ntctl` (`play`, `resolve`, `search`). I also
  used two small scratch binaries linked against `nt-core`. `pace` timestamps every frame the
  `Player` publishes, at the moment an app's waker would fire. `watch` prints every change of
  `PlaybackState` and can pause, resume and seek. Other tools: plain `mpv --mute=yes --vo=image`
  or `--vo=null`, ffmpeg (zscale, psnr, ssim), ffprobe, and a local Range-capable HTTP server
  that can return 403 or drop connections after N seconds. All playback was muted
  (`NT_MUTE=1`).
- Local test clips were generated with ffmpeg in a scratch directory. They cover lossless still
  frames at 720p and 1080p (a chroma zone plate, a Mandelbrot tile, SMPTE HD bars and testsrc2
  text), and 1080p60, 2160p30, HDR10 PQ, a 10-bit gradient, full range, SD BT.601, vertical,
  anamorphic and interlaced sources. There are also 24, 25, 30 and 60 fps 720p clips with audio.
- Live YouTube traffic: Big Buck Bunny (`aqz-KE-bpKQ`, one resolve at a 4320 cap, one play with
  seeks), one age-restricted video, one live-stream search and two live resolves.
- Hardware: Core Ultra 7 255H (Arc 140T, VA-API) and an RTX 5070 Mobile. The owner's panel is
  eDP-1, 1920×1200 at **165 Hz**, scale 1.333.
- Code: the core and apps changes since 96df4ec are rustfmt only, so the video path is unchanged.
  Line numbers below are for the current tree.

## The pipeline as it stands

1. `resolve.rs` asks `/player` as the visionOS client. It picks avc1 → vp9 at height ≤
   `NT_MAX_HEIGHT`, default **720** (`resolve.rs:96-100`, `:212`). For live streams it hands mpv
   the HLS master playlist (`:181`).
2. libmpv decodes with `hwdec=vaapi-copy,auto-copy` (`player.rs:351`). The decoded NV12 surface
   is copied back to RAM.
3. mpv's software renderer (`vo=libmpv`, render API `sw`, zimg) converts and scales into an
   RGB0 buffer. The buffer is at most the target size and never larger than the video
   (`render_size`, `player.rs:866`); the target defaults to 1280×720 (`:442`). The render loop
   then writes alpha = 255 into every pixel (`:957-960`) and publishes an `Arc<VideoFrame>`.
4. `video-timing-offset=0` (`:395`) makes mpv hand over each frame at the moment it is due. The
   app then shows it on its next UI frame:
   - **iced** (`apps/iced/src/video.rs`) does `queue.write_texture` into one persistent texture,
     samples it bilinearly (`:121-122`, `Rgba8UnormSrgb` when the surface is sRGB, `:161`) and
     draws a quad.
   - **GPUI** (`apps/gpui/src/video.rs:111`) does `to_vec()` into a new `RenderImage` per frame,
     drops the old atlas tile, and lets GPUI re-upload the image into its sprite atlas.
   - **Rune** (`apps/rune/src/video.rs:43`) does `to_vec()` into `rune::Pixels` on the render
     thread; Rune copies it into its image page and uploads it.

   All three call `set_target_size` with the drawn size in physical pixels (iced `view.rs:971`,
   `:988`; GPUI `video.rs:129`; Rune `video.rs:220`).

## Measured facts

### Picture quality

The mpv software render path (`--vo=image` with the same options) produces output bit-identical
to `ntctl play`. That let me also render sizes `ntctl` cannot ask for.

Luma PSNR against an ideal render of the same YUV (zscale spline36). Higher is better; above
about 45 dB the difference is invisible.

| Case | NativeTube path | Better path |
|---|---|---|
| 720p stream in a 1280×720 box (1:1) | 46.1 dB | n/a (1:1) |
| 1080p stream in a 1280×720 box | 46.3 dB (zimg lanczos) | 58.0 dB (lanczos from YUV, gpu-next-like) |
| 1920×1080 box (fullscreen-sized) | **34.9 dB**: 720p stream, then the app's GPU bilinear | 36.3 dB: 720p plus lanczos; **49.7 dB**: a 1080p stream rendered 1:1 |

- **Colour is right.** BT.709 for HD and BT.601 for untagged SD are both correct: on SD bars the
  green channel is 190, against 190 for 601 and 171 if 709 were wrongly assumed. Full range is
  honoured, flat patches match within ±1–2. 10-bit sources are dithered (zimg `random`), with no
  banding on a 10-bit gradient. Chroma upsampling is bilinear (`zimg-scaler-chroma` default).
  That is softer than gpu-next on saturated edges and invisible on normal content.
- **HDR (PQ) is clipped, not tone-mapped.** On an HDR10 bars clip, the 0–100% ramp hits white at
  about a third of its width. Today this never happens on YouTube, because avc1 is preferred and
  YouTube's avc1 renditions of HDR videos are SDR. It becomes real as soon as VP9.2 or AV1 is
  chosen above 1080p.
- **Anamorphic sources** come out at the right display aspect (`dwidth` includes SAR).
  **Vertical 1080×1920** renders at 400×710, correct within 0.2%. The first frame or two use the
  1280×720 default before `dwidth`/`dheight` arrive, so the size changes once.
- **Interlaced sources** are shown combed (`deinterlace=no`, the mpv default). YouTube only
  serves progressive streams, so this is a non-issue for YouTube.
- **Stream availability** (one resolve of BBB with `NT_MAX_HEIGHT=4320`): the best avc1 stream is
  `1080p60 avc1 6.9 Mb/s`. The default cap gets 720p. YouTube's 1440p and 2160p exist only as
  VP9 and AV1.

### Smoothness (frame pacing)

The core's own publish timing is excellent. For local 720p clips, the intervals between
published frames had a standard deviation of 0.15–0.21 ms. Lateness against the frame's
timestamp was 0.2–0.5 ms at p50 and at most 1.7 ms. No frame was dropped or repeated at 24, 25,
30 or 60 fps, nor for 1080p60 at a 1080p target.

For 2160p30 at a 1080p target, render time averaged 7.7 ms (max 41 ms). Publish lateness was
p99 3.3 ms, still with no drops.

The problem is presentation. The app shows a frame on the first vsync after it is published, and
nothing aligns the frame's due time with the display's vsync. I simulated presentation from the
measured publish times: 32 vsync phases, 1 ms for the UI to upload and draw, and optionally
extra UI-side jitter. I counted the frames whose on-screen duration differs from the regular
cadence a display-synced player would produce.

| Content | 60 Hz, no UI jitter | 60 Hz, +2 ms UI jitter | 165 Hz, no UI jitter | 165 Hz, +2 ms UI jitter |
|---|---|---|---|---|
| 24 fps | 2.8% (worst phase 33%) | 11.7% (56%) | 7.2% (16%) | 25.5% (32%) |
| 25 fps | 1.4% (17%) | 11.3% (27%) | 2.8% (17%) | 30.4% (41%) |
| 30 fps | 0.5% (9%) | 7.7% (53%) | 3.7% (40%) | 31.2% (71%) |
| 60 fps | 0.9% (17%); 0.45% of frames never shown | 7.7% (52%); 3.8% never shown | 6.2% (23%) | 29.0% (38%) |
| 1080p60 | 1.6%; 0.8% never shown | 7.7%; 3.9% never shown | 10.2% | 28.8% |

- The "worst phase" column is what a viewer gets periodically, not rarely. mpv's clock is the
  audio clock (`video-sync=audio`), which drifts against the display clock. The phase therefore
  sweeps through the bad region every few minutes, and while it is there, 30–70% of frames are
  held for the wrong number of vsyncs. On 60 fps content at 60 Hz, frames are dropped and
  doubled in bursts.
- mpv with `video-sync=display-resample` (and Chrome, which presents each frame at a vsync it
  chooses ahead of time) never shows this pattern. Their cadence is the regular 3:2 or 7:7:7:6
  one.
- 165 Hz is worse, not better. The vsync interval (6.06 ms) is close to the UI's own latency and
  jitter, so any UI noise moves frames across vsync boundaries.

### Start, seek, end, failures

TODO: write up from the measurements.

### CPU

TODO: perf profile.

## Findings

TODO.
