//! Chromium-faithful scroll physics for one scroll axis.
//!
//! This is a port of how Chrome on Linux desktop (Aura/Ozone Wayland) turns raw
//! input into a scroll offset, so that every frontend (iced, GPUI, Rune) scrolls
//! the same way and the way Chrome does. The frontend feeds raw input into a
//! [`ScrollAnimator`] and reads back the offset to present once per frame with
//! [`ScrollAnimator::tick`]. See `docs/scroll.md` for the wiring rules.
//!
//! Ported against chromium/chromium `main` at
//! 6aeba664bd893911381924267bc75e9e04610a8b. Every constant names the file and
//! symbol it comes from.
//!
//! Design rules:
//! * `std` only, no allocation, no clock: every time-dependent method takes
//!   `now` in seconds on the caller's monotonic clock. Pass the input event's
//!   timestamp to input methods and the frame's presentation timestamp to
//!   `tick`.
//! * All motion is a pure function of time (curves are evaluated at absolute
//!   time, never integrated per frame), so a 60 Hz caller, a 165 Hz caller and
//!   a caller that drops frames all land on the same curve.
//! * One axis. For 2D, instantiate twice. Chrome sizes 2D animations by the
//!   larger axis (`MaximumDimension` in
//!   `cc/animation/scroll_offset_animation_curve.cc`); two independent axes
//!   differ only for diagonal input, which these apps don't produce.
//!
//! Sign convention: positive deltas move the offset toward the end of the
//! content (content moves up on screen, as when you scroll down). Wayland axis
//! values use the same sign (positive is down), so you pass them through
//! unchanged after scaling.

// ---------------------------------------------------------------------------
// Input unit constants
// ---------------------------------------------------------------------------

/// Pixels per mouse-wheel notch.
/// `ui/events/event.cc` `MouseWheelEvent::kWheelDelta` = 120 (the
/// `IS_WIN || IS_FUCHSIA || IS_LINUX` branch). Chrome's Linux MouseWheelEvent
/// offset (120 per notch) is used directly as the pixel delta, with
/// `delta_units = kScrollByPixel` (`ui/events/blink/web_input_event.cc`
/// `MakeWebMouseWheelEventFromUiEvent(const MouseWheelEvent&)`).
pub const WHEEL_DELTA_PX: f64 = 120.0;

/// Wayland `wl_pointer.axis_value120` units per notch (protocol definition).
/// `ui/ozone/platform/wayland/host/wayland_pointer.cc` `OnAxisValue120` uses
/// `-value120` directly as the wheel offset, so 120 value120 = 120 px.
pub const WAYLAND_VALUE120_PER_TICK: f64 = 120.0;

/// Wayland compositors send `wl_pointer.axis` value 10 per wheel notch.
/// `ui/ozone/platform/wayland/host/wayland_pointer.cc` `OnAxis`
/// `kAxisValueScale` = 10.
pub const WAYLAND_AXIS_VALUE_SCALE: f64 = 10.0;

/// Pixels per unit of `wl_pointer.axis` value, for both legacy wheel axis
/// events and touchpad (finger/continuous source) axis events:
/// `delta / kAxisValueScale * MouseWheelEvent::kWheelDelta` = value * 12.
/// `wayland_pointer.cc` `OnAxis`, applied while the feature
/// `kWaylandUnscaledTouchpadScrolling` (`ui/ozone/common/features.cc`) is
/// disabled, which is its default.
pub const WAYLAND_AXIS_TO_PX: f64 = WHEEL_DELTA_PX / WAYLAND_AXIS_VALUE_SCALE;

/// Touchpad factor Chrome uses instead of [`WAYLAND_AXIS_TO_PX`] when the
/// (default-off) feature `kWaylandUnscaledTouchpadScrolling` is enabled.
/// `ui/ozone/common/features.cc` `kWaylandUnscaledTouchpadScrollingNewFactor`
/// = 2.5. Provided for frontends that find the default 12x too fast.
pub const WAYLAND_UNSCALED_TOUCHPAD_FACTOR: f64 = 2.5;

/// Arrow-key / scrollbar-button step.
/// `cc/input/scroll_utils.h` `kPixelsPerLineStep` = 40 (used by
/// `blink::ScrollableArea::PixelsPerLineStep` and `cc::InputHandler::LineStep`).
pub const PIXELS_PER_LINE_STEP: f64 = 40.0;

/// Page Up/Down and Space step as a fraction of the visible height.
/// `cc/input/scroll_utils.h` `kMinFractionToStepWhenPaging` = 0.875.
pub const MIN_FRACTION_TO_STEP_WHEN_PAGING: f64 = 0.875;

// ---------------------------------------------------------------------------
// Smooth-scroll curve constants (cc/animation/scroll_offset_animation_curve.cc)
// ---------------------------------------------------------------------------

/// `kConstantDuration` = 9 frames (keyboard: 9 / 60 s = 150 ms).
const CONSTANT_DURATION_FRAMES: f64 = 9.0;
/// `kDurationDivisor` = 60 (durations are in 60 Hz frames).
const DURATION_DIVISOR: f64 = 60.0;
/// `kInverseDeltaRampStartPx` = 120.
const INVERSE_DELTA_RAMP_START_PX: f64 = 120.0;
/// `kInverseDeltaRampEndPx` = 480.
const INVERSE_DELTA_RAMP_END_PX: f64 = 480.0;
/// `kInverseDeltaMinDuration` = 6 frames (100 ms).
const INVERSE_DELTA_MIN_DURATION_FRAMES: f64 = 6.0;
/// `kInverseDeltaMaxDuration` = 12 frames (200 ms).
const INVERSE_DELTA_MAX_DURATION_FRAMES: f64 = 12.0;
/// `kInverseDeltaSlope` = (min - max) / (end - start) = -1/60 frame per px.
const INVERSE_DELTA_SLOPE: f64 = (INVERSE_DELTA_MIN_DURATION_FRAMES
    - INVERSE_DELTA_MAX_DURATION_FRAMES)
    / (INVERSE_DELTA_RAMP_END_PX - INVERSE_DELTA_RAMP_START_PX);
/// `kInverseDeltaOffset` = max - start * slope = 14 frames.
const INVERSE_DELTA_OFFSET: f64 =
    INVERSE_DELTA_MAX_DURATION_FRAMES - INVERSE_DELTA_RAMP_START_PX * INVERSE_DELTA_SLOPE;
/// `kEpsilon` = 0.01f (a float literal, so widened from f32 like the C++).
const CURVE_EPSILON: f64 = 0.01_f32 as f64;
/// `EaseInOutWithInitialSlope`: `std::clamp(slope, -1000.0, 1000.0)`.
const MAX_INITIAL_SLOPE: f64 = 1000.0;
/// `VelocityBasedDurationBound`: `(new_delta / velocity) * 2.5f`.
const VELOCITY_BOUND_FUDGE: f64 = 2.5;
/// `kEaseInOutControlPoints` = cubic-bezier(0.42, 0, 0.58, 1), used for
/// mouse-wheel and keyboard curves.
const EASE_IN_OUT: [f64; 4] = [0.42, 0.0, 0.58, 1.0];
/// Programmatic curve on non-Android: `cc/base/features.cc`
/// `PROGRAMMATIC_SCROLL_ANIMATION_CURVE(0.4, 0.0, 0.0, 1.0, 1500)`, the params
/// `kCubicBezierX1/Y1/X2/Y2` of `kProgrammaticScrollAnimationOverride`
/// (enabled by default).
const PROGRAMMATIC_EASE: [f64; 4] = [0.4, 0.0, 0.0, 1.0];
/// Same macro: `kMaxAnimationDuration` = 1500 ms.
const PROGRAMMATIC_MAX_DURATION_S: f64 = 1.5;

/// `cc/trees/layer_tree_host_impl.cc` `LayerTreeHostImpl::ScrollAnimationCreate`
/// `kEpsilon` = 0.1f: wheel deltas this small are applied without animation.
const MIN_ANIMATED_DELTA: f64 = 0.1_f32 as f64;

// ---------------------------------------------------------------------------
// Cubic bezier solver constants (ui/gfx/geometry/cubic_bezier.{h,cc})
// ---------------------------------------------------------------------------

/// `CUBIC_BEZIER_SPLINE_SAMPLES` = 11.
const SPLINE_SAMPLES: usize = 11;
/// `kMaxNewtonIterations` = 4.
const MAX_NEWTON_ITERATIONS: usize = 4;
/// `kBezierEpsilon` = 1e-7.
const BEZIER_EPSILON: f64 = 1e-7;

// ---------------------------------------------------------------------------
// Fling constants
// ---------------------------------------------------------------------------
// On Linux, `features::kExperimentalFlingAnimation` (`ui/base/ui_base_features.cc`)
// is enabled by default, so `ui/events/gestures/blink/web_gesture_curve_impl.cc`
// `CreateDefaultPlatformCurve` picks `ui::PhysicsBasedFlingCurve`.

/// `ui/events/gestures/physics_based_fling_curve.cc` `kDefaultP1X` = 0.2.
const FLING_P1X: f64 = 0.2_f32 as f64;
/// `kDefaultP1Y` = 1.0.
const FLING_P1Y: f64 = 1.0;
/// `kDefaultP2X` = 0.55.
const FLING_P2X: f64 = 0.55_f32 as f64;
/// `kDefaultP2Y` = 1.0.
const FLING_P2Y: f64 = 1.0;
/// `kDefaultPixelDeceleration` = 2300 px/s^2 (sets duration = v / 2300).
const FLING_PIXEL_DECELERATION: f64 = 2300.0;
/// `kMaxCurveDurationForFling` = 2 s.
const FLING_MAX_DURATION_S: f64 = 2.0;
/// `kDefaultPhysicalDeceleration` = 2.7559e-5 inch/ms^2 (sets distance).
const FLING_PHYSICAL_DECELERATION: f64 = 2.7559e-5_f32 as f64;
/// `ui/events/gestures/physics_based_fling_curve.h` `kDefaultBoundsMultiplier`
/// = 3: a fling travels at most 3 viewports (times the boost multiplier).
const FLING_BOUNDS_MULTIPLIER: f64 = 3.0;
/// `components/input/fling_controller.h` `kDefaultPixelsPerInch` = 96,
/// returned by `content/browser/renderer_host/input/fling_scheduler.cc`
/// `FlingScheduler::GetPixelsPerInch` on every non-Windows platform.
const DEFAULT_PIXELS_PER_INCH: f64 = 96.0;

/// `components/input/fling_controller.cc` `kFrameDelta` = 1/60 s.
const FLING_FRAME_DELTA: f64 = 1.0 / 60.0;
/// `kMaxMicrosecondsFromFlingTimestampToFirstProgress` = 2/60 s.
const FLING_MAX_DELAY_TO_FIRST_PROGRESS: f64 = 2.0 / 60.0;

/// `ui/ozone/platform/wayland/host/wayland_event_source.cc`
/// `kPointerScrollDataSetMaxSize` = 3 frames kept for fling velocity.
const POINTER_SCROLL_DATA_SET_MAX: usize = 3;
/// Same file, `kFlingStartTimeoutMs` = 200: no fling if the fingers rested
/// longer than this between the last scroll frame and lifting.
const FLING_START_TIMEOUT_S: f64 = 0.2;

/// `ui/events/blink/fling_booster.cc` `kMinBoostFlingSpeedSquare` = 350^2.
const MIN_BOOST_FLING_SPEED: f64 = 350.0;
/// `kMinBoostTouchScrollSpeedSquare` = 150^2.
const MIN_BOOST_SCROLL_SPEED: f64 = 150.0;
/// `kFlingBoostTimeoutDelay` = 50 ms.
const FLING_BOOST_TIMEOUT_S: f64 = 0.05;

// ---------------------------------------------------------------------------
// Unit helpers
// ---------------------------------------------------------------------------

/// Converts a Wayland `axis_value120` value into notches for
/// [`ScrollAnimator::wheel_ticks`]. High-resolution wheels send fractions.
pub fn wayland_value120_to_ticks(value120: f64) -> f64 {
    value120 / WAYLAND_VALUE120_PER_TICK
}

/// Converts a Wayland `wl_pointer.axis` value into pixels the way Chrome does
/// by default (x12). Use it for legacy wheel axis events (pass the result /
/// [`WHEEL_DELTA_PX`] to `wheel_ticks`) and for finger/continuous axis events
/// (pass the result to `wheel_pixels`).
pub fn wayland_axis_to_px(value: f64) -> f64 {
    value * WAYLAND_AXIS_TO_PX
}

// ---------------------------------------------------------------------------
// gfx::CubicBezier port
// ---------------------------------------------------------------------------

/// Port of `ui/gfx/geometry/cubic_bezier.cc` `gfx::CubicBezier`, including its
/// spline-seeded Newton solver, bisection fallback and linear extrapolation
/// outside [0, 1].
#[derive(Clone, Copy, Debug)]
struct CubicBezier {
    ax: f64,
    bx: f64,
    cx: f64,
    ay: f64,
    by: f64,
    cy: f64,
    start_gradient: f64,
    end_gradient: f64,
    spline: [f64; SPLINE_SAMPLES],
}

fn to_finite(v: f64) -> f64 {
    // `CubicBezier::ToFinite`.
    if v.is_infinite() {
        if v > 0.0 { f64::MAX } else { f64::MIN }
    } else {
        v
    }
}

impl CubicBezier {
    fn new(p1x: f64, p1y: f64, p2x: f64, p2y: f64) -> Self {
        // InitCoefficients
        let cx = 3.0 * p1x;
        let bx = 3.0 * (p2x - p1x) - cx;
        let ax = 1.0 - cx - bx;
        let cy = to_finite(3.0 * p1y);
        let by = to_finite(3.0 * (p2y - p1y) - cy);
        let ay = to_finite(1.0 - cy - by);
        // InitGradients
        let start_gradient = if p1x > 0.0 {
            p1y / p1x
        } else if p1y == 0.0 && p2x > 0.0 {
            p2y / p2x
        } else if p1y == 0.0 && p2y == 0.0 {
            1.0
        } else {
            0.0
        };
        let end_gradient = if p2x < 1.0 {
            (p2y - 1.0) / (p2x - 1.0)
        } else if p2y == 1.0 && p1x < 1.0 {
            (p1y - 1.0) / (p1x - 1.0)
        } else if p2y == 1.0 && p1y == 1.0 {
            1.0
        } else {
            0.0
        };
        let mut b = CubicBezier {
            ax,
            bx,
            cx,
            ay,
            by,
            cy,
            start_gradient,
            end_gradient,
            spline: [0.0; SPLINE_SAMPLES],
        };
        // InitSpline
        let delta_t = 1.0 / (SPLINE_SAMPLES - 1) as f64;
        for i in 0..SPLINE_SAMPLES {
            b.spline[i] = b.sample_x(i as f64 * delta_t);
        }
        b
    }

    fn sample_x(&self, t: f64) -> f64 {
        ((self.ax * t + self.bx) * t + self.cx) * t
    }

    fn sample_y(&self, t: f64) -> f64 {
        to_finite(((self.ay * t + self.by) * t + self.cy) * t)
    }

    fn sample_dx(&self, t: f64) -> f64 {
        (3.0 * self.ax * t + 2.0 * self.bx) * t + self.cx
    }

    fn sample_dy(&self, t: f64) -> f64 {
        to_finite(to_finite(to_finite(3.0 * self.ay) * t + to_finite(2.0 * self.by)) * t + self.cy)
    }

    /// `CubicBezier::SolveCurveX`.
    fn solve_curve_x(&self, x: f64, epsilon: f64) -> f64 {
        let delta_t = 1.0 / (SPLINE_SAMPLES - 1) as f64;
        // The C++ leaves t0/t1 uninitialised when x is past the last sample
        // (only reachable through rounding at x == 1); use the full range.
        let mut t0 = 0.0;
        let mut t1 = 1.0;
        let mut t2 = x;
        for i in 1..SPLINE_SAMPLES {
            if x <= self.spline[i] {
                t1 = delta_t * i as f64;
                t0 = t1 - delta_t;
                t2 = t0
                    + (t1 - t0) * (x - self.spline[i - 1]) / (self.spline[i] - self.spline[i - 1]);
                break;
            }
        }
        let newton_epsilon = BEZIER_EPSILON.min(epsilon);
        let mut x2 = 0.0;
        for _ in 0..MAX_NEWTON_ITERATIONS {
            x2 = self.sample_x(t2) - x;
            if x2.abs() < newton_epsilon {
                return t2;
            }
            let d2 = self.sample_dx(t2);
            if d2.abs() < BEZIER_EPSILON {
                break;
            }
            t2 -= x2 / d2;
        }
        if x2.abs() < epsilon {
            return t2;
        }
        // Bisection fallback.
        while t0 < t1 {
            x2 = self.sample_x(t2);
            if (x2 - x).abs() < epsilon {
                return t2;
            }
            if x > x2 {
                t0 = t2;
            } else {
                t1 = t2;
            }
            t2 = (t1 + t0) * 0.5;
        }
        t2
    }

    /// `CubicBezier::Solve` (`SolveWithEpsilon`).
    fn solve(&self, x: f64) -> f64 {
        if x < 0.0 {
            return to_finite(self.start_gradient * x);
        }
        if x > 1.0 {
            return to_finite(1.0 + self.end_gradient * (x - 1.0));
        }
        self.sample_y(self.solve_curve_x(x, BEZIER_EPSILON))
    }

    /// `CubicBezier::Slope` (`SlopeWithEpsilon`).
    fn slope(&self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        let t = self.solve_curve_x(x, BEZIER_EPSILON);
        let dx = self.sample_dx(t);
        let dy = self.sample_dy(t);
        if dx == 0.0 && dy == 0.0 {
            return 0.0;
        }
        to_finite(dy / dx)
    }
}

// ---------------------------------------------------------------------------
// cc::ScrollOffsetAnimationCurve port (1D)
// ---------------------------------------------------------------------------

/// `cc::ScrollOffsetAnimationCurve::ScrollType`, minus `kAutoScroll` (middle
/// click autoscroll uses a linear curve and isn't ported).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CurveKind {
    /// `kMouseWheel` -> `DurationBehavior::kInverseDelta`.
    MouseWheel,
    /// `kKeyboard` -> `DurationBehavior::kConstant`.
    Keyboard,
    /// `kProgrammatic` -> `DurationBehavior::kDeltaBased`.
    Programmatic,
}

/// Port of `cc::ScrollOffsetAnimationCurve` for `AnimationType::kEaseInOut`.
/// Times passed to methods are relative to `start_time` (the keyframe
/// model's start), exactly like the `base::TimeDelta t` arguments in C++.
#[derive(Clone, Copy, Debug)]
struct EaseCurve {
    kind: CurveKind,
    /// Absolute time (caller clock) at which curve-time 0 occurs.
    start_time: f64,
    initial_value: f64,
    target_value: f64,
    /// `total_animation_duration_`, relative to `start_time`.
    total_duration: f64,
    /// `last_retarget_`, relative to `start_time`.
    last_retarget: f64,
    timing: CubicBezier,
}

impl EaseCurve {
    /// `ScrollOffsetAnimationCurveFactory::CreateAnimation` followed by
    /// `SetInitialValue(initial, delayed_by = 0)`.
    fn new(kind: CurveKind, initial: f64, target: f64, start_time: f64) -> Self {
        let mut c = EaseCurve {
            kind,
            start_time,
            initial_value: initial,
            target_value: target,
            total_duration: 0.0,
            last_retarget: 0.0,
            timing: Self::easing(kind, None),
        };
        c.total_duration = c.segment_duration(target - initial, 0.0);
        c
    }

    /// `GetEasingFunction` + `EaseInOutWithInitialSlope`.
    fn easing(kind: CurveKind, slope: Option<f64>) -> CubicBezier {
        let [x1, y1, x2, y2] = match kind {
            CurveKind::Programmatic => PROGRAMMATIC_EASE,
            CurveKind::MouseWheel | CurveKind::Keyboard => EASE_IN_OUT,
        };
        match slope {
            Some(s) => {
                let s = s.clamp(-MAX_INITIAL_SLOPE, MAX_INITIAL_SLOPE);
                CubicBezier::new(x1, x1 * s, x2, y2)
            }
            None => CubicBezier::new(x1, y1, x2, y2),
        }
    }

    /// `EaseInOutSegmentDuration`, in seconds.
    fn segment_duration(&self, delta: f64, delayed_by: f64) -> f64 {
        let frames = match self.kind {
            CurveKind::Keyboard => CONSTANT_DURATION_FRAMES,
            CurveKind::Programmatic => delta
                .abs()
                .sqrt()
                .min(PROGRAMMATIC_MAX_DURATION_S * DURATION_DIVISOR),
            CurveKind::MouseWheel => (INVERSE_DELTA_OFFSET + delta.abs() * INVERSE_DELTA_SLOPE)
                .clamp(
                    INVERSE_DELTA_MIN_DURATION_FRAMES,
                    INVERSE_DELTA_MAX_DURATION_FRAMES,
                ),
        };
        (frames / DURATION_DIVISOR - delayed_by).max(0.0)
    }

    /// `EaseInOutBoundedSegmentDuration` with `VelocityBasedDurationBound`.
    fn bounded_segment_duration(&self, new_delta: f64, t: f64, delayed_by: f64) -> f64 {
        let velocity = self.velocity(t);
        let bound = if new_delta.abs() < CURVE_EPSILON {
            0.0
        } else if velocity.abs() < CURVE_EPSILON {
            f64::INFINITY
        } else {
            let b = (new_delta / velocity) * VELOCITY_BOUND_FUDGE;
            if b < 0.0 { f64::INFINITY } else { b }
        };
        self.segment_duration(new_delta, delayed_by).min(bound)
    }

    /// `GetValue(t)`.
    fn value(&self, t: f64) -> f64 {
        let duration = self.total_duration - self.last_retarget;
        let t = t - self.last_retarget;
        if duration == 0.0 || t >= duration {
            return self.target_value;
        }
        if t <= 0.0 {
            return self.initial_value;
        }
        let progress = self.timing.solve(t / duration);
        // gfx::Tween::FloatValueBetween
        self.initial_value + (self.target_value - self.initial_value) * progress
    }

    /// `CalculateVelocity(t)`, px/s.
    fn velocity(&self, t: f64) -> f64 {
        let duration = self.total_duration - self.last_retarget;
        if duration <= 0.0 {
            return 0.0; // C++ would divide by zero here; never reached by cc.
        }
        let slope = self.timing.slope((t - self.last_retarget) / duration);
        slope * ((self.target_value - self.initial_value) / duration)
    }

    /// `UpdateTarget(t, new_target)`: retarget in flight, preserving velocity.
    fn update_target(&mut self, t: f64, new_target: f64) {
        let delayed_by = (self.last_retarget - t).max(0.0);
        let t = t.max(self.last_retarget);

        if (self.target_value - new_target).abs() < CURVE_EPSILON {
            self.target_value = new_target;
            return;
        }

        let current_position = self.value(t);
        let new_delta = new_target - current_position;

        if new_delta.abs() < CURVE_EPSILON {
            self.last_retarget = t;
            self.total_duration = t;
            self.target_value = new_target;
            return;
        }

        let old_duration = self.total_duration - self.last_retarget;
        if old_duration == 0.0 {
            self.total_duration = self.segment_duration(new_delta, delayed_by);
            self.target_value = new_target;
            return;
        }

        let new_duration = self.bounded_segment_duration(new_delta, t, delayed_by);
        if new_duration < CURVE_EPSILON {
            self.target_value = new_target;
            self.total_duration = t;
            return;
        }

        let velocity = self.velocity(t);
        let new_slope = velocity * (new_duration / new_delta);

        self.timing = Self::easing(self.kind, Some(new_slope));
        self.initial_value = current_position;
        self.target_value = new_target;
        self.total_duration = t + new_duration;
        self.last_retarget = t;
    }

    fn end_time(&self) -> f64 {
        self.start_time + self.total_duration
    }
}

// ---------------------------------------------------------------------------
// ui::PhysicsBasedFlingCurve port (1D) + FlingController timing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Fling {
    /// `current_fling_parameters_.start_time` (absolute).
    start_time: f64,
    /// Offset when the fling began; the curve's offsets are added to it.
    start_offset: f64,
    /// `distance_` (signed px).
    distance: f64,
    /// `curve_duration_` (s).
    duration: f64,
    bezier: CubicBezier,
    /// `FlingController::first_fling_update_sent()`.
    first_update_sent: bool,
}

impl Fling {
    /// `PhysicsBasedFlingCurve` constructor. `bound` is the floored
    /// `bounding_size * boost_multiplier * kDefaultBoundsMultiplier`.
    fn new(velocity: f64, start_time: f64, start_offset: f64, bound: f64) -> Self {
        // CalculateEndPoint: deceleration in px/ms^2, velocity in px/ms.
        let deceleration = DEFAULT_PIXELS_PER_INCH * FLING_PHYSICAL_DECELERATION;
        let v_ms = velocity / 1000.0;
        let duration_ms = (v_ms / deceleration).abs();
        // GetOffset
        let mut distance =
            ((v_ms.abs() - deceleration * duration_ms * 0.5) * duration_ms).copysign(v_ms);
        if distance.abs() > bound {
            distance = bound.copysign(distance);
        }
        // CalculateDurationAndConfigureControlPoints
        let duration = FLING_MAX_DURATION_S.min(velocity.abs() / FLING_PIXEL_DECELERATION);
        let slope = (velocity * duration / distance).abs();
        let (mut p1x, mut p1y) = (FLING_P1X, FLING_P1Y);
        if slope * p1x < 1.0 {
            p1y = p1x * slope;
        } else {
            p1x = p1y / slope;
        }
        Fling {
            start_time,
            start_offset,
            distance,
            duration,
            bezier: CubicBezier::new(p1x, p1y, FLING_P2X, FLING_P2Y),
            first_update_sent: false,
        }
    }

    fn end_time(&self) -> f64 {
        self.start_time + self.duration
    }

    /// `FlingController::ProgressFling`: if the first progress is two or more
    /// frames after the start time, the start moves to one frame before now
    /// so a short fling still produces motion.
    fn adjust_start_for_first_progress(&mut self, now: f64) {
        if !self.first_update_sent && now >= self.start_time + FLING_MAX_DELAY_TO_FIRST_PROGRESS {
            self.start_time = now - FLING_FRAME_DELTA;
        }
    }

    /// Curve displacement at `elapsed` seconds (`ComputeScrollOffset`).
    fn displacement(&self, elapsed: f64) -> f64 {
        if elapsed <= 0.0 {
            0.0
        } else if elapsed < self.duration {
            self.distance * self.bezier.solve(elapsed / self.duration)
        } else {
            self.distance
        }
    }

    /// Analytic velocity at `elapsed` (Chrome finite-differences frames).
    fn velocity(&self, elapsed: f64) -> f64 {
        if elapsed <= 0.0 || elapsed >= self.duration || self.duration <= 0.0 {
            0.0
        } else {
            self.distance / self.duration * self.bezier.slope(elapsed / self.duration)
        }
    }
}

// ---------------------------------------------------------------------------
// ui::FlingBooster port (1D)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct FlingBooster {
    cutoff_time_for_boost: Option<f64>,
    previous_fling_starting_velocity: f64,
    current_fling_velocity: f64,
    previous_boosting_scroll_timestamp: Option<f64>,
}

impl FlingBooster {
    fn reset(&mut self) {
        *self = FlingBooster::default();
    }

    /// Shared prelude of `ObserveGestureEvent`. Returns false to stop.
    fn observe_prelude(&mut self, ts: f64) -> bool {
        if self.previous_fling_starting_velocity == 0.0 {
            return false;
        }
        if let Some(cutoff) = self.cutoff_time_for_boost
            && ts > cutoff
        {
            self.reset();
            return false;
        }
        true
    }

    fn observe_scroll_begin(&mut self, ts: f64) {
        if self.observe_prelude(ts) {
            self.cutoff_time_for_boost = Some(ts + FLING_BOOST_TIMEOUT_S);
        }
    }

    fn observe_scroll_update(&mut self, ts: f64, delta: f64) {
        if !self.observe_prelude(ts) || self.cutoff_time_for_boost.is_none() {
            return;
        }
        if self.previous_fling_starting_velocity * delta <= 0.0 {
            self.reset();
            return;
        }
        if let Some(prev) = self.previous_boosting_scroll_timestamp {
            let dt = ts - prev;
            if dt >= 0.001 && (delta / dt).abs() < MIN_BOOST_SCROLL_SPEED {
                self.reset();
                return;
            }
        }
        self.previous_boosting_scroll_timestamp = Some(ts);
        self.cutoff_time_for_boost = Some(ts + FLING_BOOST_TIMEOUT_S);
    }

    fn observe_fling_cancel(&mut self, ts: f64) {
        if self.observe_prelude(ts) {
            self.previous_boosting_scroll_timestamp = None;
            self.cutoff_time_for_boost = Some(ts + FLING_BOOST_TIMEOUT_S);
        }
    }

    fn observe_progress(&mut self, velocity: f64) {
        if self.previous_fling_starting_velocity != 0.0 {
            self.current_fling_velocity = velocity;
        }
    }

    fn should_boost(&self, velocity: f64, ts: f64) -> bool {
        if self.previous_fling_starting_velocity == 0.0 {
            return false;
        }
        let Some(cutoff) = self.cutoff_time_for_boost else {
            return false;
        };
        ts <= cutoff
            && self.previous_fling_starting_velocity * velocity > 0.0
            && self.current_fling_velocity.abs() >= MIN_BOOST_FLING_SPEED
            && velocity.abs() >= MIN_BOOST_FLING_SPEED
    }

    /// `GetVelocityForFlingStart`.
    fn velocity_for_fling_start(&mut self, velocity: f64, ts: f64) -> f64 {
        let mut v = velocity;
        if self.should_boost(velocity, ts) {
            v += self.previous_fling_starting_velocity;
        }
        self.reset();
        self.previous_fling_starting_velocity = v;
        self.current_fling_velocity = v;
        v
    }
}

// ---------------------------------------------------------------------------
// Wayland fling velocity (WaylandEventSource::ComputeFlingVelocity)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct PointerScrollFrame {
    dx: f64,
    /// Time since the previous pointer frame.
    dt: f64,
    /// `axis_source == WL_POINTER_AXIS_SOURCE_FINGER`.
    finger: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct PointerScrollDataSet {
    /// Newest first, like the C++ `push_front` deque.
    frames: [PointerScrollFrame; POINTER_SCROLL_DATA_SET_MAX],
    len: usize,
}

impl PointerScrollDataSet {
    fn push_front(&mut self, f: PointerScrollFrame) {
        if self.len + 1 > POINTER_SCROLL_DATA_SET_MAX {
            self.len -= 1; // pop_back
        }
        self.frames.copy_within(0..self.len, 1);
        self.frames[0] = f;
        self.len += 1;
    }

    /// Port of `ComputeFlingVelocity`, including its float arithmetic, its
    /// libgestures-style regression, and its quirks (see docs/scroll.md).
    fn compute_fling_velocity(&mut self, stop_dt: f64) -> f64 {
        let count = self.len;
        if count == 0 {
            return 0.0;
        }
        if stop_dt > FLING_START_TIMEOUT_S {
            return 0.0;
        }
        if count == 1 {
            let f = self.frames[0];
            let dt = (f.dt as f32) + (stop_dt as f32);
            return ((f.dx as f32) * dt) as f64;
        }
        let (mut tt, mut ts, mut tx, mut xs) = (0f32, 0f32, 0f32, 0f32);
        let mut time = stop_dt as f32;
        let mut x = 0f32;
        for f in &self.frames[..count] {
            if !f.finger {
                break;
            }
            time += f.dt as f32;
            x += f.dx as f32;
            tt += time * time;
            ts += time;
            tx += time * x;
            xs += x;
        }
        self.len = 0;
        let n = count as f32;
        let det = n * tt - ts * ts;
        if det == 0.0 {
            if ts != 0.0 {
                return (x / ts) as f64;
            }
            return 0.0;
        }
        let det_inv = 1.0 / det;
        ((n * tx - ts * xs) * det_inv) as f64
    }
}

// ---------------------------------------------------------------------------
// ScrollAnimator
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Motion {
    Idle,
    Curve(EaseCurve),
    Fling(Fling),
}

/// One scroll axis driven the way Chrome on Linux drives it.
///
/// Feed raw input with the `wheel_*`, `fling*`, `scroll_*` and `jump_to`
/// methods (passing each event's timestamp), call [`tick`](Self::tick) once
/// per presented frame with that frame's timestamp, and request frames only
/// while [`needs_frame`](Self::needs_frame) is true.
#[derive(Clone, Debug)]
pub struct ScrollAnimator {
    offset: f64,
    max_offset: f64,
    viewport: f64,
    smooth_scroll: bool,
    motion: Motion,
    /// Offset changed outside `tick`; one frame must be presented.
    dirty: bool,
    last_tick: f64,
    // Wayland pointer-frame state (`WaylandEventSource`).
    last_pointer_frame_time: f64,
    scroll_data: PointerScrollDataSet,
    // Touchpad gesture state (`FlingController` / `FlingBooster`).
    in_touchpad_scroll: bool,
    last_seen_scroll_update: Option<f64>,
    booster: FlingBooster,
}

impl Default for ScrollAnimator {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollAnimator {
    /// A resting animator at offset 0 with nothing to scroll.
    pub fn new() -> Self {
        ScrollAnimator {
            offset: 0.0,
            max_offset: 0.0,
            viewport: f64::INFINITY,
            smooth_scroll: true,
            motion: Motion::Idle,
            dirty: false,
            last_tick: f64::NEG_INFINITY,
            last_pointer_frame_time: 0.0,
            scroll_data: PointerScrollDataSet::default(),
            in_touchpad_scroll: false,
            last_seen_scroll_update: None,
            booster: FlingBooster::default(),
        }
    }

    // ----- geometry -------------------------------------------------------

    /// Sets the scroll range: content extent minus viewport extent. Negative
    /// values mean "nothing to scroll". Shrinking it clamps the offset.
    pub fn set_max_offset(&mut self, max: f64) {
        if !max.is_finite() {
            return;
        }
        self.max_offset = if max.is_finite() { max.max(0.0) } else { 0.0 };
        let clamped = self.clamp(self.offset);
        if clamped != self.offset {
            self.offset = clamped;
            self.dirty = true;
        }
    }

    /// Sets the visible extent of the scroller (window height for a
    /// full-window feed). Chrome uses it to cap a single wheel/touchpad delta
    /// (`cc::InputHandler::LimitDeltaToScrollerSize`, feature
    /// `kLimitScrollDeltaToScrollerSize`, enabled by default) and to cap fling
    /// distance at 3 viewports (`PhysicsBasedFlingCurve` bounding size). Until
    /// it is set, neither cap applies.
    pub fn set_viewport(&mut self, viewport: f64) {
        if !viewport.is_finite() {
            return;
        }
        self.viewport = if viewport.is_finite() && viewport > 0.0 {
            viewport
        } else {
            f64::INFINITY
        };
    }

    /// Turns wheel and keyboard animation on or off (default on). Chrome on
    /// Linux animates them only when the desktop has animations enabled:
    /// `content/browser/web_contents/web_contents_impl.cc` sets
    /// `prefs.enable_scroll_animator` from
    /// `gfx::Animation::ScrollAnimationsEnabledBySystem()`, which on Linux is
    /// `ui::LinuxUi::AnimationsEnabled()` (`ui/gfx/animation/animation_linux.cc`);
    /// `cc::InputHandler::ShouldAnimateScroll` then returns false. Frontends
    /// should mirror the desktop setting (e.g. GNOME `enable-animations`).
    /// Off: wheel ticks and `scroll_by` jump instantly. Flings, touchpad and
    /// `scroll_to` are unaffected.
    pub fn set_smooth_scroll(&mut self, enabled: bool) {
        self.smooth_scroll = enabled;
    }

    /// The scroll range set with [`set_max_offset`](Self::set_max_offset).
    pub fn max_offset(&self) -> f64 {
        self.max_offset
    }

    /// The offset as of the last `tick` (or the last instant change).
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// Where the current motion will come to rest.
    pub fn target(&self) -> f64 {
        match self.motion {
            Motion::Idle => self.offset,
            Motion::Curve(c) => self.clamp(c.target_value),
            Motion::Fling(f) => self.clamp(f.start_offset + f.distance),
        }
    }

    /// Current velocity in px/s as of the last `tick` (0 when idle and for
    /// direct touchpad scrolling, which is not animated).
    pub fn velocity(&self) -> f64 {
        match self.motion {
            Motion::Idle => 0.0,
            Motion::Curve(c) => {
                let t = self.last_tick - c.start_time;
                if t < 0.0 || self.last_tick >= c.end_time() {
                    0.0
                } else {
                    c.velocity(t)
                }
            }
            Motion::Fling(f) => f.velocity(self.last_tick - f.start_time),
        }
    }

    /// True while a smooth-scroll curve or a fling is running.
    pub fn is_animating(&self) -> bool {
        !matches!(self.motion, Motion::Idle)
    }

    /// True while the frontend must keep presenting frames: motion is running
    /// or the offset changed since the last `tick`. When false, draw nothing.
    pub fn needs_frame(&self) -> bool {
        self.dirty || self.is_animating()
    }

    // ----- step helpers ---------------------------------------------------

    /// Arrow-key step: `kPixelsPerLineStep` = 40 px.
    pub fn line_step(&self) -> f64 {
        PIXELS_PER_LINE_STEP
    }

    /// Page Up/Down/Space step for a viewport of `viewport_h` px:
    /// `cc::ScrollUtils::CalculatePageStep` = max(int(h * 0.875), 1). On Linux
    /// `kMaxOverlapBetweenPages` is INT_MAX, so the overlap term never wins.
    pub fn page_step(&self, viewport_h: f64) -> f64 {
        ((viewport_h * MIN_FRACTION_TO_STEP_WHEN_PAGING) as i64).max(1) as f64
    }

    /// Home/End step: `ScrollableArea::DocumentStep` is the full scroll size,
    /// so pass `-document_step()` / `+document_step()` to `scroll_by`.
    pub fn document_step(&self) -> f64 {
        self.max_offset
            + if self.viewport.is_finite() {
                self.viewport
            } else {
                0.0
            }
    }

    // ----- input ----------------------------------------------------------

    /// Mouse-wheel notches (positive scrolls toward the end). Fractional
    /// notches from high-resolution wheels are fine. Animated with Chrome's
    /// `kMouseWheel` curve: 100-200 ms ease-in-out, retargeted in flight with
    /// velocity matching.
    pub fn wheel_ticks(&mut self, ticks: f64, now: f64) {
        if !ticks.is_finite() || !now.is_finite() {
            return;
        }
        let now = self.input_time(now);
        self.end_touchpad_scroll();
        // A wheel frame is still a pointer scroll frame for Wayland's fling
        // velocity bookkeeping (non-finger, so the regression stops at it).
        self.push_pointer_frame(ticks * WHEEL_DELTA_PX, false, now);
        // Not verified from source: what a wheel tick does to a running
        // touchpad fling. We stop it, which is what a user expects.
        self.stop_fling(now);
        // `MouseWheelEvent` uses `gfx::ToRoundedVector2d` for the offset.
        let delta = (ticks * WHEEL_DELTA_PX).round();
        if delta == 0.0 {
            return;
        }
        self.settle_finished_curve(now);
        if !self.smooth_scroll {
            self.freeze_motion(now);
            self.apply_instant(delta.clamp(-self.viewport, self.viewport));
            return;
        }
        match self.motion {
            Motion::Curve(ref mut c) if c.kind == CurveKind::MouseWheel => {
                // `ScrollOffsetAnimationImpl::ScrollAnimationUpdateTarget`.
                let new_target = (c.target_value + delta).clamp(0.0, self.max_offset);
                c.update_target(now - c.start_time, new_target);
            }
            _ => {
                // Different-kind animation: cc aborts it when the wheel
                // gesture latches (`InputHandler::DidLatchToScroller`).
                self.freeze_motion(now);
                self.create_wheel_animation(delta, now);
            }
        }
    }

    /// Precise pixel deltas from a touchpad (Wayland `axis` with finger or
    /// continuous source, already converted to px). Applied 1:1, never
    /// animated (`cc::InputHandler::ShouldAnimateScroll` is false for
    /// `kScrollByPrecisePixel`), and recorded for fling velocity.
    pub fn wheel_pixels(&mut self, delta_px: f64, now: f64) {
        if !delta_px.is_finite() || !now.is_finite() {
            return;
        }
        let now = self.input_time(now);
        // `WaylandEventSource::ProcessPointerScrollData`: a finger scroll while
        // a fling is active first sends `kScrollFlingCancel`.
        if matches!(self.motion, Motion::Fling(_)) {
            self.booster.observe_fling_cancel(now);
            self.freeze_motion(now); // `EndCurrentFling`, booster kept.
        }
        // `InputHandler::ScrollBegin` aborts animations for precise deltas.
        self.freeze_motion(now);
        if !self.in_touchpad_scroll {
            self.in_touchpad_scroll = true;
            self.booster.observe_scroll_begin(now);
        }
        self.booster.observe_scroll_update(now, delta_px);
        self.last_seen_scroll_update = Some(now);
        self.push_pointer_frame(delta_px, true, now);

        let delta = delta_px.clamp(-self.viewport, self.viewport);
        let new_offset = self.clamp(self.offset + delta);
        if new_offset != self.offset {
            self.offset = new_offset;
            self.dirty = true;
        }
    }

    /// Fingers lifted (Wayland `axis_stop`). Computes the fling velocity from
    /// the last three scroll frames the way `WaylandEventSource` does and
    /// starts a fling if it is non-zero.
    pub fn fling_start(&mut self, now: f64) {
        let now = self.input_time(now);
        let stop_dt = now - self.last_pointer_frame_time;
        self.last_pointer_frame_time = now;
        let v = self.scroll_data.compute_fling_velocity(stop_dt);
        self.start_fling(v, now);
    }

    /// Starts a fling with a known velocity in px/s (positive toward the end),
    /// through the same path as [`fling_start`](Self::fling_start).
    pub fn fling(&mut self, velocity_px_per_s: f64, now: f64) {
        if !velocity_px_per_s.is_finite() || !now.is_finite() {
            return;
        }
        let now = self.input_time(now);
        self.start_fling(velocity_px_per_s, now);
    }

    /// Stops a running fling where it is (finger placed on the touchpad, or a
    /// click). Keeps fling-boost state, as `FlingController` does for
    /// `GestureFlingCancel`.
    pub fn cancel_fling(&mut self, now: f64) {
        let now = self.input_time(now);
        if matches!(self.motion, Motion::Fling(_)) {
            self.booster.observe_fling_cancel(now);
            self.freeze_motion(now);
        }
    }

    /// Keyboard-style relative scroll (arrows, Page Up/Down, Space, Home/End):
    /// Chrome's `kKeyboard` curve, a constant 150 ms ease-in-out that is
    /// retargeted from the pending target, so held keys accumulate.
    pub fn scroll_by(&mut self, delta_px: f64, now: f64) {
        if !delta_px.is_finite() || !now.is_finite() {
            return;
        }
        let now = self.input_time(now);
        self.end_touchpad_scroll();
        // `InputRouterImpl::SendKeyboardEvent` stops any fling.
        self.stop_fling(now);
        self.settle_finished_curve(now);
        // `ScrollAnimator::DesiredTargetOffset` / `ComputeDeltaToConsume`.
        let running_keyboard =
            matches!(self.motion, Motion::Curve(c) if c.kind == CurveKind::Keyboard);
        let desired = match self.motion {
            Motion::Curve(c) if running_keyboard => c.target_value,
            _ => self.offset,
        };
        let target = self.clamp(desired + delta_px);
        if target == desired {
            return; // zero delta to consume: nothing, not even a cancel.
        }
        if !self.smooth_scroll {
            // `ScrollAnimator::UserScroll` with the animator disabled.
            self.freeze_motion(now);
            self.apply_instant(target - self.offset);
            return;
        }
        match self.motion {
            Motion::Curve(ref mut c) if running_keyboard => {
                c.update_target(now - c.start_time, target);
            }
            _ => {
                // `CancelProgrammaticScrollAnimation`, then a fresh curve.
                self.freeze_motion(now);
                self.motion = Motion::Curve(EaseCurve::new(
                    CurveKind::Keyboard,
                    self.offset,
                    target,
                    now,
                ));
            }
        }
    }

    /// Programmatic smooth scroll to an absolute offset (`scrollTo` with
    /// `behavior: smooth`): duration sqrt(distance)/60 s capped at 1.5 s,
    /// cubic-bezier(0.4, 0, 0, 1). Like Blink's `ProgrammaticScrollAnimator`,
    /// each call starts a fresh curve from the current offset.
    pub fn scroll_to(&mut self, offset: f64, now: f64) {
        if !offset.is_finite() || !now.is_finite() {
            return;
        }
        let now = self.input_time(now);
        self.end_touchpad_scroll();
        self.stop_fling(now);
        self.settle_finished_curve(now);
        let target = self.clamp(offset);
        if let Motion::Curve(c) = self.motion
            && c.kind == CurveKind::Programmatic
            && c.target_value == target
        {
            return;
        }
        self.freeze_motion(now);
        if target == self.offset {
            return;
        }
        self.motion = Motion::Curve(EaseCurve::new(
            CurveKind::Programmatic,
            self.offset,
            target,
            now,
        ));
    }

    /// Instant scroll (scrollbar thumb drag, restoring a position): stops all
    /// motion. Thumb drags in Chrome are `kScrollByPrecisePixel`, so they are
    /// never animated.
    pub fn jump_to(&mut self, offset: f64) {
        if !offset.is_finite() {
            return;
        }
        self.end_touchpad_scroll();
        if matches!(self.motion, Motion::Fling(_)) {
            self.booster.reset();
        }
        self.motion = Motion::Idle;
        self.offset = self.clamp(offset);
        self.dirty = true;
    }

    // ----- frame ----------------------------------------------------------

    /// Advances to `now` (the presented frame's timestamp) and returns the
    /// offset to draw. Cheap, allocation-free, and a pure function of time
    /// for running motion.
    pub fn tick(&mut self, now: f64) -> f64 {
        if !now.is_finite() {
            return self.offset();
        }
        let now = if now > self.last_tick {
            now
        } else {
            self.last_tick
        };
        self.last_tick = now;
        self.dirty = false;
        match self.motion {
            Motion::Idle => {}
            Motion::Curve(c) => {
                if now >= c.end_time() {
                    self.offset = self.clamp(c.target_value);
                    self.motion = Motion::Idle;
                } else {
                    // `blink::ScrollAnimator::TickAnimation` clamps too.
                    self.offset = self.clamp(c.value(now - c.start_time));
                }
            }
            Motion::Fling(mut f) => {
                f.adjust_start_for_first_progress(now);
                let elapsed = now - f.start_time;
                if elapsed <= 0.0 {
                    self.motion = Motion::Fling(f);
                    return self.offset;
                }
                f.first_update_sent = true;
                let raw = f.start_offset + f.displacement(elapsed);
                let clamped = self.clamp(raw);
                self.offset = clamped;
                if now >= f.end_time() {
                    // Curve inactive: `fling_booster_.Reset(); EndCurrentFling`.
                    self.booster.reset();
                    self.motion = Motion::Idle;
                } else if raw != clamped {
                    // Momentum update not consumed at an edge:
                    // `RenderWidgetHostViewInput::StopFlingingIfNecessary`.
                    self.booster.reset();
                    self.motion = Motion::Idle;
                } else {
                    self.booster.observe_progress(f.velocity(elapsed));
                    self.motion = Motion::Fling(f);
                }
            }
        }
        self.offset
    }

    // ----- internals ------------------------------------------------------

    fn clamp(&self, v: f64) -> f64 {
        v.clamp(0.0, self.max_offset)
    }

    /// Input can't land before the last presented frame: retargeting from a
    /// point on the curve that is already on screen avoids a visible jump.
    fn input_time(&self, now: f64) -> f64 {
        if now > self.last_tick {
            now
        } else {
            self.last_tick
        }
    }

    /// A curve whose end time has passed is finished even if no frame has
    /// shown it yet; settle it so new input starts from its end point.
    fn settle_finished_curve(&mut self, now: f64) {
        if let Motion::Curve(c) = self.motion
            && now >= c.end_time()
        {
            self.offset = self.clamp(c.target_value);
            self.motion = Motion::Idle;
            self.dirty = true;
        }
    }

    fn apply_instant(&mut self, delta: f64) {
        let o = self.clamp(self.offset + delta);
        if o != self.offset {
            self.offset = o;
            self.dirty = true;
        }
    }

    fn end_touchpad_scroll(&mut self) {
        self.in_touchpad_scroll = false;
        self.last_seen_scroll_update = None;
    }

    /// `FlingController::StopFling`: stops the fling and resets the booster.
    fn stop_fling(&mut self, now: f64) {
        if matches!(self.motion, Motion::Fling(_)) {
            self.booster.reset();
            self.freeze_motion(now);
        }
    }

    /// Stops running motion at its position at `now`. Using the curve's value
    /// at the input time (not the last presented frame) keeps the result
    /// independent of the caller's frame rate.
    fn freeze_motion(&mut self, now: f64) {
        let pos = match self.motion {
            Motion::Idle => return,
            Motion::Curve(c) => c.value(now - c.start_time),
            Motion::Fling(mut f) => {
                f.adjust_start_for_first_progress(now);
                f.start_offset + f.displacement(now - f.start_time)
            }
        };
        self.motion = Motion::Idle;
        let pos = self.clamp(pos);
        if pos != self.offset {
            self.offset = pos;
            self.dirty = true;
        }
    }

    fn push_pointer_frame(&mut self, dx: f64, finger: bool, now: f64) {
        let dt = now - self.last_pointer_frame_time;
        self.last_pointer_frame_time = now;
        self.scroll_data
            .push_front(PointerScrollFrame { dx, dt, finger });
    }

    /// `InputHandler::ScrollLatchedScroller` "CreateNewAnimation" branch:
    /// `ComputeScrollDelta` (clamp + `LimitDeltaToScrollerSize`), then
    /// `LayerTreeHostImpl::ScrollAnimationCreate`.
    fn create_wheel_animation(&mut self, delta: f64, now: f64) {
        let applied =
            (self.clamp(self.offset + delta) - self.offset).clamp(-self.viewport, self.viewport);
        if applied.abs() <= MIN_ANIMATED_DELTA {
            if applied != 0.0 {
                self.offset = self.clamp(self.offset + applied);
                self.dirty = true;
            }
            return;
        }
        let target = self.clamp(self.offset + applied);
        self.motion = Motion::Curve(EaseCurve::new(
            CurveKind::MouseWheel,
            self.offset,
            target,
            now,
        ));
    }

    /// `FlingController::ProcessGestureFlingStart` / `UpdateCurrentFlingState`.
    fn start_fling(&mut self, raw_velocity: f64, now: f64) {
        let start_time = self.last_seen_scroll_update.unwrap_or(now);
        self.end_touchpad_scroll();
        let velocity = self.booster.velocity_for_fling_start(raw_velocity, now);
        if velocity == 0.0 || !velocity.is_finite() {
            self.booster.reset();
            if matches!(self.motion, Motion::Fling(_)) {
                self.freeze_motion(now);
            }
            return;
        }
        // Fling progress arrives as precise-pixel wheel events, which abort
        // any smooth-scroll animation (`InputHandler::ScrollBegin`).
        self.freeze_motion(now);
        let boost_multiplier = velocity.abs() / raw_velocity.abs().max(f64::MIN_POSITIVE);
        // `ScaleToFlooredSize(bounding_size, boost_multiplier * 3)`.
        let bound = if self.viewport.is_finite() {
            (self.viewport * boost_multiplier * FLING_BOUNDS_MULTIPLIER).floor()
        } else {
            f64::INFINITY
        };
        self.motion = Motion::Fling(Fling::new(velocity, start_time, self.offset, bound));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn animator(max: f64) -> ScrollAnimator {
        let mut a = ScrollAnimator::new();
        a.set_max_offset(max);
        a.tick(0.0);
        a
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    fn run_until_idle(a: &mut ScrollAnimator, mut t: f64, dt: f64) -> f64 {
        while a.needs_frame() {
            t += dt;
            a.tick(t);
            assert!(t < 100.0, "never settled");
        }
        t
    }

    #[test]
    fn bezier_matches_known_values() {
        let b = CubicBezier::new(0.42, 0.0, 0.58, 1.0);
        assert!(close(b.solve(0.5), 0.5, 1e-6));
        assert!(close(b.solve(0.0), 0.0, 1e-9));
        assert!(close(b.solve(1.0), 1.0, 1e-6));
        assert!(close(b.slope(0.0), 0.0, 1e-9));
        // Ease-in-out is symmetric: f(x) + f(1 - x) = 1.
        for i in 1..10 {
            let x = i as f64 / 10.0;
            assert!(close(b.solve(x) + b.solve(1.0 - x), 1.0, 1e-6));
        }
        // Linear extrapolation outside [0, 1] (`SolveWithEpsilon`).
        let lin = CubicBezier::new(0.25, 0.25, 0.75, 0.75);
        assert!(close(lin.solve(1.5), 1.5, 1e-9));
    }

    #[test]
    fn single_tick_duration_and_endpoint_match_chromium_formula() {
        // kInverseDelta: clamp(14 - |d|/60, 6, 12) frames / 60.
        for (ticks, expected_px, expected_s) in [
            (1.0, 120.0, 12.0 / 60.0),
            (2.0, 240.0, 10.0 / 60.0),
            (3.0, 360.0, 8.0 / 60.0),
            (4.0, 480.0, 6.0 / 60.0),
            (10.0, 1200.0, 6.0 / 60.0),
            (0.25, 30.0, 12.0 / 60.0),
        ] {
            let mut a = animator(100_000.0);
            a.wheel_ticks(ticks, 1.0);
            assert!(a.is_animating());
            assert_eq!(a.target(), expected_px);
            let Motion::Curve(c) = a.motion else { panic!() };
            assert!(
                close(c.total_duration, expected_s, 1e-12),
                "{ticks}: {}",
                c.total_duration
            );
            // Halfway through, ease-in-out is exactly halfway.
            a.tick(1.0 + expected_s / 2.0);
            assert!(close(a.offset(), expected_px / 2.0, 1e-3));
            // Just before the end it hasn't arrived; at the end it has.
            a.tick(1.0 + expected_s - 0.001);
            assert!(a.offset() < expected_px && a.is_animating());
            a.tick(1.0 + expected_s);
            assert_eq!(a.offset(), expected_px);
            assert!(!a.is_animating() && !a.needs_frame());
        }
    }

    #[test]
    fn first_frame_after_input_already_moves() {
        let mut a = animator(10_000.0);
        a.wheel_ticks(1.0, 1.0);
        let o = a.tick(1.0 + 1.0 / 60.0);
        assert!(o > 0.0);
    }

    #[test]
    fn wheel_up_from_negative_wayland_value_scrolls_up() {
        let mut a = animator(10_000.0);
        a.jump_to(5000.0);
        a.tick(0.5);
        a.wheel_ticks(wayland_value120_to_ticks(-120.0), 1.0);
        assert_eq!(a.target(), 4880.0);
        assert_eq!(wayland_axis_to_px(10.0), 120.0);
    }

    /// Samples position at 0.1 ms resolution and returns (positions, dt).
    fn dense_trace(a: &mut ScrollAnimator, inputs: &[(f64, f64)], until: f64) -> Vec<f64> {
        let dt = 1e-4;
        let mut out = Vec::new();
        let mut next = 0;
        let mut t = 0.0;
        while t <= until {
            while next < inputs.len() && inputs[next].0 <= t + 1e-12 {
                a.wheel_ticks(inputs[next].1, inputs[next].0);
                next += 1;
            }
            out.push(a.tick(t));
            t += dt;
        }
        out
    }

    #[test]
    fn retarget_is_velocity_continuous_and_never_overshoots() {
        // Ticks every 50 ms, then a burst at 16 ms spacing, all downward.
        let inputs = [
            (0.0, 1.0),
            (0.05, 1.0),
            (0.10, 1.0),
            (0.15, 1.0),
            (0.166, 1.0),
            (0.182, 1.0),
            (0.198, 2.0),
        ];
        let mut a = animator(100_000.0);
        let trace = dense_trace(&mut a, &inputs, 0.7);
        let final_target = 8.0 * 120.0;
        assert_eq!(a.offset(), final_target);
        let dt = 1e-4;
        // Never past the final target, never backwards.
        for w in trace.windows(2) {
            assert!(w[1] >= w[0] - 1e-9, "moved backwards");
            assert!(w[1] <= final_target + 1e-6, "overshot: {}", w[1]);
        }
        // Velocity just before and after each retarget matches.
        for &(t_in, _) in &inputs[1..] {
            let i = (t_in / dt).round() as usize;
            let v_before = (trace[i - 1] - trace[i - 2]) / dt;
            let v_after = (trace[i + 2] - trace[i + 1]) / dt;
            let scale = v_before.abs().max(1.0);
            assert!(
                (v_after - v_before).abs() / scale < 0.03,
                "speed jump at {t_in}: {v_before} -> {v_after}"
            );
        }
    }

    #[test]
    fn retarget_does_not_overshoot_after_reversal_or_near_target() {
        // Fast scroll, then a tick arrives late when nearly settled; and a
        // direction reversal. Neither may leave [min(start,end), max].
        let mut a = animator(100_000.0);
        a.jump_to(10_000.0);
        a.tick(0.0);
        let inputs = [(0.0, 4.0), (0.02, 4.0), (0.04, 4.0), (0.095, 1.0)];
        let trace = dense_trace(&mut a, &inputs, 0.6);
        let end = 10_000.0 + 13.0 * 120.0;
        // Chrome's retargeted curve may peak a sub-pixel amount past the
        // target here (see `retarget_overshoot_is_bounded`); never visibly.
        let peak = trace.iter().cloned().fold(f64::MIN, f64::max);
        assert!(peak <= end + 0.5, "visible overshoot: {}", peak - end);
        assert_eq!(a.offset(), end);
        let mut b = animator(100_000.0);
        b.jump_to(10_000.0);
        b.tick(0.0);
        let inputs = [(0.0, 3.0), (0.05, -6.0)];
        let trace = dense_trace(&mut b, &inputs, 0.6);
        let lo = 10_000.0 - 3.0 * 120.0;
        assert!(trace.iter().all(|&o| o >= lo - 1e-6));
        assert_eq!(b.offset(), lo);
    }

    #[test]
    fn retarget_overshoot_is_bounded() {
        // When `VelocityBasedDurationBound` wins, the new slope is exactly
        // 2.5, so the retargeted bezier is (0.42, 1.05, 0.58, 1). Its peak is
        // the worst-case overshoot as a fraction of the remaining distance.
        let b = EaseCurve::easing(CurveKind::MouseWheel, Some(VELOCITY_BOUND_FUDGE));
        let peak = (0..=10_000)
            .map(|i| b.solve(i as f64 / 10_000.0))
            .fold(f64::MIN, f64::max);
        assert!(peak > 1.0 && peak - 1.0 < 5e-4, "peak {peak}");
    }

    #[test]
    fn clamps_at_both_edges() {
        let mut a = animator(1000.0);
        // Up at the top: nothing moves, nothing animates.
        a.wheel_ticks(-1.0, 0.1);
        assert!(!a.is_animating());
        assert_eq!(a.tick(0.2), 0.0);
        // Down near the bottom: target clamps to the end.
        a.jump_to(950.0);
        a.tick(0.3);
        a.wheel_ticks(1.0, 0.4);
        assert_eq!(a.target(), 1000.0);
        a.wheel_ticks(3.0, 0.45);
        assert_eq!(a.target(), 1000.0);
        let mut t = 0.45;
        while a.is_animating() {
            t += 1.0 / 60.0;
            let o = a.tick(t);
            assert!((0.0..=1000.0).contains(&o));
        }
        assert_eq!(a.offset(), 1000.0);
        // Touchpad, keyboard, programmatic and jump all clamp.
        a.wheel_pixels(50.0, t + 0.1);
        assert_eq!(a.offset(), 1000.0);
        a.scroll_by(-5000.0, t + 0.2);
        assert_eq!(a.target(), 0.0);
        a.scroll_to(-10.0, t + 0.3);
        assert_eq!(a.target(), 0.0);
        a.jump_to(1e9);
        assert_eq!(a.offset(), 1000.0);
        // Shrinking the content clamps immediately.
        a.set_max_offset(400.0);
        assert_eq!(a.offset(), 400.0);
        assert!(a.needs_frame());
    }

    #[test]
    fn precise_deltas_apply_one_to_one() {
        let mut a = animator(10_000.0);
        a.wheel_pixels(3.5, 0.1);
        assert_eq!(a.offset(), 3.5);
        assert!(!a.is_animating());
        assert!(a.needs_frame());
        assert_eq!(a.tick(0.11), 3.5);
        assert!(!a.needs_frame());
        a.wheel_pixels(-1.25, 0.12);
        assert_eq!(a.offset(), 2.25);
        // A precise delta aborts a running wheel animation where it stands.
        a.wheel_ticks(1.0, 0.2);
        let shown = a.tick(0.25);
        let Motion::Curve(c) = a.motion else { panic!() };
        let at_input = c.value(0.26 - c.start_time);
        assert!(at_input >= shown);
        a.wheel_pixels(10.0, 0.26);
        assert!(!a.is_animating());
        assert_eq!(a.offset(), at_input + 10.0);
    }

    fn expected_fling_distance(v: f64) -> f64 {
        let decel = DEFAULT_PIXELS_PER_INCH * FLING_PHYSICAL_DECELERATION; // px/ms^2
        let v_ms = v / 1000.0;
        v_ms * v_ms / (2.0 * decel) * v.signum()
    }

    #[test]
    fn fling_travels_chromium_distance_and_stops() {
        for v in [400.0, 1000.0, 2500.0, -1800.0] {
            let mut a = animator(1_000_000.0);
            a.jump_to(500_000.0);
            a.tick(0.0);
            a.fling(v, 1.0);
            let dist = expected_fling_distance(v);
            let duration = (v.abs() / 2300.0_f64).min(2.0);
            assert!(close(a.target() - 500_000.0, dist, 1e-6), "{v}");
            let mut t = 1.0;
            while t < 1.0 + duration - 0.02 {
                t += 1.0 / 60.0;
                a.tick(t);
                assert!(a.is_animating());
            }
            a.tick(1.0 + duration);
            assert!(!a.is_animating() && !a.needs_frame());
            assert!(close(a.offset() - 500_000.0, dist, 1e-6));
        }
        // 1000 px/s -> v^2 / (2 * 96 px/in * 2.7559e-5 in/ms^2) = 189.0 px.
        assert!(close(expected_fling_distance(1000.0), 188.99, 0.01));
    }

    #[test]
    fn fling_starts_at_its_initial_velocity() {
        let mut a = animator(1_000_000.0);
        a.fling(1500.0, 0.0);
        a.tick(1e-4);
        assert!(
            close(a.velocity(), 1500.0, 1500.0 * 0.01),
            "{}",
            a.velocity()
        );
    }

    #[test]
    fn fling_is_bounded_by_three_viewports_and_stops_at_edges() {
        let mut a = animator(1_000_000.0);
        a.set_viewport(800.0);
        a.fling(20_000.0, 0.0);
        assert_eq!(a.target(), 2400.0);
        // Hitting the end stops the fling there.
        let mut b = animator(100.0);
        b.fling(3000.0, 0.0);
        let t = run_until_idle(&mut b, 0.0, 1.0 / 60.0);
        assert_eq!(b.offset(), 100.0);
        assert!(t < 0.5);
    }

    #[test]
    fn touchpad_lift_starts_fling_from_regressed_velocity() {
        let mut a = animator(1_000_000.0);
        // 10 px every 10 ms = 1000 px/s.
        let mut t = 0.0;
        for _ in 0..10 {
            t += 0.01;
            a.wheel_pixels(10.0, t);
            a.tick(t);
        }
        assert_eq!(a.offset(), 100.0);
        a.fling_start(t + 0.01);
        assert!(a.is_animating());
        let expected = expected_fling_distance(1000.0);
        assert!(close(a.target() - 100.0, expected, 0.5), "{}", a.target());
        // Resting > 200 ms before lifting: no fling.
        let mut b = animator(1_000_000.0);
        for i in 1..=5 {
            b.wheel_pixels(10.0, i as f64 * 0.01);
        }
        b.fling_start(0.05 + 0.25);
        assert!(!b.is_animating());
        // New finger input cancels the fling.
        a.tick(t + 0.03);
        a.wheel_pixels(1.0, t + 0.04);
        assert!(!a.is_animating());
    }

    #[test]
    fn repeated_fast_flings_boost() {
        let mut a = animator(10_000_000.0);
        a.jump_to(5_000_000.0);
        a.tick(0.0);
        a.fling(2000.0, 0.0);
        let mut t = 0.0;
        for _ in 0..6 {
            t += 1.0 / 60.0;
            a.tick(t);
        }
        // Fingers down and swipe again quickly in the same direction.
        for _ in 0..4 {
            t += 0.01;
            a.wheel_pixels(20.0, t);
        }
        let base = a.offset();
        a.fling(2000.0, t + 0.01);
        assert!(close(
            a.target() - base,
            expected_fling_distance(4000.0),
            1e-6
        ));
        // A slow second swipe does not boost.
        let mut b = animator(10_000_000.0);
        b.fling(2000.0, 0.0);
        b.tick(0.1);
        b.wheel_pixels(0.5, 0.11);
        b.wheel_pixels(0.5, 0.12);
        let base = b.offset();
        b.fling(2000.0, 0.13);
        assert!(close(
            b.target() - base,
            expected_fling_distance(2000.0),
            1e-6
        ));
    }

    #[test]
    fn keyboard_and_programmatic_curves() {
        let mut a = animator(100_000.0);
        let line = a.line_step();
        assert_eq!(line, 40.0);
        assert_eq!(a.page_step(800.0), 700.0);
        assert_eq!(a.page_step(0.5), 1.0);
        a.scroll_by(line, 1.0);
        let Motion::Curve(c) = a.motion else { panic!() };
        assert!(close(c.total_duration, 0.15, 1e-12));
        // Held key: the next press adds to the pending target.
        a.scroll_by(line, 1.05);
        assert_eq!(a.target(), 80.0);
        run_until_idle(&mut a, 1.05, 1.0 / 60.0);
        assert_eq!(a.offset(), 80.0);

        let mut p = animator(100_000.0);
        p.scroll_to(1000.0, 0.0);
        let Motion::Curve(c) = p.motion else { panic!() };
        assert!(close(c.total_duration, 1000f64.sqrt() / 60.0, 1e-12));
        p.tick(c.total_duration);
        assert_eq!(p.offset(), 1000.0);
        p.scroll_to(50_000.0, 1.0);
        let Motion::Curve(c) = p.motion else { panic!() };
        assert!(close(c.total_duration, 1.5, 1e-12));
    }

    fn scenario(a: &mut ScrollAnimator, t: f64, fired: &mut usize) {
        // Wheel ticks, a keyboard press, then a fling.
        let events: [(f64, u8, f64); 6] = [
            (0.000, 0, 1.0),
            (0.040, 0, 1.0),
            (0.090, 0, 2.0),
            (0.400, 1, 40.0),
            (0.700, 2, 1800.0),
            (1.200, 0, -1.0),
        ];
        while *fired < events.len() && events[*fired].0 <= t {
            let (et, kind, v) = events[*fired];
            match kind {
                0 => a.wheel_ticks(v, et),
                1 => a.scroll_by(v, et),
                _ => a.fling(v, et),
            }
            *fired += 1;
        }
    }

    fn sample_at_rate(hz: f64, drop_every: usize, sample_t: f64) -> f64 {
        let mut a = animator(1_000_000.0);
        a.jump_to(10_000.0);
        a.tick(0.0);
        let mut fired = 0;
        let mut frame = 0usize;
        loop {
            frame += 1;
            let t = frame as f64 / hz;
            if t >= sample_t {
                break;
            }
            scenario(&mut a, t, &mut fired);
            if drop_every == 0 || !frame.is_multiple_of(drop_every) {
                a.tick(t);
            }
        }
        scenario(&mut a, sample_t, &mut fired);
        a.tick(sample_t)
    }

    #[test]
    fn tick_is_frame_rate_independent() {
        let mut t = 0.013;
        while t < 2.5 {
            let o60 = sample_at_rate(60.0, 0, t);
            let o165 = sample_at_rate(165.0, 0, t);
            let o_drop = sample_at_rate(60.0, 3, t);
            assert!(close(o60, o165, 0.5), "t={t}: 60Hz {o60} vs 165Hz {o165}");
            assert!(
                close(o60, o_drop, 0.5),
                "t={t}: 60Hz {o60} vs dropped {o_drop}"
            );
            t += 0.037;
        }
    }

    #[test]
    fn smooth_scroll_off_jumps_instantly() {
        let mut a = animator(10_000.0);
        a.set_smooth_scroll(false);
        a.wheel_ticks(1.0, 0.1);
        assert!(!a.is_animating());
        assert_eq!(a.offset(), 120.0);
        a.scroll_by(40.0, 0.2);
        assert_eq!(a.offset(), 160.0);
        assert!(a.needs_frame());
        a.tick(0.21);
        assert!(!a.needs_frame());
    }

    #[test]
    fn needs_frame_goes_false_when_settled() {
        let mut a = animator(10_000.0);
        assert!(!a.needs_frame());
        a.wheel_ticks(1.0, 0.0);
        assert!(a.needs_frame());
        let t = run_until_idle(&mut a, 0.0, 1.0 / 144.0);
        assert!(t <= 0.2 + 1.0 / 144.0 + 1e-9);
        assert_eq!(a.offset(), 120.0);
        assert_eq!(a.velocity(), 0.0);
        // Idle ticks keep it false.
        a.tick(t + 1.0);
        assert!(!a.needs_frame());
        a.jump_to(10.0);
        assert!(a.needs_frame());
        a.tick(t + 2.0);
        assert!(!a.needs_frame());
    }

    #[test]
    fn late_wheel_tick_after_curve_end_starts_fresh() {
        let mut a = animator(10_000.0);
        a.wheel_ticks(1.0, 0.0);
        // No frame was drawn; the next tick arrives after the curve ended.
        a.wheel_ticks(1.0, 0.5);
        assert_eq!(a.target(), 240.0);
        let Motion::Curve(c) = a.motion else { panic!() };
        assert_eq!(c.initial_value, 120.0);
        assert_eq!(c.start_time, 0.5);
    }

    #[test]
    fn non_finite_input_is_ignored() {
        let mut a = ScrollAnimator::new();
        a.set_viewport(800.0);
        a.set_max_offset(5000.0);
        a.wheel_ticks(f64::NAN, 0.0);
        a.wheel_pixels(f64::INFINITY, 0.0);
        a.scroll_to(f64::NAN, 0.0);
        a.jump_to(f64::NAN);
        a.set_max_offset(f64::NAN);
        assert_eq!(a.tick(f64::INFINITY), 0.0);
        a.wheel_ticks(1.0, 1.0);
        let end = a.tick(10.0);
        assert!(end.is_finite() && end > 0.0);
        assert!(!a.needs_frame());
    }
}
