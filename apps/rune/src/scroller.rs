//! A page's main scroller: `nt_core::scroll::ScrollAnimator` drives the offset; Rune only
//! draws it. The content element carries a `live_offset` bound to [`Scroller::live`], so a
//! scroll frame moves one translation and never re-runs the view. The view re-runs only when
//! the offset crosses into another *band* (half a viewport), which is when the mounted window
//! of blocks (the viewport plus at least one viewport above and below) has to change.

use nt_core::scroll::ScrollAnimator;
use rune::{Live, Spring};
use std::time::Instant;

static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Seconds on the clock every scroll input and frame time uses.
pub fn now() -> f64 {
    T0.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// An input event's timestamp (`Tx::input_time`) on that clock; `now()` when there is none.
pub fn at(i: Option<Instant>) -> f64 {
    let t0 = *T0.get_or_init(Instant::now);
    i.map(|i| i.saturating_duration_since(t0).as_secs_f64()).unwrap_or_else(now)
}

/// How long the scrollbar stays after the last motion.
const BAR_LINGER: f64 = 0.9;
pub const BAR_W: f32 = 8.0;
const BAR_MIN: f32 = 32.0;
/// The hidden bar's opacity: invisible, but above zero so its track still takes the pointer.
const BAR_HIDDEN: f32 = 0.004;

pub struct Scroller {
    pub anim: ScrollAnimator,
    /// `[0, -offset]` for the content element.
    pub live: Live<[f32; 2]>,
    /// The scrollbar thumb's `[x, y, w, h]` inside its track.
    pub thumb: Live<[f32; 4]>,
    pub bar_alpha: Live<f32>,
    pub band: i64,
    vh: f32,
    content_h: f32,
    last_offset: f64,
    last_motion: f64,
    bar_shown: bool,
    /// Where on the thumb a scrollbar drag grabbed it.
    pub grab: f32,
    /// The pointer is over the scrollbar's track: keep it shown.
    hovered: bool,
}

impl Default for Scroller {
    fn default() -> Self {
        Scroller {
            anim: ScrollAnimator::new(),
            live: Live::new([0.0, 0.0]),
            thumb: Live::new([0.0, 0.0, BAR_W, 0.0]),
            bar_alpha: Live::new(BAR_HIDDEN),
            band: 0,
            vh: 0.0,
            content_h: 0.0,
            last_offset: 0.0,
            last_motion: f64::NEG_INFINITY,
            bar_shown: false,
            grab: 0.0,
            hovered: false,
        }
    }
}

impl Scroller {
    /// The smoothing the scrollbar's fade uses.
    pub fn bar_spring() -> Spring {
        Spring::new(0.25, 0.0)
    }

    pub fn viewport(&self) -> f32 {
        self.vh
    }

    /// New viewport height or content height: clamp the range (`set_max_offset`).
    pub fn set_geometry(&mut self, vh: f32, content_h: f32) {
        if vh != self.vh {
            self.vh = vh;
            self.anim.set_viewport(vh as f64);
        }
        if content_h != self.content_h || self.anim.max_offset() != (content_h - vh).max(0.0) as f64 {
            self.content_h = content_h;
            self.anim.set_max_offset((content_h - vh) as f64);
        }
    }

    pub fn band_size(&self) -> f32 {
        (self.vh * 0.5).max(100.0)
    }

    pub fn band_of(&self, offset: f64) -> i64 {
        (offset / self.band_size() as f64).floor() as i64
    }

    /// The content range `[top, bottom)` mounted for the current band: everything that can be
    /// on screen anywhere inside the band, plus one viewport above and below.
    pub fn mounted(&self) -> (f32, f32) {
        let b = self.band_size();
        let top = self.band as f32 * b - self.vh;
        (top, top + b + 3.0 * self.vh)
    }

    /// Ticks the animator at the frame time and publishes the offset. Returns true when the
    /// band changed (the view must re-run to mount other blocks).
    pub fn present(&mut self, frame: f64) -> bool {
        let y = self.anim.tick(frame);
        self.publish(y, frame);
        let band = self.band_of(y);
        if band != self.band {
            self.band = band;
            return true;
        }
        false
    }

    fn publish(&mut self, y: f64, frame: f64) {
        self.live.set([0.0, -(y as f32)]);
        let max = self.anim.max_offset() as f32;
        if self.vh > 0.0 && max > 0.0 {
            let h = (self.vh * self.vh / self.content_h.max(1.0)).clamp(BAR_MIN.min(self.vh), self.vh);
            let top = (y as f32 / max) * (self.vh - h);
            self.thumb.set([0.0, top, BAR_W, h]);
        }
        if y != self.last_offset {
            self.last_offset = y;
            self.last_motion = frame;
            if !self.bar_shown && max > 0.0 {
                self.bar_shown = true;
                self.bar_alpha.set(1.0);
            }
        } else if self.bar_shown && !self.hovered && frame - self.last_motion > BAR_LINGER {
            self.bar_shown = false;
            self.bar_alpha.set(BAR_HIDDEN);
        }
    }

    /// The pointer entered or left the scrollbar's track.
    pub fn hover(&mut self, on: bool, frame: f64) {
        self.hovered = on;
        self.last_motion = frame;
        if on && !self.bar_shown && self.anim.max_offset() > 0.0 {
            self.bar_shown = true;
            self.bar_alpha.set(1.0);
        }
    }

    /// When the scrollbar should next be checked for hiding, if it shows.
    pub fn bar_deadline(&self) -> Option<f64> {
        self.bar_shown.then_some(self.last_motion + BAR_LINGER + 0.02)
    }

    pub fn needs_frame(&self) -> bool {
        self.anim.needs_frame()
    }

    /// Scrollbar drag to `y` (track points, the grab point kept): an instant jump.
    pub fn drag_to(&mut self, y: f32) {
        let max = self.anim.max_offset() as f32;
        let h = (self.vh * self.vh / self.content_h.max(1.0)).clamp(BAR_MIN.min(self.vh), self.vh);
        let room = (self.vh - h).max(1.0);
        let off = ((y - self.grab) / room).clamp(0.0, 1.0) * max;
        self.anim.jump_to(off as f64);
    }

    pub fn thumb_rect(&self) -> [f32; 4] {
        self.thumb.get()
    }
}
