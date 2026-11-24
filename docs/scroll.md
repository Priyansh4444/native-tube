# Scrolling: the Chromium model and how to wire it

`core/src/scroll.rs` (`nt_core::scroll::ScrollAnimator`) is a port of how Chrome
on Linux desktop (Aura, Ozone/Wayland) turns raw input into a scroll offset.
Every frontend (iced, GPUI, Rune) feeds raw input into it and draws the offset
it returns. This note covers what Chrome does, what makes that feel good, how
to wire the module, and where the port differs from Chrome.

All source references are to `chromium/chromium` `main` at
`6aeba664bd893911381924267bc75e9e04610a8b`, read from
`raw.githubusercontent.com` (the copies are in
`/tmp/claude-1000/nt-scroll-research/src/`). Wherever Chromium branches by
platform, this follows the Linux branch.

---

## 1. What Chrome does, by input type

### 1.1 Mouse wheel (notched)

**From Wayland event to pixels**

| Fact | Source |
|---|---|
| `wl_pointer.axis_value120` is used as is: `delta = -value120`, so one notch is 120 units. | `ui/ozone/platform/wayland/host/wayland_pointer.cc` `WaylandPointer::OnAxisValue120` |
| Legacy `wl_pointer.axis` is scaled by `delta / kAxisValueScale * MouseWheelEvent::kWheelDelta`, with `kAxisValueScale = 10`. Compositors send 10 per notch, so this is also 120 per notch. | `wayland_pointer.cc` `OnAxis` |
| `axis_discrete` is not implemented (deprecated since wl_pointer v8). | `wayland_pointer.cc` `OnAxisDiscrete` (`NOTIMPLEMENTED_LOG_ONCE`) |
| `MouseWheelEvent::kWheelDelta = 120` on Linux. The comment says it matches Windows and roughly Firefox (crbug.com/1270089). Other platforms use 53. | `ui/events/event.cc` |
| A wheel frame becomes `MouseWheelEvent(gfx::ToRoundedVector2d(dx, dy))`, so deltas are rounded to whole pixels. | `ui/ozone/platform/wayland/host/wayland_event_source.cc` `ProcessPointerScrollData` |
| The wheel offset is the pixel delta: `delta_x = event.x_offset()`, `delta_units = kScrollByPixel`. Only events with `EF_PRECISION_SCROLLING_DELTA` become `kScrollByPrecisePixel`. | `ui/events/blink/web_input_event.cc` `MakeWebMouseWheelEventFromUiEvent(const MouseWheelEvent&)` |

**One notch scrolls 120 px.**

**Animated or not.** `cc::InputHandler::ShouldAnimateScroll` (`cc/input/input_handler.cc`)
returns true unless the granularity is `kScrollByPrecisePixel`, provided
`enable_smooth_scroll` is set. That setting comes from
`prefs.enable_scroll_animator = ... gfx::Animation::ScrollAnimationsEnabledBySystem()`
(`content/browser/web_contents/web_contents_impl.cc`). On Linux that function
returns `ui::LinuxUi::AnimationsEnabled()` (`ui/gfx/animation/animation_linux.cc`),
which is the desktop's "enable animations" setting.
NativeTube always animates wheel scrolls; it has no switch for that setting (the
unused `set_smooth_scroll` mode was removed in the 2026-11-20 lint round).

**Creating an animation** (`InputHandler::ScrollLatchedScroller`, "CreateNewAnimation" branch):
- `ComputeScrollDelta` clamps `current + delta` to `[0, max]`.
- `LimitDeltaToScrollerSize` then caps the delta at the scroller's size. This
  is behind feature `kLimitScrollDeltaToScrollerSize`, which is
  `FEATURE_ENABLED_BY_DEFAULT` in `ui/base/ui_base_features.cc`.
- `LayerTreeHostImpl::ScrollAnimationCreate` (`cc/trees/layer_tree_host_impl.cc`)
  scrolls deltas of 0.1 px or less without animating (`kEpsilon = 0.1f`).
  Otherwise it calls `MouseWheelScrollAnimationCreate` with the target clamped
  to the limits.
- The animation starts "one full frame in":
  `animation_start_offset = CurrentBeginFrameArgs().interval`. The comment
  says this is so the animation doesn't start a frame late.

**The curve.** `ScrollOffsetAnimationCurveFactory::CreateAnimation`
(`cc/animation/scroll_offset_animation_curve_factory.cc`) maps
`kMouseWheel` to `DurationBehavior::kInverseDelta`. In
`cc/animation/scroll_offset_animation_curve.cc`:

- **Easing:** `kEaseInOutControlPoints` = cubic-bezier(0.42, 0, 0.58, 1).
- **Duration:** `EaseInOutSegmentDuration` computes
  `clamp(kInverseDeltaOffset + |Δ|·kInverseDeltaSlope, 6, 12) / kDurationDivisor` s.
  The constants are `kInverseDeltaRampStartPx = 120`, `kInverseDeltaRampEndPx = 480`,
  `kInverseDeltaMinDuration = 6`, `kInverseDeltaMaxDuration = 12` and
  `kDurationDivisor = 60`. That gives slope −1/60 and offset 14, so the
  duration is `clamp(14 − |Δ|/60, 6, 12) / 60` s:

  | Δ | 120 px (1 notch) | 240 px | 360 px | ≥ 480 px |
  |---|---|---|---|---|
  | duration | 200 ms | 166.7 ms | 133.3 ms | 100 ms |

  Bigger jumps are faster, which keeps fast wheel spins snappy and slow ones
  smooth (per the `kInverseDelta` comment in the header).
- **Input delay:** the duration is shortened by `delayed_by`, the time between
  the input and when it is handled, and floored at 0.

**A tick that arrives mid-animation** goes through
`ScrollOffsetAnimationImpl::ScrollAnimationUpdateTarget`
(`cc/animation/scroll_offset_animations_impl.cc`):
- `new_target = clamp(curve.target + Δ, 0, max)`. The delta is added to the
  old **target**, not to the current position, so no notch is lost.
- `curve->UpdateTarget(now − start − delayed_by, new_target)`.

`ScrollOffsetAnimationCurve::UpdateTarget` then works through these cases in order:
1. If the new target is within `kEpsilon = 0.01` of the old one, it only stores
   the target. This is how extra notches at an edge are swallowed.
2. If `new_target` is within 0.01 of the current position, the animation stops there.
3. Otherwise the new duration is
   `min(EaseInOutSegmentDuration(new_delta), VelocityBasedDurationBound)`.
   The bound is `new_delta / velocity · 2.5`, or ∞ if velocity is near 0 or
   points the other way. The comment says it "minimizes the rubber-band
   bouncing effect". If the result is under 0.01 s, it jumps to the target.
4. **Velocity matching.** It computes `velocity = CalculateVelocity(t)`, the
   bezier slope converted to px/s. It sets
   `new_slope = velocity · new_duration / new_delta` and builds the new easing
   as `EaseInOutWithInitialSlope`: cubic-bezier(0.42, 0.42·slope, 0.58, 1),
   with the slope clamped to ±1000. The new segment starts at the current
   position, with the old curve's speed and the ease-out shape kept.

**Latching.** Notches stay in one gesture, and keep retargeting, while they are
less than `kDefaultMouseWheelLatchingTransaction = 500 ms` apart
(`content/browser/renderer_host/input/mouse_wheel_phase_handler.h`). That is
longer than any wheel animation (200 ms or less).
`InputHandler::DidLatchToScroller` aborts an impl animation when a new gesture
latches.

### 1.2 Precise touchpad scrolling

| Fact | Source |
|---|---|
| Finger and continuous axis frames become `ui::ScrollEvent(kScroll)` with the summed dx/dy of the `wl_pointer.frame`. | `wayland_event_source.cc` `ProcessPointerScrollData` |
| Finger deltas go through `OnAxis`, so they are also ×12 (`value/10·120`). That holds while the default-**off** feature `kWaylandUnscaledTouchpadScrolling` is off; when it is on, they use factor 2.5 (`kWaylandUnscaledTouchpadScrollingNewFactor`). | `wayland_pointer.cc` `OnAxis`; `ui/ozone/common/features.cc` |
| ScrollEvents become `kScrollByPrecisePixel`. | `web_input_event.cc` `MakeWebMouseWheelEventFromUiEvent(const ScrollEvent&)` |
| Precise deltas are **not animated**: `ShouldAnimateScroll` returns false. They are applied directly with `Viewport::ScrollBy` / `ScrollSingleNode` and clamped. | `cc/input/input_handler.cc` |
| A precise scroll begin **aborts** any running smooth-scroll animation. | `InputHandler::ScrollBegin` (`ScrollAnimationAbort`) |
| If a fling is active, a new finger frame first sends `kScrollFlingCancel`. The comment says that on Linux there is no zero-delta axis event for this, so it relies on new axis events. | `ProcessPointerScrollData` (`is_fling_active_`) |

### 1.3 Touchpad fling

**Velocity on Wayland** (`WaylandEventSource::ComputeFlingVelocity`):
- `axis_stop` produces a frame with zero delta and `is_axis_stop`, which
  dispatches `kScrollFlingStart` with the computed velocity.
- Only the last `kPointerScrollDataSetMaxSize = 3` scroll frames are kept.
  Each frame's `dt` is the time since the previous `wl_pointer.frame`.
- If the stop frame's `dt` exceeds `kFlingStartTimeoutMs = 200`, the velocity
  is 0, so there is no fling after the fingers rest.
- With one frame, the velocity is `dx · dt`. This multiplies rather than
  divides. It is a quirk that is ported as is.
- Otherwise it fits a least-squares line of cumulative distance against time,
  newest first, starting from the stop frame's dt. The code says it "matches
  libgestures's RegressScrollVelocity()". The fit stops at the first frame
  that isn't from a finger source.

**Which curve.** `components/input/fling_controller.cc` runs flings
browser-side. `web_gesture_curve_impl.cc` `CreateDefaultPlatformCurve` picks
`PhysicsBasedFlingCurve` because `features::kExperimentalFlingAnimation` is
`FEATURE_ENABLED_BY_DEFAULT` on `IS_WIN || IS_LINUX`
(`ui/base/ui_base_features.cc`). The older exponential `ui::FlingCurve`
(`fling_curve.cc`, α=−5707.62, β=172, γ=3.7) is **not** used on Linux.

**`ui/events/gestures/physics_based_fling_curve.cc`:**
- **Distance:** `CalculateEndPoint` uses deceleration
  `pixels_per_inch · kDefaultPhysicalDeceleration`, where
  `kDefaultPhysicalDeceleration = 2.7559e-5` in/ms² and the pixels per inch is
  `kDefaultPixelsPerInch = 96` (`components/input/fling_controller.h`, returned
  by `FlingScheduler::GetPixelsPerInch` on all non-Windows platforms). With
  v in px/ms, distance is `v²/(2·decel)`.
- **Distance cap:** the distance is capped at
  `floor(viewport · boost_multiplier · kDefaultBoundsMultiplier)`, with
  `kDefaultBoundsMultiplier = 3`.
- **Duration:** `min(kMaxCurveDurationForFling = 2 s, v / kDefaultPixelDeceleration)`,
  with `kDefaultPixelDeceleration = 2300` px/s².
- **Shape:** cubic-bezier(p1, (0.55, 1)). Start from p1 = (0.2, 1) and set
  `slope = v·duration/distance`. If `0.2·slope < 1`, then p1 = (0.2, 0.2·slope);
  otherwise p1 = (1/slope, 1). This makes the curve start at the lift-off velocity.

| v (px/s) | 500 | 1000 | 2000 | 3000 | 4000 | 5000 |
|---|---|---|---|---|---|---|
| distance (px, uncapped) | 47 | 189 | 756 | 1701 | 3024 | 4725 |
| duration (s) | 0.22 | 0.43 | 0.87 | 1.30 | 1.74 | 2.00 |

**Timing in `FlingController`:**
- The fling starts at the **last scroll update's** timestamp
  (`last_seen_scroll_update_`) "to maintain a smooth, continuous transition".
- If the first progress comes two or more frames later
  (`kMaxMicrosecondsFromFlingTimestampToFirstProgress = 2/60 s`), the start
  moves to `now − kFrameDelta (1/60 s)`.
- Progress is sent as precise-pixel wheel events, so it is applied directly
  and clamped.
- Deltas of 0.1 px or less are not sent (`kMinInertialScrollDelta`).
- When the curve becomes inactive, the fling ends **without** sending the
  last delta.

**Stopping a fling:**

| Cause | Source |
|---|---|
| A momentum update that isn't consumed, for example at an edge. | `RenderWidgetHostViewInput::StopFlingingIfNecessary` |
| Any keyboard event. | `InputRouterImpl::SendKeyboardEvent` → `StopFling` |
| A new finger scroll (GestureFlingCancel). | `FlingController::ProcessGestureFlingCancel` |

**Boosting** (`ui/events/blink/fling_booster.cc`):

A new fling adds the previous fling's starting velocity when all of these hold:
- the new fling starts within `kFlingBoostTimeoutDelay = 50 ms` of the last
  scroll update, and each update came within 50 ms of the previous one;
- every update goes the same direction as the previous fling;
- the updates themselves are at least `kMinBoostTouchScrollSpeedSquare = 150²`
  (px/s)²;
- both the current fling's velocity and the new fling's velocity are at least
  `kMinBoostFlingSpeedSquare = 350²` (px/s)².

`boost_multiplier = boosted / raw` also scales the distance cap.

### 1.4 Keyboard

| Fact | Source |
|---|---|
| Arrows are `kScrollByLine`, PageUp/PageDown are `kScrollByPage`, Home/End are `kScrollByDocument`. Space and Shift+Space are `kScrollByPage`. | `third_party/blink/renderer/core/input/keyboard_event_manager.cc` (`MapKeyCodeForScroll`, `DefaultSpaceEventHandler`) |
| These go through `ScrollManager::LogicalScroll` → `ScrollableArea::UserScroll` (main thread). | `scroll_manager.cc` |
| Line step is `kPixelsPerLineStep = 40`. | `cc/input/scroll_utils.h`, used by `ScrollableArea::LineStep` |
| Page step is `ScrollUtils::CalculatePageStep(h) = max(int(h·kMinFractionToStepWhenPaging), h − kMaxOverlapBetweenPages, 1)`, with `kMinFractionToStepWhenPaging = 0.875`. `kMaxOverlapBetweenPages` is `INT_MAX` off Mac, so a page step is 87.5 % of the visible height (the scroll-snapport size). | `scroll_utils.cc`; `ScrollableArea::PageStep` |
| Document step is the scroll size. | `ScrollableArea::DocumentStep` |
| `blink::ScrollAnimator::CreateAnimationCurve` uses `kKeyboard` for any granularity other than `kScrollByPixel`. That gives `DurationBehavior::kConstant`: `kConstantDuration = 9` frames, which is **150 ms**, ease-in-out (0.42, 0, 0.58, 1). | `third_party/blink/renderer/core/scroll/scroll_animator.cc` |
| Repeated keys add to the **pending target** (`DesiredTargetOffset` + `ComputeDeltaToConsume`, clamped) and retarget with the same `UpdateTarget` velocity matching. | `scroll_animator.cc` `UserScroll` / `WillAnimateToOffset` |
| A user scroll cancels a programmatic smooth scroll. | `ScrollableArea::UserScroll` → `CancelProgrammaticScrollAnimation` |

### 1.5 Programmatic smooth scroll (`scrollTo({behavior: 'smooth'})`)

- `ProgrammaticScrollAnimator` (`programmatic_scroll_animator.cc`) creates a
  **new** `kProgrammatic` curve on every call. If the target is unchanged it
  does nothing. There is no velocity matching.
- `kDeltaBased` duration: `min(√|Δ|, kMaxAnimationDuration·60) / 60` s.
  That is about 0.53 s for 1000 px, with a cap of 1.5 s.
- The easing is cubic-bezier(0.4, 0, 0, 1): fast start, long ease-out.
- On non-Android, both come from `cc/base/features.cc`
  `PROGRAMMATIC_SCROLL_ANIMATION_CURVE(0.4, 0.0, 0.0, 1.0, 1500)`, the
  parameters of `kProgrammaticScrollAnimationOverride`, which is enabled by
  default.

### 1.6 Scrollbar

- Dragging the thumb sends `kScrollByPrecisePixel`, so it is not animated (comment in
  `InputHandler::ScrollUpdate`: "Interactions with the scrollbar thumb have
  kScrollByPrecisePixel").
- Track clicks and arrow buttons use page and line granularity (`ScrollbarController`, not ported).

### 1.7 Edges and overscroll on Linux

- **Chrome clamps at both edges on Linux.** All paths clamp to
  `[0, max]`: `ClampScrollOffsetToLimits`, the target clamp in
  `ScrollAnimationUpdateTarget`, and `ScrollAnimator::TickAnimation` →
  `ClampScrollOffset`.
- **Elastic overscroll is off on Linux.** Elastic overscroll is the
  rubber-band stretch past the edge that springs back, as on macOS and
  Android 12+. `layer_tree_settings.cc` sets
  `enable_elastic_overscroll_on_root` from `IsElasticOverscrollEnabledOnRoot()`.
  On Linux that is `IsElasticOverscrollSupported()`
  (`ui/base/ui_base_switches_util.cc`), which is
  `FeatureList::IsEnabled(kElasticOverscroll)`, and `kElasticOverscroll` is
  `FEATURE_DISABLED_BY_DEFAULT` off Android (`ui/base/ui_base_features.cc`).
- A fling that hits an edge stops (see 1.3).

### 1.8 Things the brief mentioned that no longer exist

- **Impulse curve.** There is no impulse curve type any more. Only a stale
  comment in `UpdateTarget` mentions "Impulse-style" curves. The only animation
  types are `kLinear` (autoscroll) and `kEaseInOut`.
- **Percent-based scrolling.** There is no percent-based wheel scrolling.
  Code search finds no `PercentBasedScrolling` in the repo. The only remnant is
  `ScrollUtils::ResolveScrollPercentageToPixels` for OS "scroll by page" wheel
  settings (`kScrollByPage` × 0.875).

---

## 2. What makes it feel good, as checkable properties

| # | Property | Value in Chrome | Port | Checked by |
|---|---|---|---|---|
| P1 | Wheel notch distance | 120 px | yes | `single_tick_duration_and_endpoint_match_chromium_formula` |
| P2 | Wheel tick duration | 100–200 ms, shorter for bigger deltas | yes | same |
| P3 | Easing shape | cubic-bezier(0.42,0,0.58,1) for wheel and keys; (0.4,0,0,1) for programmatic | yes | `bezier_matches_known_values`, midpoint asserts |
| P4 | Retarget keeps speed continuous | slope-matched bezier | yes, < 3 % speed change at the retarget | `retarget_is_velocity_continuous_and_never_overshoots` |
| P5 | Retarget never accumulates lag | new target = old target + Δ | yes | same, final offset = Σ notches |
| P6 | No visible overshoot | velocity bound ×2.5; worst case 0.038 % of the remaining distance (bezier (0.42, 1.05, 0.58, 1) peaks at 1.000378) | yes, faithful sub-pixel | `retarget_overshoot_is_bounded`, `..._after_reversal_or_near_target` |
| P7 | Touchpad 1:1, no animation, no smoothing | precise deltas applied directly | yes | `precise_deltas_apply_one_to_one` |
| P8 | Fling starts at lift-off velocity, then decelerates | bezier p1 fit to v | yes | `fling_starts_at_its_initial_velocity` |
| P9 | Fling distance and duration | v²/(2·96·2.7559e-5), ≤ 3 viewports, ≤ 2 s | yes | `fling_travels_chromium_distance_and_stops`, `fling_is_bounded_...` |
| P10 | Fast repeated flings accelerate | boost rules (1.3) | yes | `repeated_fast_flings_boost` |
| P11 | Hard clamp, no rubber-band on Linux | clamp | yes | `clamps_at_both_edges` |
| P12 | Frame-rate independence | curves are functions of time, ticked at frame time | yes: 60 Hz, 165 Hz and dropped-frame stepping agree to < 0.5 px at every sampled time | `tick_is_frame_rate_independent` |
| P13 | Motion is visible on the first frame after input | animation starts "one full frame in" | yes: the curve's t=0 is the event time, so the first frame shows `curve(frame − event)` > 0 | `first_frame_after_input_already_moves` |
| P14 | Idle means zero work | animation stops at curve end | yes: `needs_frame()` | `needs_frame_goes_false_when_settled` |
| P15 | Scrolling never waits on layout, paint or JS | cc runs wheel, touchpad and fling scrolls on the **compositor thread**; the main thread can be busy | **only partly** (see below) | — |
| P16 | Rasterisation stays ahead of scrolling | cc pre-rasters tiles around the viewport on worker threads and checkerboards rather than block | **no**, frontend's job | — |
| P17 | Input coalescing and resampling | cc coalesces input per BeginFrame; `kResampleScrollEventsForFling` aligns timestamps | partly: last-write-wins per frame, because the model is time-based | — |

**What an app-level port can't reproduce.**
- **P15/P16 (threading).** Chrome's feel depends on cc producing a frame from
  already-rastered tiles even when the main thread is janky. In these apps, the
  thread that calls `tick` is usually the same UI thread that does layout. The
  model is cheap (a few hundred flops per tick, no allocation), so it is never
  the bottleneck. A frame still only arrives on time if the frontend's
  per-frame work stays small. See section 3.
- **P17 (presentation timestamps).** cc animates with the BeginFrame's
  frame time. Frontends should pass the closest thing they have, such as the
  `wl_surface.frame` callback time or the framework's frame timestamp. See 3.2.

---

## 3. Wiring a frontend

### 3.1 Feed raw input to the right method

Turn **off** the framework's own scroll handling for the feed. Don't wrap the
feed in iced `scrollable`, a GPUI `ScrollHandle` with wheel handling, or a Rune
scroll view that also consumes wheel events. Otherwise the content scrolls
twice: the framework's delta plus ours. Position content yourself at `-offset`.

| Raw event (Wayland via winit/smithay/etc.) | Call |
|---|---|
| `axis_value120` (wheel source) | `wheel_ticks(v120 / 120.0, t_event)` (120 units per notch) |
| legacy `axis` with wheel source, no value120 | `wheel_ticks(wayland_axis_to_px(v) / 120.0, t_event)` (one notch = 120 px) |
| winit `MouseScrollDelta::LineDelta(_, y)` | `wheel_ticks(-y, t_event)` (winit documents +y as "away from the user", i.e. up) |
| `axis` with finger or continuous source (touchpad) | `wheel_pixels(wayland_axis_to_px(v), t_event)` (Chrome default ×12) |
| winit `MouseScrollDelta::PixelDelta(p)` | `wheel_pixels(-p.y * k, t_event)`. Pick `k` so the result equals `wayland_axis_to_px(raw wl axis value)`. If your winit version passes the raw axis value through, `k = 12`. **Unverified**: check your winit version's Wayland backend before you rely on this. |
| `axis_stop` (finger lift) / winit `TouchPhase::Ended` on a touchpad scroll | `fling_start(t_event)` |
| touchscreen flick with a known velocity | `fling(v_px_per_s, t_event)` |
| pointer button press on the feed, or hold gesture begin | `cancel_fling(t_event)` |
| ↑ / ↓ | `scroll_by(∓line_step(), t)` |
| PageUp / PageDown / Space / Shift+Space | `scroll_by(±page_step(viewport_h), t)` |
| Home / End | `scroll_by(∓document_step(), t)` |
| "scroll to top", jump to an item | `scroll_to(y, t)` |
| scrollbar thumb drag, restoring a saved position | `jump_to(y)` |
| content or viewport size change | `set_max_offset(content_h − viewport_h)`, `set_viewport(viewport_h)` |

Sign: positive values move toward the end of the content. Wayland axis values
are already positive for "down". Most toolkits (winit included) flip that, so
negate their values.

Pass each event's own timestamp, on the same monotonic clock as the frame
times, in seconds. `now` before the last tick is treated as the last tick.

### 3.2 Present

- Call `tick(frame_time)` **exactly once per presented frame**, before layout.
  Draw at the returned offset.
- `frame_time` should be the frame's target presentation or BeginFrame time:
  the `wl_surface.frame` callback timestamp, or the framework's frame
  timestamp. Don't use "time when my handler ran". Jitter there becomes jitter
  on screen.
- Request another frame only while `needs_frame()` is true. When it is false,
  the app must draw nothing. It becomes true again on the next input or
  geometry change, so check it after input handlers too.
- Never also apply the framework's scroll offset, inertia or smoothing. Never
  smooth the returned value. It is already the curve.

### 3.3 Keep per-frame work small during scroll

Chrome's smoothness is mostly cc never blocking on the main thread (P15/P16).
The frontends have to earn that themselves:
- **Virtualise rows.** Lay out and build only the visible rows plus about one
  screen of margin in each direction. Row heights should be known or cached,
  so `max_offset` doesn't need a full layout.
- **Do no layout for off-screen content** and don't rebuild the widget tree on
  every tick. A scroll frame should change only a translation (or, for
  virtualised rows, recycle a few rows).
- **Never decode images on the UI thread.** Decode and resize thumbnails on
  a worker into GPU-ready textures ahead of the viewport. Show a placeholder
  rather than wait (Chrome checkerboards rather than block).
- **Do no allocation or I/O in the frame path.** Prefetch the next feed page
  when the target (`target()`, not `offset()`) gets within a few screens of
  `max_offset`. That way the fetch starts at the start of a fling, not at its end.

---

## 4. Deliberate differences from Chromium

1. **Time origin and `delayed_by`.**
   - Chrome: curve time 0 is set when the animation is attached plus one frame
     interval (`animation_start_offset`), and the duration is shortened by the
     input latency `delayed_by`.
   - Port: curve time 0 is the **input event's timestamp**, with
     `delayed_by = 0`. That gives the same intent (motion on the first frame,
     the animation ends about `duration` after the input) with no dependence
     on frame interval, which is what makes P12 exact.
   - Input timestamps earlier than the last tick are moved up to the last
     tick, so a retarget never starts from a point before what is already on
     screen.
2. **Stopping motion uses the curve's value at the input time**, not the last
   presented frame (cc keeps the last ticked offset). This differs by at most
   one frame of motion and keeps aborts frame-rate independent.
3. **Fling arithmetic.**
   - Velocity is analytic (`distance/T · bezier'`). Chrome finite-differences
     consecutive frames, and that velocity only feeds the boost threshold.
   - The fling reaches exactly `distance`. Chrome drops the last frame's
     delta and every delta of 0.1 px or less, both sub-pixel on an ease-out
     tail. Dropping them would make the endpoint frame-rate dependent.
   - At an edge the fling ends on the frame that hits the clamp. Chrome ends
     it on the next frame, the first one that isn't consumed. The same pixels
     are drawn either way.
4. **Wayland velocity bookkeeping.**
   - `dt` for each sample is the time since the previous **scroll** call
     (`wheel_pixels`, `wheel_ticks`, `fling_start`). Chrome counts every
     `wl_pointer.frame`, including motion and button frames, which the module
     doesn't see.
   - Before any input, the first `dt` is `now − 0`, which matches Chrome's
     `now − TimeTicks()`.
   - The f32 arithmetic and the quirks of `ComputeFlingVelocity` are kept:
     the single-sample `dx·dt` case, and not clearing samples on the 200 ms
     early return.
5. **One axis.** Chrome sizes 2D animations by the larger axis
   (`MaximumDimension`). Two independent instances differ only for diagonal
   input.
6. **One animator for all input kinds.** In Chrome, wheel animations live in
   cc and keyboard and programmatic animations in Blink. When a different kind
   of input arrives mid-animation, the port stops the running animation at its
   current position and starts the new kind from there. Same-kind input
   retargets exactly as in Chrome.
7. **`cancel_fling(now)` takes a timestamp.** The fling-boost window
   (`observe_fling_cancel`) and stopping at the current position both need it.
8. **Not ported:** autoscroll (middle-click, linear curve), scroll snapping,
   scroll anchoring adjustments (`ApplyAdjustment`), scrollbar track and
   button repeat, nested scroller chaining, and browser-controls offsets.

### Not verified from source (behaviour chosen)

- **A mouse-wheel notch during a touchpad fling.** I didn't trace this path.
  The port stops the fling and resets boosting.
- **Mixing curve kinds mid-flight** (keyboard during a cc wheel animation, wheel
  during a Blink programmatic animation). I didn't trace this across the
  Blink/cc boundary. Behaviour as in item 6.
- **Exact touchpad scroll-begin and scroll-end boundaries on Wayland**
  (`MouseWheelPhaseHandler` phases). The port starts a gesture on the first
  `wheel_pixels` after anything else and ends it at fling start or at any
  other input. This affects only the boost window and the fling start time.
- **`FlingController::first_fling_update_sent()`.** I didn't read its
  definition. It is assumed to become true at the first progress that moves.
- **Device scale factor.** I assume wheel and touchpad deltas and the 96 ppi
  fling constants are in logical (DIP) pixels. I didn't trace the DSF scaling
  in the renderer.
- **Disabled animations and programmatic smooth scroll.** I didn't check
  whether `scrollTo({behavior:'smooth'})` respects the disabled-animations
  setting. `scroll_to` always animates.
